//! Remote input injection (Windows: SendInput). Linux backend arrives in M6.

use crate::capture::MonitorRect;
use crate::proto::InputEvent;

#[cfg(windows)]
mod win {
    use super::*;
    use anyhow::{bail, Result};
    use std::mem::size_of;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE,
        MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
        MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
        MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP,
        MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    fn virtual_screen() -> MonitorRect {
        unsafe {
            MonitorRect {
                x: GetSystemMetrics(SM_XVIRTUALSCREEN),
                y: GetSystemMetrics(SM_YVIRTUALSCREEN),
                width: GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1) as u32,
                height: GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1) as u32,
            }
        }
    }

    /// Map a normalized 0..=65535 point in a monitor to absolute virtual-desktop
    /// coordinates in the 0..=65535 range used by MOUSEEVENTF_ABSOLUTE.
    pub fn map_normalized(
        nx: u16,
        ny: u16,
        monitor: MonitorRect,
        virtual_rect: MonitorRect,
    ) -> (i32, i32) {
        let mw = monitor.width.max(2) as i64 - 1;
        let mh = monitor.height.max(2) as i64 - 1;
        let px = monitor.x as i64 + (nx as i64 * mw + 32_767) / 65_535;
        let py = monitor.y as i64 + (ny as i64 * mh + 32_767) / 65_535;

        let vw = virtual_rect.width.max(2) as i64 - 1;
        let vh = virtual_rect.height.max(2) as i64 - 1;
        let ax = (((px - virtual_rect.x as i64) * 65_535 + vw / 2) / vw).clamp(0, 65_535) as i32;
        let ay = (((py - virtual_rect.y as i64) * 65_535 + vh / 2) / vh).clamp(0, 65_535) as i32;
        (ax, ay)
    }

    fn send(inputs: &[INPUT]) -> Result<()> {
        if inputs.is_empty() {
            return Ok(());
        }
        let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            bail!("SendInput injected {sent}/{} events", inputs.len());
        }
        Ok(())
    }

    fn mouse_input(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
        let mut input: INPUT = unsafe { std::mem::zeroed() };
        input.r#type = INPUT_MOUSE;
        input.Anonymous.mi = MOUSEINPUT {
            dx,
            dy,
            mouseData: data,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };
        input
    }

    fn key_input(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        let mut input: INPUT = unsafe { std::mem::zeroed() };
        input.r#type = INPUT_KEYBOARD;
        input.Anonymous.ki = KEYBDINPUT {
            wVk: VIRTUAL_KEY(vk),
            wScan: scan,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };
        input
    }

    pub fn inject(event: &InputEvent, monitor: MonitorRect) -> Result<()> {
        match event {
            InputEvent::MouseMove { nx, ny } => {
                let (ax, ay) = map_normalized(*nx, *ny, monitor, virtual_screen());
                send(&[mouse_input(
                    ax,
                    ay,
                    0,
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                )])
            }
            InputEvent::MouseButton { button, down } => {
                let (flags, data) = match (button, down) {
                    (0, true) => (MOUSEEVENTF_LEFTDOWN, 0),
                    (0, false) => (MOUSEEVENTF_LEFTUP, 0),
                    (1, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
                    (1, false) => (MOUSEEVENTF_RIGHTUP, 0),
                    (2, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
                    (2, false) => (MOUSEEVENTF_MIDDLEUP, 0),
                    (3, true) => (MOUSEEVENTF_XDOWN, 1),
                    (3, false) => (MOUSEEVENTF_XUP, 1),
                    (4, true) => (MOUSEEVENTF_XDOWN, 2),
                    (4, false) => (MOUSEEVENTF_XUP, 2),
                    _ => return Ok(()),
                };
                send(&[mouse_input(0, 0, data, flags)])
            }
            InputEvent::Wheel { dx, dy } => {
                let mut inputs = Vec::with_capacity(2);
                let dy = (*dy).clamp(-20, 20);
                let dx = (*dx).clamp(-20, 20);
                if dy != 0 {
                    inputs.push(mouse_input(
                        0,
                        0,
                        (dy as i32 * 120) as u32,
                        MOUSEEVENTF_WHEEL,
                    ));
                }
                if dx != 0 {
                    inputs.push(mouse_input(
                        0,
                        0,
                        (dx as i32 * 120) as u32,
                        MOUSEEVENTF_HWHEEL,
                    ));
                }
                send(&inputs)
            }
            InputEvent::Key { vk, down, extended } => {
                let mut flags = KEYBD_EVENT_FLAGS(0);
                if !*down {
                    flags |= KEYEVENTF_KEYUP;
                }
                if *extended {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                send(&[key_input(*vk, 0, flags)])
            }
            InputEvent::Text { text } => {
                // Bound one message to 4096 UTF-16 units and send in small
                // batches so a malicious peer cannot force a huge allocation.
                let units: Vec<u16> = text.encode_utf16().take(4096).collect();
                for chunk in units.chunks(32) {
                    let mut inputs = Vec::with_capacity(chunk.len() * 2);
                    for unit in chunk {
                        inputs.push(key_input(0, *unit, KEYEVENTF_UNICODE));
                        inputs.push(key_input(0, *unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
                    }
                    send(&inputs)?;
                }
                Ok(())
            }
        }
    }

    /// Best-effort release of everything a remote peer might have left held.
    pub fn release_all() {
        let mut inputs = Vec::new();
        for vk in [0x10u16, 0x11, 0x12, 0x5B] {
            inputs.push(key_input(vk, 0, KEYEVENTF_KEYUP));
        }
        for flags in [
            MOUSEEVENTF_LEFTUP,
            MOUSEEVENTF_RIGHTUP,
            MOUSEEVENTF_MIDDLEUP,
        ] {
            inputs.push(mouse_input(0, 0, 0, flags));
        }
        let _ = send(&inputs);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn normalized_mapping_primary() {
            let monitor = MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            };
            let virtual_rect = monitor;
            assert_eq!(map_normalized(0, 0, monitor, virtual_rect), (0, 0));
            assert_eq!(
                map_normalized(65_535, 65_535, monitor, virtual_rect),
                (65_535, 65_535)
            );
            let (x, y) = map_normalized(32_768, 32_768, monitor, virtual_rect);
            assert!((x - 32_768).abs() <= 40);
            assert!((y - 32_768).abs() <= 40);
        }

        #[test]
        fn normalized_mapping_secondary_monitor() {
            let monitor = MonitorRect {
                x: 1920,
                y: 0,
                width: 1920,
                height: 1080,
            };
            let virtual_rect = MonitorRect {
                x: 0,
                y: 0,
                width: 3840,
                height: 1080,
            };
            let (x, _) = map_normalized(0, 0, monitor, virtual_rect);
            // 1920 / 3839 * 65535 ~= 32768
            assert!((x - 32_768).abs() <= 40, "x={x}");
            let (x2, _) = map_normalized(65_535, 0, monitor, virtual_rect);
            assert_eq!(x2, 65_535);
        }
    }
}

#[cfg(windows)]
pub use win::{inject, release_all};

#[cfg(not(windows))]
pub fn inject(_event: &InputEvent, _monitor: MonitorRect) -> anyhow::Result<()> {
    anyhow::bail!("input injection is not implemented on this platform yet")
}

#[cfg(not(windows))]
pub fn release_all() {}
