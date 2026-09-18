use anyhow::{Context, Result};
use windows::{
    Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE},
    },
    core::{PCWSTR, w},
};

/// Asks the existing instance to use its normal, bounded shutdown path.
///
/// The hidden window class is intentionally stable across releases, so a newly
/// downloaded executable can hand work over from an older or newer version.
pub fn request_existing_instance_shutdown() -> Result<bool> {
    let Ok(window) = (unsafe { FindWindowW(w!("VRChatX3DStartFixEventWindow"), PCWSTR::null()) })
    else {
        // FindWindowW documents a null result without setting last-error, so
        // absence is a normal state rather than a diagnostic error.
        return Ok(false);
    };

    unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) }
        .context("post shutdown request to existing instance")?;
    Ok(true)
}
