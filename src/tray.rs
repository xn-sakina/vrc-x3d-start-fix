use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    thread,
};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
};

use crate::{
    autostart,
    config::{AppConfig, CpuCoverage, DisturbanceParams, Language, Profile},
    supervisor::{SupervisorCommand, UiStatus, UiUpdate},
};

pub struct TrayUi {
    tray: TrayIcon,
    status_icons: StatusIcons,
    header: MenuItem,
    version_item: MenuItem,
    status_item: MenuItem,
    config_item: MenuItem,
    profile_menu: Submenu,
    profile_items: Vec<(Profile, CheckMenuItem)>,
    advanced_menu: Submenu,
    duty_menu: Submenu,
    duty_items: Vec<(u8, CheckMenuItem)>,
    duration_menu: Submenu,
    duration_items: Vec<(u64, CheckMenuItem)>,
    coverage_menu: Submenu,
    coverage_items: Vec<(CpuCoverage, CheckMenuItem)>,
    reset_item: MenuItem,
    language_menu: Submenu,
    language_items: Vec<(Language, CheckMenuItem)>,
    autostart_item: CheckMenuItem,
    autostart_state: Cell<Option<bool>>,
    autostart_phase: Cell<AutoStartPhase>,
    autostart_result: RefCell<Option<Receiver<autostart::UpdateResult>>>,
    open_logs_item: MenuItem,
    exit_item: MenuItem,
    command_tx: Sender<SupervisorCommand>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AutoStartPhase {
    Checking,
    Ready,
    Applying,
    Failed,
}

struct StatusIcons {
    ready: Icon,
    working: Icon,
    attention: Icon,
}

impl StatusIcons {
    fn load() -> Result<Self> {
        Ok(Self {
            ready: load_icon(include_bytes!("../assets/tray-icons/ready-32.png"), "ready")?,
            working: load_icon(
                include_bytes!("../assets/tray-icons/working-32.png"),
                "working",
            )?,
            attention: load_icon(
                include_bytes!("../assets/tray-icons/attention-32.png"),
                "attention",
            )?,
        })
    }

