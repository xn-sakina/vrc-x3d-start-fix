use std::panic;

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
    let supervisor = supervisor::spawn(
        loaded.config.clone(),
        store,
        guards.log_dir.clone(),
        command_rx,
        update_tx,
    );

    let event_loop = NativeEventLoop::new(command_tx.clone())?;
    let tray = match TrayUi::new(&loaded.config, command_tx.clone()) {
        Ok(tray) => tray,
        Err(error) => {
            let _ = command_tx.send(SupervisorCommand::Shutdown);
            let _ = supervisor.join();
            return Err(error);
        }
    };

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
    }

    if !shutdown_complete {
        let _ = command_tx.send(SupervisorCommand::Shutdown);
        while let Ok(update) = update_rx.recv() {
            if update.status == UiStatus::ShutdownComplete {
                break;
            }
        }
    }
    drop(tray);
    supervisor
        .join()
        .map_err(|_| anyhow::anyhow!("supervisor thread panicked"))?;
    drop(guards);
    Ok(())
}
