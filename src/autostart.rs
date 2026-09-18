use std::{env, path::Path, thread, time::Duration};

use anyhow::{Context, Result, anyhow};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, VARIANT_BOOL, VARIANT_TRUE},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            TaskScheduler::{
                IExecAction, ILogonTrigger, ITaskFolder, ITaskService, TASK_ACTION_EXEC,
                TASK_CREATE_OR_UPDATE, TASK_INSTANCES_IGNORE_NEW, TASK_LOGON_INTERACTIVE_TOKEN,
                TASK_RUNLEVEL_LUA, TASK_TRIGGER_LOGON, TASK_TRIGGER_TYPE2, TaskScheduler,
            },
            Variant::VARIANT,
        },
    },
    core::{BSTR, HRESULT, Interface},
};

const TASK_NAME: &str = "VRChatX3DStartFix-AutoStart";
const VERIFICATION_DELAYS: [Duration; 6] = [
    Duration::ZERO,
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(3),
    Duration::from_secs(4),
    Duration::from_secs(5),
];
const REQUIRED_STABLE_READS: usize = 3;

#[derive(Debug)]
pub struct UpdateResult {
    pub enabled_for_current_executable: Option<bool>,
    pub error: Option<String>,
}

pub fn query_current_executable() -> Result<bool> {
    let executable = env::current_exe().context("locate current executable")?;
    with_root_folder(|folder| task_targets_executable(folder, &executable))
}

/// Establishes fresh UI state after a restart. Both enabled and disabled must
/// survive the stability window because an out-of-process scheduler request
/// from a process killed moments earlier may still be settling.
pub fn query_current_executable_verified() -> UpdateResult {
    observe_stable_state()
}

/// Changes the task and then reads it back. The UI must only adopt the returned
/// state; a successful API call alone is not proof that security software kept
/// the task.
pub fn set_current_executable_and_verify(enabled: bool) -> UpdateResult {
    let mutation = if enabled {
        enable_current_executable()
    } else {
        disable_current_executable()
    };
    if let Err(error) = mutation {
        let actual = query_current_executable().ok();
        return UpdateResult {
            enabled_for_current_executable: actual,
            error: Some(format!("change failed: {error:#}")),
        };
    }

    verify_stable_state(enabled)
}

/// Security products may accept the Task Scheduler write and remove or disable
/// it shortly afterwards. Keep the UI pending until several later reads agree.
/// The delays are incremental and total 15 seconds.
fn verify_stable_state(expected: bool) -> UpdateResult {
    let mut consecutive_matches = 0usize;
    let mut last_actual = None;
    let mut last_error = None;

    for delay in VERIFICATION_DELAYS {
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        match query_current_executable() {
            Ok(actual) => {
                last_actual = Some(actual);
                last_error = None;
                if actual == expected {
                    consecutive_matches += 1;
                } else {
                    consecutive_matches = 0;
                }
            }
            Err(error) => {
                consecutive_matches = 0;
                last_actual = None;
                last_error = Some(format!("{error:#}"));
            }
        }
    }

    if consecutive_matches >= REQUIRED_STABLE_READS && last_actual == Some(expected) {
        UpdateResult {
            enabled_for_current_executable: last_actual,
            error: None,
        }
    } else {
        let detail = last_error.unwrap_or_else(|| {
            format!(
                "state did not remain stable: requested {expected}, last actual {last_actual:?}"
            )
        });
        UpdateResult {
            // A value that has not survived the stability window is not safe to
            // present as verified, even if the final instantaneous read matched.
            enabled_for_current_executable: (last_actual == Some(!expected)).then_some(!expected),
            error: Some(format!("verification failed: {detail}")),
        }
    }
}

fn observe_stable_state() -> UpdateResult {
    let mut consecutive_matches = 0usize;
    let mut last_actual = None;
    let mut last_error = None;

    for delay in VERIFICATION_DELAYS {
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        match query_current_executable() {
            Ok(actual) => {
                consecutive_matches = if last_actual == Some(actual) {
                    consecutive_matches + 1
                } else {
                    1
                };
                last_actual = Some(actual);
                last_error = None;
            }
            Err(error) => {
                consecutive_matches = 0;
                last_actual = None;
                last_error = Some(format!("{error:#}"));
            }
        }
    }

    if consecutive_matches >= REQUIRED_STABLE_READS {
        UpdateResult {
            enabled_for_current_executable: last_actual,
            error: None,
        }
    } else {
        UpdateResult {
            enabled_for_current_executable: None,
            error: Some(format!(
                "initial state did not stabilize: {}",
                last_error.unwrap_or_else(|| format!("last actual {last_actual:?}"))
            )),
        }
    }
}

