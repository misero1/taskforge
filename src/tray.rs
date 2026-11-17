//! System tray icon (Windows).
//!
//! Runs on its own thread with its own Win32 message loop, so it keeps
//! working while the main window is hidden — egui stops calling `update` for
//! hidden windows, which is also why rules and alerts live on the sampler.
//!
//! The icon is redrawn every second as two live bars (CPU and GPU, or memory
//! when there is no GPU) and the tooltip shows the current numbers.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, IsIconic, MsgWaitForMultipleObjects, PeekMessageW, PostMessageW,
    SetForegroundWindow, ShowWindow, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT, SW_RESTORE,
    SW_SHOW, WM_CLOSE,
};

use crate::monitor::Shared;

/// Something the UI should do once the window is visible again.
#[derive(Clone, Debug, PartialEq)]
pub enum TrayCmd {
    Mini,
    Health,
}

#[derive(Clone)]
pub struct Tray {
    /// Main window handle (set by the UI on its first frame).
    pub hwnd: Arc<AtomicIsize>,
    /// The user chose Quit from the tray menu.
    pub quit: Arc<AtomicBool>,
    pub pending: Arc<Mutex<Vec<TrayCmd>>>,
}

impl Tray {
    pub fn start(
        shared: Arc<Mutex<Shared>>,
        paused: Arc<AtomicBool>,
        ctx: eframe::egui::Context,
    ) -> Tray {
        let tray = Tray {
            hwnd: Arc::new(AtomicIsize::new(0)),
            quit: Arc::new(AtomicBool::new(false)),
            pending: Arc::new(Mutex::new(Vec::new())),
        };
        let t = tray.clone();
        let spawned = std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || {
                if let Err(e) = run(t, shared, paused, ctx) {
                    crate::diag::warn(&format!("Tray icon unavailable: {e}"));
                }
            });
        if let Err(e) = spawned {
            crate::diag::warn(&format!("Could not start tray thread: {e}"));
        }
        tray
    }

    fn window(&self) -> Option<HWND> {
        let h = self.hwnd.load(Ordering::Relaxed);
        (h != 0).then_some(HWND(h as *mut _))
    }

    /// Bring the main window back from the tray / minimised state.
    pub fn show_window(&self) {
        if let Some(h) = self.window() {
            // SAFETY: plain calls on our own top-level window handle.
            unsafe {
                if IsIconic(h).as_bool() {
                    let _ = ShowWindow(h, SW_RESTORE);
                } else {
                    let _ = ShowWindow(h, SW_SHOW);
                }
                let _ = SetForegroundWindow(h);
            }
        }
    }
}

/// 32×32 RGBA icon: rounded dark tile with two live bars.
pub fn render_icon(a: f32, b: f32, b_is_gpu: bool) -> Vec<u8> {
    const N: usize = 32;
    let mut px = vec![0u8; N * N * 4];
    let mut put = |x: usize, y: usize, c: [u8; 4]| {
        let i = (y * N + x) * 4;
        px[i..i + 4].copy_from_slice(&c);
    };
    let r = 6.0f32;
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let cx = fx.clamp(r, N as f32 - r);
            let cy = fy.clamp(r, N as f32 - r);
            if (fx - cx).powi(2) + (fy - cy).powi(2) <= r * r {
                put(x, y, [22, 26, 34, 255]);
            }
        }
    }
    let bar = |frac: f32, x0: usize, col: [u8; 4], put: &mut dyn FnMut(usize, usize, [u8; 4])| {
        let (top, bottom) = (5usize, 27usize);
        let h = ((bottom - top) as f32 * frac.clamp(0.0, 1.0)).round() as usize;
        for y in top..bottom {
            for x in x0..x0 + 9 {
                let filled = y >= bottom - h.max(1);
                put(x, y, if filled { col } else { [48, 54, 66, 255] });
            }
        }
    };
    bar(a / 100.0, 5, [76, 141, 255, 255], &mut put);
    bar(
        b / 100.0,
        18,
        if b_is_gpu {
            [140, 214, 80, 255]
        } else {
            [170, 120, 255, 255]
        },
        &mut put,
    );
    px
}

