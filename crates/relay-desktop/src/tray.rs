use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone)]
pub enum TrayAction {
    Show,
    Quit,
}

#[cfg(not(windows))]
pub struct Tray;

#[cfg(not(windows))]
pub fn start(_sender: UnboundedSender<crate::Signal>) -> Result<Tray, String> {
    Err("此平台未启用托盘；关闭窗口会退出服务。".into())
}

#[cfg(windows)]
pub struct Tray {
    thread_id: u32,
}

#[cfg(windows)]
impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            // The thread owns all tray handles and destroys them after quitting
            // its own Windows message loop.
            windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                self.thread_id,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_QUIT,
                0,
                0,
            );
        }
    }
}

#[cfg(windows)]
pub fn start(sender: UnboundedSender<crate::Signal>) -> Result<Tray, String> {
    use tray_icon::{
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
        Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
    };
    use windows_sys::Win32::{
        System::Threading::GetCurrentThreadId,
        UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG},
    };
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("relay-tray".into())
        .spawn(move || {
            let result = (|| -> Result<_, String> {
                let menu = Menu::new();
                let show = MenuItem::new("打开随手传", true, None);
                let quit = MenuItem::new("退出并停止服务", true, None);
                menu.append_items(&[&show, &PredefinedMenuItem::separator(), &quit])
                    .map_err(|e| e.to_string())?;
                let show_id = show.id().clone();
                let quit_id = quit.id().clone();
                let menu_sender = sender.clone();
                MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                    let action = if event.id == show_id {
                        Some(TrayAction::Show)
                    } else if event.id == quit_id {
                        Some(TrayAction::Quit)
                    } else {
                        None
                    };
                    if let Some(action) = action {
                        let _ = menu_sender.send(crate::Signal::Tray(action));
                    }
                }));
                TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        let _ = sender.send(crate::Signal::Tray(TrayAction::Show));
                    }
                }));
                let icon = Icon::from_rgba(icon_pixels(), 32, 32).map_err(|e| e.to_string())?;
                TrayIconBuilder::new()
                    .with_icon(icon)
                    .with_menu(Box::new(menu))
                    .with_tooltip("随手传 · 局域网服务正在运行")
                    .with_menu_on_left_click(false)
                    .build()
                    .map_err(|e| e.to_string())
            })();
            match result {
                Ok(icon) => {
                    let thread_id = unsafe { GetCurrentThreadId() };
                    let _ = ready_tx.send(Ok(thread_id));
                    let mut message: MSG = unsafe { std::mem::zeroed() };
                    unsafe {
                        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                            TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                    }
                    drop(icon);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| format!("托盘启动失败：{e}"))?
        .map(|thread_id| Tray { thread_id })
}

#[cfg(windows)]
fn icon_pixels() -> Vec<u8> {
    let mut pixels = vec![0; 32 * 32 * 4];
    for y in 0..32usize {
        for x in 0..32usize {
            let rounded = (4..28).contains(&x)
                || (4..28).contains(&y)
                || ((x as f32 - 15.5).powi(2) + (y as f32 - 15.5).powi(2) < 18.0f32.powi(2));
            let arrow = ((8..24).contains(&x) && (9..12).contains(&y))
                || ((8..24).contains(&x) && (20..23).contains(&y))
                || ((19..26).contains(&x)
                    && (6..15).contains(&y)
                    && (x as i32 - 19) >= (y as i32 - 10).abs())
                || ((6..13).contains(&x)
                    && (17..26).contains(&y)
                    && (12 - x as i32) >= (y as i32 - 21).abs());
            let color = if arrow {
                [255, 255, 255, 255]
            } else if rounded {
                [255, 106, 0, 255]
            } else {
                [0, 0, 0, 0]
            };
            pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4].copy_from_slice(&color);
        }
    }
    pixels
}