fn enable_current_executable() -> Result<()> {
    let executable = env::current_exe().context("locate current executable")?;
    if !executable.is_file() {
        return Err(anyhow!(
            "current executable is not a regular file: {}",
            executable.display()
        ));
    }
    let executable_text = executable
        .to_str()
        .ok_or_else(|| anyhow!("executable path is not valid Unicode"))?;
    let working_directory = executable
        .parent()
        .and_then(Path::to_str)
        .ok_or_else(|| anyhow!("executable has no valid parent directory"))?;

    with_service_and_root(|service, folder| unsafe {
        let definition = service.NewTask(0).context("create task definition")?;
        let connected_user = service
            .ConnectedUser()
            .context("read connected Task Scheduler user")?
            .to_string();
        let connected_domain = service
            .ConnectedDomain()
            .context("read connected Task Scheduler domain")?
            .to_string();
        let account = if connected_domain.is_empty() {
            connected_user
        } else {
            format!(r"{connected_domain}\{connected_user}")
        };

        let registration = definition
            .RegistrationInfo()
            .context("read task registration info")?;
        registration
            .SetAuthor(&BSTR::from("VRChat X3D Start Fix"))
            .context("set task author")?;
        registration
            .SetDescription(&BSTR::from(
                "Starts VRChat X3D Start Fix when the current user signs in.",
            ))
            .context("set task description")?;
        registration
            .SetVersion(&BSTR::from(env!("CARGO_PKG_VERSION")))
            .context("set task version")?;

        let principal = definition.Principal().context("read task principal")?;
        principal
            .SetUserId(&BSTR::from(account.as_str()))
            .context("limit task to current user")?;
        principal
            .SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN)
            .context("set interactive user logon")?;
        principal
            .SetRunLevel(TASK_RUNLEVEL_LUA)
            .context("set least-privilege run level")?;

        let logon_trigger: ILogonTrigger = definition
            .Triggers()
            .context("read task triggers")?
            .Create(TASK_TRIGGER_LOGON)
            .context("create logon trigger")?
            .cast()
            .context("open logon trigger")?;
        logon_trigger
            .SetUserId(&BSTR::from(account.as_str()))
            .context("limit logon trigger to current user")?;

        let settings = definition.Settings().context("read task settings")?;
        settings.SetEnabled(VARIANT_TRUE).context("enable task")?;
        settings
            .SetHidden(VARIANT_BOOL(0))
            .context("keep task visible")?;
        settings
            .SetMultipleInstances(TASK_INSTANCES_IGNORE_NEW)
            .context("prevent duplicate scheduled instances")?;
        settings
            .SetDisallowStartIfOnBatteries(VARIANT_BOOL(0))
            .context("allow task on battery")?;
        settings
            .SetStopIfGoingOnBatteries(VARIANT_BOOL(0))
            .context("keep task running on battery")?;
        settings
            .SetExecutionTimeLimit(&BSTR::from("PT0S"))
            .context("remove scheduler execution limit")?;

        let action: IExecAction = definition
            .Actions()
            .context("read task actions")?
            .Create(TASK_ACTION_EXEC)
            .context("create executable action")?
            .cast()
            .context("open executable action")?;
        action
            .SetPath(&BSTR::from(executable_text))
            .context("set executable path")?;
        action
            .SetWorkingDirectory(&BSTR::from(working_directory))
            .context("set working directory")?;

        let empty = VARIANT::default();
        folder
            .RegisterTaskDefinition(
                &BSTR::from(TASK_NAME),
                &definition,
                TASK_CREATE_OR_UPDATE.0,
                &empty,
                &empty,
                TASK_LOGON_INTERACTIVE_TOKEN,
                &empty,
            )
            .context("register per-user logon task")?;
        Ok(())
    })
}

