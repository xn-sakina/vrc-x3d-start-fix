use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Gentle,
    Balanced,
    Robust,
    Maximum,
    Custom,
}

impl Profile {
    pub const PRESETS: [Self; 4] = [Self::Gentle, Self::Balanced, Self::Robust, Self::Maximum];

    pub fn params(self) -> DisturbanceParams {
        match self {
            Self::Gentle => DisturbanceParams::new(25, 100, 35, 45),
            Self::Balanced => DisturbanceParams::new(35, 100, 40, 50),
            Self::Robust | Self::Custom => DisturbanceParams::new(50, 100, 45, 60),
            Self::Maximum => DisturbanceParams::new(70, 100, 60, 75),
        }
    }

    pub fn locale_key(self) -> &'static str {
        match self {
            Self::Gentle => "profile.gentle",
            Self::Balanced => "profile.balanced",
            Self::Robust => "profile.robust",
            Self::Maximum => "profile.maximum",
            Self::Custom => "profile.custom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuCoverage {
    AllLogical,
    PhysicalCores,
    Unpinned,
}

impl CpuCoverage {
    pub const ALL: [Self; 3] = [Self::AllLogical, Self::PhysicalCores, Self::Unpinned];

    pub fn locale_key(self) -> &'static str {
        match self {
            Self::AllLogical => "coverage.all_logical",
            Self::PhysicalCores => "coverage.physical_cores",
            Self::Unpinned => "coverage.unpinned",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerPriority {
    BelowNormal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisturbanceParams {
    pub duty_percent: u8,
    pub period_ms: u64,
    pub success_after_secs: u64,
    pub hard_stop_secs: u64,
    pub coverage: CpuCoverage,
    pub priority: WorkerPriority,
}

impl DisturbanceParams {
    pub const DUTY_OPTIONS: [u8; 5] = [25, 35, 50, 65, 70];
    pub const DURATION_OPTIONS: [u64; 5] = [30, 45, 60, 75, 90];

    pub const fn new(
        duty_percent: u8,
        period_ms: u64,
        success_after_secs: u64,
        hard_stop_secs: u64,
    ) -> Self {
        Self {
            duty_percent,
            period_ms,
            success_after_secs,
            hard_stop_secs,
            coverage: CpuCoverage::AllLogical,
            priority: WorkerPriority::BelowNormal,
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(10..=80).contains(&self.duty_percent) {
            return Err(ConfigError::Validation(
                "duty_percent must be between 10 and 80",
            ));
        }
        if !(50..=500).contains(&self.period_ms) {
            return Err(ConfigError::Validation(
                "period_ms must be between 50 and 500",
            ));
        }
        if !(20..=90).contains(&self.success_after_secs) {
            return Err(ConfigError::Validation(
                "success_after_secs must be between 20 and 90",
            ));
        }
        if self.hard_stop_secs < self.success_after_secs || self.hard_stop_secs > 120 {
            return Err(ConfigError::Validation(
                "hard_stop_secs must be >= success_after_secs and <= 120",
            ));
        }
        Ok(())
    }

    pub fn busy_ms(&self) -> u64 {
        self.period_ms * u64::from(self.duty_percent) / 100
    }

    pub fn rest_ms(&self) -> u64 {
        self.period_ms.saturating_sub(self.busy_ms())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    En,
    ZhCn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    pub schema_version: u32,
    pub profile: Profile,
    pub custom: Option<DisturbanceParams>,
    pub language_override: Option<Language>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            profile: Profile::Robust,
            custom: None,
            language_override: None,
        }
    }
}

impl AppConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            return Err(ConfigError::Validation("unsupported config schema version"));
        }
        if self.profile == Profile::Custom {
            self.custom
                .as_ref()
                .ok_or(ConfigError::Validation(
                    "custom profile requires custom parameters",
                ))?
                .validate()?;
        }
        if let Some(custom) = &self.custom {
            custom.validate()?;
        }
        Ok(())
    }

    pub fn resolved_params(&self) -> DisturbanceParams {
        if self.profile == Profile::Custom {
            self.custom
                .clone()
                .unwrap_or_else(|| Profile::Robust.params())
        } else {
            self.profile.params()
        }
    }

    pub fn select_profile(&mut self, profile: Profile) {
        debug_assert_ne!(profile, Profile::Custom);
        self.profile = profile;
        self.custom = None;
    }

