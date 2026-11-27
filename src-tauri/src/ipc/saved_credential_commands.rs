//! Saved credentials: a login kept once and used by many sessions.
//!
//! A credential lives in a vault like the sessions that use it: a shared
//! vault's credentials in that vault, this device's own in `__sessions__`.
//! A session links one by `credential_id`, or takes the one its nearest
//! folder names; only a credential in the session's own vault counts.
//!
//! The link is resolved when a session is read ([`fill`]): the credential's
//! username, domain and login take the place of the session's own, which
//! stay stored as they were. So a session whose credential is gone (deleted,
//! or not synced to this device yet) still has a login, and an older Reach
//! reading the session sees the login it always had. Everything that reads
//! a session (the list, the editor, the connect commands, the approvals)
//! sees it filled in, so the approvals are given for the login really used.
//!
//! A credential's secret can be a password manager entry
//! ([`crate::pwmanager::SecretRef`]) instead of a stored password or
//! passphrase. Such an entry is approved like one of a session's own: per
//! person, for the session it is used in, as that session is (see
//! `session_commands::ref_slots`). So a credential never carries an
//! approval to the sessions that use it: the reference in the credential
//! record holds none (any found there are dropped), and each session keeps
//! the approvals for the entries its credential brings in its own record
//! (`SessionConfig::credential_ref_approvals`, by slot). [`fill`] puts them
//! on the filled-in reference when a session is read, where they are
//! verified like any other; [`keep_ref_approvals`] takes them back off
//! before the session is stored. A session whose credential points at an
//! entry therefore asks once, the first time each person connects with it,
//! and again if the credential is pointed at another entry.

use crate::ipc::password_history as pwh;
use crate::ipc::personal_login_commands::{DEFAULT_KEY, PERSONAL_LOGIN};
use crate::ipc::session_commands as sessions;
use crate::state::{AppState, AuthMethod, CredentialUse, Folder, JumpHostConfig, SavedCredential, SessionConfig};
use crate::vault::types::SecretCategory;
use secrecy::{ExposeSecret, SecretBox};
use tauri::State;

/// A session's `credential_id` that says: its own login, even in a folder
/// that names a credential.
pub(crate) const OWN_LOGIN: &str = "own";

/// The person's own choices of login for sessions and folders that ask for
/// each person's own, by session or folder id (see
/// `personal_login_commands`).
pub(crate) type Choices = std::collections::HashMap<String, String>;

/// A session's own login, as stored, while a credential's is filled in.
#[derive(Debug, Clone)]
pub(crate) struct OwnLogin {
    pub username: String,
    pub domain: Option<String>,
    pub auth_method: AuthMethod,
}

impl OwnLogin {
    pub(crate) fn of(s: &SessionConfig) -> Self {
        OwnLogin { username: s.username.clone(), domain: s.domain.clone(), auth_method: s.auth_method.clone() }
    }

    pub(crate) fn put_into(self, s: &mut SessionConfig) {
        s.username = self.username;
        s.domain = self.domain;
        s.auth_method = self.auth_method;
    }

    fn same(&self, other: &OwnLogin) -> bool {
        // A reference's approvals and flags are not part of which login it
        // is: they differ between a read and what the editor sends back.
        let method = |m: &AuthMethod| {
            let mut m = m.clone();
            if let Some((_, r)) = sessions::auth_ref_mut(&mut m) {
                *r = bare_ref(r);
            }
            serde_json::to_value(&m).ok()
        };
        self.username == other.username
            && self.domain.as_deref().filter(|d| !d.is_empty()) == other.domain.as_deref().filter(|d| !d.is_empty())
            && method(&self.auth_method) == method(&other.auth_method)
    }
}

impl OwnLogin {
    fn of_hop(j: &JumpHostConfig) -> Self {
        OwnLogin { username: j.username.clone(), domain: None, auth_method: j.auth_method.clone() }
    }

    fn put_into_hop(self, j: &mut JumpHostConfig) {
        j.username = self.username;
        j.auth_method = self.auth_method;
    }

    /// The same login with no secret: what is stored as a session's own
    /// when all it was given is a credential's.
    fn bare(&self) -> Self {
        OwnLogin { auth_method: AuthMethod::Agent, ..self.clone() }
    }
}

/// Every login in a session: its own, then each jump host's, in order.
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionLogins {
    pub session: Option<OwnLogin>,
    pub hops: Vec<Option<OwnLogin>>,
}

/// Which parts a credential filled in, and the login each had before.
pub(crate) type Replaced = SessionLogins;

impl SessionLogins {
    /// All of a session's logins as they are now.
    pub(crate) fn all(s: &SessionConfig) -> Self {
        SessionLogins {
            session: Some(OwnLogin::of(s)),
            hops: s.jump_chain.iter().flatten().map(|j| Some(OwnLogin::of_hop(j))).collect(),
        }
    }

    /// Only the parts that `replaced` says a credential filled in.
    pub(crate) fn only(mut self, replaced: &Replaced) -> Self {
        if replaced.session.is_none() {
            self.session = None;
        }
        for (i, h) in self.hops.iter_mut().enumerate() {
            if replaced.hops.get(i).is_none_or(|r| r.is_none()) {
                *h = None;
            }
        }
        self
    }

    /// Put each login that is set into the session.
    pub(crate) fn put_into(self, s: &mut SessionConfig) {
        if let Some(l) = self.session {
            l.put_into(s);
        }
        for (j, l) in s.jump_chain.iter_mut().flatten().zip(self.hops) {
            if let Some(l) = l {
                l.put_into_hop(j);
            }
        }
    }
}

/// A reference with nothing but what it points at: no approvals, no flags.
fn bare_ref(r: &crate::pwmanager::SecretRef) -> crate::pwmanager::SecretRef {
    crate::pwmanager::SecretRef {
        provider: r.provider.clone(),
        entry_id: r.entry_id.clone(),
        field: r.field.clone(),
        label: r.label.clone(),
        ..Default::default()
    }
}

/// A credential's login for the part of a session filed under `slot`
/// (`""` for the session's own, `"jump:0:"` for its first jump host). A
/// password manager entry in it gets the approvals this session keeps for
/// it, and none from the credential record.
fn login_of(c: &SavedCredential, slot: &str, kept: &std::collections::BTreeMap<String, Vec<String>>) -> OwnLogin {
    let mut auth_method = c.auth_method.clone();
    if let Some((field, r)) = sessions::auth_ref_mut(&mut auth_method) {
        *r = bare_ref(r);
        r.approvals = kept.get(&format!("{slot}{field}")).cloned().unwrap_or_default();
    }
    OwnLogin { username: c.username.clone(), domain: c.domain.clone(), auth_method }
}

