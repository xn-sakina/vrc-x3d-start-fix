use windows::{
    Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW},
    core::PCWSTR,
};

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

pub fn second_instance() {
    let title = wide(&rust_i18n::t!("app.name"));
    let message = wide(&rust_i18n::t!("app.second_instance"));
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

pub fn fatal_startup(details: &str) {
    let title = wide(&rust_i18n::t!("app.name"));
    let message = wide(&format!(
        "{}\n\n{details}",
        rust_i18n::t!("app.fatal_startup")
    ));
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}
