//! Host and viewer session orchestration.

use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{
    bounded, unbounded, Receiver, RecvTimeoutError, Sender, TrySendError,
};
use std::io::ErrorKind;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::capture;
use crate::codec::{bgra_to_i420, I420Frame, Vp8Decoder, Vp8Encoder};
use crate::crypto::{self, SharedTransport};
use crate::input;
use crate::proto::{
    self, AcceptBody, AcceptRequestBody, InputMessage, Message, MonitorInfo, MsgType,
    PingBody, RejectBody, SessionInfo, StatsBody, VideoFrame, ViewerHello,
};
use crate::util;

pub const DEFAULT_PORT: u16 = 7878;
const TARGET_FPS: u16 = 30;
const INITIAL_BITRATE: u32 = 4_000_000;

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum HostEvent {
    Listening {
        code: String,
        addresses: Vec<String>,
        port: u16,
    },
    Incoming {
        peer: String,
        client_name: String,
        session_id: u64,
    },
    IncomingCancelled,
    AuthFailed {
        peer: String,
        reason: String,
    },
    SessionStarted {
        peer: String,
        session_id: u64,
    },
    SessionEnded {
        reason: String,
    },
    Stats {
        fps: f32,
        kbps: f32,
        encode_ms: f32,
        queue: usize,
        bitrate_kbps: u32,
    },
    CodeRotated {
        code: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accept,
    Reject,
}

#[derive(Clone)]
struct ActiveControl {
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<TcpStream>>>,
}

impl ActiveControl {
    fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(sock) = self.socket.lock() {
            if let Some(s) = sock.as_ref() {
                let _ = s.shutdown(Shutdown::Both);
            }
        }
    }
}

pub struct HostSession {
    events: Receiver<HostEvent>,
    pending: Arc<Mutex<Option<Sender<Decision>>>>,
    active: Arc<Mutex<Option<ActiveControl>>>,
    handshake: Arc<Mutex<Option<TcpStream>>>,
    listener_stop: Arc<AtomicBool>,
}

impl HostSession {
    pub fn start(port: u16) -> Self {
        let (event_tx, event_rx) = unbounded();
        let pending = Arc::new(Mutex::new(None));
        let active = Arc::new(Mutex::new(None));
        let handshake = Arc::new(Mutex::new(None));
        let listener_stop = Arc::new(AtomicBool::new(false));

        let p = pending.clone();
        let a = active.clone();
        let h = handshake.clone();
        let s = listener_stop.clone();
        thread::Builder::new()
            .name("deskbeam-listener".into())
            .spawn(move || {
                if let Err(e) = listener_loop(port, event_tx.clone(), p, a, h, s) {
                    let _ = event_tx.send(HostEvent::Error {
                        message: format!("{e:#}"),
                    });
                }
            })
            .expect("spawn listener");

        Self {
            events: event_rx,
            pending,
            active,
            handshake,
            listener_stop,
        }
    }

    pub fn events(&self) -> &Receiver<HostEvent> {
        &self.events
    }

    pub fn accept(&self) {
        if let Ok(mut slot) = self.pending.lock() {
            if let Some(tx) = slot.take() {
                let _ = tx.send(Decision::Accept);
            }
        }
    }

    pub fn reject(&self) {
        if let Ok(mut slot) = self.pending.lock() {
            if let Some(tx) = slot.take() {
                let _ = tx.send(Decision::Reject);
            }
        }
    }

    pub fn disconnect(&self) {
        if let Ok(active) = self.active.lock() {
            if let Some(a) = active.as_ref() {
                a.shutdown();
            }
        }
    }

