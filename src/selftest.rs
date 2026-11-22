//! Loopback end-to-end self-test (no UI).

use anyhow::{bail, Context, Result};
use std::time::Duration;

use crate::session::{HostEvent, HostSession, ViewerEvent, ViewerSession};

macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        // A release build uses the Windows GUI subsystem and may have no
        // console; never panic just because stdout is unavailable.
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

fn wait_for_code(host: &HostSession) -> Result<(String, u16)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            bail!("listener did not report an address");
        }
        match host.events().recv_timeout(remaining.min(Duration::from_millis(250))) {
            Ok(HostEvent::Listening { code, port, .. }) => return Ok((code, port)),
            Ok(HostEvent::Error { message }) => bail!("host error: {message}"),
            Ok(_) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                bail!("host events disconnected")
            }
        }
    }
}

fn positive() -> Result<()> {
    let host = HostSession::start(0);
    let (code, port) = wait_for_code(&host)?;
    say!("  host listening on 127.0.0.1:{port}");

    let viewer = ViewerSession::connect(&format!("127.0.0.1:{port}"), &code);

    // Host must see the request and accept it.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut incoming = false;
    while std::time::Instant::now() < deadline {
        match host.events().recv_timeout(Duration::from_millis(250)) {
            Ok(HostEvent::Incoming { .. }) => {
                incoming = true;
                break;
            }
            Ok(HostEvent::Error { message }) => bail!("host error before accept: {message}"),
            Ok(_) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => bail!("host events disconnected"),
        }
    }
    if !incoming {
        bail!("host did not receive the incoming connection");
    }
    host.accept();
    say!("  host accepted the session");

    // Viewer should become ready and produce at least one decoded frame.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut ready = false;
    while std::time::Instant::now() < deadline && !ready {
        match viewer.events.recv_timeout(Duration::from_millis(250)) {
            Ok(ViewerEvent::Ready(info)) => {
                say!(
                    "  session ready: {}x{}, codec {:?}",
                    info.width, info.height, info.codec
                );
                ready = true;
            }
            Ok(ViewerEvent::Error(message)) => bail!("viewer error: {message}"),
            Ok(ViewerEvent::Closed(reason)) => bail!("viewer closed early: {reason}"),
            Ok(_) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => bail!("viewer events disconnected"),
        }
    }
    if !ready {
        bail!("viewer did not become ready");
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut got_frame = false;
    while std::time::Instant::now() < deadline {
        match viewer.frames.recv_timeout(Duration::from_millis(250)) {
            Ok(frame) => {
                if frame.width == 0 || frame.height == 0 {
                    bail!("decoded frame has zero dimensions");
                }
                if frame.rgba.len() != frame.width * frame.height * 4 {
                    bail!(
                        "decoded frame size mismatch: {} bytes for {}x{}",
                        frame.rgba.len(),
                        frame.width,
                        frame.height
                    );
                }
                say!(
                    "  decoded frame {}x{} ({:.0} KiB)",
                    frame.width,
                    frame.height,
                    frame.rgba.len() as f32 / 1024.0
                );
                got_frame = true;
                break;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(_) => bail!("frame channel disconnected"),
        }
    }
    if !got_frame {
        bail!("no decoded frame within the timeout");
    }

    viewer.disconnect();
    host.stop_listener();
    drop(viewer);
    drop(host);
    Ok(())
}

fn negative() -> Result<()> {
    let host = HostSession::start(0);
    let (code, port) = wait_for_code(&host)?;
    let wrong = if code == "000-000-000" {
        "111-111-111"
    } else {
        "000-000-000"
    };
    say!("  attempting connection with a wrong code");

    let viewer = ViewerSession::connect(&format!("127.0.0.1:{port}"), wrong);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut failed_closed = false;
    while std::time::Instant::now() < deadline {
        match viewer.events.recv_timeout(Duration::from_millis(250)) {
            Ok(ViewerEvent::Error(_)) | Ok(ViewerEvent::Closed(_)) => {
                failed_closed = true;
                break;
            }
            Ok(ViewerEvent::Ready(_)) => bail!("wrong code was accepted!"),
            Ok(_) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    if !failed_closed {
        bail!("wrong code was not rejected in time");
    }
    say!("  wrong code rejected");

    viewer.disconnect();
    host.stop_listener();
    drop(viewer);
    drop(host);
    Ok(())
}

pub fn run() -> Result<()> {
    say!("DeskBeam self-test");
    say!("[1/2] correct code, accept, capture, encode, decode");
    positive().context("positive loopback test")?;
    say!("[2/2] wrong code must be rejected");
    negative().context("negative loopback test")?;
    Ok(())
}
