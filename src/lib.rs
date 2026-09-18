rust_i18n::i18n!("locales", fallback = "en");

pub mod config;
pub mod disturbance;
pub mod heuristic;
pub mod i18n;
pub mod lifecycle;
pub mod logging;

#[cfg(target_os = "windows")]
pub mod app;
#[cfg(target_os = "windows")]
pub mod autostart;
#[cfg(target_os = "windows")]
pub mod monitor;
#[cfg(target_os = "windows")]
pub mod platform;
#[cfg(target_os = "windows")]
pub mod supervisor;
#[cfg(target_os = "windows")]
pub mod tray;
