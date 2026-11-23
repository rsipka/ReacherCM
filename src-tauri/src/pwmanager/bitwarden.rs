//! Bitwarden (and Vaultwarden), through Bitwarden's official command-line
//! tool `bw`.
//!
//! Reach does not sign in: the user does that once, in a terminal, with
//! `bw login` (after `bw config server …` for a self-hosted server), with
//! whatever two-step login their account has. What that leaves on the
//! computer is an encrypted copy of the vault, locked. Reach only unlocks
//! it:
//!
//! - The **master password** is asked for in the window when a call needs
//!   it, never stored, and kept encrypted in memory for as long as the user
//!   said (see [`super::unlock`]). It is handed to `bw unlock` in the
//!   child's environment (`--passwordenv`), never on the command line.
//! - `bw unlock --raw` answers with a **session key**. It is kept in
//!   memory, shielded like the master password, for as long as the master
//!   password it came from is the one in use ([`sessions`]), so one connect
//!   unlocks once however many entries it reads. It goes to the other calls
//!   in `BW_SESSION`, again the environment.
//!
//! The rules for running the CLI are the ones in [`super::cli`] and
//! [`super::process`]: the binary must be `bw` (`bw.exe`) on this computer
//! and changeable only by the user or the system; the child gets an empty
//! environment plus an allowlist, with `bw`'s and Node's own settings
//! removed (`BW_SESSION`, `BITWARDENCLI_APPDATA_DIR`, `NODE_OPTIONS`,
//! `NODE_EXTRA_CA_CERTS`, `NODE_TLS_REJECT_UNAUTHORIZED`, …);
//! `--nointeraction` and no stdin, so a prompt fails at once; a deadline,
//! capped output, one of the shared CLI slots.
//!
//! Calls: `bw sync` (so a password changed elsewhere is the one read; a
//! failed sync is not an error, the local copy is used), `bw list items`
//! for the picker and `bw get password <id>` at connect time. `bw list
//! items` prints every item whole, passwords included: `bw` has no
//! metadata-only listing. Its output is read into a buffer that is wiped,
//! and only names, usernames and addresses are taken out of it.

use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::path::Path;
#[cfg(desktop)]
use std::time::Duration;

use serde::Deserialize;
use zeroize::Zeroizing;

use super::cli::{self, CliSpec};
use super::config::BitwardenConfig;
use super::unlock::Passphrase;
use super::{sanitize_display, EntrySummary, Failure, PasswordManager, ProviderId, ProviderStatus, SecretKind};

pub(super) const NAME: &str = "Bitwarden";

pub const INSTALL_HINT: &str = "Install the Bitwarden CLI (the `bw` command): `brew install bitwarden-cli` on a Mac, `npm install -g @bitwarden/cli`, or see https://bitwarden.com/help/cli/. Then run `bw login` once in a terminal.";

/// What `bw` inherits from Reach's environment: what a Node program needs
/// to run and reach the server, and the folders it keeps its data in.
pub const INHERITED_ENV: &[&str] = &[
    "PATH",
    "SystemRoot",
    "SYSTEMROOT",
    "windir",
    "SystemDrive",
    "TEMP",
    "TMP",
    "TMPDIR",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "HOME",
    "XDG_CONFIG_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];

/// The master password's variable, named to `bw unlock --passwordenv`.
const PASSWORD_ENV: &str = "BW_PASSWORD";
const SESSION_ENV: &str = "BW_SESSION";

/// `bw`'s and Node's own settings, removed by name in case one ends up in
/// the allowlist.
pub const BW_ENV_KEYS: &[&str] = &[
    SESSION_ENV,
    PASSWORD_ENV,
    "BW_CLIENTID",
    "BW_CLIENTSECRET",
    "BW_NOINTERACTION",
    "BW_RAW",
    "BW_RESPONSE",
    "BW_PRETTY",
    "BW_QUIET",
    "BW_CLEANEXIT",
    "BITWARDENCLI_APPDATA_DIR",
    "BITWARDENCLI_DEBUG",
    "BITWARDEN_CONNECTOR_DEBUG",
    "NODE_OPTIONS",
    "NODE_EXTRA_CA_CERTS",
    "NODE_TLS_REJECT_UNAUTHORIZED",
    "NODE_PATH",
    "NODE_DEBUG",
];

pub(crate) const SPEC: CliSpec = CliSpec {
    name: NAME,
    program: "bw",
    file_names: &["bw", "bw.exe"],
    install_hint: INSTALL_HINT,
    inherited_env: INHERITED_ENV,
    removed_env: BW_ENV_KEYS,
};

/// The most a listing may print: every item whole, as JSON.
pub const STDOUT_CAP: usize = 64 * 1024 * 1024;
pub const STDERR_CAP: usize = 64 * 1024;
#[cfg(desktop)]
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);
/// The longest error text shown.
const MAX_ERROR_CHARS: usize = 300;
/// A session key is 88 base64 characters today; anything much longer is not
/// one.
const MAX_SESSION_CHARS: usize = 512;
/// A connect syncs first unless Reach synced this recently.
#[cfg(desktop)]
const SYNC_EVERY: Duration = Duration::from_secs(60);