/// Before a session read with [`fill`] is stored with its own logins
/// (`own`: the parts a credential filled in) put back: keep the approvals
/// on the entries the credential brought, by slot, in the session. A part
/// no credential fills now keeps what it had, for the day the credential is
/// back (it may only be missing on this device).
pub(crate) fn keep_ref_approvals(s: &mut SessionConfig, own: &SessionLogins) {
    let put = |slot: &str, m: &AuthMethod, kept: &mut std::collections::BTreeMap<String, Vec<String>>| {
        kept.retain(|k, _| !(k.starts_with(slot) && !k[slot.len()..].contains(':')));
        if let Some((field, r)) = sessions::auth_ref(m) {
            if !r.approvals.is_empty() {
                kept.insert(format!("{slot}{field}"), r.approvals.clone());
            }
        }
    };
    let mut kept = std::mem::take(&mut s.credential_ref_approvals);
    if own.session.is_some() {
        put("", &s.auth_method, &mut kept);
    }
    for (i, (j, l)) in s.jump_chain.iter().flatten().zip(&own.hops).enumerate() {
        if l.is_some() {
            put(&format!("jump:{i}:"), &j.auth_method, &mut kept);
        }
    }
    s.credential_ref_approvals = kept;
}

/// The credential a session in `vault` (None: this device's own) logs in
/// with, and the folder it comes from (None: the session links it).
/// `Some((id, None))` with no folder means the session names it itself.
fn wanted(s: &SessionConfig, vault: Option<&str>, folders: &[Folder]) -> Option<(String, Option<String>)> {
    if let Some(id) = s.credential_id.as_deref().filter(|id| !id.is_empty()) {
        return (id != OWN_LOGIN).then(|| (id.to_string(), None));
    }
    let mut current = s.folder_id.as_deref().filter(|f| !f.is_empty());
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            break;
        }
        // A folder in another vault is not this session's (they cannot be
        // nested across vaults); its credential would not be either.
        let folder = folders.iter().find(|f| f.id == id && f.vault_id.as_deref() == vault)?;
        if let Some(cred) = folder.credential_id.as_deref().filter(|c| !c.is_empty()) {
            return Some((cred.to_string(), Some(folder.id.clone())));
        }
        current = folder.parent_id.as_deref();
    }
    None
}

/// Fill in the credentials that apply to a session in `vault`: the
/// session's own (linked, or from its folder) and each jump host's (linked
/// only: a folder's credential is for the servers in it, not the hosts on
/// the way). Returns the logins replaced, per part (None where nothing was:
/// no credential applies, or it is missing).
///
/// A session or folder that asks for each person's own login gets the one
/// the reading person chose (`choices`): one of their own credentials, or
/// the session's user name with their default key. None chosen yet: the
/// session's own stored login stays, and the use says it is missing.
pub(crate) fn fill(s: &mut SessionConfig, vault: Option<&str>, folders: &[Folder], creds: &[SavedCredential], choices: &Choices) -> Replaced {
    let find = |id: &str| creds.iter().find(|c| c.id == id && c.vault_id.as_deref() == vault);
    let mut replaced = Replaced::default();
    s.credential = None;
    if let Some((id, folder_id)) = wanted(s, vault, folders) {
        if id == PERSONAL_LOGIN {
            let target = folder_id.clone().unwrap_or_else(|| s.id.clone());
            let chosen = choices.get(&target).and_then(|c| personal_login(s, c, creds));
            let missing = chosen.is_none();
            let own_username = Some(s.username.clone());
            let name = match chosen {
                Some((name, login)) => {
                    replaced.session = Some(OwnLogin::of(s));
                    login.put_into(s);
                    name
                }
                None => String::new(),
            };
            s.credential = Some(CredentialUse { id, name, folder_id, missing, personal_target: Some(target), own_username });
        } else {
            match find(&id) {
                Some(c) => {
                    replaced.session = Some(OwnLogin::of(s));
                    login_of(c, "", &s.credential_ref_approvals).put_into(s);
                    s.credential = Some(CredentialUse { id, name: c.name.clone(), folder_id, missing: false, personal_target: None, own_username: None });
                }
                None => s.credential = Some(CredentialUse { id, name: String::new(), folder_id, missing: true, personal_target: None, own_username: None }),
            }
        }
    }
    let kept = &s.credential_ref_approvals;
    for (i, j) in s.jump_chain.iter_mut().flatten().enumerate() {
        j.credential = None;
        let Some(id) = j.credential_id.clone().filter(|id| !id.is_empty() && id != OWN_LOGIN && id != PERSONAL_LOGIN) else {
            replaced.hops.push(None);
            continue;
        };
        match find(&id) {
            Some(c) => {
                replaced.hops.push(Some(OwnLogin::of_hop(j)));
                let l = login_of(c, &format!("jump:{i}:"), kept);
                j.username = l.username;
                j.auth_method = l.auth_method;
                j.credential = Some(CredentialUse { id, name: c.name.clone(), folder_id: None, missing: false, personal_target: None, own_username: None });
            }
            None => {
                replaced.hops.push(None);
                j.credential = Some(CredentialUse { id, name: String::new(), folder_id: None, missing: true, personal_target: None, own_username: None });
            }
        }
    }
    replaced
}

/// The login a person's choice `chosen` gives session `s`, and its name:
/// one of their own credentials (vault None), or [`DEFAULT_KEY`] with the
/// session's user name. None when the credential is gone.
fn personal_login(s: &SessionConfig, chosen: &str, creds: &[SavedCredential]) -> Option<(String, OwnLogin)> {
    if chosen == DEFAULT_KEY {
        let auth_method = AuthMethod::Key { path: None, passphrase: None, key_content: None, key_id: Some(DEFAULT_KEY.into()), passphrase_ref: None };
        return Some((String::new(), OwnLogin { username: s.username.clone(), domain: s.domain.clone(), auth_method }));
    }
    let c = creds.iter().find(|c| c.id == chosen && c.vault_id.is_none())?;
    Some((c.name.clone(), login_of(c, "", &s.credential_ref_approvals)))
}

/// Fill in a session being saved, and work out which of its logins to
/// store as its own (see [`login_to_store`]), part by part. `before` is the
/// stored record as read: its own logins and the ones a credential filled
/// in; None for a new session. A part a credential fills in now keeps the
/// filled-in login in `s` (it is sealed with that) and its own is returned
/// for storing; a part without one gets the chosen login put into `s`.
pub(crate) fn fill_for_save(
    s: &mut SessionConfig,
    vault: Option<&str>,
    folders: &[Folder],
    creds: &[SavedCredential],
    choices: &Choices,
    before: Option<(&SessionLogins, &SessionLogins)>,
) -> SessionLogins {
    let sent = SessionLogins::all(s);
    let replaced = fill(s, vault, folders, creds, choices);
    let now = SessionLogins::all(s).only(&replaced);
    let (stored_own, stored_filled) = match before {
        Some((own, filled)) => (own.clone(), filled.clone()),
        None => (SessionLogins::default(), SessionLogins::default()),
    };
    let choose = |sent: Option<OwnLogin>, now: Option<&OwnLogin>, then: Option<&OwnLogin>, own: Option<OwnLogin>| {
        let sent = sent?;
        let filled: Vec<&OwnLogin> = now.into_iter().chain(then).collect();
        // A new session, or a new hop: nothing of its own is stored yet.
        let own = own.or_else(|| now.map(OwnLogin::bare));
        Some(login_to_store(sent, &filled, own))
    };
    let mut keep = SessionLogins::default();
    let session = choose(sent.session, now.session.as_ref(), stored_filled.session.as_ref(), stored_own.session);
    match (&now.session, session) {
        (Some(_), chosen) => keep.session = chosen,
        (None, Some(chosen)) => chosen.put_into(s),
        (None, None) => {}
    }
    let hop = |v: &Vec<Option<OwnLogin>>, i: usize| v.get(i).cloned().flatten();
    for (i, sent_hop) in sent.hops.into_iter().enumerate() {
        let filled_now = hop(&now.hops, i);
        let chosen = choose(sent_hop, filled_now.as_ref(), hop(&stored_filled.hops, i).as_ref(), hop(&stored_own.hops, i));
        if filled_now.is_some() {
            keep.hops.push(chosen);
        } else {
            if let (Some(chosen), Some(j)) = (chosen, s.jump_chain.as_mut().and_then(|c| c.get_mut(i))) {
                chosen.put_into_hop(j);
            }
            keep.hops.push(None);
        }
    }
    keep
}

