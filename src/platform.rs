use anyhow::{Context, Result, bail};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::{
    Foundation::{HWND, POINT, RECT},
    UI::WindowsAndMessaging::{
        GetCursorPos, GetSystemMetrics, GetWindowRect, SM_CXSCREEN, SetWindowDisplayAffinity,
        WDA_EXCLUDEFROMCAPTURE,
    },
};

pub fn exclude_from_capture(window: &impl HasWindowHandle) -> Result<HWND> {
    let handle = window
        .window_handle()
        .context("Janela nativa indisponível")?;
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        bail!("Este aplicativo requer Windows");
    };
    let hwnd = HWND(raw.hwnd.get() as *mut _);
    // The HWND belongs to this process and remains owned by eframe.
    unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }
        .context("O Windows não conseguiu ocultar os controles da captura")?;
    Ok(hwnd)
}

pub fn screen_width() -> f32 {
    unsafe { GetSystemMetrics(SM_CXSCREEN) as f32 }
}

pub fn hovered(hwnd: HWND) -> bool {
    let mut cursor = POINT::default();
    let mut rect = RECT::default();
    unsafe {
        if GetCursorPos(&mut cursor).is_err() || GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }
    }
    cursor.x >= rect.left && cursor.x < rect.right && cursor.y >= rect.top && cursor.y < rect.bottom
}