    fn for_status(&self, status: &UiStatus) -> &Icon {
        match status {
            UiStatus::Waiting | UiStatus::Success => &self.ready,
            UiStatus::Detected | UiStatus::Disturbing => &self.working,
            UiStatus::Failed | UiStatus::Unknown => &self.attention,
            UiStatus::ShuttingDown | UiStatus::ShutdownComplete => &self.ready,
        }
    }
}

impl TrayUi {
    pub fn new(config: &AppConfig, command_tx: Sender<SupervisorCommand>) -> Result<Self> {
        let status_icons = StatusIcons::load()?;
        let menu = Menu::new();
        let header = MenuItem::new(rust_i18n::t!("app.name"), false, None);
        let version_item = MenuItem::new("", false, None);
        let status_item = MenuItem::new("", false, None);
        let config_item = MenuItem::new("", false, None);
        menu.append_items(&[&header, &version_item, &status_item, &config_item])?;
        menu.append(&PredefinedMenuItem::separator())?;

        let profile_menu = Submenu::new(rust_i18n::t!("menu.profile"), true);
        let profile_items = Profile::PRESETS
            .into_iter()
            .map(|profile| {
                let item =
                    CheckMenuItem::new(rust_i18n::t!(profile.locale_key()), true, false, None);
                profile_menu.append(&item)?;
                Ok((profile, item))
            })
            .collect::<Result<Vec<_>, tray_icon::menu::Error>>()?;
        menu.append(&profile_menu)?;

        let advanced = Submenu::new(rust_i18n::t!("menu.advanced"), true);
        let duty_menu = Submenu::new(rust_i18n::t!("menu.duty"), true);
        let duty_items = DisturbanceParams::DUTY_OPTIONS
            .into_iter()
            .map(|value| {
                let item = CheckMenuItem::new(format!("{value}%"), true, false, None);
                duty_menu.append(&item)?;
                Ok((value, item))
            })
            .collect::<Result<Vec<_>, tray_icon::menu::Error>>()?;
        advanced.append(&duty_menu)?;

        let duration_menu = Submenu::new(rust_i18n::t!("menu.duration"), true);
        let duration_items = DisturbanceParams::DURATION_OPTIONS
            .into_iter()
            .map(|value| {
                let item = CheckMenuItem::new(
                    format!("{value} {}", rust_i18n::t!("unit.seconds_short")),
                    true,
                    false,
                    None,
                );
                duration_menu.append(&item)?;
                Ok((value, item))
            })
            .collect::<Result<Vec<_>, tray_icon::menu::Error>>()?;
        advanced.append(&duration_menu)?;

        let coverage_menu = Submenu::new(rust_i18n::t!("menu.coverage"), true);
        let coverage_items = CpuCoverage::ALL
            .into_iter()
            .map(|coverage| {
                let item =
                    CheckMenuItem::new(rust_i18n::t!(coverage.locale_key()), true, false, None);
                coverage_menu.append(&item)?;
                Ok((coverage, item))
            })
            .collect::<Result<Vec<_>, tray_icon::menu::Error>>()?;
        advanced.append(&coverage_menu)?;
        advanced.append(&PredefinedMenuItem::separator())?;
        let reset_item = MenuItem::new(rust_i18n::t!("menu.reset"), true, None);
        advanced.append(&reset_item)?;
        menu.append(&advanced)?;

        menu.append(&PredefinedMenuItem::separator())?;
        let language_menu = Submenu::new(rust_i18n::t!("menu.language"), true);
        let language_items = Language::ALL
            .into_iter()
            .map(|language| {
                let item = CheckMenuItem::new(language_label(language), true, false, None);
                language_menu.append(&item)?;
                Ok((language, item))
            })
            .collect::<Result<Vec<_>, tray_icon::menu::Error>>()?;
        menu.append(&language_menu)?;
        menu.append(&PredefinedMenuItem::separator())?;

        let autostart_item =
            CheckMenuItem::new(rust_i18n::t!("menu.autostart"), false, false, None);
        menu.append(&autostart_item)?;
        menu.append(&PredefinedMenuItem::separator())?;

        let open_logs_item = MenuItem::new(rust_i18n::t!("menu.open_logs"), true, None);
        menu.append(&open_logs_item)?;
        menu.append(&PredefinedMenuItem::separator())?;
        let exit_item = MenuItem::new(rust_i18n::t!("menu.exit"), true, None);
        menu.append(&exit_item)?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip(rust_i18n::t!("app.name"))
            .with_icon(status_icons.for_status(&UiStatus::Waiting).clone())
            .build()
            .context("create Windows tray icon")?;

        let ui = Self {
            tray,
            status_icons,
            header,
            version_item,
            status_item,
            config_item,
            profile_menu,
            profile_items,
            advanced_menu: advanced,
            duty_menu,
            duty_items,
            duration_menu,
            duration_items,
            coverage_menu,
            coverage_items,
            reset_item,
            language_menu,
            language_items,
            autostart_item,
            autostart_state: Cell::new(None),
            autostart_phase: Cell::new(AutoStartPhase::Checking),
            autostart_result: RefCell::new(None),
            open_logs_item,
            exit_item,
            command_tx,
        };
        ui.apply_update(&UiUpdate {
            status: UiStatus::Waiting,
            config: config.clone(),
        });
        ui.start_autostart_query();
        Ok(ui)
    }