/// Which login to store for a session being saved. `sent` is the login in
/// the saved session as it came in; `filled` the logins a credential filled
/// in, now and when the stored record was read; `stored` the stored
/// record's own login. When what came in is one a credential filled in, it
/// is not the session's own (the editor shows the filled-in login): the
/// stored one is kept, and a credential's password is never copied into the
/// session.
pub(crate) fn login_to_store(sent: OwnLogin, filled: &[&OwnLogin], stored: Option<OwnLogin>) -> OwnLogin {
    match stored {
        Some(own) if filled.iter().any(|f| f.same(&sent)) => own,
        _ => sent,
    }
}

/// Every saved credential: this device's own (vault None) and each user
/// vault's. One a vault does not open here is left out until it does.
pub(crate) async fn read_credentials(manager: &crate::vault::VaultManager) -> Vec<SavedCredential> {
    let mut out = Vec::new();
    if let Some(vault) = sessions::sessions_vault_id(manager) {
        out.extend(credentials_in(manager, &vault, None).await);
    }
    for vault in manager.list_vaults().await.unwrap_or_default() {
        if vault.name.starts_with("__") {
            continue;
        }
        for c in credentials_in(manager, &vault.id, Some(vault.id.clone())).await {
            // The same credential in two vaults (a vault joined twice): once.
            if !out.iter().any(|o: &SavedCredential| o.id == c.id) {
                out.push(c);
            }
        }
    }
    out
}

async fn credentials_in(manager: &crate::vault::VaultManager, vault_id: &str, as_vault: Option<String>) -> Vec<SavedCredential> {
    let Ok(secrets) = manager.read_secrets_in(vault_id, &["credential"]).await else { return Vec::new() };
    secrets
        .into_iter()
        .filter_map(|(_, plaintext)| plaintext.ok())
        .filter_map(|p| serde_json::from_slice::<SavedCredential>(p.expose_secret()).ok())
        .map(|mut c| {
            // A vault has its own id on each device.
            c.vault_id = as_vault.clone();
            c
        })
        .collect()
}

/// What `fill` needs, read once.
pub(crate) struct Logins {
    pub folders: Vec<Folder>,
    pub creds: Vec<SavedCredential>,
    pub choices: Choices,
}

pub(crate) async fn logins(manager: &crate::vault::VaultManager) -> Logins {
    Logins {
        folders: sessions::all_folders(manager).await,
        creds: read_credentials(manager).await,
        choices: crate::ipc::personal_login_commands::read_choices(manager).await,
    }
}

/// The vault id a session's credential must be in, from the vault it is
/// stored in: None for this device's own sessions vault.
pub(crate) fn vault_of(manager: &crate::vault::VaultManager, storage_vault: &str) -> Option<String> {
    (sessions::sessions_vault_id(manager).as_deref() != Some(storage_vault)).then(|| storage_vault.to_string())
}

/// A credential as it can be saved: named, with a username. A password
/// manager entry in it must name a manager Reach knows and an entry, is
/// stored with no approvals (see the module docs), and takes the place of
/// a typed secret, which is dropped.
fn check(name: &str, username: &str, auth: &mut AuthMethod) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("A credential needs a name".to_string());
    }
    if username.trim().is_empty() {
        return Err("A credential needs a username".to_string());
    }
    if let Some((_, r)) = sessions::auth_ref_mut(auth) {
        crate::pwmanager::ProviderId::parse(&r.provider)?;
        if r.entry_id.trim().is_empty() {
            return Err("The password manager entry has no id".to_string());
        }
        *r = bare_ref(r);
    }
    match auth {
        AuthMethod::Password { password, password_ref: Some(_) } => *password = None,
        AuthMethod::Key { passphrase, passphrase_ref: Some(_), .. } => *passphrase = None,
        _ => {}
    }
    Ok(())
}

/// Where a credential's row is: its vault (as stored) and row id.
async fn locate(manager: &crate::vault::VaultManager, id: &str) -> Option<(String, String)> {
    if let Some(vault) = sessions::sessions_vault_id(manager) {
        if let Some(row) = sessions::row_id_in(manager, &vault, id).await {
            return Some((vault, row));
        }
    }
    let vault = sessions::user_vault_holding(manager, id).await?;
    Some((vault, id.to_string()))
}

/// All saved credentials the user can use, each with its vault.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn saved_credential_list(state: State<'_, AppState>) -> Result<Vec<SavedCredential>, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Ok(Vec::new());
    }
    let mut list = read_credentials(&manager).await;
    // Old passwords are read one credential at a time, when asked for.
    for c in &mut list {
        c.password_history = None;
    }
    Ok(list)
}

/// Save a new credential in a vault (None, or one not a user vault: this
/// device's own).
#[tauri::command]
// The login carries a password or key; only the vault is logged.
#[tracing::instrument(skip_all, fields(vault = ?vault_id))]
pub async fn saved_credential_create(
    state: State<'_, AppState>,
    name: String,
    username: String,
    domain: Option<String>,
    mut auth_method: AuthMethod,
    vault_id: Option<String>,
    folder_id: Option<String>,
) -> Result<SavedCredential, String> {
    check(&name, &username, &mut auth_method)?;
    let mut manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked. Set a master password first.".to_string());
    }
    let vault_id = match vault_id {
        Some(v) if sessions::user_vault(&manager, &v).await? => Some(v),
        _ => None,
    };
    let storage = match vault_id.clone() {
        Some(v) => v,
        None => sessions::sessions_vault(&mut manager).await?,
    };
    let cred = SavedCredential {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.trim().to_string(),
        username: username.trim().to_string(),
        domain: domain.filter(|d| !d.trim().is_empty()),
        auth_method,
        vault_id: vault_id.clone(),
        password_history: None,
        folder_id: crate::ipc::credential_folder_commands::folder_to_keep(&manager, folder_id, vault_id.as_deref()).await,
    };
    let json = serde_json::to_vec(&cred).map_err(|e| e.to_string())?;
    manager
        .create_secret_with_id(&storage, &cred.id, &cred.name, SecretCategory::Credential, SecretBox::new(Box::new(json)))
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("Created saved credential {}", cred.id);
    Ok(cred)
}

