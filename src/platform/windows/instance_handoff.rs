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
pub fn request_existing_instance_shutdown() -> bool {
    let Ok(window) = (unsafe { FindWindowW(w!("VRChatX3DStartFixEventWindow"), PCWSTR::null()) })
    else {
        return false;
    };

    unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok() }
}