    pub fn poll_menu_events(&self) -> bool {
        let mut ui_changed = self.poll_autostart_result();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id() == self.exit_item.id() {
                let _ = self.command_tx.send(SupervisorCommand::Shutdown);
            } else if event.id() == self.autostart_item.id() {
                // Native check items toggle before delivering their event. Put
                // the last verified state back until Windows confirms the edit.
                self.autostart_item
                    .set_checked(self.autostart_state.get().unwrap_or(false));
                let desired = self.autostart_state.get().is_none_or(|enabled| !enabled);
                self.start_autostart_change(desired);
                ui_changed = true;
            } else if event.id() == self.open_logs_item.id() {
                let _ = self.command_tx.try_send(SupervisorCommand::OpenLogFolder);
            } else if event.id() == self.reset_item.id() {
                let _ = self.command_tx.try_send(SupervisorCommand::ResetDefaults);
            } else if let Some((language, _)) = self
                .language_items
                .iter()
                .find(|(_, item)| event.id() == item.id())
            {
                let _ = self
                    .command_tx
                    .try_send(SupervisorCommand::SetLanguage(*language));
            } else if let Some((profile, _)) = self
                .profile_items
                .iter()
                .find(|(_, item)| event.id() == item.id())
            {
                let _ = self
                    .command_tx
                    .try_send(SupervisorCommand::SelectProfile(*profile));
            } else if let Some((duty, _)) = self
                .duty_items
                .iter()
                .find(|(_, item)| event.id() == item.id())
            {
                let _ = self.command_tx.try_send(SupervisorCommand::SetDuty(*duty));
            } else if let Some((duration, _)) = self
                .duration_items
                .iter()
                .find(|(_, item)| event.id() == item.id())
            {
                let _ = self
                    .command_tx
                    .try_send(SupervisorCommand::SetDuration(*duration));
            } else if let Some((coverage, _)) = self
                .coverage_items
                .iter()
                .find(|(_, item)| event.id() == item.id())
            {
                let _ = self
                    .command_tx
                    .try_send(SupervisorCommand::SetCoverage(*coverage));
            }
        }
        ui_changed
    }

    pub fn apply_update(&self, update: &UiUpdate) {
        if let Err(error) = self
            .tray
            .set_icon(Some(self.status_icons.for_status(&update.status).clone()))
        {
            tracing::warn!(event = "tray_icon_update_failed", error = %error);
        }
        self.header.set_text(rust_i18n::t!("app.name"));
        self.version_item.set_text(format!(
            "{}: v{}",
            rust_i18n::t!("menu.version"),
            env!("CARGO_PKG_VERSION")
        ));
        self.profile_menu.set_text(rust_i18n::t!("menu.profile"));
        self.advanced_menu.set_text(rust_i18n::t!("menu.advanced"));
        self.duty_menu.set_text(rust_i18n::t!("menu.duty"));
        self.duration_menu.set_text(rust_i18n::t!("menu.duration"));
        self.coverage_menu.set_text(rust_i18n::t!("menu.coverage"));
        self.reset_item.set_text(rust_i18n::t!("menu.reset"));
        self.language_menu.set_text(rust_i18n::t!("menu.language"));
        self.refresh_autostart_visual();
        self.open_logs_item
            .set_text(rust_i18n::t!("menu.open_logs"));
        self.exit_item.set_text(rust_i18n::t!("menu.exit"));

        let status_text = rust_i18n::t!(update.status.locale_key()).to_string();
        self.status_item.set_text(format!(
            "{}: {status_text}",
            rust_i18n::t!("menu.status_label")
        ));
        let params = update.config.resolved_params();
        let profile = rust_i18n::t!(update.config.profile.locale_key());
        self.config_item.set_text(format!(
            "{}: {profile} / CPU {}% / {} {}",
            rust_i18n::t!("menu.config_label"),
            params.duty_percent,
            params.hard_stop_secs,
            rust_i18n::t!("unit.seconds_short")
        ));
        let _ = self.tray.set_tooltip(Some(format!(
            "{} — {status_text}",
            rust_i18n::t!("app.name")
        )));
        for (profile, item) in &self.profile_items {
            item.set_text(rust_i18n::t!(profile.locale_key()));
            item.set_checked(update.config.profile == *profile);
        }
        for (duty, item) in &self.duty_items {
            item.set_checked(params.duty_percent == *duty);
        }
        for (duration, item) in &self.duration_items {
            item.set_text(format!(
                "{duration} {}",
                rust_i18n::t!("unit.seconds_short")
            ));
            item.set_checked(params.hard_stop_secs == *duration);
        }
        for (coverage, item) in &self.coverage_items {
            item.set_text(rust_i18n::t!(coverage.locale_key()));
            item.set_checked(params.coverage == *coverage);
        }
        let effective_language = crate::i18n::language_for_locale(&rust_i18n::locale());
        for (language, item) in &self.language_items {
            item.set_checked(effective_language == *language);
        }
    }

