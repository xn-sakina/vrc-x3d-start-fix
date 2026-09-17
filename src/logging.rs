use std::{fs, io, path::PathBuf};

use directories::ProjectDirs;
use file_rotate::{ContentLimit, FileRotate, compression::Compression, suffix::AppendCount};
use thiserror::Error;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::util::SubscriberInitExt;

pub const LOG_FILE_NAME: &str = "vrchat-x3d-start-fix.jsonl";

pub struct AppGuards {
    _log_guard: WorkerGuard,
    pub log_dir: PathBuf,
}

#[derive(Debug, Error)]
pub enum LoggingError {
    #[error("log directory is unavailable")]
    NoProjectDirectory,
    #[error("failed to create log directory: {0}")]
    Io(#[from] io::Error),
    #[error("failed to install logging subscriber: {0}")]
    Subscriber(String),
}

pub fn initialize() -> Result<AppGuards, LoggingError> {
    let dirs = ProjectDirs::from("com", "local", "VRChatX3DStartFix")
        .ok_or(LoggingError::NoProjectDirectory)?;
    let log_dir = dirs.cache_dir().join("logs");
    fs::create_dir_all(&log_dir)?;
    let writer = FileRotate::new(
        log_dir.join(LOG_FILE_NAME),
        AppendCount::new(10),
        ContentLimit::BytesSurpassed(5 * 1024 * 1024),
        Compression::None,
        None,
    );
    let (non_blocking, guard) = tracing_appender::non_blocking(writer);
    tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(non_blocking)
        .finish()
        .try_init()
        .map_err(|error| LoggingError::Subscriber(error.to_string()))?;
    Ok(AppGuards {
        _log_guard: guard,
        log_dir,
    })
}