/// Bitwarden's item type for a login. Notes, cards and identities hold no
/// password and are left out of the picker.
const TYPE_LOGIN: u8 = 1;

/// What one CLI call does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Version,
    /// Master password in, session key out.
    Unlock,
    Sync,
    List,
    /// The item's id, already checked by [`validate_entry_id`].
    Get(String),
}

impl Op {
    fn args(&self) -> Vec<String> {
        let own: &[&str] = match self {
            Op::Version => return vec!["--version".into()],
            Op::Unlock => &["unlock", "--passwordenv", PASSWORD_ENV, "--raw"],
            Op::Sync => &["sync"],
            Op::List => &["list", "items"],
            Op::Get(id) => return vec!["get".into(), "password".into(), id.clone(), "--raw".into(), "--nointeraction".into()],
        };
        own.iter().map(|s| (*s).to_string()).chain(["--nointeraction".to_string()]).collect()
    }
}

/// A stored entry: the item's UUID, re-formatted so it can never be read as
/// an option.
pub fn validate_entry_id(id: &str) -> Result<String, String> {
    uuid::Uuid::parse_str(id.trim())
        .map(|u| u.hyphenated().to_string())
        .map_err(|_| format!("{NAME}: '{}' is not a Bitwarden entry id", sanitize_display(id.trim(), 60)))
}

/// The command for one call, inheriting from `parent_env` only what the
/// allowlist lets through. `secret` is the master password for
/// [`Op::Unlock`], the session key for every other call but the version.
/// Pure apart from building it, so tests can look at its arguments and
/// environment.
pub fn build_command_with_env(
    bin: &Path,
    op: &Op,
    secret: Option<&str>,
    parent_env: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<tokio::process::Command, String> {
    #[cfg(desktop)]
    let mut cmd = super::process::silent_command(bin);
    #[cfg(not(desktop))]
    let mut cmd = tokio::process::Command::new(bin);

    cli::set_base_env(&SPEC, &mut cmd, parent_env);
    cmd.args(op.args());
    let var = match op {
        Op::Version => return Ok(cmd),
        Op::Unlock => PASSWORD_ENV,
        _ => SESSION_ENV,
    };
    let secret = secret.filter(|s| !s.is_empty()).ok_or_else(|| format!("{NAME}: the master password was not entered."))?;
    cmd.env(var, secret);
    Ok(cmd)
}

// ---------------------------------------------------------------------------
// Output

#[derive(Deserialize)]
struct RawUri {
    #[serde(default)]
    uri: Option<String>,
}

#[derive(Deserialize)]
struct RawLogin {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    uris: Option<Vec<RawUri>>,
}

/// One item, as far as Reach reads it: the password and every other field
/// are skipped by the parser and never leave the wiped buffer.
#[derive(Deserialize)]
struct RawItem {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<u8>,
    #[serde(default)]
    login: Option<RawLogin>,
    #[serde(default, rename = "folderId")]
    folder_id: Option<String>,
}

fn clean_opt(s: Option<String>, max: usize) -> Option<String> {
    s.map(|v| sanitize_display(v.trim(), max)).filter(|v| !v.is_empty())
}

/// The logins in `bw list items` output, metadata only. Items without a
/// valid id are skipped.
pub fn parse_list(stdout: &[u8]) -> Result<Vec<EntrySummary>, String> {
    let raw: Vec<RawItem> = serde_json::from_slice(stdout)
        .map_err(|_| format!("{NAME}: the CLI's list output was not understood"))?;
    let mut out: Vec<EntrySummary> = raw
        .into_iter()
        .filter(|e| e.kind == Some(TYPE_LOGIN))
        .filter_map(|e| {
            let id = validate_entry_id(e.id.as_deref()?).ok()?;
            let (username, uri) = match e.login {
                Some(l) => (
                    clean_opt(l.username, 256),
                    l.uris.and_then(|u| u.into_iter().find_map(|u| clean_opt(u.uri, 1024))),
                ),
                None => (None, None),
            };
            Some(EntrySummary {
                id,
                name: clean_opt(e.name, 256).unwrap_or_default(),
                username,
                uri,
                folder_id: clean_opt(e.folder_id, 64),
            })
        })
        .collect();
    out.sort_by_cached_key(|e| e.name.to_lowercase());
    Ok(out)
}

/// The password `bw get password` printed.
pub fn parse_get(stdout: &[u8]) -> Result<Zeroizing<String>, String> {
    let text = std::str::from_utf8(stdout).map_err(|_| format!("{NAME}: the entry's password is not text"))?;
    let trimmed = text.strip_suffix('\n').unwrap_or(text);
    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
    let value = Zeroizing::new(trimmed.to_string());
    if value.is_empty() {
        return Err(format!("{NAME}: the entry has no password"));
    }
    Ok(value)
}

/// The session key `bw unlock --raw` printed, shielded.
pub fn parse_session(stdout: &[u8]) -> Result<Passphrase, String> {
    let refuse = || format!("{NAME}: the CLI's answer to unlock was not understood");
    let text = Zeroizing::new(std::str::from_utf8(stdout).map_err(|_| refuse())?.trim().to_string());
    let base64 = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_');
    if text.is_empty() || text.len() > MAX_SESSION_CHARS || !text.chars().all(base64) {
        return Err(refuse());
    }
    Passphrase::new(text.to_string()).map_err(|_| refuse())
}

/// The version from `bw --version`: `2025.1.3`.
pub fn parse_version(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    text.split_whitespace().next().map(|v| sanitize_display(v, 40)).filter(|v| !v.is_empty())
}

/// What a failed call's error text says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The master password was rejected: worth asking again.
    WrongPassword,
    /// The session key no longer opens the vault (the user ran `bw lock`,
    /// or signed in anew): unlock again.
    Locked,
    NotLoggedIn,
    Other,
}

