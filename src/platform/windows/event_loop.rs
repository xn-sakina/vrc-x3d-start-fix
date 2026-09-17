use std::{
    mem::MaybeUninit,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use crossbeam_channel::{Receiver, Sender};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
            KillTimer, MSG, PostQuitMessage, RegisterClassW, SetTimer, TranslateMessage, WM_CLOSE,
            WM_DESTROY, WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW, WS_EX_TOOLWINDOW,
            WS_OVERLAPPED,
        },
    },
    core::w,
};

use crate::supervisor::SupervisorCommand;

struct ShutdownBridge {
    command_tx: Sender<SupervisorCommand>,
    done_rx: Receiver<()>,
    system_shutdown_requested: AtomicBool,
}

static SHUTDOWN_BRIDGE: OnceLock<ShutdownBridge> = OnceLock::new();

pub struct NativeEventLoop {
    hwnd: HWND,
}

impl NativeEventLoop {
    pub fn new(command_tx: Sender<SupervisorCommand>, done_rx: Receiver<()>) -> Result<Self> {
        if SHUTDOWN_BRIDGE.get().is_some() {
            return Err(anyhow!("shutdown bridge was already installed"));
        }
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
                WS_EX_TOOLWINDOW,
                class,
                w!("VRChat X3D Start Fix"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .context("create hidden event window")?;
        let timer = unsafe { SetTimer(Some(hwnd), 1, 100, None) };
        if timer == 0 {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Err(anyhow!("failed to start hidden event window timer"));
        }
        SHUTDOWN_BRIDGE
            .set(ShutdownBridge {
                command_tx,
                done_rx,
                system_shutdown_requested: AtomicBool::new(false),
            })
            .map_err(|_| anyhow!("shutdown bridge was already installed"))?;
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

    pub fn system_shutdown_requested(&self) -> bool {
        SHUTDOWN_BRIDGE
            .get()
            .is_some_and(|bridge| bridge.system_shutdown_requested.load(Ordering::Acquire))
    }

    pub fn take_shutdown_completed(&self) -> bool {
        SHUTDOWN_BRIDGE
            .get()
            .is_some_and(|bridge| bridge.done_rx.try_recv().is_ok())
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
            // Windows may still cancel shutdown. Acknowledge immediately and
            // defer all cleanup until WM_ENDSESSION confirms it.
            LRESULT(1)
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                request_shutdown(true);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            request_shutdown(false);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn request_shutdown(wait_for_cleanup: bool) {
    let Some(bridge) = SHUTDOWN_BRIDGE.get() else {
        return;
    };
    bridge
        .system_shutdown_requested
        .store(true, Ordering::Release);
    let _ = bridge
        .command_tx
        .send_timeout(SupervisorCommand::Shutdown, Duration::from_millis(100));
    if wait_for_cleanup {
        // Keep this bounded so the application never stalls system shutdown.
        let _ = bridge.done_rx.recv_timeout(Duration::from_millis(1500));
        crate::logging::flush();
    }
}