    fn start_autostart_query(&self) {
        if self.autostart_result.borrow().is_some() {
            return;
        }
        self.autostart_phase.set(AutoStartPhase::Checking);
        self.refresh_autostart_visual();
        self.start_autostart_worker(autostart::query_current_executable_verified);
    }

    fn start_autostart_change(&self, enabled: bool) {
        if self.autostart_result.borrow().is_some() {
            return;
        }
        self.autostart_phase.set(AutoStartPhase::Applying);
        self.refresh_autostart_visual();
        self.start_autostart_worker(move || autostart::set_current_executable_and_verify(enabled));
    }

    fn start_autostart_worker(
        &self,
        operation: impl FnOnce() -> autostart::UpdateResult + Send + 'static,
    ) {
        if self.autostart_result.borrow().is_some() {
            return;
        }
        self.autostart_item.set_enabled(false);
        let (result_tx, result_rx) = bounded(1);
        match thread::Builder::new()
            .name("autostart-operation".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(operation)).unwrap_or_else(|_| {
                    autostart::UpdateResult {
                        enabled_for_current_executable: None,
                        error: Some("autostart operation panicked".into()),
                    }
                });
                let _ = result_tx.send(result);
            }) {
            Ok(handle) => {
                drop(handle);
                *self.autostart_result.borrow_mut() = Some(result_rx);
            }
            Err(error) => {
                tracing::warn!(event = "autostart_worker_spawn_failed", error = %error);
                self.autostart_phase.set(AutoStartPhase::Failed);
                self.refresh_autostart_visual();
            }
        }
    }

    fn poll_autostart_result(&self) -> bool {
        let result = {
            let slot = self.autostart_result.borrow();
            let Some(receiver) = slot.as_ref() else {
                return false;
            };
            match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => None,
            }
        };
        self.autostart_result.borrow_mut().take();

        let Some(result) = result else {
            tracing::warn!(event = "autostart_worker_disconnected");
            self.autostart_state.set(None);
            self.autostart_phase.set(AutoStartPhase::Failed);
            self.refresh_autostart_visual();
            return true;
        };
        self.autostart_state
            .set(result.enabled_for_current_executable);
        if let Some(error) = result.error {
            tracing::warn!(event = "autostart_operation_failed", error = %error);
            self.autostart_phase.set(AutoStartPhase::Failed);
        } else {
            self.autostart_phase.set(AutoStartPhase::Ready);
            tracing::info!(
                event = "autostart_state_verified",
                enabled = self.autostart_state.get().unwrap_or(false)
            );
        }
        self.refresh_autostart_visual();
        true
    }

    fn refresh_autostart_visual(&self) {
        self.autostart_item
            .set_checked(self.autostart_state.get().unwrap_or(false));
        let (label, enabled) = match self.autostart_phase.get() {
            AutoStartPhase::Checking => (rust_i18n::t!("menu.autostart_checking"), false),
            AutoStartPhase::Applying => (rust_i18n::t!("menu.autostart_confirming"), false),
            AutoStartPhase::Failed => (rust_i18n::t!("menu.autostart_failed"), true),
            AutoStartPhase::Ready => (rust_i18n::t!("menu.autostart"), true),
        };
        self.autostart_item.set_text(label);
        self.autostart_item.set_enabled(enabled);
    }
}

fn load_icon(bytes: &[u8], label: &str) -> Result<Icon> {
    let image = image::load_from_memory(bytes)
        .with_context(|| format!("decode embedded {label} tray icon"))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height)
        .with_context(|| format!("create {label} tray icon"))
}

fn language_label(language: Language) -> &'static str {
    match language {
        Language::ZhCn => "中文",
        Language::En => "English",
    }
}