/// Change a credential's name or login. It stays in its vault.
#[tauri::command]
#[tracing::instrument(skip_all, fields(id = %credential.id))]
pub async fn saved_credential_update(
    state: State<'_, AppState>,
    mut credential: SavedCredential,
) -> Result<SavedCredential, String> {
    check(&credential.name, &credential.username, &mut credential.auth_method)?;
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let (vault, row) = locate(&manager, &credential.id)
        .await
        .ok_or_else(|| format!("Credential not found: {}", credential.id))?;
    // The history is the stored one, with the stored password in front
    // when this save replaces it; never what the window sent.
    let stored = manager
        .read_secret(&vault, &row)
        .await
        .ok()
        .and_then(|p| serde_json::from_slice::<SavedCredential>(p.expose_secret()).ok());
    let password_history = pwh::carry(
        stored.as_ref().and_then(|c| c.password_history.as_deref()),
        stored.as_ref().and_then(|c| pwh::typed_password(&c.auth_method)),
        pwh::typed_password(&credential.auth_method),
        pwh::limit(&manager).await,
        pwh::now(),
    );
    let vault_id = vault_of(&manager, &vault);
    let folder_id = crate::ipc::credential_folder_commands::folder_to_keep(&manager, credential.folder_id.take(), vault_id.as_deref()).await;
    let mut cred = SavedCredential {
        name: credential.name.trim().to_string(),
        username: credential.username.trim().to_string(),
        domain: credential.domain.filter(|d| !d.trim().is_empty()),
        vault_id,
        password_history,
        folder_id,
        ..credential
    };
    let json = serde_json::to_vec(&cred).map_err(|e| e.to_string())?;
    manager.update_secret(&vault, &row, SecretBox::new(Box::new(json))).await.map_err(|e| e.to_string())?;
    tracing::info!("Updated saved credential {}", cred.id);
    cred.password_history = None;
    Ok(cred)
}

/// Delete a credential. Sessions and folders that name it stop naming it,
/// and go back to their own stored login. Returns how many were changed.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn saved_credential_delete(state: State<'_, AppState>, credential_id: String) -> Result<u32, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let Some((vault, row)) = locate(&manager, &credential_id).await else {
        tracing::warn!("saved_credential_delete: {credential_id} is not in any open vault");
        return Ok(0);
    };
    let unlinked = sessions::unlink_credential(&manager, &credential_id).await?;
    manager.delete_secret(&vault, &row).await.map_err(|e| e.to_string())?;
    tracing::info!("Deleted saved credential {credential_id}, unlinked from {unlinked} sessions and folders");
    Ok(unlinked)
}

/// A passphrase-protected key that saved credentials log in with.
struct LockedKey {
    material: String,
    /// The key's name in the key store, or the path of its file.
    label: String,
    /// The credentials that use it, by name.
    used_by: Vec<String>,
}

/// One credential's key as it is used at connect: an imported key before a
/// key file. `None` when it has no key here, or its passphrase comes with
/// it (stored, or a password manager entry asked for at connect).
fn key_of(c: &SavedCredential, stored: &[(String, String, crate::ssh::keystore::StoredKeyMaterial)]) -> Option<(String, String, Option<String>)> {
    let AuthMethod::Key { path, passphrase, key_id, passphrase_ref: None, .. } = &c.auth_method else { return None };
    let typed = passphrase.clone().filter(|p| !p.is_empty());
    if let Some(id) = key_id.as_deref().filter(|i| !i.is_empty()) {
        let (_, name, m) = stored.iter().find(|(k, _, _)| k == id)?;
        return Some((m.private_key.clone(), name.clone(), typed.or_else(|| m.passphrase.clone().filter(|p| !p.is_empty()))));
    }
    let path = path.as_deref().map(str::trim).filter(|p| !p.is_empty())?;
    let material = std::fs::read_to_string(crate::ssh::client::expand_tilde(path)).ok()?;
    Some((material, path.to_string(), typed))
}

/// The keys to ask for: each one once, whichever credentials use it, and
/// only those that do not open with what is stored or already kept.
fn locked_keys(creds: &[SavedCredential], stored: &[(String, String, crate::ssh::keystore::StoredKeyMaterial)]) -> Vec<LockedKey> {
    let mut out: Vec<(Vec<u8>, LockedKey)> = Vec::new();
    for c in creds {
        let Some((material, label, passphrase)) = key_of(c, stored) else { continue };
        if crate::ssh::client::key_is_encrypted(&material) != Some(true) || crate::ssh::unlocked::opens(&material, passphrase.as_deref()) {
            continue;
        }
        let hash = crate::ssh::unlocked::hash(&material).to_vec();
        match out.iter_mut().find(|(h, _)| *h == hash) {
            Some((_, k)) => k.used_by.push(c.name.clone()),
            None => out.push((hash, LockedKey { material, label, used_by: vec![c.name.clone()] })),
        }
    }
    out.into_iter().map(|(_, k)| k).collect()
}

/// What unlocking at start did.
#[derive(Debug, Default, serde::Serialize)]
pub struct KeysUnlocked {
    /// Keys opened now and kept for this run.
    pub unlocked: u32,
    /// Keys left locked (cancelled, or the passphrase was wrong three
    /// times): they ask at connect.
    pub skipped: u32,
    /// Of `unlocked`, keys opened by the passphrases remembered behind the
    /// biometric.
    pub by_biometric: u32,
    /// Passphrases newly remembered behind the biometric.
    pub remembered: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remember_error: Option<String>,
}

/// Remembering key passphrases behind the device biometric
/// (`ssh::biometric_keys`): on, with Android's prompt texts. iOS words its
/// own prompt.
#[derive(serde::Deserialize)]
pub struct KeysBiometric {
    /// Shown when the remembered passphrases are opened.
    open: crate::ipc::vault_commands::PromptText,
    /// Shown when new passphrases are remembered.
    save: crate::ipc::vault_commands::PromptText,
}

/// Only one round of questions at a time.
static UNLOCKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static BIOMETRIC_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static BIOMETRIC_STORE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Settings → Security → "Unlock credential keys at start", switched on or
/// off. On: a key passphrase typed from now on is kept in memory until the
/// vault locks or the app quits. Off: what was kept is forgotten.
#[tauri::command]
pub fn credential_keys_unlock_enabled(enabled: bool) {
    crate::ssh::unlocked::set_enabled(enabled);
}

/// Forget the key passphrases remembered behind the device biometric
/// (the setting went off). The passphrases kept for this run stay.
#[tauri::command]
#[tracing::instrument(skip(app, state))]
pub async fn credential_keys_biometric_forget(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if crate::ssh::biometric_keys::supported() {
        BIOMETRIC_REVISION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _store = BIOMETRIC_STORE.lock().await;
        let dir = state.vault_manager.lock().await.app_dir().to_path_buf();
        crate::ssh::biometric_keys::forget(&app, &dir).await?;
    }
    Ok(())
}

