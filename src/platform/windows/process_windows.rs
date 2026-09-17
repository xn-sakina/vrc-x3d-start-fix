use windows::{
    Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::{
            EnumChildWindows, EnumWindows, GetWindowTextLengthW, GetWindowTextW,
            GetWindowThreadProcessId, IsHungAppWindow, IsWindowVisible,
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
}

pub fn inspect_process_windows(target_pid: u32) -> WindowInspection {
    let mut context = ScanContext {
        target_pid,
        inspection: WindowInspection::default(),
    };
    unsafe {
        let _ = EnumWindows(
            Some(enum_top_level),
            LPARAM((&mut context as *mut ScanContext) as isize),
        );
    }
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
        context.inspection.hung_top_level |= unsafe { IsHungAppWindow(hwnd) }.as_bool();
    }
    inspect_text(hwnd, context);
    unsafe {
        let _ = EnumChildWindows(Some(hwnd), Some(enum_child), lparam);
    }
    true.into()
}

unsafe extern "system" fn enum_child(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let context = unsafe { &mut *(lparam.0 as *mut ScanContext) };
    inspect_text(hwnd, context);
    true.into()
}

fn inspect_text(hwnd: HWND, context: &mut ScanContext) {
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

pub fn is_fatal_gc_text(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("fatal error in gc")
        || lower.contains("suspendthread loop failed")
        || (lower.contains("fatal") && lower.contains("gc") && lower.contains("suspendthread"))
}