/// Which failure the CLI's stderr reports.
pub fn error_kind(stderr: &[u8]) -> ErrorKind {
    let lower = cli::strip_control(&String::from_utf8_lossy(stderr)).to_lowercase();
    if lower.contains("invalid master password") {
        ErrorKind::WrongPassword
    } else if lower.contains("you are not logged in") {
        ErrorKind::NotLoggedIn
    } else if lower.contains("vault is locked") || lower.contains("master password is required") {
        ErrorKind::Locked
    } else {
        ErrorKind::Other
    }
}

/// A user-facing message for a failed call, from its stderr: a plain
/// explanation of the usual failures, or the CLI's last line.
pub fn friendly_error(stderr: &[u8]) -> String {
    let text = cli::strip_control(&String::from_utf8_lossy(stderr));
    let last = text.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("");
    let lower = text.to_lowercase();
    let has = |needle: &str| lower.contains(needle);
    let friendly = match error_kind(stderr) {
        ErrorKind::WrongPassword => "the master password was not accepted.".to_string(),
        ErrorKind::NotLoggedIn => "not signed in. Run `bw login` once in a terminal, then try again.".to_string(),
        ErrorKind::Locked => "the vault is locked.".to_string(),
        ErrorKind::Other if has("not found") => {
            "entry not found. It may have been deleted, or you no longer have access to it.".to_string()
        }
        ErrorKind::Other if has("more than one result") => "the entry id matches more than one entry.".to_string(),
        ErrorKind::Other if last.is_empty() => "the CLI failed without saying why.".to_string(),
        ErrorKind::Other => cli::truncate_chars(last, MAX_ERROR_CHARS),
    };
    format!("{NAME}: {friendly}")
}

fn failure(stderr: &[u8]) -> Failure {
    let message = friendly_error(stderr);
    match error_kind(stderr) {
        ErrorKind::WrongPassword => Failure::WrongPassphrase(message),
        _ => Failure::Other(message),
    }
}

// ---------------------------------------------------------------------------
// The session key, kept while its master password is.

/// The session key `bw unlock` gave for one typing of the master password,
/// and when Reach last synced. In memory only.
pub(crate) mod sessions {
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use super::Passphrase;

    struct Kept {
        /// The serial of the master password it was unlocked with: when
        /// that one is forgotten or replaced, this key is no longer used.
        master: u64,
        key: Arc<Passphrase>,
    }

    #[derive(Default)]
    struct State {
        kept: Option<Kept>,
        synced: Option<Instant>,
    }

    static STATE: Mutex<State> = Mutex::new(State { kept: None, synced: None });

    // Other tests can clear the process-wide cache while a fake CLI call awaits.
    #[cfg(test)]
    tokio::task_local! {
        static TEST_STATE: Mutex<State>;
    }