/// Ask for the passphrase of every passphrase-protected key the saved
/// credentials use, and keep each in memory for this run, so connecting
/// with those credentials does not ask again. Called by the window once the
/// vault is open, when the setting is on. Cancel leaves that key locked.
#[tauri::command]
#[tracing::instrument(skip(app, state, biometric))]
pub async fn credential_keys_unlock(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    biometric: Option<KeysBiometric>,
) -> Result<KeysUnlocked, String> {
    use crate::ssh::prompt::{ask, Field, Kind};
    use crate::ssh::unlocked;
    use std::sync::atomic::Ordering;

    crate::ssh::unlocked::set_enabled(true);
    if UNLOCKING.swap(true, Ordering::SeqCst) {
        return Ok(KeysUnlocked::default());
    }
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            UNLOCKING.store(false, Ordering::SeqCst);
        }
    }
    let _done = Done;
    let biometric_revision = BIOMETRIC_REVISION.load(Ordering::SeqCst);

    // Read under the lock, ask without it: a question can stay open for
    // minutes and everything else needs the vault meanwhile.
    let biometric = biometric.filter(|_| crate::ssh::biometric_keys::supported());
    let (keys, dir, revision) = {
        let manager = state.vault_manager.lock().await;
        if manager.is_locked() || manager.is_held() {
            return Ok(KeysUnlocked::default());
        }
        let creds = read_credentials(&manager).await;
        let stored = crate::ipc::sshkey_commands::all_materials(&manager).await;
        (locked_keys(&creds, &stored), manager.app_dir().to_path_buf(), unlocked::revision())
    };
    let round_current = || unlocked::enabled() && unlocked::revision() == revision;
    let biometric_current = || BIOMETRIC_REVISION.load(Ordering::SeqCst) == biometric_revision;

    let mut result = KeysUnlocked::default();
    // One biometric check opens every passphrase remembered behind it.
    // Cancelled, the keys are asked for as usual, and nothing typed now is
    // remembered (that would lose the ones not opened).
    let mut remembered: Option<Vec<crate::ssh::biometric_keys::Entry>> = None;
    let keys = match biometric.as_ref().filter(|_| biometric_current()) {
        Some(bio) if !keys.is_empty() && crate::ssh::biometric_keys::exists(&dir) => {
            match crate::ssh::biometric_keys::open(&app, &dir, &bio.open).await {
                Ok(entries) if biometric_current() => {
                    let manager = state.vault_manager.lock().await;
                    if !round_current() || manager.is_locked() || manager.is_held() {
                        return Ok(result);
                    }
                    for entry in &entries {
                        unlocked::remember_ids(&entry.ids, &entry.passphrase, revision);
                    }
                    drop(manager);
                    let (open, still): (Vec<_>, Vec<_>) =
                        keys.into_iter().partition(|key| unlocked::opens(&key.material, None));
                    result.by_biometric = open.len() as u32;
                    result.unlocked += result.by_biometric;
                    remembered = Some(entries);
                    still
                }
                Ok(_) => keys,
                Err(crate::ssh::biometric_keys::OpenError::Gone) => {
                    tracing::info!("Remembered key passphrases are void (biometric changed); asking again");
                    remembered = Some(Vec::new());
                    keys
                }
                Err(crate::ssh::biometric_keys::OpenError::Declined(e)) => {
                    tracing::info!("Remembered key passphrases not opened: {}", e);
                    keys
                }
            }
        }
        Some(_) => {
            remembered = Some(Vec::new());
            keys
        }
        None => keys,
    };

    let mut typed: Vec<crate::ssh::biometric_keys::Entry> = Vec::new();
    for key in keys {
        let used_by = key.used_by.join(", ");
        let mut opened = false;
        for attempt in 0..3 {
            if !round_current() {
                return Ok(result);
            }
            // The window words the question itself (`unlock`): the
            // credential names as the host, the key as the title, and
            // "retry" after a wrong passphrase.
            let field = vec![Field { text: key.label.clone(), echo: false }];
            let Some(answers) = ask(Some(&app), &used_by, 0, Kind::Unlock, &key.label, if attempt == 0 { "" } else { "retry" }, field).await else {
                break;
            };
            // Locked, or switched off, while the question was open.
            let pass = zeroize::Zeroizing::new(answers.into_iter().next().unwrap_or_default());
            if crate::ssh::client::decode_key(&key.material, Some(&pass)).is_ok_and(|k| !k.is_encrypted()) {
                let manager = state.vault_manager.lock().await;
                if !round_current() || manager.is_locked() || manager.is_held() {
                    return Ok(result);
                }
                let ids = unlocked::key_ids(&key.material);
                unlocked::remember_ids(&ids, &pass, revision);
                if remembered.is_some() && biometric_current() {
                    typed.push(crate::ssh::biometric_keys::Entry { ids, passphrase: pass.to_string() });
                }
                opened = true;
                break;
            }
        }
        if opened {
            result.unlocked += 1;
        } else {
            result.skipped += 1;
        }
    }
    if let (Some(bio), Some(old), false) = (&biometric, remembered, typed.is_empty()) {
        let _store = BIOMETRIC_STORE.lock().await;
        if !round_current() || !biometric_current() {
            return Ok(result);
        }
        let count = typed.len() as u32;
        let entries = crate::ssh::biometric_keys::merge(old, typed);
        match crate::ssh::biometric_keys::save(&app, &dir, &entries, &bio.save).await {
            Ok(()) if biometric_current() && round_current() => result.remembered = count,
            Ok(()) => {}
            Err(e) if biometric_current() && round_current() => result.remember_error = Some(e),
            Err(_) => {}
        }
    }
    tracing::info!(
        "Credential keys at start: {} unlocked ({} by biometric), {} left locked, {} remembered",
        result.unlocked,
        result.by_biometric,
        result.skipped,
        result.remembered
    );
    Ok(result)
}

/// Check that a credential a session or folder in `vault` names is one it
/// can use: a credential in that same vault.
pub(crate) fn check_link(creds: &[SavedCredential], credential_id: Option<&str>, vault: Option<&str>) -> Result<(), String> {
    let Some(id) = credential_id.filter(|id| !id.is_empty() && *id != OWN_LOGIN && *id != PERSONAL_LOGIN) else { return Ok(()) };
    if creds.iter().any(|c| c.id == id && c.vault_id.as_deref() == vault) {
        Ok(())
    } else {
        Err("That credential is not in the same vault, so it cannot be used here".to_string())
    }
}

