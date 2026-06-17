mod audio;

use std::sync::mpsc::{self, Sender, SyncSender};

use audio::{Command, DeviceConfig, DeviceInfo, StreamInfo};
use tauri::State;

/// Tauri-managed application state: a channel to the audio engine thread.
struct AppState {
    tx: Sender<Command>,
}

#[tauri::command]
fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    audio::list_input_devices()
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (tx, rx) = mpsc::channel::<Command>();

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(window_guard_plugin())
        .manage(AppState { tx })
        .setup(move |app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || audio::engine_loop(rx, handle));
            Ok(())
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