    #[cfg(test)]
    pub(super) async fn isolated<F: std::future::Future>(test: F) -> F::Output {
        TEST_STATE.scope(Mutex::new(State::default()), test).await
    }

    fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
        #[cfg(test)]
        if TEST_STATE.try_with(|_| ()).is_ok() {
            return TEST_STATE.with(|state| f(&mut state.lock().unwrap_or_else(|e| e.into_inner())));
        }
        f(&mut STATE.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The key unlocked with the master password `master`, if kept.
    pub(crate) fn get(master: u64) -> Option<Arc<Passphrase>> {
        with_state(|state| state.kept.as_ref().filter(|k| k.master == master).map(|k| k.key.clone()))
    }

    pub(crate) fn keep(master: u64, key: Arc<Passphrase>) {
        with_state(|state| state.kept = Some(Kept { master, key }));
    }

    /// Drop this key, if it is still the one kept: the CLI refused it.
    pub(crate) fn drop_key(serial: u64) {
        with_state(|state| {
            if state.kept.as_ref().is_some_and(|k| k.key.serial() == serial) {
                state.kept = None;
            }
        });
    }

    /// Forget the key: every time the master password is forgotten.
    pub(crate) fn forget() {
        with_state(|state| *state = State::default());
    }

    pub(crate) fn synced_within(window: std::time::Duration) -> bool {
        with_state(|state| state.synced.is_some_and(|at| at.elapsed() < window))
    }

    pub(crate) fn mark_synced() {
        with_state(|state| state.synced = Some(Instant::now()));
    }
}

// ---------------------------------------------------------------------------
// The provider

/// Bitwarden, with the configuration for one call.
pub(crate) struct Bitwarden {
    cfg: BitwardenConfig,
    master_enabled: bool,
}

impl Bitwarden {
    pub(crate) fn new(cfg: BitwardenConfig, master_enabled: bool) -> Self {
        Self { cfg, master_enabled }
    }

    /// One CLI call. `secret` is opened only once a slot is free, right
    /// before the child starts.
    #[cfg(desktop)]
    async fn run(&self, op: &Op, secret: Option<&Passphrase>) -> Result<Zeroizing<Vec<u8>>, (ErrorKind, Failure)> {
        use super::process::{run, RunError};

        let other = |m: String| (ErrorKind::Other, Failure::Other(m));
        if *op != Op::Version && !self.cfg.enabled {
            return Err(other(format!("{NAME} is turned off. Turn it on in Settings → Password managers.")));
        }
        let bin = cli::resolve_binary(&SPEC, self.cfg.cli_path.as_deref()).map_err(other)?;
        let timeout = match op {
            Op::Version => VERSION_TIMEOUT,
            _ => Duration::from_secs(u64::from(self.cfg.timeout_secs())),
        };
        let slot = async {
            match op {
                Op::Version => Ok(None),
                _ => super::process::cli_slot().await.map(Some).map_err(|e| format!("{NAME}: {e}")),
            }
        };
        let (_slot, cmd) = super::passbolt::build_when_ready(slot, || {
            // The opened secret is wiped as this returns.
            let opened = secret.map(Passphrase::reveal);
            build_command_with_env(&bin, op, opened.as_ref().map(|s| s.as_str()), std::env::vars_os())
        })
        .await
        .map_err(other)?;
        let out = run(cmd, timeout, STDOUT_CAP, STDERR_CAP).await.map_err(|e| {
            other(match e {
                RunError::Spawn(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    format!("{NAME}: the bw CLI was not found. {INSTALL_HINT}")
                }
                RunError::Spawn(e) => format!("{NAME}: could not start the CLI: {}", e.kind()),
                RunError::Timeout => format!("{NAME}: no answer within {} seconds.", timeout.as_secs()),
                RunError::TooLarge => format!("{NAME}: the CLI printed more than Reacher CM accepts."),
                RunError::Io(e) => format!("{NAME}: reading the CLI's output failed: {}", e.kind()),
            })
        })?;
        if !out.status.success() {
            return Err((error_kind(&out.stderr), failure(&out.stderr)));
        }
        Ok(out.stdout)
    }

    #[cfg(not(desktop))]
    async fn run(&self, _op: &Op, _secret: Option<&Passphrase>) -> Result<Zeroizing<Vec<u8>>, (ErrorKind, Failure)> {
        Err((ErrorKind::Other, Failure::Other(format!("{NAME}: available on desktop only"))))
    }

