use anyhow::{anyhow, Result};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tauri::WebviewWindow;

/// Applies click-through, no-activation and capture-exclusion policies to the answer window.
/// It deliberately returns an error instead of falling back to an unprotected answer surface.
#[cfg(windows)]
pub fn secure_overlay(window: &WebviewWindow) -> Result<()> {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            GetWindowDisplayAffinity, GetWindowLongPtrW, SetWindowDisplayAffinity,
            SetWindowLongPtrW, GWL_EXSTYLE, WINDOW_DISPLAY_AFFINITY, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
        },
    };

    window
        .set_always_on_top(true)
        .map_err(|error| anyhow!(error.to_string()))?;
    window
        .set_focusable(false)
        .map_err(|error| anyhow!(error.to_string()))?;
    // Transparent windows can otherwise receive a compositor shadow/frame,
    // which appears as a second empty border around the answer card.
    window
        .set_shadow(false)
        .map_err(|error| anyhow!(error.to_string()))?;
    // This is the actual hit-test policy; WS_EX_TRANSPARENT alone only changes
    // paint ordering and does not guarantee mouse click-through.
    window
        .set_ignore_cursor_events(true)
        .map_err(|error| anyhow!(error.to_string()))?;

    let handle = window
        .window_handle()
        .map_err(|error| anyhow!(error.to_string()))?;
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return Err(anyhow!("answer overlay is not a Win32 window"));
    };
    let hwnd = HWND(raw.hwnd.get() as *mut _);
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            style
                | WS_EX_TRANSPARENT.0 as isize
                | WS_EX_NOACTIVATE.0 as isize
                | WS_EX_TOOLWINDOW.0 as isize,
        );
        // WDA_EXCLUDEFROMCAPTURE is 0x11 and is supported from Windows 10 2004.
        let exclusion = WINDOW_DISPLAY_AFFINITY(0x11);
        SetWindowDisplayAffinity(hwnd, exclusion)
            .map_err(|error| anyhow!("Windows refused capture exclusion: {error}"))?;
        let mut observed = 0u32;
        GetWindowDisplayAffinity(hwnd, &mut observed)?;
        if observed != exclusion.0 {
            return Err(anyhow!("Windows did not confirm capture exclusion"));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn secure_overlay(_: &WebviewWindow) -> Result<()> {
    Err(anyhow!("the protected answer overlay is Windows-only"))
}

pub fn show(window: &WebviewWindow) -> Result<()> {
    window.show()?;
    Ok(())
}

pub fn hide(window: &WebviewWindow) -> Result<()> {
    window.hide()?;
    Ok(())
}

/// Samples pixels just inside the transparent margins of the overlay. This is
/// intentionally a best-effort visual hint for the follow-background theme;
/// it never feeds screen pixels into Codex.
#[cfg(windows)]
pub fn sample_background(window: &WebviewWindow) -> Result<(u8, u8, u8)> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::HWND,
        Graphics::Gdi::{GetDC, GetPixel, ReleaseDC},
        UI::WindowsAndMessaging::GetWindowRect,
    };
    let handle = window
        .window_handle()
        .map_err(|error| anyhow!(error.to_string()))?;
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return Err(anyhow!("not a Win32 window"));
    };
    let hwnd = HWND(raw.hwnd.get() as *mut _);
    unsafe {
        let mut rect = windows::Win32::Foundation::RECT::default();
        GetWindowRect(hwnd, &mut rect)?;
        let dc = GetDC(HWND(std::ptr::null_mut()));
        if dc.0.is_null() {
            return Err(anyhow!("cannot acquire desktop device context"));
        }
        let width = (rect.right - rect.left).max(1);
        let height = (rect.bottom - rect.top).max(1);
        // Sample a 3x3 grid, not only the four corners. Interior samples make
        // follow-background mode stable on windows with gradients or images.
        let fractions = [0.18_f32, 0.50, 0.82];
        let points: Vec<(i32, i32)> = fractions
            .iter()
            .flat_map(|y| {
                fractions.iter().map(move |x| {
                    (
                        rect.left + (width as f32 * x) as i32,
                        rect.top + (height as f32 * y) as i32,
                    )
                })
            })
            .collect();
        let mut r = 0u32;
        let mut g = 0u32;
        let mut b = 0u32;
        let mut samples = 0u32;
        for (x, y) in points {
            let color = GetPixel(dc, x, y).0;
            if color == u32::MAX {
                continue;
            }
            samples += 1;
            r += color & 0xff;
            g += (color >> 8) & 0xff;
            b += (color >> 16) & 0xff;
        }
        let _ = ReleaseDC(HWND(std::ptr::null_mut()), dc);
        if samples == 0 {
            return Err(anyhow!("desktop returned no sample pixels"));
        }
        Ok((
            (r / samples) as u8,
            (g / samples) as u8,
            (b / samples) as u8,
        ))
    }
}

#[cfg(not(windows))]
pub fn sample_background(_: &WebviewWindow) -> Result<(u8, u8, u8)> {
    Err(anyhow!("background sampling is Windows-only"))
}