fn run(
    tray: Tray,
    shared: Arc<Mutex<Shared>>,
    paused: Arc<AtomicBool>,
    ctx: eframe::egui::Context,
) -> Result<(), String> {
    let open = MenuItem::new("Open TaskForge", true, None);
    let mini = MenuItem::new("Mini monitor", true, None);
    let health = MenuItem::new("Health check", true, None);
    let end_top = MenuItem::new("End top CPU user", true, None);
    let pause = MenuItem::new("Pause updates", true, None);
    let quit = MenuItem::new("Quit TaskForge", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &open,
        &mini,
        &health,
        &PredefinedMenuItem::separator(),
        &end_top,
        &pause,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|e| e.to_string())?;

    let icon = Icon::from_rgba(render_icon(0.0, 0.0, true), 32, 32).map_err(|e| e.to_string())?;
    let tray_icon = TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("TaskForge")
        .with_menu(Box::new(menu))
        .build()
        .map_err(|e| e.to_string())?;
    crate::diag::info("Tray icon started");

    let mut top: Option<(u32, String)> = None;
    // Last drawn bar heights / tooltip, so the icon is only rebuilt when it changes.
    let mut drawn: Option<(u8, u8, bool)> = None;
    let mut last_tip = String::new();
    let mut last_draw = Instant::now() - Duration::from_secs(5);
    let mut msg = MSG::default();
    loop {
        // SAFETY: standard message pump for windows created on this thread (the tray's hidden window).
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            match ev {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
                | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => {
                    tray.show_window();
                    ctx.request_repaint();
                }
                _ => {}
            }
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let id = ev.id;
            if id == *open.id() {
                tray.show_window();
            } else if id == *mini.id() || id == *health.id() {
                if let Ok(mut p) = tray.pending.lock() {
                    p.push(if id == *mini.id() {
                        TrayCmd::Mini
                    } else {
                        TrayCmd::Health
                    });
                }
                tray.show_window();
            } else if id == *pause.id() {
                let now = !paused.load(Ordering::Relaxed);
                paused.store(now, Ordering::Relaxed);
                pause.set_text(if now {
                    "Resume updates"
                } else {
                    "Pause updates"
                });
            } else if id == *end_top.id() {
                if let Some((pid, name)) = top
                    .clone()
                    .filter(|(_, n)| !crate::app::App::is_critical(n))
                {
                    match crate::platform::kill(pid, false) {
                        Ok(_) => crate::diag::info(&format!("Tray: ended {name} ({pid})")),
                        Err(e) => crate::diag::warn(&format!("Tray: could not end {name}: {e}")),
                    }
                }
            } else if id == *quit.id() {
                tray.quit.store(true, Ordering::Relaxed);
                tray.show_window();
                if let Some(h) = tray.window() {
                    // SAFETY: posting WM_CLOSE to our own window; the UI sees `quit` and exits.
                    unsafe {
                        let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                }
                ctx.request_repaint();
                return Ok(());
            }
            ctx.request_repaint();
        }

        if last_draw.elapsed() >= Duration::from_secs(1) {
            last_draw = Instant::now();
            let snap = shared.lock().map(|g| g.snap.clone()).ok();
            if let Some(s) = snap {
                let has_gpu = !s.gpus.is_empty();
                let second = if has_gpu { s.gpu_util() } else { s.mem_pct() };
                // Bars are 22 px tall, so only redraw when a bar moves by a pixel.
                let key = (
                    (s.cpu_total / 100.0 * 22.0).round() as u8,
                    (second / 100.0 * 22.0).round() as u8,
                    has_gpu,
                );
                if drawn != Some(key) {
                    drawn = Some(key);
                    if let Ok(icon) =
                        Icon::from_rgba(render_icon(s.cpu_total, second, has_gpu), 32, 32)
                    {
                        let _ = tray_icon.set_icon(Some(icon));
                    }
                }
                let mut tip = format!(
                    "TaskForge\nCPU {:.0}%  ·  Memory {:.0}%",
                    s.cpu_total,
                    s.mem_pct()
                );
                if has_gpu {
                    tip.push_str(&format!("  ·  GPU {:.0}%", s.gpu_util()));
                }
                if tip != last_tip {
                    let _ = tray_icon.set_tooltip(Some(&tip));
                    last_tip = tip;
                }
                top = s
                    .procs
                    .iter()
                    .filter(|p| p.pid > 4 && p.pid != std::process::id())
                    .max_by(|a, b| a.cpu.total_cmp(&b.cpu))
                    .map(|p| (p.pid, p.name.to_string()));
                match &top {
                    Some((_, name)) => {
                        let cpu = s
                            .procs
                            .iter()
                            .find(|p| Some(p.pid) == top.as_ref().map(|t| t.0))
                            .map(|p| p.cpu)
                            .unwrap_or(0.0);
                        end_top.set_text(format!("End {name} ({cpu:.0}% CPU)"));
                        end_top.set_enabled(true);
                    }
                    None => {
                        end_top.set_text("End top CPU user");
                        end_top.set_enabled(false);
                    }
                }
            }
        }
        // Sleep until a window message (tray click / menu) arrives or the next
        // icon refresh is due — no busy polling.
        let wait = Duration::from_secs(1)
            .saturating_sub(last_draw.elapsed())
            .as_millis()
            .max(1) as u32;
        // SAFETY: no handles are passed; this only waits on this thread's message queue.
        unsafe {
            MsgWaitForMultipleObjects(None, false, wait, QS_ALLINPUT);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_size() {
        assert_eq!(super::render_icon(50.0, 100.0, true).len(), 32 * 32 * 4);
    }
}
