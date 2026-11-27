//! esp32sim desktop app (see lib.rs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let target = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    esp32sim_desktop::builder(tauri::Builder::default(), target)
        .build(tauri::generate_context!())
        .expect("error while starting esp32sim")
        .run(esp32sim_desktop::on_run_event);
}
