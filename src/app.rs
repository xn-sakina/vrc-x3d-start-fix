use std::{
    cell::Cell,
    panic,
    rc::Rc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

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
    crate::i18n::initialize(None);
    let Some(_instance) = acquire_single_instance()? else {
        return Ok(());
    };

    let store = ConfigStore::discover().context("locate configuration")?;
    let loaded = store.load_or_recover().context("load configuration")?;
    crate::i18n::initialize(loaded.config.language_override);

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

    let shutdown_complete = Rc::new(Cell::new(false));
    let ui_shutdown_complete = Rc::clone(&shutdown_complete);
    event_loop.set_tick_handler(move || {
        let mut ui_changed = tray.poll_menu_events();
        loop {
            match update_rx.try_recv() {
                Ok(update) => {
                    ui_shutdown_complete.set(update.status == UiStatus::ShutdownComplete);
                    crate::i18n::initialize(update.config.language_override);
                    tray.apply_update(&update);
                    ui_changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    ui_shutdown_complete.set(true);
                    break;
                }
            }
        }
        ui_changed
    })?;

    while !shutdown_complete.get() && event_loop.next_message()? {
        shutdown_complete.set(
            shutdown_complete.get()
                || event_loop.system_shutdown_requested()
                || event_loop.take_shutdown_completed(),
        );
    }

    drop(event_loop);
    supervisor.shutdown_and_join()?;
    drop(guards);
    Ok(())
}

fn acquire_single_instance() -> Result<Option<SingleInstance>> {
    const MUTEX_NAME: &str = "Local\\VRChatX3DStartFix";
    const HANDOFF_TIMEOUT: Duration = Duration::from_secs(8);
    const RETRY_INTERVAL: Duration = Duration::from_millis(75);

    let initial = SingleInstance::new(MUTEX_NAME).context("create single-instance mutex")?;
    if initial.is_single() {
        return Ok(Some(initial));
    }
    drop(initial);

    let deadline = Instant::now() + HANDOFF_TIMEOUT;
    let mut handoff_error = None;
    while Instant::now() < deadline {
        if let Err(error) =
            crate::platform::windows::instance_handoff::request_existing_instance_shutdown()
        {
            handoff_error = Some(error);
        }
        std::thread::sleep(RETRY_INTERVAL);

        let candidate = SingleInstance::new(MUTEX_NAME).context("retry single-instance mutex")?;
        if candidate.is_single() {
            return Ok(Some(candidate));
        }
    }

    if let Some(error) = handoff_error {
        return Err(error).context("request existing instance shutdown");
    }
    message_box::second_instance();
    Ok(None)
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
