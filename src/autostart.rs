use std::{
    env,
    ffi::OsString,
    fs,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, HLOCAL, LocalFree, VARIANT_BOOL, VARIANT_TRUE},
        Security::{Authorization::ConvertSidToStringSidW, LookupAccountNameW, PSID, SID_NAME_USE},
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
    core::{BSTR, HRESULT, Interface, PCWSTR, PWSTR},
};

const TASK_NAME_PREFIX: &str = "VRChatX3DStartFix-AutoStart";
const LEGACY_TASK_NAME: &str = TASK_NAME_PREFIX;
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
    let Ok(executable) = env::current_exe() else {
        return Ok(false);
    };
    if !executable.is_file() {
        // A deleted or delete-pending executable cannot be a valid autostart
        // target. Cleanup is best-effort because the safe observable state is
        // still "off" when there is nothing Windows can launch.
        if let Err(error) = disable_current_executable() {
            tracing::warn!(event = "autostart_missing_executable_cleanup_failed", error = %error);
        }
        return Ok(false);
    }

    with_service_and_root(|service, folder| {
        let identity = connected_identity(service)?;
        query_owned_tasks(folder, &identity, &executable)
    })
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
        match enable_current_executable() {
            Ok(true) => Ok(()),
            Ok(false) => {
                return UpdateResult {
                    enabled_for_current_executable: Some(false),
                    error: None,
                };
            }
            Err(error) => Err(error),
        }
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

/// Returns `Ok(false)` when the running image no longer has a usable path.
/// This is a normal disabled state rather than an error: Windows cannot safely
/// persist a task for an executable that is already gone.
fn enable_current_executable() -> Result<bool> {
    let Ok(executable) = env::current_exe() else {
        return Ok(false);
    };
    if !executable.is_file() {
        if let Err(error) = disable_current_executable() {
            tracing::warn!(event = "autostart_missing_executable_cleanup_failed", error = %error);
        }
        return Ok(false);
    }
    let executable_text = path_to_bstr(&executable);
    let working_directory = executable
        .parent()
        .ok_or_else(|| anyhow!("executable has no parent directory"))?;
    let working_directory_text = path_to_bstr(working_directory);

    with_service_and_root(|service, folder| unsafe {
        let definition = service.NewTask(0).context("create task definition")?;
        let identity = connected_identity(service)?;
        let task_name = task_name_for_sid(&identity.sid);

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
            .SetUserId(&BSTR::from(identity.sid.as_str()))
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
            .SetUserId(&BSTR::from(identity.sid.as_str()))
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
            .SetPath(&executable_text)
            .context("set executable path")?;
        action
            .SetWorkingDirectory(&working_directory_text)
            .context("set working directory")?;

        let empty = VARIANT::default();
        folder
            .RegisterTaskDefinition(
                &BSTR::from(task_name.as_str()),
                &definition,
                TASK_CREATE_OR_UPDATE.0,
                &empty,
                &empty,
                TASK_LOGON_INTERACTIVE_TOKEN,
                &empty,
            )
            .context("register per-user logon task")?;

        // Migrate the fixed-name task used by older releases only when it
        // belongs to this account. Leaving it would allow two tasks to race at
        // the next sign-in.
        delete_legacy_task_if_owned(folder, &identity)?;
        Ok(true)
    })
}

