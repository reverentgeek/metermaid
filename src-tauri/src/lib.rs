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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (tx, rx) = mpsc::channel::<Command>();

    tauri::Builder::default()
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
