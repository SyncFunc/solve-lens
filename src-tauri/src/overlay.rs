use anyhow::{anyhow, Result};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tauri::WebviewWindow;

/// Controls whether mouse events in the answer window reach the window below.
/// Mouse shortcuts are observed by a separate low-level hook and never consume
/// the original input.
#[cfg(windows)]
pub fn set_mouse_passthrough(window: &WebviewWindow, passthrough: bool) -> Result<()> {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, SWP_FRAMECHANGED,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
        },
    };

    window
        .set_focusable(false)
        .map_err(|error| anyhow!(error.to_string()))?;
    window
        .set_ignore_cursor_events(passthrough)
        .map_err(|error| anyhow!(error.to_string()))?;

    let handle = window
        .window_handle()
        .map_err(|error| anyhow!(error.to_string()))?;
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return Err(anyhow!("answer overlay is not a Win32 window"));
    };
    let hwnd = HWND(raw.hwnd.get() as *mut _);
    unsafe {
        let mut style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        style |= WS_EX_TOOLWINDOW.0 as isize;
        style |= WS_EX_NOACTIVATE.0 as isize;
        let transparent = WS_EX_TRANSPARENT.0 as isize;
        if passthrough {
            style |= transparent;
        } else {
            style &= !transparent;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn set_mouse_passthrough(_: &WebviewWindow, _: bool) -> Result<()> {
    Err(anyhow!("interactive answer overlay is Windows-only"))
}

#[derive(Clone, Copy)]
pub enum MouseShortcutEvent {
    LeftDown { x: i32, y: i32 },
    LeftUp { x: i32, y: i32 },
    MouseMove { x: i32, y: i32 },
    RightClick { x: i32, y: i32 },
    Wheel { delta: i16, x: i32, y: i32 },
}

/// Observes desktop-wide mouse shortcuts and left-button gestures while the
/// overlay is visible. The hook always forwards the original input.
#[cfg(windows)]
pub fn start_mouse_shortcut_listener<F>(window: &WebviewWindow, on_event: F) -> Result<()>
where
    F: Fn(MouseShortcutEvent) + Send + Sync + 'static,
{
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, OnceLock,
        },
        thread,
        time::Duration,
    };
    use windows::Win32::{
        Foundation::{HINSTANCE, HWND},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, IsWindowVisible, SetWindowsHookExW, TranslateMessage,
            UnhookWindowsHookEx, HHOOK, MSG, WH_MOUSE_LL,
        },
    };

    #[derive(Clone, Copy)]
    struct MouseInput {
        event: MouseShortcutEvent,
    }

    static MOUSE_INPUT_SENDER: OnceLock<mpsc::Sender<MouseInput>> = OnceLock::new();

    unsafe extern "system" fn mouse_hook_proc(
        code: i32,
        wparam: windows::Win32::Foundation::WPARAM,
        lparam: windows::Win32::Foundation::LPARAM,
    ) -> windows::Win32::Foundation::LRESULT {
        use windows::Win32::{
            Foundation::{LPARAM, WPARAM},
            UI::WindowsAndMessaging::{
                CallNextHookEx, HC_ACTION, MSLLHOOKSTRUCT, WM_LBUTTONDOWN, WM_LBUTTONUP,
                WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
            },
        };

        static LEFT_BUTTON_PRESSED: AtomicBool = AtomicBool::new(false);

        if code >= HC_ACTION as i32 {
            let message = wparam.0 as u32;
            let mouse = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            let event = if message == WM_LBUTTONDOWN {
                LEFT_BUTTON_PRESSED.store(true, Ordering::Relaxed);
                Some(MouseShortcutEvent::LeftDown {
                    x: mouse.pt.x,
                    y: mouse.pt.y,
                })
            } else if message == WM_LBUTTONUP {
                LEFT_BUTTON_PRESSED.store(false, Ordering::Relaxed);
                Some(MouseShortcutEvent::LeftUp {
                    x: mouse.pt.x,
                    y: mouse.pt.y,
                })
            } else if message == WM_MOUSEMOVE && LEFT_BUTTON_PRESSED.load(Ordering::Relaxed) {
                Some(MouseShortcutEvent::MouseMove {
                    x: mouse.pt.x,
                    y: mouse.pt.y,
                })
            } else if message == WM_RBUTTONDOWN {
                Some(MouseShortcutEvent::RightClick {
                    x: mouse.pt.x,
                    y: mouse.pt.y,
                })
            } else if message == WM_MOUSEWHEEL {
                let delta = ((mouse.mouseData >> 16) as u16) as i16;
                (delta != 0).then_some(MouseShortcutEvent::Wheel {
                    delta,
                    x: mouse.pt.x,
                    y: mouse.pt.y,
                })
            } else {
                None
            };
            if let (Some(event), Some(sender)) = (event, MOUSE_INPUT_SENDER.get()) {
                let _ = sender.send(MouseInput { event });
            }
        }
        unsafe { CallNextHookEx(HHOOK::default(), code, WPARAM(wparam.0), LPARAM(lparam.0)) }
    }

    let handle = window
        .window_handle()
        .map_err(|error| anyhow!(error.to_string()))?;
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return Err(anyhow!("answer overlay is not a Win32 window"));
    };
    let overlay_hwnd = raw.hwnd.get() as usize;
    let module = unsafe { GetModuleHandleW(windows::core::PCWSTR::null()) }
        .map_err(|error| anyhow!(error.to_string()))?;
    let module_handle = module.0 as usize;
    let (sender, receiver) = mpsc::channel::<MouseInput>();
    MOUSE_INPUT_SENDER
        .set(sender)
        .map_err(|_| anyhow!("mouse shortcut listener is already running"))?;

    let (ready_sender, ready_receiver) = mpsc::sync_channel::<std::result::Result<(), String>>(1);
    thread::Builder::new()
        .name("overlay-mouse-hook".into())
        .spawn(move || unsafe {
            let hook = match SetWindowsHookExW(
                WH_MOUSE_LL,
                Some(mouse_hook_proc),
                HINSTANCE(module_handle as *mut _),
                0,
            ) {
                Ok(hook) => hook,
                Err(error) => {
                    let _ = ready_sender.send(Err(error.to_string()));
                    return;
                }
            };
            let _ = ready_sender.send(Ok(()));
            let mut message = MSG::default();
            loop {
                let result = GetMessageW(&mut message, HWND::default(), 0, 0);
                if result.0 <= 0 {
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let _ = UnhookWindowsHookEx(hook);
        })?;
    match ready_receiver.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(())) => {
            thread::Builder::new()
                .name("overlay-mouse-dispatch".into())
                .spawn(move || {
                    let hwnd = HWND(overlay_hwnd as *mut _);
                    while let Ok(input) = receiver.recv() {
                        if unsafe { IsWindowVisible(hwnd).as_bool() } {
                            on_event(input.event);
                        }
                    }
                })?;
            Ok(())
        }
        Ok(Err(error)) => Err(anyhow!("cannot install mouse shortcut hook: {error}")),
        Err(error) => Err(anyhow!("mouse shortcut hook startup timed out: {error}")),
    }
}

