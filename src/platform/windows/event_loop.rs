use std::{mem::MaybeUninit, sync::OnceLock};

use anyhow::{Context, Result, anyhow};
use crossbeam_channel::Sender;
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
            HWND_MESSAGE, KillTimer, MSG, PostQuitMessage, RegisterClassW, SetTimer,
            TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_DESTROY, WM_ENDSESSION,
            WM_QUERYENDSESSION, WNDCLASSW,
        },
    },
    core::w,
};

use crate::supervisor::SupervisorCommand;

static SHUTDOWN_SENDER: OnceLock<Sender<SupervisorCommand>> = OnceLock::new();

pub struct NativeEventLoop {
    hwnd: HWND,
}

impl NativeEventLoop {
    pub fn new(shutdown_sender: Sender<SupervisorCommand>) -> Result<Self> {
        let _ = SHUTDOWN_SENDER.set(shutdown_sender);
        let module = unsafe { GetModuleHandleW(None) }.context("get current module")?;
        let instance = HINSTANCE(module.0);
        let class = w!("VRChatX3DStartFixEventWindow");
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        let atom = unsafe { RegisterClassW(&window_class) };
        if atom == 0 {
            return Err(anyhow!("failed to register hidden event window class"));
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                w!("VRChat X3D Start Fix"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance),
                None,
            )
        }
        .context("create hidden event window")?;
        let timer = unsafe { SetTimer(Some(hwnd), 1, 50, None) };
        if timer == 0 {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Err(anyhow!("failed to start hidden event window timer"));
        }
        Ok(Self { hwnd })
    }

    pub fn next_message(&self) -> Result<bool> {
        let mut message = MaybeUninit::<MSG>::zeroed();
        let result = unsafe { GetMessageW(message.as_mut_ptr(), None, 0, 0) };
        if result.0 == -1 {
            return Err(anyhow!("GetMessageW failed"));
        }
        if !result.as_bool() {
            return Ok(false);
        }
        let message = unsafe { message.assume_init() };
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        Ok(true)
    }
}

impl Drop for NativeEventLoop {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), 1);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_QUERYENDSESSION => {
            if let Some(sender) = SHUTDOWN_SENDER.get() {
                let _ = sender.try_send(SupervisorCommand::Shutdown);
            }
            LRESULT(1)
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                if let Some(sender) = SHUTDOWN_SENDER.get() {
                    let _ = sender.try_send(SupervisorCommand::Shutdown);
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}