/// Check every credential a session names: its own and its jump hosts'.
pub(crate) fn check_links(creds: &[SavedCredential], s: &SessionConfig, vault: Option<&str>) -> Result<(), String> {
    check_link(creds, s.credential_id.as_deref(), vault)?;
    for j in s.jump_chain.iter().flatten() {
        if j.credential_id.as_deref() == Some(PERSONAL_LOGIN) {
            return Err("A jump host cannot ask for each person's own login yet; link a credential or use its own login".to_string());
        }
        check_link(creds, j.credential_id.as_deref(), vault)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The throwaway keys of the keystore tests.
    const PLAIN_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACCwsCZ1pFDo+/0R+iXN9Emzi+emmN0GEbCoIL9hE1eafgAAAJhbzNn6W8zZ\n+gAAAAtzc2gtZWQyNTUxOQAAACCwsCZ1pFDo+/0R+iXN9Emzi+emmN0GEbCoIL9hE1eafg\nAAAEDs2ThleIPEbrfMS5+KvabXLE0113+oUG6NIEaDtPChvLCwJnWkUOj7/RH6Jc30SbOL\n56aY3QYRsKggv2ETV5p+AAAAEnJlYWNoLXRlc3RAZXhhbXBsZQECAw==\n-----END OPENSSH PRIVATE KEY-----\n";
    const ENCRYPTED_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABCcqk0FV4\nKt6ydcd+Fwf0h1AAAAGAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIJk+zQU8O+k3l0Y9\nx4epBs/36Jir8BpbhqTJH0+tr864AAAAoPYA20IkAMre5HEKk0iDCkEk72vCMulAvfM7LK\nO3hbWbXJAzOgl5v1AzwoTzi73AvzN7rTxU+wsQ9vuWntsc7WjaqdQeg/ZHi3pNKRkyX+Ay\nGmXQTdgeHjjkQ/mc7GTjpqYFV32+LlFRoC+zP50ivDKINyUXgzd0zz/G2xR5zQjU/boKQU\n719oRMkiGz8P1eib8piYDdiJ3fY0MDqbac8QM=\n-----END OPENSSH PRIVATE KEY-----\n";

    #[test]
    fn keys_to_unlock_are_the_locked_ones_each_once() {
        use crate::ssh::keystore::StoredKeyMaterial;
        let _alone = crate::ssh::unlocked::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::ssh::unlocked::forget();
        let material = |key: &str, passphrase: Option<&str>| StoredKeyMaterial {
            private_key: key.to_string(),
            passphrase: passphrase.map(String::from),
            public_key: None,
        };
        let stored = vec![
            ("plain".to_string(), "Plain".to_string(), material(PLAIN_KEY, None)),
            ("locked".to_string(), "Locked".to_string(), material(ENCRYPTED_KEY, None)),
            ("saved".to_string(), "Saved".to_string(), material(ENCRYPTED_KEY.trim_end(), Some("hunter2"))),
            // The same key imported again, encrypted anew: other text.
            ("copy".to_string(), "Copy".to_string(), material(&{
                let key = russh::keys::ssh_key::PrivateKey::from_openssh(ENCRYPTED_KEY).unwrap().decrypt("hunter2").unwrap();
                key.encrypt(&mut rand::rng(), "hunter2").unwrap().to_openssh(russh::keys::ssh_key::LineEnding::LF).unwrap().to_string()
            }, None)),
        ];
        let cred = |name: &str, auth: serde_json::Value| -> SavedCredential {
            serde_json::from_value(serde_json::json!({ "id": name, "name": name, "username": "u", "auth_method": auth })).unwrap()
        };
        let creds = vec![
            cred("password", serde_json::json!({ "type": "Password", "password": "x" })),
            cred("plain", serde_json::json!({ "type": "Key", "key_id": "plain" })),
            cred("a", serde_json::json!({ "type": "Key", "key_id": "locked" })),
            cred("b", serde_json::json!({ "type": "Key", "key_id": "locked" })),
            cred("c", serde_json::json!({ "type": "Key", "key_id": "copy" })),
            // Its passphrase is saved with the key, typed into the
            // credential, or comes from a password manager at connect.
            cred("saved", serde_json::json!({ "type": "Key", "key_id": "saved" })),
            cred("typed", serde_json::json!({ "type": "Key", "key_id": "locked", "passphrase": "hunter2" })),
            cred("manager", serde_json::json!({ "type": "Key", "key_id": "locked", "passphrase_ref": { "provider": "passbolt", "entry_id": "e" } })),
            cred("gone", serde_json::json!({ "type": "Key", "key_id": "not here" })),
            cred("no file", serde_json::json!({ "type": "Key", "path": "/nonexistent/reachercm-test-key" })),
        ];
        let keys = locked_keys(&creds, &stored);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].label, "Locked");
        assert_eq!(keys[0].used_by, ["a", "b", "c"]);
    }

    fn session(folder: Option<&str>, cred: Option<&str>, vault: Option<&str>) -> SessionConfig {
        serde_json::from_value(serde_json::json!({
            "id": "s1",
            "name": "web",
            "host": "web.example",
            "port": 22,
            "username": "own",
            "auth_method": { "type": "Password", "password": "own-pw" },
            "folder_id": folder,
            "tags": [],
            "vault_id": vault,
            "credential_id": cred,
        }))
        .unwrap()
    }

    fn folder(id: &str, parent: Option<&str>, cred: Option<&str>, vault: Option<&str>) -> Folder {
        Folder {
            id: id.into(),
            name: id.into(),
            parent_id: parent.map(Into::into),
            vault_id: vault.map(Into::into),
            credential_id: cred.map(Into::into),
            tmux: None,
            icon: None,
            color: None,
        }
    }

    fn cred(id: &str, user: &str, vault: Option<&str>) -> SavedCredential {
        SavedCredential {
            id: id.into(),
            name: format!("{id} name"),
            username: user.into(),
            domain: None,
            auth_method: AuthMethod::Password { password: Some(format!("{user}-pw")), password_ref: None },
            vault_id: vault.map(Into::into),
            password_history: None,
            folder_id: None,
        }
    }

    fn password(s: &SessionConfig) -> Option<&str> {
        match &s.auth_method {
            AuthMethod::Password { password, .. } => password.as_deref(),
            _ => None,
        }
    }

    #[test]
    fn a_linked_credential_fills_in_the_login() {
        let mut s = session(None, Some("c1"), None);
        let own = fill(&mut s, None, &[], &[cred("c1", "admin", None)], &Choices::new()).session.unwrap();
        assert_eq!(s.username, "admin");
        assert_eq!(password(&s), Some("admin-pw"));
        assert_eq!(own.username, "own");
        let used = s.credential.unwrap();
        assert_eq!((used.id.as_str(), used.folder_id, used.missing), ("c1", None, false));
    }

    #[test]
    fn the_nearest_folder_credential_applies() {
        let folders = [folder("top", None, Some("c1"), None), folder("mid", Some("top"), Some("c2"), None), folder("leaf", Some("mid"), None, None)];
        let creds = [cred("c1", "one", None), cred("c2", "two", None)];
        let mut s = session(Some("leaf"), None, None);
        fill(&mut s, None, &folders, &creds, &Choices::new());
        assert_eq!(s.username, "two");
        assert_eq!(s.credential.unwrap().folder_id.as_deref(), Some("mid"));
    }

    #[test]
    fn the_session_link_wins_over_the_folder() {
        let folders = [folder("f", None, Some("c1"), None)];
        let creds = [cred("c1", "one", None), cred("c2", "two", None)];
        let mut s = session(Some("f"), Some("c2"), None);
        fill(&mut s, None, &folders, &creds, &Choices::new());
        assert_eq!(s.username, "two");
    }

    #[test]
    fn a_credential_in_another_vault_does_not_count() {
        let mut s = session(None, Some("c1"), Some("team"));
        assert!(fill(&mut s, Some("team"), &[], &[cred("c1", "admin", None)], &Choices::new()).session.is_none());
        assert_eq!(s.username, "own");
        assert_eq!(password(&s), Some("own-pw"));
        assert!(s.credential.unwrap().missing);
    }

    #[test]
    fn a_folder_in_another_vault_gives_nothing() {
        let folders = [folder("f", None, Some("c1"), Some("other"))];
        let mut s = session(Some("f"), None, Some("team"));
        assert!(fill(&mut s, Some("team"), &folders, &[cred("c1", "admin", Some("other"))], &Choices::new()).session.is_none());
        assert_eq!(s.username, "own");
        assert!(s.credential.is_none());
    }

    #[test]
    fn a_session_can_keep_its_own_login_in_a_folder_with_one() {
        let folders = [folder("f", None, Some("c1"), None)];
        let mut s = session(Some("f"), Some(OWN_LOGIN), None);
        assert!(fill(&mut s, None, &folders, &[cred("c1", "one", None)], &Choices::new()).session.is_none());
        assert_eq!(s.username, "own");
        assert!(s.credential.is_none());
        assert!(check_link(&[], Some(OWN_LOGIN), None).is_ok());
    }

    #[test]
    fn a_folder_cycle_ends() {
        let folders = [folder("a", Some("b"), None, None), folder("b", Some("a"), None, None)];
        let mut s = session(Some("a"), None, None);
        assert!(fill(&mut s, None, &folders, &[], &Choices::new()).session.is_none());
        assert!(s.credential.is_none());
    }

    #[test]
    fn no_credential_leaves_the_session_alone() {
        let mut s = session(None, None, None);
        s.credential = Some(CredentialUse { id: "sent".into(), name: "x".into(), folder_id: None, missing: false, personal_target: None, own_username: None });
        assert!(fill(&mut s, None, &[], &[cred("c1", "admin", None)], &Choices::new()).session.is_none());
        assert_eq!(s.username, "own");
        assert!(s.credential.is_none(), "what the webview sent is not kept");
    }

    #[test]
    fn a_filled_in_login_sent_back_is_not_stored() {
        let c = login_of(&cred("c1", "admin", None), "", &Default::default());
        let stored = OwnLogin::of(&session(None, None, None));
        let kept = login_to_store(c.clone(), &[&c], Some(stored));
        assert_eq!(kept.username, "own");
    }

    #[test]
    fn a_login_the_user_typed_is_stored() {
        let c = login_of(&cred("c1", "admin", None), "", &Default::default());
        let typed = OwnLogin { username: "new".into(), domain: None, auth_method: AuthMethod::Agent };
        let stored = OwnLogin::of(&session(None, None, None));
        assert_eq!(login_to_store(typed, &[&c], Some(stored)).username, "new");
    }

    fn with_hop(s: &mut SessionConfig, cred: Option<&str>) {
        s.jump_chain = Some(vec![JumpHostConfig {
            host: "bastion".into(),
            port: 22,
            username: "hop-own".into(),
            auth_method: AuthMethod::Password { password: Some("hop-pw".into()), password_ref: None },
            credential_id: cred.map(Into::into),
            credential: None,
        }]);
    }

    fn hop(s: &SessionConfig) -> &JumpHostConfig {
        &s.jump_chain.as_ref().unwrap()[0]
    }

    #[test]
    fn a_jump_host_can_use_a_credential() {
        let mut s = session(None, None, None);
        with_hop(&mut s, Some("c1"));
        let replaced = fill(&mut s, None, &[], &[cred("c1", "jumper", None)], &Choices::new());
        assert_eq!(hop(&s).username, "jumper");
        assert_eq!(replaced.hops[0].as_ref().unwrap().username, "hop-own");
        assert!(replaced.session.is_none());
        assert_eq!(s.username, "own", "the session's own login is untouched");
    }

    #[test]
    fn a_folder_credential_is_not_used_for_jump_hosts() {
        let folders = [folder("f", None, Some("c1"), None)];
        let mut s = session(Some("f"), None, None);
        with_hop(&mut s, None);
        fill(&mut s, None, &folders, &[cred("c1", "one", None)], &Choices::new());
        assert_eq!(s.username, "one");
        assert_eq!(hop(&s).username, "hop-own");
    }

    #[test]
    fn saving_keeps_the_hops_own_login() {
        let creds = [cred("c1", "jumper", None)];
        // As stored, and as read (filled in).
        let mut stored = session(None, None, None);
        with_hop(&mut stored, Some("c1"));
        let own = SessionLogins::all(&stored);
        let mut read = stored.clone();
        let replaced = fill(&mut read, None, &[], &creds, &Choices::new());
        let filled = SessionLogins::all(&read).only(&replaced);
        // The editor sends the read session back unchanged.
        let mut sent = read.clone();
        let keep = fill_for_save(&mut sent, None, &[], &creds, &Choices::new(), Some((&own, &filled)));
        assert_eq!(hop(&sent).username, "jumper", "sealed as used");
        let mut out = sent.clone();
        keep.put_into(&mut out);
        assert_eq!(hop(&out).username, "hop-own");
        match &hop(&out).auth_method {
            AuthMethod::Password { password, .. } => assert_eq!(password.as_deref(), Some("hop-pw")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_new_session_does_not_copy_a_credentials_password() {
        let creds = [cred("c1", "admin", None)];
        let mut s = session(None, Some("c1"), None);
        // The editor sent the credential's own login as the session's.
        s.username = "admin".into();
        s.auth_method = AuthMethod::Password { password: Some("admin-pw".into()), password_ref: None };
        let keep = fill_for_save(&mut s, None, &[], &creds, &Choices::new(), None);
        let mut out = s.clone();
        keep.put_into(&mut out);
        assert_eq!(out.username, "admin");
        assert!(matches!(out.auth_method, AuthMethod::Agent));
    }

    fn choices(pairs: &[(&str, &str)]) -> Choices {
        pairs.iter().map(|(t, c)| (t.to_string(), c.to_string())).collect()
    }

    #[test]
    fn each_person_gets_their_own_choice() {
        let creds = [cred("mine", "me", None), cred("team", "shared", Some("team"))];
        let mut s = session(None, Some(PERSONAL_LOGIN), Some("team"));
        let own = fill(&mut s, Some("team"), &[], &creds, &choices(&[("s1", "mine")])).session.unwrap();
        assert_eq!(s.username, "me");
        assert_eq!(password(&s), Some("me-pw"));
        assert_eq!(own.username, "own");
        let used = s.credential.unwrap();
        assert_eq!(used.personal_target.as_deref(), Some("s1"));
        assert_eq!(used.own_username.as_deref(), Some("own"));
        assert!(!used.missing);
    }

    #[test]
    fn nobody_elses_choice_or_a_shared_credential_counts() {
        // A choice naming a credential of the shared vault is not one's own.
        let creds = [cred("team", "shared", Some("team"))];
        let mut s = session(None, Some(PERSONAL_LOGIN), Some("team"));
        assert!(fill(&mut s, Some("team"), &[], &creds, &choices(&[("s1", "team")])).session.is_none());
        assert_eq!(s.username, "own");
        let used = s.credential.unwrap();
        assert!(used.missing, "not chosen yet");
        assert_eq!(used.personal_target.as_deref(), Some("s1"));
    }

    #[test]
    fn a_folder_asks_once_for_everything_in_it() {
        let folders = [folder("f", None, Some(PERSONAL_LOGIN), Some("team")), folder("sub", Some("f"), None, Some("team"))];
        let mut s = session(Some("sub"), None, Some("team"));
        fill(&mut s, Some("team"), &folders, &[], &choices(&[("f", DEFAULT_KEY)]));
        assert_eq!(s.username, "own", "the default key signs in as the session's user");
        assert!(matches!(&s.auth_method, AuthMethod::Key { key_id: Some(k), .. } if k == DEFAULT_KEY));
        assert_eq!(s.credential.unwrap().personal_target.as_deref(), Some("f"));
    }

    #[test]
    fn saving_never_stores_someones_own_login_in_the_shared_session() {
        let creds = [cred("mine", "me", None)];
        let ch = choices(&[("s1", "mine")]);
        let mut stored = session(None, Some(PERSONAL_LOGIN), Some("team"));
        stored.auth_method = AuthMethod::Agent;
        let own = SessionLogins::all(&stored);
        let mut read = stored.clone();
        let replaced = fill(&mut read, Some("team"), &[], &creds, &ch);
        let filled = SessionLogins::all(&read).only(&replaced);
        // Sent back unchanged (the filled-in login), or as the editor sends
        // it (the session's own user name, agent): either way nothing of mine.
        for sent in [read.clone(), { let mut e = read.clone(); e.username = "own".into(); e.auth_method = AuthMethod::Agent; e }] {
            let mut sent = sent;
            let keep = fill_for_save(&mut sent, Some("team"), &[], &creds, &ch, Some((&own, &filled)));
            let mut out = sent.clone();
            keep.put_into(&mut out);
            assert_eq!(out.username, "own");
            assert!(matches!(out.auth_method, AuthMethod::Agent));
        }
    }

    #[test]
    fn a_jump_host_cannot_ask_for_each_persons_own_yet() {
        let mut s = session(None, None, Some("team"));
        with_hop(&mut s, Some(PERSONAL_LOGIN));
        assert!(check_links(&[], &s, Some("team")).is_err());
        let s = session(None, Some(PERSONAL_LOGIN), Some("team"));
        assert!(check_links(&[], &s, Some("team")).is_ok());
    }

    fn entry(id: &str) -> crate::pwmanager::SecretRef {
        crate::pwmanager::SecretRef { provider: "passbolt".into(), entry_id: id.into(), ..Default::default() }
    }

    fn cred_with_entry(id: &str, r: crate::pwmanager::SecretRef) -> SavedCredential {
        SavedCredential { auth_method: AuthMethod::Password { password: None, password_ref: Some(r) }, ..cred(id, "admin", None) }
    }

    #[test]
    fn a_credential_can_name_a_password_manager_entry() {
        let r = crate::pwmanager::SecretRef {
            approvals: vec!["signed".into()],
            trusted: true,
            confirm: true,
            label: Some("Web admin".into()),
            ..entry("e")
        };
        let mut auth = AuthMethod::Password { password: Some("typed".into()), password_ref: Some(r) };
        check("n", "u", &mut auth).unwrap();
        match &auth {
            AuthMethod::Password { password, password_ref: Some(r) } => {
                assert!(password.is_none(), "the entry takes the place of a typed password");
                assert!(r.approvals.is_empty() && !r.trusted && !r.confirm, "stored with no approvals");
                assert_eq!((r.entry_id.as_str(), r.label.as_deref()), ("e", Some("Web admin")));
            }
            other => panic!("{other:?}"),
        }
        let mut unknown = AuthMethod::Password {
            password: None,
            password_ref: Some(crate::pwmanager::SecretRef { provider: "nope".into(), ..entry("e") }),
        };
        assert!(check("n", "u", &mut unknown).is_err());
        let mut no_id = AuthMethod::Password { password: None, password_ref: Some(entry(" ")) };
        assert!(check("n", "u", &mut no_id).is_err());
        assert!(check("n", "u", &mut AuthMethod::Agent).is_ok());
        assert!(check(" ", "u", &mut AuthMethod::Agent).is_err());
    }

    #[test]
    fn a_credentials_entry_gets_only_the_sessions_own_approvals() {
        // Approvals written into the credential record count for nothing.
        let forged = crate::pwmanager::SecretRef { approvals: vec!["forged".into()], trusted: true, ..entry("e") };
        let creds = [cred_with_entry("c1", forged)];
        let mut s = session(None, Some("c1"), None);
        with_hop(&mut s, Some("c1"));
        s.credential_ref_approvals.insert("jump:0:password".into(), vec!["hop".into()]);
        let replaced = fill(&mut s, None, &[], &creds, &Choices::new());
        let r = sessions::auth_ref(&s.auth_method).unwrap().1;
        assert!(r.approvals.is_empty() && !r.trusted);
        assert_eq!(sessions::auth_ref(&hop(&s).auth_method).unwrap().1.approvals, ["hop"]);

        // What is signed while the session is open is kept when it is stored.
        sessions::auth_ref_mut(&mut s.auth_method).unwrap().1.approvals = vec!["mine".into()];
        keep_ref_approvals(&mut s, &replaced);
        assert_eq!(s.credential_ref_approvals.get("password").unwrap(), &["mine"]);
        assert_eq!(s.credential_ref_approvals.get("jump:0:password").unwrap(), &["hop"]);

        // An approval taken back goes; a part no credential fills is left.
        sessions::auth_ref_mut(&mut s.auth_method).unwrap().1.approvals.clear();
        keep_ref_approvals(&mut s, &Replaced { session: replaced.session.clone(), hops: vec![None] });
        assert!(!s.credential_ref_approvals.contains_key("password"));
        assert!(s.credential_ref_approvals.contains_key("jump:0:password"));
    }

    #[test]
    fn approvals_are_not_part_of_which_login_it_is() {
        let c = cred_with_entry("c1", entry("e"));
        let read = login_of(&c, "", &[("password".to_string(), vec!["mine".to_string()])].into());
        let sent = login_of(&c, "", &Default::default());
        assert!(read.same(&sent));
        let other = login_of(&cred_with_entry("c1", entry("e2")), "", &Default::default());
        assert!(!read.same(&other));
    }

    #[test]
    fn links_stay_in_their_vault() {
        let creds = [cred("c1", "a", Some("team"))];
        assert!(check_link(&creds, Some("c1"), Some("team")).is_ok());
        assert!(check_link(&creds, Some("c1"), None).is_err());
        assert!(check_link(&creds, None, None).is_ok());
    }
}