#[cfg(not(windows))]
pub fn start_mouse_shortcut_listener<F>(_: &WebviewWindow, _: F) -> Result<()>
where
    F: Fn(MouseShortcutEvent) + Send + Sync + 'static,
{
    Err(anyhow!("mouse shortcut listener is Windows-only"))
}

/// Applies mouse and capture-exclusion policies to the answer window.
/// It deliberately returns an error instead of falling back to an unprotected answer surface.
#[cfg(windows)]
pub fn secure_overlay(window: &WebviewWindow, mouse_passthrough: bool) -> Result<()> {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            GetWindowDisplayAffinity, SetWindowDisplayAffinity, WINDOW_DISPLAY_AFFINITY,
        },
    };

    window
        .set_always_on_top(true)
        .map_err(|error| anyhow!(error.to_string()))?;
    // Transparent windows can otherwise receive a compositor shadow/frame,
    // which appears as a second empty border around the answer card.
    window
        .set_shadow(false)
        .map_err(|error| anyhow!(error.to_string()))?;
    set_mouse_passthrough(window, mouse_passthrough)?;

    let handle = window
        .window_handle()
        .map_err(|error| anyhow!(error.to_string()))?;
    let RawWindowHandle::Win32(raw) = handle.as_raw() else {
        return Err(anyhow!("answer overlay is not a Win32 window"));
    };
    let hwnd = HWND(raw.hwnd.get() as *mut _);
    unsafe {
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
pub fn secure_overlay(_: &WebviewWindow, _: bool) -> Result<()> {
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