fn disable_current_executable() -> Result<()> {
    let executable = env::current_exe().context("locate current executable")?;
    with_root_folder(|folder| {
        if task_targets_executable(folder, &executable)? {
            unsafe {
                folder
                    .DeleteTask(&BSTR::from(TASK_NAME), 0)
                    .context("delete logon task")?;
            }
        }
        Ok(())
    })
}

fn task_targets_executable(folder: &ITaskFolder, executable: &Path) -> Result<bool> {
    let task = match unsafe { folder.GetTask(&BSTR::from(TASK_NAME)) } {
        Ok(task) => task,
        Err(error) if error.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => {
            return Ok(false);
        }
        Err(error) => return Err(error).context("query logon task"),
    };
    if !unsafe { task.Enabled() }
        .context("read logon task enabled state")?
        .as_bool()
    {
        return Ok(false);
    }

    let definition = unsafe { task.Definition() }.context("read logon task definition")?;

    let triggers = unsafe { definition.Triggers() }.context("read logon task triggers")?;
    let mut trigger_count = 0;
    unsafe { triggers.Count(&mut trigger_count) }.context("count logon task triggers")?;
    if trigger_count != 1 {
        return Ok(false);
    }
    let trigger = unsafe { triggers.get_Item(1) }.context("read logon task trigger")?;
    let mut trigger_type = TASK_TRIGGER_TYPE2(0);
    unsafe { trigger.Type(&mut trigger_type) }.context("read logon task trigger type")?;
    let mut trigger_enabled = VARIANT_BOOL(0);
    unsafe { trigger.Enabled(&mut trigger_enabled) }.context("read logon trigger enabled state")?;
    if trigger_type != TASK_TRIGGER_LOGON || !trigger_enabled.as_bool() {
        return Ok(false);
    }

    let actions = unsafe { definition.Actions() }.context("read logon task actions")?;
    let mut action_count = 0;
    unsafe { actions.Count(&mut action_count) }.context("count logon task actions")?;
    if action_count != 1 {
        return Ok(false);
    }
    let action: IExecAction = unsafe { actions.get_Item(1) }
        .context("read logon task action")?
        .cast()
        .context("open logon task executable action")?;
    let mut registered_path = BSTR::new();
    unsafe { action.Path(&mut registered_path) }.context("read logon task executable path")?;

    Ok(windows_paths_equal(
        registered_path.to_string().as_str(),
        executable,
    ))
}

fn windows_paths_equal(registered: &str, executable: &Path) -> bool {
    fn normalize(value: &str) -> String {
        value
            .strip_prefix(r"\\?\")
            .unwrap_or(value)
            .replace('/', r"\")
            .trim_end_matches('\u{5c}')
            .to_lowercase()
    }

    executable
        .to_str()
        .is_some_and(|current| normalize(registered) == normalize(current))
}

fn with_root_folder<T>(operation: impl FnOnce(&ITaskFolder) -> Result<T>) -> Result<T> {
    with_service_and_root(|_, folder| operation(folder))
}

fn with_service_and_root<T>(
    operation: impl FnOnce(&ITaskService, &ITaskFolder) -> Result<T>,
) -> Result<T> {
    let _apartment = ComApartment::initialize()?;
    unsafe {
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)
            .context("create Task Scheduler service")?;
        let empty = VARIANT::default();
        service
            .Connect(&empty, &empty, &empty, &empty)
            .context("connect to Task Scheduler")?;
        let root = service
            .GetFolder(&BSTR::from(r"\"))
            .context("open Task Scheduler root folder")?;
        operation(&service, &root)
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .context("initialize COM")?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_path_comparison_is_case_and_separator_insensitive() {
        assert!(windows_paths_equal(
            r"C:\Tools\VRChatFix.exe",
            Path::new("c:/tools/vrchatfix.exe")
        ));
        assert!(windows_paths_equal(
            r"\\?\C:\Tools\VRChatFix.exe",
            Path::new(r"C:\Tools\VRChatFix.exe")
        ));
        assert!(!windows_paths_equal(
            r"C:\Tools\v1\VRChatFix.exe",
            Path::new(r"C:\Tools\v2\VRChatFix.exe")
        ));
    }
}
