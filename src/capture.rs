//! Screen capture (Windows: GDI). Linux backend arrives in milestone M6.

use crate::proto::MonitorInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl MonitorInfo {
    pub fn rect(&self) -> MonitorRect {
        MonitorRect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use anyhow::{bail, Context, Result};
    use std::mem::size_of;
    use windows::core::{w, BOOL, PCWSTR};
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDCW, DeleteDC, DeleteObject,
        EnumDisplayMonitors, GdiFlush, GetDIBits, GetMonitorInfoW, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HDC, HGDIOBJ, HMONITOR, MONITORINFO,
        MONITORINFOEXW, SRCCOPY,
    };
    use windows::Win32::UI::HiDpi::{
        GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        MDT_EFFECTIVE_DPI,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
    };

    pub fn init_dpi_awareness() {
        // Best effort: failing here only affects coordinate fidelity.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }

    unsafe extern "system" fn enum_cb(
        monitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let list = &mut *(lparam.0 as *mut Vec<MonitorInfo>);
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        let ok = GetMonitorInfoW(monitor, &mut mi.monitorInfo as *mut MONITORINFO);
        if ok.as_bool() {
            let rect = mi.monitorInfo.rcMonitor;
            let mut dx = 96u32;
            let mut dy = 96u32;
            let scale = if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_ok()
                && dx > 0
            {
                dx as f32 / 96.0
            } else {
                1.0
            };
            let name = String::from_utf16_lossy(&mi.szDevice)
                .trim_end_matches('\0')
                .to_string();
            list.push(MonitorInfo {
                id: monitor.0 as usize as u32,
                name,
                x: rect.left,
                y: rect.top,
                width: (rect.right - rect.left).max(0) as u32,
                height: (rect.bottom - rect.top).max(0) as u32,
                primary: (mi.monitorInfo.dwFlags & 1) != 0,
                scale,
            });
        }
        BOOL(1)
    }

    pub fn monitors() -> Vec<MonitorInfo> {
        let mut list: Vec<MonitorInfo> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(enum_cb),
                LPARAM(&mut list as *mut _ as isize),
            );
        }
        if list.is_empty() {
            let w = unsafe { GetSystemMetrics(SM_CXSCREEN) }.max(1) as u32;
            let h = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(1) as u32;
            list.push(MonitorInfo {
                id: 1,
                name: "\\\\.\\DISPLAY1".to_string(),
                x: 0,
                y: 0,
                width: w,
                height: h,
                primary: true,
                scale: 1.0,
            });
        }
        list
    }

    pub struct ScreenCapture {
        screen_dc: HDC,
        mem_dc: HDC,
        bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
        old: HGDIOBJ,
        width: usize,
        height: usize,
        origin_x: i32,
        origin_y: i32,
        buffer: Vec<u8>,
        bmi: BITMAPINFO,
    }

    impl ScreenCapture {
        pub fn new(monitor: &MonitorInfo) -> Result<Self> {
            let width = (monitor.width as usize) & !1;
            let height = (monitor.height as usize) & !1;
            if width < 2 || height < 2 {
                bail!("monitor is too small to capture: {width}x{height}");
            }

            unsafe {
                let screen_dc = CreateDCW(
                    w!("DISPLAY"),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    None,
                );
                if screen_dc.is_invalid() {
                    bail!("CreateDCW(DISPLAY) failed");
                }
                let mem_dc = CreateCompatibleDC(Some(screen_dc));
                if mem_dc.is_invalid() {
                    let _ = DeleteDC(screen_dc);
                    bail!("CreateCompatibleDC failed");
                }
                let bitmap = CreateCompatibleBitmap(screen_dc, width as i32, height as i32);
                if bitmap.is_invalid() {
                    let _ = DeleteDC(mem_dc);
                    let _ = DeleteDC(screen_dc);
                    bail!("CreateCompatibleBitmap failed");
                }
                let old = SelectObject(mem_dc, HGDIOBJ(bitmap.0));

                let mut bmi: BITMAPINFO = std::mem::zeroed();
                bmi.bmiHeader = BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32), // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: (width * height * 4) as u32,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                };

                Ok(Self {
                    screen_dc,
                    mem_dc,
                    bitmap,
                    old,
                    width,
                    height,
                    origin_x: monitor.x,
                    origin_y: monitor.y,
                    buffer: vec![0u8; width * height * 4],
                    bmi,
                })
            }
        }

        pub fn width(&self) -> usize {
            self.width
        }

        pub fn height(&self) -> usize {
            self.height
        }

        /// Capture one frame and return BGRA pixels (top-down).
        pub fn capture(&mut self) -> Result<&[u8]> {
            unsafe {
                let rop = windows::Win32::Graphics::Gdi::ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0);
                BitBlt(
                    self.mem_dc,
                    0,
                    0,
                    self.width as i32,
                    self.height as i32,
                    Some(self.screen_dc),
                    self.origin_x,
                    self.origin_y,
                    rop,
                )
                .context("BitBlt failed")?;
                let _ = GdiFlush();

                let got = GetDIBits(
                    self.mem_dc,
                    self.bitmap,
                    0,
                    self.height as u32,
                    Some(self.buffer.as_mut_ptr() as *mut _),
                    &mut self.bmi,
                    DIB_RGB_COLORS,
                );
                if got == 0 {
                    bail!("GetDIBits failed");
                }
            }
            Ok(&self.buffer)
        }
    }

    impl Drop for ScreenCapture {
        fn drop(&mut self) {
            unsafe {
                let _ = SelectObject(self.mem_dc, self.old);
                let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
                let _ = DeleteDC(self.mem_dc);
                let _ = DeleteDC(self.screen_dc);
            }
        }
    }
}

#[cfg(windows)]
pub use win::{init_dpi_awareness, monitors, ScreenCapture};

#[cfg(not(windows))]
pub fn init_dpi_awareness() {}

#[cfg(not(windows))]
pub fn monitors() -> Vec<MonitorInfo> {
    vec![MonitorInfo {
        id: 1,
        name: "display".into(),
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        primary: true,
        scale: 1.0,
    }]
}

/// Placeholder backend so the protocol, UI, and viewer build on Linux/macOS.
/// A host on these platforms currently ends the session with a clear error.
/// The real backends (PipeWire/Portal, XTest/uinput, ScreenCaptureKit) are M6.
#[cfg(not(windows))]
pub struct ScreenCapture {
    width: usize,
    height: usize,
}

#[cfg(not(windows))]
impl ScreenCapture {
    pub fn new(monitor: &MonitorInfo) -> anyhow::Result<Self> {
        Ok(Self {
            width: (monitor.width as usize) & !1,
            height: (monitor.height as usize) & !1,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn capture(&mut self) -> anyhow::Result<&[u8]> {
        anyhow::bail!("screen capture backend is not implemented on this platform yet (M6)")
    }
}

