use windows::{
    Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::{
            EnumChildWindows, EnumWindows, GetWindowTextLengthW, GetWindowTextW,
            GetWindowThreadProcessId, IsWindowVisible, SMTO_ABORTIFHUNG, SMTO_BLOCK,
            SMTO_ERRORONEXIT, SendMessageTimeoutW, WM_GETTEXT, WM_NULL,
        },
    },
    core::BOOL,
};

#[derive(Debug, Default)]
pub struct WindowInspection {
    pub visible_top_level: bool,
    pub hung_top_level: bool,
    pub fatal_text: Option<String>,
}

struct ScanContext {
    target_pid: u32,
    inspection: WindowInspection,
    responsive_top_level: bool,
    child_text_budget: usize,
}

pub fn inspect_process_windows(target_pid: u32) -> WindowInspection {
    let mut context = ScanContext {
        target_pid,
        inspection: WindowInspection::default(),
        responsive_top_level: false,
        child_text_budget: 16,
    };
    unsafe {
        let _ = EnumWindows(
            Some(enum_top_level),
            LPARAM((&mut context as *mut ScanContext) as isize),
        );
    }
    context.inspection.hung_top_level =
        context.inspection.visible_top_level && !context.responsive_top_level;
    context.inspection
}

unsafe extern "system" fn enum_top_level(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let context = unsafe { &mut *(lparam.0 as *mut ScanContext) };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid != context.target_pid {
        return true.into();
    }
    if unsafe { IsWindowVisible(hwnd) }.as_bool() {
        context.inspection.visible_top_level = true;
        context.responsive_top_level |= is_window_responsive(hwnd);
    }
    inspect_caption(hwnd, context);
    unsafe {
        let _ = EnumChildWindows(Some(hwnd), Some(enum_child), lparam);
    }
    true.into()
}

unsafe extern "system" fn enum_child(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let context = unsafe { &mut *(lparam.0 as *mut ScanContext) };
    inspect_child_text(hwnd, context);
    true.into()
}

fn inspect_caption(hwnd: HWND, context: &mut ScanContext) {
    if context.inspection.fatal_text.is_some() {
        return;
    }
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return;
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let read = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    if read <= 0 {
        return;
    }
    let text = String::from_utf16_lossy(&buffer[..read as usize]);
    if is_fatal_gc_text(&text) {
        context.inspection.fatal_text = Some(text);
    }
}

fn inspect_child_text(hwnd: HWND, context: &mut ScanContext) {
    if context.inspection.fatal_text.is_some() || context.child_text_budget == 0 {
        return;
    }
    context.child_text_budget -= 1;
    let mut buffer = [0u16; 1024];
    let mut copied = 0usize;
    let delivered = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXT,
            windows::Win32::Foundation::WPARAM(buffer.len()),
            LPARAM(buffer.as_mut_ptr() as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
            25,
            Some(&mut copied),
        )
    };
    if delivered.0 == 0 || copied == 0 {
        return;
    }
    let copied = copied.min(buffer.len().saturating_sub(1));
    let text = String::from_utf16_lossy(&buffer[..copied]);
    if is_fatal_gc_text(&text) {
        context.inspection.fatal_text = Some(text);
    }
}

fn is_window_responsive(hwnd: HWND) -> bool {
    unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_NULL,
            windows::Win32::Foundation::WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
            100,
            None,
        )
        .0 != 0
    }
}

pub fn is_fatal_gc_text(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("fatal error in gc")
        || lower.contains("suspendthread loop failed")
        || (lower.contains("fatal") && lower.contains("gc") && lower.contains("suspendthread"))
}
