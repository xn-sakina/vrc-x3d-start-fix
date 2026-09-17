use std::{panic, thread::JoinHandle, time::Duration};

use anyhow::{Context, Result};
use crossbeam_channel::{TryRecvError, bounded};
use single_instance::SingleInstance;

use crate::{
    config::ConfigStore,
    logging,
    platform::windows::{event_loop::NativeEventLoop, message_box},
    supervisor::{self, SupervisorCommand, UiStatus},
    tray::TrayUi,
};

pub fn run() -> Result<()> {
    let store = ConfigStore::discover().context("locate configuration")?;
    let loaded = store.load_or_recover().context("load configuration")?;
    crate::i18n::initialize(loaded.config.language_override);

    let instance =
        SingleInstance::new("Local\\VRChatX3DStartFix").context("create single-instance mutex")?;
    if !instance.is_single() {
        message_box::second_instance();
        return Ok(());
    }

    let guards = logging::initialize().context("initialize logging")?;
    if let Some(path) = loaded.invalid_backup.as_ref() {
        tracing::warn!(event = "config_recovered", invalid_backup = %path.display());
    }
    panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        let location = info
            .location()
            .map(|value| format!("{}:{}", value.file(), value.line()))
            .unwrap_or_else(|| "unknown".into());
        tracing::error!(event = "panic", thread = name, location, payload = %info);
    }));

    let (command_tx, command_rx) = bounded(32);
    let (update_tx, update_rx) = bounded(32);
    let (shutdown_done_tx, shutdown_done_rx) = bounded(1);
    let supervisor_handle = supervisor::spawn(
        loaded.config.clone(),
        store,
        guards.log_dir.clone(),
        command_rx,
        update_tx,
        shutdown_done_tx,
    );
    let mut supervisor = SupervisorGuard::new(command_tx.clone(), supervisor_handle);

    let event_loop = NativeEventLoop::new(command_tx.clone(), shutdown_done_rx)?;
    let tray = TrayUi::new(&loaded.config, command_tx.clone())?;

    let mut shutdown_complete = false;
    while !shutdown_complete && event_loop.next_message()? {
        tray.poll_menu_events();
        loop {
            match update_rx.try_recv() {
                Ok(update) => {
                    shutdown_complete = update.status == UiStatus::ShutdownComplete;
                    tray.apply_update(&update);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    shutdown_complete = true;
                    break;
                }
            }
        }
        shutdown_complete |= event_loop.system_shutdown_requested();
        shutdown_complete |= event_loop.take_shutdown_completed();
    }

    drop(tray);
    supervisor.shutdown_and_join()?;
    drop(guards);
    Ok(())
}

struct SupervisorGuard {
    command_tx: crossbeam_channel::Sender<SupervisorCommand>,
    handle: Option<JoinHandle<()>>,
}

impl SupervisorGuard {
    fn new(
        command_tx: crossbeam_channel::Sender<SupervisorCommand>,
        handle: JoinHandle<()>,
    ) -> Self {
        Self {
            command_tx,
            handle: Some(handle),
        }
    }

    fn shutdown_and_join(&mut self) -> Result<()> {
        let _ = self
            .command_tx
            .send_timeout(SupervisorCommand::Shutdown, Duration::from_millis(250));
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("supervisor thread panicked"))?;
        }
        Ok(())
    }
}

impl Drop for SupervisorGuard {
    fn drop(&mut self) {
        let _ = self.shutdown_and_join();
    }
}