fn disable_current_executable() -> Result<()> {
    with_service_and_root(|service, folder| {
        let identity = connected_identity(service)?;
        delete_task_if_exists(folder, &task_name_for_sid(&identity.sid))?;
        delete_legacy_task_if_owned(folder, &identity)?;
        Ok(())
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskState {
    Missing,
    Foreign,
    Current,
    /// The task cannot currently launch anything (disabled or missing target).
    InactiveOwned,
    /// The task may launch, but not with the definition this version expects.
    StaleOwned,
}

fn query_owned_tasks(
    folder: &ITaskFolder,
    identity: &UserIdentity,
    executable: &Path,
) -> Result<bool> {
    let task_name = task_name_for_sid(&identity.sid);
    match inspect_task(folder, &task_name, identity, executable)? {
        TaskState::Current => {
            // A per-user task supersedes the legacy fixed-name task.
            delete_legacy_task_if_owned(folder, identity)?;
            return Ok(true);
        }
        TaskState::Missing => {}
        TaskState::Foreign => {
            return Err(anyhow!(
                "per-user autostart task belongs to another account"
            ));
        }
        TaskState::InactiveOwned => {
            if let Err(error) = delete_task_if_exists(folder, &task_name) {
                tracing::warn!(event = "autostart_inactive_task_cleanup_failed", task = %task_name, error = %error);
            }
        }
        TaskState::StaleOwned => {
            delete_task_if_exists(folder, &task_name)
                .context("remove stale per-user autostart task")?;
        }
    }

    let legacy_state = match inspect_task(folder, LEGACY_TASK_NAME, identity, executable) {
        Ok(state) => state,
        Err(error)
            if error
                .downcast_ref::<windows::core::Error>()
                .is_some_and(|error| {
                    error.code() == windows::Win32::Foundation::E_ACCESSDENIED
                }) =>
        {
            // The legacy fixed-name task may belong to another local user.
            tracing::info!(event = "autostart_legacy_task_owned_by_other_user");
            TaskState::Foreign
        }
        Err(error) => return Err(error),
    };
    match legacy_state {
        TaskState::Current => Ok(true),
        TaskState::Missing | TaskState::Foreign => Ok(false),
        TaskState::InactiveOwned => {
            if let Err(error) = delete_task_if_exists(folder, LEGACY_TASK_NAME) {
                tracing::warn!(event = "autostart_inactive_legacy_cleanup_failed", error = %error);
            }
            Ok(false)
        }
        TaskState::StaleOwned => {
            delete_task_if_exists(folder, LEGACY_TASK_NAME)
                .context("remove stale legacy autostart task")?;
            Ok(false)
        }
    }
}

fn inspect_task(
    folder: &ITaskFolder,
    task_name: &str,
    identity: &UserIdentity,
    executable: &Path,
) -> Result<TaskState> {
    let task = match unsafe { folder.GetTask(&BSTR::from(task_name)) } {
        Ok(task) => task,
        Err(error) if error.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => {
            return Ok(TaskState::Missing);
        }
        Err(error) => return Err(error).context("query logon task"),
    };

    let definition = unsafe { task.Definition() }.context("read logon task definition")?;
    let principal = unsafe { definition.Principal() }.context("read logon task principal")?;
    let mut principal_user = BSTR::new();
    unsafe { principal.UserId(&mut principal_user) }.context("read task principal user")?;
    if !identity_matches(principal_user.to_string().as_str(), identity) {
        return Ok(TaskState::Foreign);
    }

    if !unsafe { task.Enabled() }
        .context("read logon task enabled state")?
        .as_bool()
    {
        return Ok(TaskState::InactiveOwned);
    }

    let triggers = unsafe { definition.Triggers() }.context("read logon task triggers")?;
    let mut trigger_count = 0;
    unsafe { triggers.Count(&mut trigger_count) }.context("count logon task triggers")?;
    if trigger_count != 1 {
        return Ok(TaskState::StaleOwned);
    }
    let trigger = unsafe { triggers.get_Item(1) }.context("read logon task trigger")?;
    let mut trigger_type = TASK_TRIGGER_TYPE2(0);
    unsafe { trigger.Type(&mut trigger_type) }.context("read logon task trigger type")?;
    let mut trigger_enabled = VARIANT_BOOL(0);
    unsafe { trigger.Enabled(&mut trigger_enabled) }.context("read logon trigger enabled state")?;
    if trigger_type != TASK_TRIGGER_LOGON || !trigger_enabled.as_bool() {
        return Ok(TaskState::StaleOwned);
    }
    let logon_trigger: ILogonTrigger = trigger.cast().context("open logon task trigger")?;
    let mut trigger_user = BSTR::new();
    unsafe { logon_trigger.UserId(&mut trigger_user) }.context("read logon trigger user")?;
    if !identity_matches(trigger_user.to_string().as_str(), identity) {
        return Ok(TaskState::StaleOwned);
    }

    let actions = unsafe { definition.Actions() }.context("read logon task actions")?;
    let mut action_count = 0;
    unsafe { actions.Count(&mut action_count) }.context("count logon task actions")?;
    if action_count != 1 {
        return Ok(TaskState::StaleOwned);
    }
    let action: IExecAction = unsafe { actions.get_Item(1) }
        .context("read logon task action")?
        .cast()
        .context("open logon task executable action")?;
    let mut registered_path = BSTR::new();
    unsafe { action.Path(&mut registered_path) }.context("read logon task executable path")?;
    let Some(registered_path) = bstr_to_path(&registered_path) else {
        return Ok(TaskState::InactiveOwned);
    };
    if !registered_path.is_file() {
        return Ok(TaskState::InactiveOwned);
    }

    if windows_paths_equal(&registered_path, executable) {
        Ok(TaskState::Current)
    } else {
        Ok(TaskState::StaleOwned)
    }
}

fn windows_paths_equal(registered: &Path, executable: &Path) -> bool {
    fn normalize(value: &Path) -> String {
        let text = value.to_string_lossy();
        text.strip_prefix(r"\\?\")
            .unwrap_or(text.as_ref())
            .replace('/', r"\")
            .trim_end_matches('\u{5c}')
            .to_lowercase()
    }

    if normalize(registered) == normalize(executable) {
        return true;
    }

    match (fs::canonicalize(registered), fs::canonicalize(executable)) {
        (Ok(registered), Ok(executable)) => normalize(&registered) == normalize(&executable),
        _ => false,
    }
}

fn path_to_bstr(path: &Path) -> BSTR {
    BSTR::from_wide(&path.as_os_str().encode_wide().collect::<Vec<_>>())
}

fn bstr_to_path(value: &BSTR) -> Option<PathBuf> {
    let mut wide: &[u16] = value;
    if wide.len() >= 2
        && wide.first() == Some(&(b'"' as u16))
        && wide.last() == Some(&(b'"' as u16))
    {
        wide = &wide[1..wide.len() - 1];
    }
    (!wide.is_empty()).then(|| PathBuf::from(OsString::from_wide(wide)))
}

struct UserIdentity {
    account: String,
    sid: String,
}

fn connected_identity(service: &ITaskService) -> Result<UserIdentity> {
    let connected_user = unsafe { service.ConnectedUser() }
        .context("read connected Task Scheduler user")?
        .to_string();
    let connected_domain = unsafe { service.ConnectedDomain() }
        .context("read connected Task Scheduler domain")?
        .to_string();
    if connected_user.is_empty() {
        return Err(anyhow!("Task Scheduler returned an empty connected user"));
    }
    let account = if connected_domain.is_empty() {
        connected_user
    } else {
        format!(r"{connected_domain}\{connected_user}")
    };
    let sid = sid_for_account(&account)
        .with_context(|| format!("resolve Task Scheduler account {account} to SID"))?;
    Ok(UserIdentity { account, sid })
}

fn task_name_for_sid(sid: &str) -> String {
    // Stable FNV-1a over the canonical SID keeps the name short while
    // separating users in Task Scheduler's machine-wide root namespace.
    let mut hash = 0xcbf29ce484222325u64;
    for unit in sid.to_uppercase().encode_utf16() {
        for byte in unit.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("{TASK_NAME_PREFIX}-{hash:016x}")
}

fn identity_matches(value: &str, identity: &UserIdentity) -> bool {
    value.eq_ignore_ascii_case(&identity.sid)
        || value.eq_ignore_ascii_case(&identity.account)
        || sid_for_account(value).is_ok_and(|sid| sid.eq_ignore_ascii_case(&identity.sid))
}

fn sid_for_account(account: &str) -> Result<String> {
    let account_wide = account
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut sid_size = 0u32;
    let mut domain_size = 0u32;
    let mut sid_use = SID_NAME_USE(0);
    let _ = unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR(account_wide.as_ptr()),
            None,
            &mut sid_size,
            None,
            &mut domain_size,
            &mut sid_use,
        )
    };
    if sid_size == 0 {
        return Err(windows::core::Error::from_win32()).context("size account SID buffer");
    }

    // Keep the variable-length SID buffer naturally aligned even though the
    // API exposes it as an untyped byte count.
    let sid_words = (sid_size as usize).div_ceil(std::mem::size_of::<usize>());
    let mut sid_buffer = vec![0usize; sid_words];
    let mut domain_buffer = vec![0u16; domain_size.max(1) as usize];
    let sid = PSID(sid_buffer.as_mut_ptr().cast());
    unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR(account_wide.as_ptr()),
            Some(sid),
            &mut sid_size,
            Some(PWSTR(domain_buffer.as_mut_ptr())),
            &mut domain_size,
            &mut sid_use,
        )
    }
    .context("resolve account SID")?;

    let mut sid_text = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid, &mut sid_text) }.context("format account SID")?;
    let result = unsafe { sid_text.to_string() }.context("decode account SID");
    unsafe {
        let _ = LocalFree(Some(HLOCAL(sid_text.0.cast())));
    }
    result
}