    fn edit_custom(&mut self, update: impl FnOnce(&mut DisturbanceParams)) {
        let mut params = self.resolved_params();
        update(&mut params);
        self.profile = Profile::Custom;
        self.custom = Some(params);
    }

    pub fn set_duty(&mut self, duty_percent: u8) {
        self.edit_custom(|params| params.duty_percent = duty_percent);
    }

    pub fn set_hard_stop(&mut self, hard_stop_secs: u64) {
        self.edit_custom(|params| {
            params.hard_stop_secs = hard_stop_secs;
            params.success_after_secs = params.success_after_secs.min(hard_stop_secs).max(20);
        });
    }

    pub fn set_coverage(&mut self, coverage: CpuCoverage) {
        self.edit_custom(|params| params.coverage = coverage);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug)]
pub struct LoadResult {
    pub config: AppConfig,
    pub invalid_backup: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("configuration directory is unavailable")]
    NoProjectDirectory,
    #[error("configuration I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("configuration parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("configuration serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("invalid configuration: {0}")]
    Validation(&'static str),
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn discover() -> Result<Self, ConfigError> {
        let dirs = ProjectDirs::from("com", "local", "VRChatX3DStartFix")
            .ok_or(ConfigError::NoProjectDirectory)?;
        Ok(Self::at(dirs.config_dir().join("config.toml")))
    }

    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_or_recover(&self) -> Result<LoadResult, ConfigError> {
        if !self.path.exists() {
            let config = AppConfig::default();
            self.save(&config)?;
            return Ok(LoadResult {
                config,
                invalid_backup: None,
            });
        }

        let parsed = fs::read_to_string(&self.path)
            .map_err(ConfigError::Io)
            .and_then(|text| toml::from_str::<AppConfig>(&text).map_err(ConfigError::Parse))
            .and_then(|config| {
                config.validate()?;
                Ok(config)
            });

        match parsed {
            Ok(config) => Ok(LoadResult {
                config,
                invalid_backup: None,
            }),
            Err(_) => {
                let backup = self.invalid_backup_path();
                fs::rename(&self.path, &backup).or_else(|_| {
                    fs::copy(&self.path, &backup)?;
                    fs::remove_file(&self.path)
                })?;
                let config = AppConfig::default();
                self.save(&config)?;
                Ok(LoadResult {
                    config,
                    invalid_backup: Some(backup),
                })
            }
        }
    }

    pub fn save(&self, config: &AppConfig) -> Result<(), ConfigError> {
        config.validate()?;
        let parent = self
            .path
            .parent()
            .ok_or(ConfigError::Validation("config path has no parent"))?;
        fs::create_dir_all(parent)?;
        let temp = parent.join(format!(".config.{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp)?;
        file.write_all(toml::to_string_pretty(config)?.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        replace_file(&temp, &self.path)?;
        Ok(())
    }

    fn invalid_backup_path(&self) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.path
            .with_file_name(format!("config.invalid.{timestamp}.toml"))
    }
}

#[cfg(not(target_os = "windows"))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(target_os = "windows")]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        core::PCWSTR,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|error| io::Error::other(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_values_match_the_plan() {
        assert_eq!(Profile::Gentle.params().duty_percent, 25);
        assert_eq!(Profile::Balanced.params().hard_stop_secs, 50);
        assert_eq!(
            Profile::Robust.params(),
            AppConfig::default().resolved_params()
        );
        assert_eq!(Profile::Maximum.params().success_after_secs, 60);
    }

    #[test]
    fn advanced_changes_create_a_valid_custom_profile() {
        let mut config = AppConfig::default();
        config.set_hard_stop(30);
        config.set_duty(65);
        config.set_coverage(CpuCoverage::Unpinned);
        assert_eq!(config.profile, Profile::Custom);
        assert_eq!(config.resolved_params().success_after_secs, 30);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn corrupt_config_is_backed_up_and_reset() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConfigStore::at(temp.path().join("config.toml"));
        fs::write(store.path(), "not valid = [").unwrap();
        let loaded = store.load_or_recover().unwrap();
        assert_eq!(loaded.config, AppConfig::default());
        assert!(loaded.invalid_backup.unwrap().exists());
        assert!(store.path().exists());
    }

    #[test]
    fn save_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConfigStore::at(temp.path().join("config.toml"));
        let mut config = AppConfig::default();
        config.select_profile(Profile::Maximum);
        store.save(&config).unwrap();
        assert_eq!(store.load_or_recover().unwrap().config, config);
    }
}