    pub fn stop_listener(&self) {
        self.listener_stop.store(true, Ordering::SeqCst);
        // Unblock an in-progress handshake so the listener port is released.
        if let Ok(slot) = self.handshake.lock() {
            if let Some(socket) = slot.as_ref() {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
        self.disconnect();
        self.reject();
    }
}

impl Drop for HostSession {
    fn drop(&mut self) {
        self.stop_listener();
    }
}

fn random_u64() -> u64 {
    let mut buf = [0u8; 8];
    if getrandom::fill(&mut buf).is_err() {
        return util::now_ms();
    }
    u64::from_le_bytes(buf)
}

fn listener_loop(
    port: u16,
    events: Sender<HostEvent>,
    pending: Arc<Mutex<Option<Sender<Decision>>>>,
    active: Arc<Mutex<Option<ActiveControl>>>,
    handshake: Arc<Mutex<Option<TcpStream>>>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port)))
        .with_context(|| format!("bind TCP port {port}"))?;
    listener
        .set_nonblocking(true)
        .context("set listener nonblocking")?;
    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);

    let addresses = util::local_ipv4_addresses();
    let mut code = util::generate_code();
    let _ = events.send(HostEvent::Listening {
        code: code.clone(),
        addresses,
        port: actual_port,
    });

    let rotate = |code: &mut String, events: &Sender<HostEvent>| {
        *code = util::generate_code();
        let _ = events.send(HostEvent::CodeRotated { code: code.clone() });
    };

    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, peer)) => {
                // The listener is non-blocking for the accept loop; accepted
                // sockets must be blocking for the handshake and session.
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_nodelay(true);
                if let Ok(mut slot) = handshake.lock() {
                    *slot = stream.try_clone().ok();
                }
                let result = handle_connection(&mut stream, peer, &code, &events, &pending, &active, &stop);
                if let Ok(mut slot) = handshake.lock() {
                    *slot = None;
                }
                if let Err(e) = result {
                    let _ = events.send(HostEvent::AuthFailed {
                        peer: peer.to_string(),
                        reason: format!("{e:#}"),
                    });
                    let _ = stream.shutdown(Shutdown::Both);
                    rotate(&mut code, &events);
                }
                // After a completed or rejected session, rotate the code.
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                rotate(&mut code, &events);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = events.send(HostEvent::Error {
                    message: format!("accept failed: {e}"),
                });
                thread::sleep(Duration::from_millis(250));
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_connection(
    stream: &mut TcpStream,
    peer: SocketAddr,
    code: &str,
    events: &Sender<HostEvent>,
    pending: &Arc<Mutex<Option<Sender<Decision>>>>,
    active: &Arc<Mutex<Option<ActiveControl>>>,
    stop: &Arc<AtomicBool>,
) -> Result<()> {
    let transport = match crypto::server_handshake(stream, code) {
        Ok(t) => t,
        Err(e) => {
            // Best-effort plaintext hint; the peer may already be gone.
            let close = Message::AuthFailed(proto::AuthFailedBody { code: 3 });
            let _ = proto::write_plain(stream, close.msg_type(), &close.encode()?);
            return Err(e);
        }
    };
    let shared = crypto::shared(transport);
    let mut reassembler = proto::Reassembler::new();
    let mut msg_id = 0u32;

    // A peer must send Hello promptly; otherwise it can stall the listener.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let (ty, body) = crypto::decrypt_read(&shared, stream, &mut reassembler)?;
    let _ = stream.set_read_timeout(None);
    let client = match Message::decode(ty, &body)? {
        Message::Hello(h) => h,
        other => bail!("expected Hello, got {:?}", other.msg_type()),
    };

    let client_name = if client.client_name.chars().count() > 128 {
        client.client_name.chars().take(128).collect::<String>()
    } else {
        client.client_name.clone()
    };

    let session_id = random_u64();
    let accept_request = Message::AcceptRequest(AcceptRequestBody { session_id });
    crypto::encrypt_write(
        &shared,
        stream,
        accept_request.msg_type(),
        &accept_request.encode()?,
        &mut msg_id,
    )?;

    let (decision_tx, decision_rx) = bounded(1);
    if let Ok(mut slot) = pending.lock() {
        *slot = Some(decision_tx);
    }
    let _ = events.send(HostEvent::Incoming {
        peer: peer.to_string(),
        client_name: client_name.clone(),
        session_id,
    });

    let decision = wait_for_decision(&decision_rx, stop, Duration::from_secs(120));
    if let Ok(mut slot) = pending.lock() {
        *slot = None;
    }
    if decision.is_none() {
        // The viewer went away, the host stopped, or the prompt timed out.
        let _ = events.send(HostEvent::IncomingCancelled);
    }

    match decision {
        Some(Decision::Accept) => {
            let monitors = capture::monitors();
            let primary = monitors
                .iter()
                .find(|m| m.primary)
                .cloned()
                .or_else(|| monitors.first().cloned())
                .ok_or_else(|| anyhow!("no monitors available"))?;

            let accept = Message::Accept(AcceptBody { session_id });
            crypto::encrypt_write(
                &shared,
                stream,
                accept.msg_type(),
                &accept.encode()?,
                &mut msg_id,
            )?;

            let info = SessionInfo {
                session_id,
                width: (primary.width & !1).max(2),
                height: (primary.height & !1).max(2),
                fps: TARGET_FPS,
                monitors,
                codec: proto::Codec::Vp8,
            };
            let info_msg = Message::SessionInfo(info.clone());
            crypto::encrypt_write(
                &shared,
                stream,
                info_msg.msg_type(),
                &info_msg.encode()?,
                &mut msg_id,
            )?;

            let control = ActiveControl {
                stop: Arc::new(AtomicBool::new(false)),
                socket: Arc::new(Mutex::new(Some(stream.try_clone()?))),
            };
            if let Ok(mut slot) = active.lock() {
                *slot = Some(control.clone());
            }

            let _ = events.send(HostEvent::SessionStarted {
                peer: peer.to_string(),
                session_id,
            });

            let reason = run_host_session(
                stream,
                shared,
                msg_id,
                &primary,
                session_id,
                events.clone(),
                control.stop.clone(),
                client_name,
            )
            .unwrap_or_else(|e| format!("{e:#}"));

            if let Ok(mut slot) = active.lock() {
                *slot = None;
            }
            let _ = events.send(HostEvent::SessionEnded { reason });
            Ok(())
        }
        Some(Decision::Reject) | None => {
            let reject = Message::Reject(RejectBody {
                reason: "host rejected the session".into(),
            });
            let _ = crypto::encrypt_write(
                &shared,
                stream,
                reject.msg_type(),
                &reject.encode()?,
                &mut msg_id,
            );
            let _ = stream.shutdown(Shutdown::Both);
            // Rejection is a normal outcome; do not report it as an auth failure.
            Ok(())
        }
    }
}

fn wait_for_decision(
    rx: &Receiver<Decision>,
    stop: &Arc<AtomicBool>,
    timeout: Duration,
) -> Option<Decision> {
    let deadline = Instant::now() + timeout;
    loop {
        if stop.load(Ordering::SeqCst) {
            return None;
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(d) => return Some(d),
            Err(RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    return None;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

struct Outbound {
    msg_type: MsgType,
    body: Vec<u8>,
}

#[derive(Default)]
struct ViewerSignals {
    fps: AtomicU32,
    rtt_ms: AtomicU32,
}

#[allow(clippy::too_many_arguments)]
fn run_host_session(
    stream: &mut TcpStream,
    shared: SharedTransport,
    msg_id: u32,
    primary: &MonitorInfo,
    _session_id: u64,
    events: Sender<HostEvent>,
    stop: Arc<AtomicBool>,
    _client_name: String,
) -> Result<String> {
    let read_stream = stream.try_clone().context("clone socket for reader")?;
    let write_stream = stream.try_clone().context("clone socket for writer")?;

    let (out_tx, out_rx) = bounded::<Outbound>(16);

    // Writer thread: sole owner of the write half and of its Noise send state.
    let writer_shared = shared.clone();
    let writer_stop = stop.clone();
    let writer = thread::Builder::new()
        .name("deskbeam-host-writer".into())
        .spawn(move || {
            let mut write_stream = write_stream;
            let mut id = msg_id;
            loop {
                match out_rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(out) => {
                        if crypto::encrypt_write(
                            &writer_shared,
                            &mut write_stream,
                            out.msg_type,
                            &out.body,
                            &mut id,
                        )
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if writer_stop.load(Ordering::SeqCst) {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            let _ = write_stream.shutdown(Shutdown::Both);
        })
        .context("spawn host writer")?;

    // Reader thread: input, ping, stats, bye.
    let reader_shared = shared.clone();
    let reader_stop = stop.clone();
    let reader_out = out_tx.clone();
    let reader_events = events.clone();
    let monitor_rect = primary.rect();
    let signals = Arc::new(ViewerSignals::default());
    let reader_signals = signals.clone();
    let keyframe_flag = Arc::new(AtomicBool::new(true));
    let reader_keyframe = keyframe_flag.clone();
    let reader = thread::Builder::new()
        .name("deskbeam-host-reader".into())
        .spawn(move || {
            let mut read_stream = read_stream;
            let mut reassembler = proto::Reassembler::new();
            let reason: String;
            let mut injected_any = false;
            loop {
                if reader_stop.load(Ordering::SeqCst) {
                    reason = "local disconnect".into();
                    break;
                }
                match crypto::decrypt_read(&reader_shared, &mut read_stream, &mut reassembler) {
                    Ok((ty, body)) => match Message::decode(ty, &body) {
                        Ok(Message::Input(m)) => {
                            injected_any = true;
                            if let Err(e) = input::inject(&m.event, monitor_rect) {
                                let _ = reader_events.send(HostEvent::Error {
                                    message: format!("input injection failed: {e}"),
                                });
                            }
                        }
                        Ok(Message::Ping(p)) => {
                            if let Ok(body) = Message::Pong(p).encode() {
                                let _ = reader_out.send(Outbound {
                                    msg_type: MsgType::Pong,
                                    body,
                                });
                            }
                        }
                        Ok(Message::Stats(s)) => {
                            reader_signals.fps.store(s.recv_fps as u32, Ordering::Relaxed);
                            reader_signals
                                .rtt_ms
                                .store(s.rtt_ms as u32, Ordering::Relaxed);
                        }
                        Ok(Message::RequestKeyframe) => {
                            reader_keyframe.store(true, Ordering::SeqCst);
                        }
                        Ok(Message::Bye(b)) => {
                            reason = b.reason;
                            break;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            reason = format!("protocol error: {e:#}");
                            break;
                        }
                    },
                    Err(e) => {
                        reason = format!("connection ended: {e:#}");
                        break;
                    }
                }
            }
            reader_stop.store(true, Ordering::SeqCst);
            // Only release keys/buttons if the remote peer actually injected
            // something; otherwise we would interfere with the local user.
            if injected_any {
                input::release_all();
            }
            let _ = read_stream.shutdown(Shutdown::Both);
            reason
        })
        .context("spawn host reader")?;

    // Capture + encode thread.
    let capture_stop = stop.clone();
    let capture_events = events.clone();
    let capture_out = out_tx.clone();
    let capture_signals = signals.clone();
    let capture_keyframe = keyframe_flag.clone();
    let capture_monitor = primary.clone();
    let capture_socket = stream.try_clone().context("clone socket for capture stop")?;

    let capturer = thread::Builder::new()
        .name("deskbeam-host-capture".into())
        .spawn(move || {
            let mut capture = match capture::ScreenCapture::new(&capture_monitor) {
                Ok(c) => c,
                Err(e) => {
                    let _ = capture_events.send(HostEvent::Error {
                        message: format!("screen capture init failed: {e:#}"),
                    });
                    capture_stop.store(true, Ordering::SeqCst);
                    let _ = capture_socket.shutdown(Shutdown::Both);
                    return;
                }
            };
            let w = capture.width();
            let h = capture.height();
            let mut i420 = match I420Frame::new(w, h) {
                Ok(f) => f,
                Err(e) => {
                    let _ = capture_events.send(HostEvent::Error {
                        message: format!("frame allocation failed: {e:#}"),
                    });
                    capture_stop.store(true, Ordering::SeqCst);
                    let _ = capture_socket.shutdown(Shutdown::Both);
                    return;
                }
            };
            let mut encoder = match Vp8Encoder::new(w, h, INITIAL_BITRATE, TARGET_FPS) {
                Ok(e) => e,
                Err(e) => {
                    let _ = capture_events.send(HostEvent::Error {
                        message: format!("encoder init failed: {e:#}"),
                    });
                    capture_stop.store(true, Ordering::SeqCst);
                    let _ = capture_socket.shutdown(Shutdown::Both);
                    return;
                }
            };

            let frame_time = Duration::from_millis(1000 / TARGET_FPS as u64);
            let mut seq = 0u64;
            let mut last_stats = Instant::now();
            let mut frames_in_window = 0u32;
            let mut bytes_in_window = 0u64;
            let mut encode_ms_total = 0f32;
            let mut window_encode_count = 0u32;
            let mut last_bitrate = encoder.bitrate() as u32;
            let mut encode_failures = 0u32;

            while !capture_stop.load(Ordering::SeqCst) {
                let frame_start = Instant::now();
                let bgra = match capture.capture() {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = capture_events.send(HostEvent::Error {
                            message: format!("capture failed: {e:#}"),
                        });
                        capture_stop.store(true, Ordering::SeqCst);
                        let _ = capture_socket.shutdown(Shutdown::Both);
                        break;
                    }
                };
                if let Err(e) = bgra_to_i420(bgra, w, h, &mut i420) {
                    let _ = capture_events.send(HostEvent::Error {
                        message: format!("color conversion failed: {e:#}"),
                    });
                    capture_stop.store(true, Ordering::SeqCst);
                    let _ = capture_socket.shutdown(Shutdown::Both);
                    break;
                }

                let force_key = capture_keyframe.swap(false, Ordering::SeqCst);
                let data = match encoder.encode(&i420, force_key) {
                    Ok(d) => d,
                    Err(e) => {
                        let _ = capture_events.send(HostEvent::Error {
                            message: format!("encode failed: {e:#}"),
                        });
                        capture_keyframe.store(true, Ordering::SeqCst);
                        encode_failures += 1;
                        if encode_failures >= 5 {
                            capture_stop.store(true, Ordering::SeqCst);
                            let _ = capture_socket.shutdown(Shutdown::Both);
                            break;
                        }
                        continue;
                    }
                };
                encode_failures = 0;
                if data.is_empty() {
                    let elapsed = frame_start.elapsed();
                    if elapsed < frame_time {
                        thread::sleep(frame_time - elapsed);
                    }
                    continue;
                }

                seq = seq.wrapping_add(1);
                let data_len = data.len();
                let video = VideoFrame {
                    seq,
                    pts_ms: util::now_ms(),
                    keyframe: force_key,
                    width: w as u32,
                    height: h as u32,
                    data,
                };
                let msg = Message::VideoFrame(video);
                match msg.encode() {
                    Ok(body) => {
                        let out = Outbound {
                            msg_type: MsgType::VideoFrame,
                            body,
                        };
                        match capture_out.try_send(out) {
                            Ok(()) => {
                                frames_in_window += 1;
                                bytes_in_window += data_len as u64;
                            }
                            Err(TrySendError::Full(_)) => {
                                // Drop this frame; ensure the next one is a keyframe
                                // if we just lost one.
                                if force_key {
                                    capture_keyframe.store(true, Ordering::SeqCst);
                                }
                            }
                            Err(TrySendError::Disconnected(_)) => break,
                        }
                    }
                    Err(_) => {}
                }

                let encode_ms = frame_start.elapsed().as_secs_f32() * 1000.0;
                encode_ms_total += encode_ms;
                window_encode_count += 1;

                if last_stats.elapsed() >= Duration::from_secs(1) {
                    let fps = frames_in_window as f32;
                    let kbps = (bytes_in_window as f64 * 8.0 / 1000.0) as f32;
                    let avg_encode = if window_encode_count > 0 {
                        encode_ms_total / window_encode_count as f32
                    } else {
                        0.0
                    };
                    let queue = capture_out.len();

                    // Adaptive bitrate: local pressure + viewer telemetry.
                    let viewer_fps = capture_signals.fps.load(Ordering::Relaxed);
                    let rtt = capture_signals.rtt_ms.load(Ordering::Relaxed);
                    let mut target = last_bitrate;
                    let pressured = queue > 2
                        || avg_encode > 30.0
                        || (viewer_fps > 0 && viewer_fps < 8)
                        || rtt > 250;
                    let relaxed =
                        queue == 0 && avg_encode < 12.0 && (rtt == 0 || rtt < 80);
                    if pressured {
                        target = ((target as f64 * 0.75) as u32).max(600_000);
                    } else if relaxed {
                        target = ((target as f64 * 1.10) as u32).min(12_000_000);
                    }
                    if target != last_bitrate {
                        if encoder.set_bitrate(target).is_ok() {
                            last_bitrate = target;
                        }
                    }

                    let _ = capture_events.send(HostEvent::Stats {
                        fps,
                        kbps,
                        encode_ms: avg_encode,
                        queue,
                        bitrate_kbps: last_bitrate / 1000,
                    });

                    frames_in_window = 0;
                    bytes_in_window = 0;
                    encode_ms_total = 0.0;
                    window_encode_count = 0;
                    last_stats = Instant::now();
                }

                let elapsed = frame_start.elapsed();
                if elapsed < frame_time {
                    thread::sleep(frame_time - elapsed);
                }
            }
        })
        .context("spawn host capture")?;

    // Reader ends the session; capture and writer wind down from the stop flag.
    let reason = reader
        .join()
        .unwrap_or_else(|_| "reader thread panicked".to_string());
    stop.store(true, Ordering::SeqCst);
    if let Ok(sock) = stream.try_clone() {
        let _ = sock.shutdown(Shutdown::Both);
    }
    drop(out_tx);
    let _ = writer.join();
    let _ = capturer.join();
    Ok(reason)
}

// ---------------------------------------------------------------------------
// Viewer
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct FrameData {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ViewerStats {
    pub fps: f32,
    pub kbps: f32,
    pub rtt_ms: u32,
    pub decode_ms: f32,
}

#[derive(Debug, Clone)]
pub enum ViewerEvent {
    Status(String),
    Ready(Box<SessionInfo>),
    Stats(ViewerStats),
    Closed(String),
    Error(String),
}

#[derive(Default)]
struct Counters {
    frames: AtomicU64,
    bytes: AtomicU64,
    decode_us: AtomicU64,
    rtt_ms: AtomicU32,
    want_keyframe: AtomicBool,
    ping_nonce: AtomicU64,
    ping_sent_ms: AtomicU64,
}

struct OutboundViewer {
    msg_type: MsgType,
    body: Vec<u8>,
}

pub struct ViewerSession {
    pub events: Receiver<ViewerEvent>,
    pub frames: Receiver<FrameData>,
    out_tx: Sender<OutboundViewer>,
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<TcpStream>>>,
}

impl ViewerSession {
    pub fn connect(address: &str, code: &str) -> Self {
        let (event_tx, event_rx) = unbounded();
        let (frame_tx, frame_rx) = bounded::<FrameData>(2);
        let (out_tx, out_rx) = bounded::<OutboundViewer>(64);
        let stop = Arc::new(AtomicBool::new(false));
        let socket = Arc::new(Mutex::new(None));

        let net_stop = stop.clone();
        let net_socket = socket.clone();
        let net_out_tx = out_tx.clone();
        let address = address.to_string();
        let code = code.to_string();
        thread::Builder::new()
            .name("deskbeam-viewer-net".into())
            .spawn(move || {
                if let Err(e) = viewer_loop(
                    &address,
                    &code,
                    event_tx.clone(),
                    frame_tx,
                    net_out_tx,
                    out_rx,
                    net_stop,
                    net_socket,
                ) {
                    let _ = event_tx.send(ViewerEvent::Error(format!("{e:#}")));
                }
            })
            .expect("spawn viewer");

        Self {
            events: event_rx,
            frames: frame_rx,
            out_tx,
            stop,
            socket,
        }
    }

    pub fn send_input(&self, event: crate::proto::InputEvent) {
        let message = Message::Input(InputMessage { event });
        if let (Ok(body), ty) = (message.encode(), message.msg_type()) {
            // Never block the UI on a slow link; drop input rather than freeze.
            let _ = self.out_tx.try_send(OutboundViewer { msg_type: ty, body });
        }
    }

    pub fn disconnect(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let bye = Message::Bye(proto::ByeBody {
            reason: "viewer disconnect".into(),
        });
        if let (Ok(body), ty) = (bye.encode(), bye.msg_type()) {
            let _ = self.out_tx.try_send(OutboundViewer { msg_type: ty, body });
        }
        if let Ok(sock) = self.socket.lock() {
            if let Some(s) = sock.as_ref() {
                let _ = s.shutdown(Shutdown::Both);
            }
        }
    }
}

impl Drop for ViewerSession {
    fn drop(&mut self) {
        self.disconnect();
    }
}

fn resolve(address: &str) -> Result<SocketAddr> {
    let address = address.trim();
    address
        .to_socket_addrs()
        .with_context(|| format!("resolve {address}"))?
        .next()
        .ok_or_else(|| anyhow!("no address for {address}"))
}

fn connect_with_timeout(addr: SocketAddr, timeout: Duration) -> Result<TcpStream> {
    let stream = TcpStream::connect_timeout(&addr, timeout)
        .with_context(|| format!("connect to {addr}"))?;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

#[allow(clippy::too_many_arguments)]
fn viewer_loop(
    address: &str,
    code: &str,
    events: Sender<ViewerEvent>,
    frames: Sender<FrameData>,
    out_tx: Sender<OutboundViewer>,
    out_rx: Receiver<OutboundViewer>,
    stop: Arc<AtomicBool>,
    socket_slot: Arc<Mutex<Option<TcpStream>>>,
) -> Result<()> {
    let _ = events.send(ViewerEvent::Status("Connecting…".into()));
    let addr = resolve(address)?;
    let mut stream = connect_with_timeout(addr, Duration::from_secs(10))?;
    if let Ok(mut slot) = socket_slot.lock() {
        *slot = Some(stream.try_clone()?);
    }
    if stop.load(Ordering::SeqCst) {
        let _ = stream.shutdown(Shutdown::Both);
        return Ok(());
    }

    let _ = events.send(ViewerEvent::Status("Authenticating…".into()));
    let transport = crypto::client_handshake(&mut stream, code)?;
    if stop.load(Ordering::SeqCst) {
        let _ = stream.shutdown(Shutdown::Both);
        return Ok(());
    }
    let shared = crypto::shared(transport);
    let mut msg_id = 0u32;

    let hello = Message::Hello(ViewerHello {
        client_name: util::hostname(),
        platform: std::env::consts::OS.to_string(),
        capabilities: 0,
    });
    crypto::encrypt_write(
        &shared,
        &mut stream,
        hello.msg_type(),
        &hello.encode()?,
        &mut msg_id,
    )?;

    // Writer thread (input, pong, ping, stats, bye).
    let write_stream = stream.try_clone()?;
    let writer_shared = shared.clone();
    let writer_stop = stop.clone();
    let writer = thread::Builder::new()
        .name("deskbeam-viewer-writer".into())
        .spawn(move || {
            let mut write_stream = write_stream;
            let mut id = msg_id;
            loop {
                match out_rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(out) => {
                        if crypto::encrypt_write(
                            &writer_shared,
                            &mut write_stream,
                            out.msg_type,
                            &out.body,
                            &mut id,
                        )
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if writer_stop.load(Ordering::SeqCst) {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            let _ = write_stream.shutdown(Shutdown::Both);
        })
        .context("spawn viewer writer")?;

    // Reader side owns the decoder.
    let mut reassembler = proto::Reassembler::new();
    let mut decoder = Vp8Decoder::new()?;
    let counters = Arc::new(Counters::default());

    let _ = events.send(ViewerEvent::Status(
        "Waiting for the host to accept…".into(),
    ));

    // AcceptRequest promptly, then allow the host user time to decide.
    if stop.load(Ordering::SeqCst) {
        return Ok(());
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let (ty, body) = crypto::decrypt_read(&shared, &mut stream, &mut reassembler)?;
    match Message::decode(ty, &body)? {
        Message::AcceptRequest(_) => {}
        Message::Reject(r) => {
            let _ = events.send(ViewerEvent::Closed(r.reason));
            return Ok(());
        }
        other => bail!("unexpected message {:?} before accept", other.msg_type()),
    }

    let _ = stream.set_read_timeout(Some(Duration::from_secs(180)));
    let session_info = loop {
        let (ty, body) = crypto::decrypt_read(&shared, &mut stream, &mut reassembler)?;
        match Message::decode(ty, &body)? {
            Message::Accept(_) => continue,
            Message::SessionInfo(info) => break info,
            Message::Reject(r) => {
                let _ = events.send(ViewerEvent::Closed(r.reason));
                return Ok(());
            }
            Message::Bye(b) => {
                let _ = events.send(ViewerEvent::Closed(b.reason));
                return Ok(());
            }
            other => bail!("unexpected message {:?} while authorizing", other.msg_type()),
        }
    };
    let _ = stream.set_read_timeout(None);
    let _ = events.send(ViewerEvent::Ready(Box::new(session_info)));

    // Ticker: Ping and Stats once per second, even on a static screen.
    let ticker_counters = counters.clone();
    let ticker_events = events.clone();
    let ticker_out = out_tx.clone();
    let ticker_stop = stop.clone();
    let ticker = thread::Builder::new()
        .name("deskbeam-viewer-ticker".into())
        .spawn(move || {
            let mut last = Instant::now();
            while !ticker_stop.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(200));
                if last.elapsed() < Duration::from_secs(1) {
                    continue;
                }
                last = Instant::now();
                let frame_count = ticker_counters.frames.swap(0, Ordering::Relaxed);
                let bytes = ticker_counters.bytes.swap(0, Ordering::Relaxed);
                let decode_us = ticker_counters.decode_us.swap(0, Ordering::Relaxed);
                let fps = frame_count as f32;
                let kbps = (bytes as f64 * 8.0 / 1000.0) as f32;
                let decode_ms = if frame_count > 0 {
                    decode_us as f32 / 1000.0 / frame_count as f32
                } else {
                    0.0
                };
                let rtt = ticker_counters.rtt_ms.load(Ordering::Relaxed);
                let want_keyframe = ticker_counters
                    .want_keyframe
                    .swap(false, Ordering::SeqCst);

                let stats = Message::Stats(StatsBody {
                    recv_fps: fps.min(65535.0) as u16,
                    kbps: kbps.min(u32::MAX as f32) as u32,
                    rtt_ms: rtt.min(65535) as u16,
                    decode_ms: decode_ms.min(65535.0) as u16,
                    want_keyframe,
                });
                if let Ok(body) = stats.encode() {
                    let _ = ticker_out.send(OutboundViewer {
                        msg_type: MsgType::Stats,
                        body,
                    });
                }

                let nonce = util::now_ms();
                ticker_counters.ping_nonce.store(nonce, Ordering::SeqCst);
                ticker_counters
                    .ping_sent_ms
                    .store(util::now_ms(), Ordering::SeqCst);
                let ping = Message::Ping(PingBody {
                    nonce,
                    sent_ms: util::now_ms(),
                });
                if let Ok(body) = ping.encode() {
                    let _ = ticker_out.send(OutboundViewer {
                        msg_type: MsgType::Ping,
                        body,
                    });
                }

                let _ = ticker_events.send(ViewerEvent::Stats(ViewerStats {
                    fps,
                    kbps,
                    rtt_ms: rtt,
                    decode_ms,
                }));

                if want_keyframe {
                    let req = Message::RequestKeyframe;
                    let _ = ticker_out.send(OutboundViewer {
                        msg_type: req.msg_type(),
                        body: req.encode()?,
                    });
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .context("spawn viewer ticker")?;

    // Main read loop.
    let closed_reason: String;
    loop {
        if stop.load(Ordering::SeqCst) {
            closed_reason = "local disconnect".into();
            break;
        }
        match crypto::decrypt_read(&shared, &mut stream, &mut reassembler) {
            Ok((ty, body)) => match Message::decode(ty, &body) {
                Ok(Message::VideoFrame(v)) => {
                    counters
                        .bytes
                        .fetch_add(v.data.len() as u64, Ordering::Relaxed);
                    let t = Instant::now();
                    match decoder.decode(&v.data) {
                        Ok(Some(image)) => {
                            counters.frames.fetch_add(1, Ordering::Relaxed);
                            counters
                                .decode_us
                                .fetch_add(t.elapsed().as_micros() as u64, Ordering::Relaxed);
                            let _ = frames.try_send(FrameData {
                                width: image.width,
                                height: image.height,
                                rgba: image.pixels,
                            });
                        }
                        Ok(None) => {}
                        Err(_) => {
                            counters.want_keyframe.store(true, Ordering::SeqCst);
                        }
                    }
                }
                Ok(Message::Ping(p)) => {
                    if let Ok(body) = Message::Pong(p).encode() {
                        let _ = out_tx.send(OutboundViewer {
                            msg_type: MsgType::Pong,
                            body,
                        });
                    }
                }
                Ok(Message::Pong(p)) => {
                    let expected = counters.ping_nonce.load(Ordering::SeqCst);
                    if p.nonce == expected {
                        let sent = counters.ping_sent_ms.load(Ordering::SeqCst);
                        let rtt = util::now_ms().saturating_sub(sent);
                        counters.rtt_ms.store(rtt.min(u32::MAX as u64) as u32, Ordering::Relaxed);
                    }
                }
                Ok(Message::Bye(b)) => {
                    closed_reason = b.reason;
                    break;
                }
                Ok(Message::Reject(r)) => {
                    closed_reason = r.reason;
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    closed_reason = format!("protocol error: {e:#}");
                    break;
                }
            },
            Err(e) => {
                closed_reason = format!("connection ended: {e:#}");
                break;
            }
        }
    }

    stop.store(true, Ordering::SeqCst);
    if let Ok(sock) = socket_slot.lock() {
        if let Some(s) = sock.as_ref() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
    drop(out_tx);
    let _ = writer.join();
    let _ = ticker.join();
    let _ = events.send(ViewerEvent::Closed(closed_reason));
    Ok(())
}
