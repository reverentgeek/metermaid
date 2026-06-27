mod audio;

use std::sync::mpsc::{self, Sender, SyncSender};

use audio::{Command, DeviceConfig, DeviceInfo, StreamInfo};
use tauri::{Emitter, State};

/// Tauri-managed application state: a channel to the audio engine thread.
struct AppState {
    tx: Sender<Command>,
}

#[tauri::command]
fn list_devices(include_asio: bool) -> Result<Vec<DeviceInfo>, String> {
    audio::list_input_devices(include_asio)
}

#[tauri::command]
fn get_device_config(device: Option<String>) -> Result<DeviceConfig, String> {
    audio::device_config(device)
}

#[tauri::command]
fn start_capture(
    device: Option<String>,
    sample_rate: Option<u32>,
    channels: Vec<u32>,
    state: State<AppState>,
) -> Result<StreamInfo, String> {
    let (reply, rx): (SyncSender<Result<StreamInfo, String>>, _) = mpsc::sync_channel(1);
    state
        .tx
        .send(Command::Start {
            device,
            sample_rate,
            channels,
            reply,
        })
        .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())?
}

#[tauri::command]
fn stop_capture(state: State<AppState>) -> Result<(), String> {
    let (reply, rx): (SyncSender<()>, _) = mpsc::sync_channel(1);
    state
        .tx
        .send(Command::Stop { reply })
        .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())
}

#[tauri::command]
fn reset_integrated(state: State<AppState>) -> Result<(), String> {
    state.tx.send(Command::Reset).map_err(|e| e.to_string())
}

/// After `tauri-plugin-window-state` restores the saved geometry, make sure the
/// window is still usable: if the monitor it was last on is gone (e.g. an
/// external display was unplugged) the restored position can land entirely
/// off-screen. Nudge the window back so it overlaps an available monitor by at
/// least `MIN_VISIBLE` physical pixels on each axis; otherwise recenter it on
/// the primary monitor.
fn ensure_window_on_screen<R: tauri::Runtime>(window: &tauri::Window<R>) {
    use tauri::{PhysicalPosition, PhysicalSize};

    const MIN_VISIBLE: i32 = 80;

    let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) else {
        return;
    };
    let Ok(monitors) = window.available_monitors() else {
        return;
    };
    if monitors.is_empty() {
        return;
    }

    let win = (pos.x, pos.y, size.width as i32, size.height as i32);
    let overlaps = |m: &tauri::Monitor| -> bool {
        let mp = m.position();
        let ms = m.size();
        let (mx, my, mw, mh) = (mp.x, mp.y, ms.width as i32, ms.height as i32);
        let ox = (win.0 + win.2).min(mx + mw) - win.0.max(mx);
        let oy = (win.1 + win.3).min(my + mh) - win.1.max(my);
        ox >= MIN_VISIBLE.min(win.2) && oy >= MIN_VISIBLE.min(win.3)
    };

    if monitors.iter().any(overlaps) {
        return;
    }

    // Off-screen: recenter on the primary monitor (falling back to the first).
    let target = window
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| monitors.into_iter().next());
    if let Some(m) = target {
        let mp = m.position();
        let ms = m.size();
        let x = mp.x + ((ms.width as i32 - win.2) / 2).max(0);
        let y = mp.y + ((ms.height as i32 - win.3) / 2).max(0);
        let _ = window.set_position(PhysicalPosition::new(x, y));
        // Also clamp the size so an oversized restore fits the target monitor.
        let w = (win.2 as u32).min(ms.width);
        let h = (win.3 as u32).min(ms.height);
        if w != size.width || h != size.height {
            let _ = window.set_size(PhysicalSize::new(w, h));
        }
    }
}

/// A tiny plugin whose only job is to run [`ensure_window_on_screen`] in
/// `on_window_ready`. It must be registered *after* `tauri-plugin-window-state`
/// so its callback fires after that plugin has restored the saved geometry —
/// otherwise we'd be correcting the window's default position, not the restored
/// one.
fn window_guard_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("metermaid-window-guard")
        .on_window_ready(|window| {
            if window.label() == "main" {
                ensure_window_on_screen(&window);
            }
        })
        .build()
}

/// Build the desktop application menu, replacing Tauri's default. **About
/// MeterMaid** and **Check for Updates…** are custom items that signal the
/// frontend (via the `menu-about` / `menu-check-updates` events) — About opens
/// an in-app dialog (centered, with clickable links) rather than the native
/// panel, which can't be centered or hyperlinked.
///
/// On macOS these live in the application menu (alongside the standard Edit and
/// Window submenus). Windows and Linux have no app menu, so they get a window
/// menu bar with a single **Help** submenu carrying the same two items.
#[cfg(desktop)]
fn build_app_menu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};

    let about = MenuItemBuilder::with_id("about", "About MeterMaid").build(app)?;
    let check_updates =
        MenuItemBuilder::with_id("check-for-updates", "Check for Updates…").build(app)?;

    #[cfg(target_os = "macos")]
    {
        let app_menu = SubmenuBuilder::new(app, "MeterMaid")
            .item(&about)
            .separator()
            .item(&check_updates)
            .separator()
            .services()
            .separator()
            .hide()
            .hide_others()
            .show_all()
            .separator()
            .quit()
            .build()?;

        let edit_menu = SubmenuBuilder::new(app, "Edit")
            .undo()
            .redo()
            .separator()
            .cut()
            .copy()
            .paste()
            .select_all()
            .build()?;

        let window_menu = SubmenuBuilder::new(app, "Window")
            .minimize()
            .maximize()
            .fullscreen()
            .separator()
            .close_window()
            .build()?;

        MenuBuilder::new(app)
            .items(&[&app_menu, &edit_menu, &window_menu])
            .build()
    }

    #[cfg(not(target_os = "macos"))]
    {
        let help_menu = SubmenuBuilder::new(app, "Help")
            .item(&about)
            .item(&check_updates)
            .build()?;

        MenuBuilder::new(app).item(&help_menu).build()
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (tx, rx) = mpsc::channel::<Command>();

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(window_guard_plugin());

    // Desktop-only: the self-updater (`relaunch` after install comes from the
    // process plugin above, registered on every platform) and the app menu. The
    // menu is set here, before any window is created, so the Windows/Linux menu
    // bar attaches to the window rather than missing the initial creation.
    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .menu(build_app_menu);
    }

    builder
        .manage(AppState { tx })
        .setup(move |app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || audio::engine_loop(rx, handle));
            Ok(())
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            // "Check for Updates…" reuses the frontend's updater flow (same banner
            // and feedback as the in-app button); "About" opens the in-app dialog.
            "check-for-updates" => {
                let _ = app.emit("menu-check-updates", ());
            }
            "about" => {
                let _ = app.emit("menu-about", ());
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            list_devices,
            get_device_config,
            start_capture,
            stop_capture,
            reset_integrated
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
