#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(all(not(debug_assertions), not(feature = "custom-protocol")))]
compile_error!("Release desktop builds require --features custom-protocol. Use scripts/Publish-RustPortable.ps1 or tauri build --no-bundle.");

fn main() {
    better_apex_lab::desktop::run();
}