    /// The session key for this master password: the kept one, or a new
    /// one from `bw unlock`.
    async fn session(&self, master: &Passphrase) -> Result<std::sync::Arc<Passphrase>, Failure> {
        if let Some(key) = sessions::get(master.serial()) {
            return Ok(key);
        }
        let out = self.run(&Op::Unlock, Some(master)).await.map_err(|(_, f)| f)?;
        let key = std::sync::Arc::new(parse_session(&out)?);
        sessions::keep(master.serial(), key.clone());
        Ok(key)
    }

    /// Run `op` unlocked. If the CLI says the kept session key no longer
    /// opens the vault, unlock once more and try again.
    async fn run_unlocked(&self, op: &Op, master: Option<&Passphrase>) -> Result<Zeroizing<Vec<u8>>, Failure> {
        let master = master.ok_or_else(|| Failure::Other(format!("{NAME}: the master password was not entered.")))?;
        let key = self.session(master).await?;
        match self.run(op, Some(&key)).await {
            Err((ErrorKind::Locked, _)) => {
                sessions::drop_key(key.serial());
                let key = self.session(master).await?;
                self.run(op, Some(&key)).await.map_err(|(_, f)| f)
            }
            other => other.map_err(|(_, f)| f),
        }
    }

    /// `bw sync`, unless `fresh_enough` and Reach synced a moment ago.
    /// Failing (offline, say) is not an error: the local copy is used.
    #[cfg(desktop)]
    async fn sync(&self, master: Option<&Passphrase>, fresh_enough: bool) {
        if fresh_enough && sessions::synced_within(SYNC_EVERY) {
            return;
        }
        match self.run_unlocked(&Op::Sync, master).await {
            Ok(_) => sessions::mark_synced(),
            Err(e) => tracing::info!("pwmanager: bitwarden sync failed, using the local copy: {}", e.into_message()),
        }
    }

    #[cfg(not(desktop))]
    async fn sync(&self, _master: Option<&Passphrase>, _fresh_enough: bool) {}
}

impl PasswordManager for Bitwarden {
    async fn status(&self) -> ProviderStatus {
        let mut status = ProviderStatus {
            id: ProviderId::Bitwarden.as_str().into(),
            name: NAME.into(),
            enabled: self.master_enabled && self.cfg.enabled,
            available: false,
            configured: self.cfg.enabled,
            version: None,
            detail: None,
        };
        if !cfg!(desktop) {
            status.detail = Some("Available on desktop only".into());
            return status;
        }
        match cli::resolve_binary(&SPEC, self.cfg.cli_path.as_deref()) {
            Ok(bin) => {
                status.available = true;
                status.detail = Some(bin.display().to_string());
                if status.enabled {
                    if let Ok(out) = self.run(&Op::Version, None).await {
                        status.version = parse_version(&out);
                    }
                }
            }
            Err(e) => status.detail = Some(e),
        }
        status
    }

    /// The master password is always typed, never stored.
    fn passphrase_prompt(&self) -> Option<u32> {
        Some(self.cfg.remember_secs())
    }

    fn secret_kind(&self) -> SecretKind {
        SecretKind::MasterPassword
    }

    fn server_host(&self) -> Option<String> {
        None
    }

    fn hash_account(&self, mut h: &mut dyn Hasher) {
        self.cfg.cli_path.as_deref().map(str::trim).hash(&mut h);
    }

    /// The password only, for now.
    fn check_entry(&self, entry_id: &str, field: Option<&str>) -> Result<String, String> {
        if let Some(f) = field {
            return Err(super::unsupported_field(ProviderId::Bitwarden, f));
        }
        validate_entry_id(entry_id)
    }

    async fn list(&self, passphrase: Option<&Passphrase>) -> Result<Vec<EntrySummary>, Failure> {
        // Unlock first, so a wrong master password is reported as one and
        // not as a failed sync.
        if let Some(master) = passphrase {
            self.session(master).await?;
        }
        self.sync(passphrase, false).await;
        let out = self.run_unlocked(&Op::List, passphrase).await?;
        Ok(parse_list(&out)?)
    }

    async fn resolve(&self, entry_id: &str, passphrase: Option<&Passphrase>) -> Result<Zeroizing<String>, Failure> {
        let id = validate_entry_id(entry_id)?;
        if let Some(master) = passphrase {
            self.session(master).await?;
        }
        self.sync(passphrase, true).await;
        let out = self.run_unlocked(&Op::Get(id), passphrase).await?;
        Ok(parse_get(&out)?)
    }
}

#[cfg(test)]
#[path = "bitwarden_tests.rs"]
mod tests;