fn delete_legacy_task_if_owned(folder: &ITaskFolder, identity: &UserIdentity) -> Result<()> {
    let task = match unsafe { folder.GetTask(&BSTR::from(LEGACY_TASK_NAME)) } {
        Ok(task) => task,
        Err(error) if error.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => return Ok(()),
        // A fixed-name legacy task owned by another user may not be readable.
        // It must not block this user's per-user task.
        Err(error) if error.code() == windows::Win32::Foundation::E_ACCESSDENIED => {
            tracing::warn!(event = "autostart_legacy_task_unreadable", error = %error);
            return Ok(());
        }
        Err(error) => return Err(error).context("query legacy autostart task"),
    };
    let definition = unsafe { task.Definition() }.context("read legacy task definition")?;
    let principal = unsafe { definition.Principal() }.context("read legacy task principal")?;
    let mut principal_user = BSTR::new();
    unsafe { principal.UserId(&mut principal_user) }.context("read legacy task principal user")?;
    if identity_matches(principal_user.to_string().as_str(), identity) {
        delete_task_if_exists(folder, LEGACY_TASK_NAME)?;
    }
    Ok(())
}

fn delete_task_if_exists(folder: &ITaskFolder, task_name: &str) -> Result<()> {
    match unsafe { folder.DeleteTask(&BSTR::from(task_name), 0) } {
        Ok(()) => Ok(()),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => Ok(()),
        Err(error) => Err(error).with_context(|| format!("delete autostart task {task_name}")),
    }
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
            Path::new(r"C:\Tools\VRChatFix.exe"),
            Path::new("c:/tools/vrchatfix.exe")
        ));
        assert!(windows_paths_equal(
            Path::new(r"\\?\C:\Tools\VRChatFix.exe"),
            Path::new(r"C:\Tools\VRChatFix.exe")
        ));
        assert!(!windows_paths_equal(
            Path::new(r"C:\Tools\v1\VRChatFix.exe"),
            Path::new(r"C:\Tools\v2\VRChatFix.exe")
        ));
    }

    #[test]
    fn per_user_task_names_are_stable_and_distinct() {
        assert_eq!(
            task_name_for_sid("S-1-5-21-123-1001"),
            task_name_for_sid("s-1-5-21-123-1001")
        );
        assert_ne!(
            task_name_for_sid("S-1-5-21-123-1001"),
            task_name_for_sid("S-1-5-21-123-1002")
        );
    }
}
