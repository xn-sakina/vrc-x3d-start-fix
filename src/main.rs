#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "windows")]
fn main() {
    if let Err(error) = vrchat_x3d_start_fix::app::run() {
        vrchat_x3d_start_fix::platform::windows::message_box::fatal_startup(&error.to_string());
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!(
        "vrchat-x3d-start-fix is a Windows-only application; run `just test` or `just build` on macOS."
    );
}
