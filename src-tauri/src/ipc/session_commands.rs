//! Session commands using encrypted vault storage.
//!
//! All sessions are stored encrypted in SQLite using XChaCha20-Poly1305.
//! Lookups are O(1) using session_id as the primary key.

use crate::ipc::password_history as pwh;
use crate::ipc::saved_credential_commands as creds;
use crate::state::{AppState, AuthMethod, Folder, JumpHostConfig, SessionConfig};
use crate::vault::types::SecretCategory;
use secrecy::SecretBox;
use tauri::State;

const SESSIONS_VAULT_NAME: &str = "__sessions__";
const FOLDERS_VAULT_NAME: &str = "__folders__";

/// List all saved sessions from all accessible vaults. O(n) where n = total sessions.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_list(state: State<'_, AppState>) -> Result<Vec<SessionConfig>, String> {
    let manager = state.vault_manager.lock().await;

    if manager.is_locked() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    let logins = creds::logins(&manager).await;

    // 1. Get sessions from __sessions__ vault (private sessions)
    if let Some(vault_id) = get_sessions_vault_id_if_exists(&manager) {
        if let Ok(secrets) = manager.read_secrets_in(&vault_id, &["session", "custom:session"]).await {
            for (_, plaintext) in secrets {
                if let Ok(plaintext) = plaintext {
                    use secrecy::ExposeSecret;
                    if let Ok(json) = String::from_utf8(plaintext.expose_secret().clone()) {
                        if let Ok(mut session) = serde_json::from_str::<SessionConfig>(&json) {
                            creds::fill(&mut session, None, &logins.folders, &logins.creds);
                            sessions.push(session);
                        }
                    }
                }
            }
        }
    }

    // 2. Get sessions from all user vaults (shared vaults)
    let user_vaults = manager.list_vaults().await.unwrap_or_default();
    tracing::info!("session_list: checking {} user vaults", user_vaults.len());
    for vault_info in user_vaults {
        // Skip internal vaults
        if vault_info.name.starts_with("__") {
            continue;
        }

        tracing::info!("session_list: checking vault {} ({})", vault_info.name, vault_info.id);
        // Session secrets only, including the legacy "custom:session" category.
        match manager.read_secrets_in(&vault_info.id, &["session", "custom:session"]).await {
            Ok(secrets) => {
                tracing::info!("session_list: vault {} has {} sessions", vault_info.name, secrets.len());
                for (secret, plaintext) in secrets {
                    match plaintext {
                        Ok(plaintext) => {
                            use secrecy::ExposeSecret;
                            if let Ok(json) = String::from_utf8(plaintext.expose_secret().clone()) {
                                if let Ok(mut session) = serde_json::from_str::<SessionConfig>(&json) {
                                    // Ensure vault_id is set correctly
                                    session.vault_id = Some(vault_info.id.clone());
                                    creds::fill(&mut session, Some(&vault_info.id), &logins.folders, &logins.creds);
                                    tracing::info!("session_list: loaded session: {}", session.name);
                                    sessions.push(session);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("session_list: failed to read secret {}: {}", secret.id, e);
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!("session_list: failed to list secrets for vault {}: {}", vault_info.name, e);
            }
        }
    }

    let mut sessions = unique_sessions(sessions);
    // The via sessions, as stored (approvals do not enter a context), from
    // the same list rather than read again one by one.
    let stored: std::collections::HashMap<String, SessionConfig> =
        sessions.iter().map(|s| (s.id.clone(), s.clone())).collect();
    for s in &mut sessions {
        let via = via_id(s).and_then(|id| stored.get(id));
        open_approvals_with(&manager, s, via);
        // Old passwords are read one session at a time, when asked for.
        s.password_history = None;
    }
    Ok(sessions)
}

/// What an approval is given for: the session and everything that decides
/// where and how it connects. Change any of it (a shared vault's other
/// members can) and the approvals given before no longer count. Credentials
/// are left out, so saving a password does not cost the approvals; that
/// includes the proxy's password and its password manager entry, which are
/// left out of the proxy as well (a proxy with neither hashes as it did
/// before they existed).
pub(crate) fn approval_context(s: &SessionConfig) -> String {
    use sha2::Digest;
    let jumps: Option<Vec<(String, u16, String)>> =
        s.jump_chain.as_ref().map(|c| c.iter().map(|j| (j.host.clone(), j.port, j.username.clone())).collect());
    let proxy = s.proxy.as_ref().map(|p| crate::state::ProxyConfig { password: None, password_ref: None, ..p.clone() });
    let o = s.ssh_options.as_ref();
    let mut v = serde_json::json!([
        s.id,
        s.host,
        s.port,
        s.username,
        s.kind,
        s.via_session_id,
        s.shell,
        jumps,
        proxy,
        o.map(|o| &o.imported),
        o.map(|o| &o.lines),
    ]);
    // The other addresses are where the login may be sent too, so they are
    // part of what was approved. Only when there are any: a session without
    // them hashes as it did before they existed.
    if let (Some(alt), Some(list)) = (s.alt_addresses.as_ref(), v.as_array_mut()) {
        list.push(serde_json::json!(alt));
    }
    // A SPICE session's TLS port and certificate checks decide where its
    // password goes, so they are part of what was approved too.
    if let (Some(spice), Some(list)) = (s.spice.as_ref(), v.as_array_mut()) {
        list.push(serde_json::json!(["spice", spice]));
    }
    // The WireGuard tunnel decides which network the login goes into.
    if let (Some(wg), Some(list)) = (s.wireguard_id.as_ref(), v.as_array_mut()) {
        list.push(serde_json::json!(["wireguard", wg]));
    }
    sha2::Sha256::digest(v.to_string().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The SSH session a session reaches its server through: a VNC or SPICE
/// session's `via_session_id` (the field is theirs only; other kinds ignore it when they
/// connect, so it decides nothing for them).
pub(crate) fn via_id(s: &SessionConfig) -> Option<&str> {
    if !matches!(s.kind, crate::state::SessionKind::Vnc | crate::state::SessionKind::Spice) {
        return None;
    }
    s.via_session_id.as_deref().filter(|v| !v.is_empty())
}

/// What a session's password manager references are approved for: its own
/// [`approval_context`], and for a session that goes through an SSH session
/// ([`via_id`]), that session's as it is stored now. The tunnel is built
/// from the via session when connecting, and another member of its vault
/// can re-point it; a reference approved for one way in must not follow it
/// to another server. `via` is the via session as read now; `None` when the
/// session has one and it is gone (or is not the one named): then nothing
/// can be approved and no reference counts.
///
/// Only the references' context gains the via part; the ssh_config
/// approvals keep [`approval_context`], since they are about the session's
/// own login.
pub(crate) fn ref_context(s: &SessionConfig, via: Option<&SessionConfig>) -> Option<String> {
    let own = approval_context(s);
    match via_id(s) {
        None => Some(own),
        Some(id) => via.filter(|v| v.id == id).map(|v| format!("{own}\tvia\t{}", approval_context(v))),
    }
}

/// The via session of `s` as stored now, if it has one and it still exists.
async fn via_for(manager: &crate::vault::VaultManager, s: &SessionConfig) -> Option<SessionConfig> {
    let id = via_id(s)?;
    let loc = locate_session(manager, id).await?;
    read_session_filled(manager, &loc).await.ok().map(|(s, _)| s)
}

/// After reading a session: the user's approvals are the ones signed with
/// their key for this session as it now is; nothing else in the record
/// counts. Returns the via session the references were checked against
/// (see [`ref_context`]).
async fn open_approvals(manager: &crate::vault::VaultManager, session: &mut SessionConfig) -> Option<SessionConfig> {
    let via = via_for(manager, session).await;
    open_approvals_with(manager, session, via.as_ref());
    via
}

fn open_approvals_with(manager: &crate::vault::VaultManager, session: &mut SessionConfig, via: Option<&SessionConfig>) {
    let key = manager.approval_key();
    let who = manager.get_user_uuid();
    open_for(key.as_deref(), who.as_deref(), session, via);
}

fn open_for(key: Option<&[u8; 32]>, who: Option<&str>, session: &mut SessionConfig, via: Option<&SessionConfig>) {
    use crate::ssh::sshconf::approvals::{verified, Kind};
    let context = approval_context(session);
    let refs_context = ref_context(session, via);
    if let Some(o) = session.ssh_options.as_mut() {
        o.open_for(key, who, &context);
    }
    for (slot, r) in ref_slots_mut(session) {
        let what = ref_what(&slot, r);
        r.trusted = match (key, who, refs_context.as_deref()) {
            (Some(k), Some(w), Some(c)) => verified(&r.approvals, k, w, Kind::SecretRef, c).contains(&what),
            _ => false,
        };
        r.confirm = false;
    }
}

/// Before storing a session: the user's approvals signed into it, for it as
/// it is saved. `stored` is the record being replaced, as read for this
/// user (`None` for a new session).
async fn seal_approvals(manager: &crate::vault::VaultManager, session: &mut SessionConfig, stored: Option<&SessionConfig>) {
    let via = via_for(manager, session).await;
    let key = manager.approval_key();
    let who = manager.get_user_uuid();
    seal_for(key.as_deref(), who.as_deref(), session, stored, via.as_ref());
}

/// Which password manager references the saving person signs.
///
/// A reference is signed only when this person said so in this save: the
/// editor sets `confirm` on a reference they picked from their manager (or
/// confirmed in the connect dialog, via `session_approve_secret_refs`).
/// Besides that, a reference they had already approved in the stored record,
/// in the same place and for the same entry, stays approved: they are the one
/// changing the session, as with the ssh_config approvals, which the editor
/// sends back to be signed for the session as saved.
///
/// Nothing else is signed. In particular a reference that was in the record
/// unapproved (another member of a shared vault may have put it there) does
/// not become approved because the user renamed the session or moved it to a
/// folder, and the `trusted` flag the frontend saw is never read back. What
/// counts as "already approved" is worked out here from the stored record
/// and the user's key, not from anything sent in.
///
/// `via` is the session's via session as stored now (see [`ref_context`]).
/// When it has one and that is gone, nothing is signed and the references'
/// approvals stay as they were: they count for nothing until it is back as
/// it was.
fn seal_for(
    key: Option<&[u8; 32]>,
    who: Option<&str>,
    session: &mut SessionConfig,
    stored: Option<&SessionConfig>,
    via: Option<&SessionConfig>,
) {
    use crate::ssh::sshconf::approvals::{replace_mine, Kind};
    let context = approval_context(session);
    let refs_context = ref_context(session, via);
    if let Some(o) = session.ssh_options.as_mut() {
        o.seal_for(key, who, &context);
    }
    let approved_before: Vec<(String, crate::pwmanager::SecretRef)> = stored
        .map(|s| ref_slots(s).into_iter().filter(|(_, r)| r.trusted).map(|(slot, r)| (slot, r.clone())).collect())
        .unwrap_or_default();
    for (slot, r) in ref_slots_mut(session) {
        let mine = r.confirm || approved_before.iter().any(|(s, before)| *s == slot && before.same_entry(r));
        if let (Some(k), Some(w), Some(c)) = (key, who, refs_context.as_deref()) {
            let what = ref_what(&slot, r);
            let list: &[String] = if mine { std::slice::from_ref(&what) } else { &[] };
            r.approvals = replace_mine(&r.approvals, list, k, w, Kind::SecretRef, c);
        }
        r.confirm = false;
        r.trusted = false;
    }
}

// ---------------------------------------------------------------------------
// Password manager references.
//
// A session can name an entry in the user's password manager for its
// password, its key passphrase, each jump host's and the proxy's. Resolving
// one uses the connecting person's own manager login, so a session in a
// shared vault must not be able to send that person's secrets anywhere they
// did not agree to: another member could point the session at a host they
// own, add a jump host or proxy, or swap the entry for another the victim can
// read. Each reference is therefore approved per person, signed like the
// ssh_config approvals, over its slot, its entry and the session's context
// (`approval_context`); the connect commands use only references the person
// approved, and take them from the stored session, not from the webview.

/// The approved statement for a reference: where in the session it sits and
/// which entry it is.
fn ref_what(slot: &str, r: &crate::pwmanager::SecretRef) -> String {
    // The password field is left out, so approvals made before references
    // named a field still hold.
    match r.field() {
        None => format!("{slot}\t{}\t{}", r.provider, r.entry_id),
        Some(field) => format!("{slot}\t{}\t{}\t{field}", r.provider, r.entry_id),
    }
}

pub(crate) fn auth_ref(m: &AuthMethod) -> Option<(&'static str, &crate::pwmanager::SecretRef)> {
    match m {
        AuthMethod::Password { password_ref: Some(r), .. } => Some(("password", r)),
        AuthMethod::Key { passphrase_ref: Some(r), .. } => Some(("passphrase", r)),
        _ => None,
    }
}

pub(crate) fn auth_ref_mut(m: &mut AuthMethod) -> Option<(&'static str, &mut crate::pwmanager::SecretRef)> {
    match m {
        AuthMethod::Password { password_ref: Some(r), .. } => Some(("password", r)),
        AuthMethod::Key { passphrase_ref: Some(r), .. } => Some(("passphrase", r)),
        _ => None,
    }
}

/// Every reference a session holds, by slot: `password` or `passphrase` for
/// the session's own login (RDP and VNC included), `jump:<n>:password` or
/// `jump:<n>:passphrase` for the n-th jump host (from 0), `proxy` for the
/// proxy's password.
pub(crate) fn ref_slots(s: &SessionConfig) -> Vec<(String, &crate::pwmanager::SecretRef)> {
    let mut out = Vec::new();
    if let Some((field, r)) = auth_ref(&s.auth_method) {
        out.push((field.to_string(), r));
    }
    for (i, j) in s.jump_chain.iter().flatten().enumerate() {
        if let Some((field, r)) = auth_ref(&j.auth_method) {
            out.push((format!("jump:{i}:{field}"), r));
        }
    }
    if let Some(r) = s.proxy.as_ref().and_then(|p| p.password_ref.as_ref()) {
        out.push(("proxy".to_string(), r));
    }
    out
}

fn ref_slots_mut(s: &mut SessionConfig) -> Vec<(String, &mut crate::pwmanager::SecretRef)> {
    let SessionConfig { auth_method, jump_chain, proxy, .. } = s;
    let mut out = Vec::new();
    if let Some((field, r)) = auth_ref_mut(auth_method) {
        out.push((field.to_string(), r));
    }
    for (i, j) in jump_chain.iter_mut().flatten().enumerate() {
        if let Some((field, r)) = auth_ref_mut(&mut j.auth_method) {
            out.push((format!("jump:{i}:{field}"), r));
        }
    }
    if let Some(r) = proxy.as_mut().and_then(|p| p.password_ref.as_mut()) {
        out.push(("proxy".to_string(), r));
    }
    out
}

/// The reference in one slot of a session.
pub(crate) fn ref_at<'a>(s: &'a SessionConfig, slot: &str) -> Option<&'a crate::pwmanager::SecretRef> {
    ref_slots(s).into_iter().find(|(name, _)| name == slot).map(|(_, r)| r)
}

/// The slots an SSH login with this session's own settings resolves: all of
/// them, except the proxy's when there are jump hosts (the proxy is used
/// only without).
pub(crate) fn slots_in_use(s: &SessionConfig) -> Vec<String> {
    let jumps = s.jump_chain.as_ref().is_some_and(|c| !c.is_empty());
    ref_slots(s).into_iter().map(|(slot, _)| slot).filter(|slot| !(jumps && slot == "proxy")).collect()
}

/// What an error from a connect command starts with when the session holds
/// references the connecting person has not approved; JSON
/// ([`UntrustedRefs`]) follows. The session list recognises it and asks.
pub const UNTRUSTED_PREFIX: &str = "PWMANAGER_UNTRUSTED:";

/// One reference waiting for approval.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RefSlot {
    pub slot: String,
    pub provider: String,
    pub entry_id: String,
}

/// Everything the confirmation dialog shows, read from the stored session
/// in one go, so what the person agrees to is what is signed. `context`
/// goes back with the approval, which is refused if the session changed in
/// between.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct UntrustedRefs {
    pub session_id: String,
    pub session_name: String,
    pub context: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// `user@host:port` for each jump host, in order.
    pub jumps: Vec<String>,
    /// `type user@host:port` of the proxy, when there is one and no jump host.
    pub proxy: Option<String>,
    /// The session's own ssh_config lines that change where it connects
    /// (`HostName`, `ProxyJump`, `ProxyCommand`), as written: they win over
    /// the host shown above when the login is made.
    pub route_lines: Vec<String>,
    /// The SSH session a VNC session goes through, as stored now: the
    /// secret travels through it.
    pub via: Option<ViaRoute>,
    pub refs: Vec<RefSlot>,
}

/// Where a via session goes, for the confirmation dialog.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ViaRoute {
    pub session_id: String,
    pub session_name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub jumps: Vec<String>,
    pub proxy: Option<String>,
    pub route_lines: Vec<String>,
}

/// The ssh_config keywords that decide where a login goes.
const ROUTE_KEYWORDS: &[&str] = &["hostname", "proxyjump", "proxycommand"];

/// A session's way to its server, for display: `user@host:port` for each
/// jump host, the proxy when it is used (no jump hosts), and its own
/// ssh_config lines that re-route it.
fn route_of(s: &SessionConfig) -> (Vec<String>, Option<String>, Vec<String>) {
    let show = |t: &str| crate::pwmanager::sanitize_display(t, 200);
    let jumps: Vec<String> =
        s.jump_chain.iter().flatten().map(|j| show(&format!("{}@{}:{}", j.username, j.host, j.port))).collect();
    let proxy = s.proxy.as_ref().filter(|_| jumps.is_empty()).map(|p| match p.username.as_deref().filter(|u| !u.is_empty()) {
        Some(u) => show(&format!("{} {u}@{}:{}", p.proxy_type, p.host, p.port)),
        None => show(&format!("{} {}:{}", p.proxy_type, p.host, p.port)),
    });
    let route_lines = s
        .ssh_options
        .iter()
        .flat_map(|o| o.lines.iter())
        .filter_map(|line| crate::ssh::sshconf::lex::split_keyword(line))
        .filter(|l| ROUTE_KEYWORDS.contains(&l.keyword.to_ascii_lowercase().as_str()))
        .map(|l| show(&format!("{} {}", l.keyword, l.rest)))
        .collect();
    (jumps, proxy, route_lines)
}

/// Where a session another one goes through really leads, as stored now.
pub(crate) fn via_route(v: &SessionConfig) -> ViaRoute {
    let (jumps, proxy, route_lines) = route_of(v);
    ViaRoute {
        session_id: v.id.clone(),
        session_name: v.name.clone(),
        host: v.host.clone(),
        port: v.port,
        username: v.username.clone(),
        jumps,
        proxy,
        route_lines,
    }
}

/// The references among `slots` the reading person has not approved;
/// `Ok(None)` when there are none. `via` is the session's via session as
/// read when the references were checked (see [`ref_context`]); when the
/// session has one and it is gone, there is nothing to ask: the error says
/// so.
pub(crate) fn untrusted_refs(s: &SessionConfig, via: Option<&SessionConfig>, slots: &[String]) -> Result<Option<UntrustedRefs>, String> {
    let refs: Vec<RefSlot> = ref_slots(s)
        .into_iter()
        .filter(|(slot, r)| slots.contains(slot) && !r.trusted)
        .map(|(slot, r)| RefSlot { slot, provider: r.provider.clone(), entry_id: r.entry_id.clone() })
        .collect();
    if refs.is_empty() {
        return Ok(None);
    }
    let Some(context) = ref_context(s, via) else {
        return Err(format!(
            "'{}' goes through an SSH session that no longer exists, so its password manager entry is not used. Choose the SSH session to go through in the session's editor.",
            s.name
        ));
    };
    let (jumps, proxy, route_lines) = route_of(s);
    let via = via_id(s).and(via).map(via_route);
    Ok(Some(UntrustedRefs {
        session_id: s.id.clone(),
        session_name: s.name.clone(),
        context,
        host: s.host.clone(),
        port: s.port,
        username: s.username.clone(),
        jumps,
        proxy,
        route_lines,
        via,
        refs,
    }))
}

/// Refuse, with an error the session list can act on, unless every
/// reference in `slots` is approved by the connecting person. `via` as for
/// [`untrusted_refs`].
pub(crate) fn require_trusted_refs(s: &SessionConfig, via: Option<&SessionConfig>, slots: &[String]) -> Result<(), String> {
    match untrusted_refs(s, via, slots)? {
        None => Ok(()),
        Some(u) => Err(format!("{UNTRUSTED_PREFIX}{}", serde_json::to_string(&u).map_err(|e| e.to_string())?)),
    }
}

/// A stored session as the connect commands use it: read and opened for the
/// current user, exactly as `session_get` returns it.
pub(crate) async fn stored_session(state: &AppState, session_id: &str) -> Result<SessionConfig, String> {
    stored_session_and_via(state, session_id).await.map(|(s, _)| s)
}

/// [`stored_session`], with the via session its references were checked
/// against (see [`ref_context`]).
pub(crate) async fn stored_session_and_via(
    state: &AppState,
    session_id: &str,
) -> Result<(SessionConfig, Option<SessionConfig>), String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let loc = locate_session(&manager, session_id)
        .await
        .ok_or_else(|| format!("Session not found: {}", session_id))?;
    let (mut session, _) = read_session_filled(&manager, &loc).await?;
    let via = open_approvals(&manager, &mut session).await;
    Ok((session, via))
}

/// A connect that uses password manager entries must say which saved session
/// they are from: there is nothing else to check them against.
pub(crate) fn require_session_id(session_id: Option<&str>) -> Result<&str, String> {
    session_id
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "A password manager entry is used only for a saved session; this connection names none".to_string())
}

/// The stored session an RDP or VNC connect takes its password manager entry
/// from. The connect must go where the session goes (its server, and for VNC
/// its way in), and the person connecting must have approved the entry.
/// Returns the session and, for VNC through an SSH session, that session as
/// the approval was checked against: the tunnel must be built from it as it
/// was then (see `db_commands::forward_route`'s `pin`).
pub(crate) async fn stored_session_for_desktop(
    state: &AppState,
    session_id: Option<&str>,
    kind: crate::state::SessionKind,
    host: &str,
    port: u16,
    username: Option<&str>,
    via_session_id: Option<Option<&str>>,
) -> Result<(SessionConfig, Option<SessionConfig>), String> {
    let (stored, via_session) = stored_session_and_via(state, require_session_id(session_id)?).await?;
    let via = |v: Option<&str>| v.filter(|s| !s.is_empty()).map(str::to_owned);
    let same = stored.kind == kind
        && stored.host == host
        && stored.port == port
        && username.is_none_or(|u| stored.username == u)
        && via_session_id.is_none_or(|v| via(v) == via(stored.via_session_id.as_deref()));
    if !same {
        return Err(format!(
            "This connection does not match the saved session '{}', so its password manager entry is not used. Reload the session list and connect again.",
            stored.name
        ));
    }
    require_trusted_refs(&stored, via_session.as_ref(), &["password".to_string()])?;
    Ok((stored, via_session))
}

/// A desktop session's password manager entry.
pub(crate) fn desktop_ref(s: &SessionConfig) -> Result<&crate::pwmanager::SecretRef, String> {
    ref_at(s, "password").ok_or_else(|| {
        format!("The saved session '{}' has no password manager entry any more. Reload the session list and connect again.", s.name)
    })
}

async fn read_session(manager: &crate::vault::VaultManager, vault_id: &str, session_id: &str) -> Result<SessionConfig, String> {
    let plaintext = manager
        .read_secret(vault_id, session_id)
        .await
        .map_err(|_| format!("Session not found: {}", session_id))?;
    use secrecy::ExposeSecret;
    let json = String::from_utf8(plaintext.expose_secret().clone()).map_err(|e| format!("Invalid UTF-8: {}", e))?;
    serde_json::from_str(&json).map_err(|e| format!("Invalid session data: {}", e))
}

/// A stored session as everything that uses it reads it: with the saved
/// credential that applies to it filled in. Also returns the session's own
/// stored login that the credential's took the place of, if it did.
async fn read_session_filled(
    manager: &crate::vault::VaultManager,
    loc: &SessionLoc,
) -> Result<(SessionConfig, creds::SessionLogins), String> {
    let mut session = read_session(manager, &loc.vault_id, &loc.secret_id).await?;
    let logins = creds::logins(manager).await;
    let vault = creds::vault_of(manager, &loc.vault_id);
    let own = creds::fill(&mut session, vault.as_deref(), &logins.folders, &logins.creds);
    Ok((session, own))
}

/// A session as it is written to its vault: with `login` as its own (see
/// `saved_credential_commands`), and nothing that is only ever read.
fn to_store(session: &SessionConfig, logins: creds::SessionLogins) -> SessionConfig {
    let mut s = session.clone();
    // Before the credential's login goes: the approvals just signed for the
    // entries it brought stay with the session.
    creds::keep_ref_approvals(&mut s, &logins);
    logins.put_into(&mut s);
    s.credential = None;
    for j in s.jump_chain.iter_mut().flatten() {
        j.credential = None;
    }
    for (_, r) in ref_slots_mut(&mut s) {
        r.trusted = false;
        r.confirm = false;
    }
    s
}

/// The password manager references of a session the current user has not
/// approved yet, among those a connect would use; `None` when there are
/// none. For the session list to ask before it connects.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_untrusted_secret_refs(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Option<UntrustedRefs>, String> {
    let (s, via) = stored_session_and_via(&state, &session_id).await?;
    untrusted_refs(&s, via.as_ref(), &slots_in_use(&s))
}

/// The current user approves these references of a session, after saying so
/// in the confirmation dialog: each is signed with their key for the session
/// as it is. `context` and the entries are what the dialog showed (from
/// [`UntrustedRefs`]); if the session or an entry changed since, nothing is
/// approved.
#[tauri::command]
#[tracing::instrument(skip(state, refs), fields(count = refs.len()))]
pub async fn session_approve_secret_refs(
    state: State<'_, AppState>,
    session_id: String,
    context: String,
    refs: Vec<RefSlot>,
) -> Result<SessionConfig, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let loc = locate_session(&manager, &session_id)
        .await
        .ok_or_else(|| format!("Session not found: {}", session_id))?;
    let (mut session, own) = read_session_filled(&manager, &loc).await?;
    // The via session is read once: what is checked against `context` is
    // what is signed.
    let via = via_for(&manager, &session).await;
    open_approvals_with(&manager, &mut session, via.as_ref());
    approve_refs(&mut session, via.as_ref(), &context, &refs)?;
    let stored = session.clone();
    let (key, who) = (manager.approval_key(), manager.get_user_uuid());
    seal_for(key.as_deref(), who.as_deref(), &mut session, Some(&stored), via.as_ref());
    let json = serde_json::to_string(&to_store(&session, own)).map_err(|e| e.to_string())?;
    manager
        .update_secret(&loc.vault_id, &loc.secret_id, SecretBox::new(Box::new(json.into_bytes())))
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("Approved {} password manager entries for session {}", refs.len(), session.id);
    open_approvals_with(&manager, &mut session, via.as_ref());
    Ok(session)
}

/// Mark `refs` for signing, if the session, and the via session it goes
/// through, still are what the dialog showed.
fn approve_refs(session: &mut SessionConfig, via: Option<&SessionConfig>, context: &str, refs: &[RefSlot]) -> Result<(), String> {
    const CHANGED: &str = "The session changed while you were asked. Connect again to see what it now wants.";
    if refs.is_empty() {
        return Err("Nothing to approve".to_string());
    }
    if ref_context(session, via).as_deref() != Some(context) {
        return Err(CHANGED.to_string());
    }
    for want in refs {
        let found = ref_at(session, &want.slot).is_some_and(|r| r.provider == want.provider && r.entry_id == want.entry_id);
        if !found {
            return Err(CHANGED.to_string());
        }
    }
    for (slot, r) in ref_slots_mut(session) {
        if refs.iter().any(|w| w.slot == slot) {
            r.confirm = true;
        }
    }
    Ok(())
}

/// One entry per session id, the first one found.
///
/// The same session can be in two vaults: a shared vault joined twice is two
/// vaults over one database. The list keys its rows by id, and a duplicate
/// key is an error that stopped the whole list from rendering (it sat on
/// "loading" for good). Both copies are the same session, so one is shown.
fn unique_sessions(sessions: Vec<SessionConfig>) -> Vec<SessionConfig> {
    let mut seen = std::collections::HashMap::<String, Option<String>>::new();
    let mut unique = Vec::with_capacity(sessions.len());
    for session in sessions {
        if let Some(first) = seen.get(&session.id) {
            tracing::warn!(
                "session_list: session {} is in vault {:?} and again in {:?}; listing it once",
                session.id, first, session.vault_id
            );
            continue;
        }
        seen.insert(session.id.clone(), session.vault_id.clone());
        unique.push(session);
    }
    unique
}

/// Get a specific session by ID. O(1) lookup.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_get(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionConfig, String> {
    let manager = state.vault_manager.lock().await;

    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }

    // Find which vault contains this session
    let loc = locate_session(&manager, &session_id).await
        .ok_or_else(|| format!("Session not found: {}", session_id))?;

    // O(1) lookup by primary key
    let (mut session, _) = read_session_filled(&manager, &loc).await?;
    open_approvals(&manager, &mut session).await;
    session.password_history = None;
    Ok(session)
}

/// The RDP clipboard choice as stored: only the two that differ from the
/// default, and only on an RDP session.
pub(crate) fn rdp_clipboard_setting(value: Option<String>, kind: crate::state::SessionKind) -> Option<String> {
    value.filter(|v| kind == crate::state::SessionKind::Rdp && (v == "text" || v == "off"))
}

/// Create a new session configuration. O(1) insert.
/// If vault_id is provided and is a user vault, stores in that vault (for sharing).
/// Otherwise stores in __sessions__ (private).
#[tauri::command]
// Only what names the session: the login, jump hosts and proxy carry
// passwords, passphrases and embedded keys, and every argument left in an
// instrument span is written to reach.log.
#[tracing::instrument(skip_all, fields(host = %host, port = port, kind = ?kind))]
#[expect(clippy::too_many_arguments, reason = "a Tauri command takes each argument from the frontend's invoke by name")]
pub async fn session_create(
    state: State<'_, AppState>,
    name: String,
    host: String,
    port: u16,
    username: String,
    auth_method: AuthMethod,
    folder_id: Option<String>,
    tags: Vec<String>,
    vault_id: Option<String>,
    jump_chain: Option<Vec<JumpHostConfig>>,
    proxy: Option<crate::state::ProxyConfig>,
    wireguard_id: Option<String>,
    shell: Option<String>,
    kind: Option<crate::state::SessionKind>,
    domain: Option<String>,
    detected_os: Option<String>,
    os_manual: Option<bool>,
    share_path: Option<String>,
    rdp_clipboard: Option<String>,
    rdp_use_freerdp: Option<bool>,
    rdp_forward_lock_keys: Option<bool>,
    rdp: Option<crate::rdp::options::RdpOptions>,
    via_session_id: Option<String>,
    try_agent_keys: Option<bool>,
    ssh_options: Option<crate::ssh::sshconf::session::SshOptions>,
    wsl_distro: Option<String>,
    credential_id: Option<String>,
    telnet: Option<crate::telnet::protocol::TelnetOptions>,
    serial: Option<crate::serial::options::SerialOptions>,
    spice: Option<crate::spice::SpiceOptions>,
    notes: Option<String>,
    custom_fields: Option<Vec<crate::state::CustomField>>,
    terminal_theme: Option<String>,
    cursor_style: Option<String>,
    cursor_blink: Option<bool>,
    monitoring: Option<bool>,
    history: Option<String>,
    tmux: Option<crate::ssh::tmux::TmuxSetting>,
    x11_desktop: Option<crate::ssh::x11_desktop::X11Desktop>,
    terminal: Option<crate::ssh::terminal::TerminalOptions>,
    host_label: Option<String>,
    alt_addresses: Option<Vec<crate::state::SessionAddress>>,
    connect_mode: Option<String>,
    address_check: Option<bool>,
    web: Option<crate::web::WebOptions>,
    wol: Option<crate::state::WakeOnLan>,
    files: Option<crate::state::FilesProtocol>,
    ftp: Option<crate::ftp::FtpOptions>,
    proxmox: Option<crate::proxmox::ProxmoxOptions>,
    bmc: Option<crate::bmc::BmcOptions>,
) -> Result<SessionConfig, String> {
    let mut manager = state.vault_manager.lock().await;
    let kind = kind.unwrap_or_default();

    if manager.is_locked() {
        return Err("Vault is locked. Set a master password first.".to_string());
    }

    // Determine storage vault: user vault (shared) or __sessions__ (private)
    let storage_vault_id = if let Some(ref vid) = vault_id {
        // Check if this is a user vault (not internal)
        let vaults = manager.list_vaults().await.map_err(|e| e.to_string())?;
        let is_user_vault = vaults.iter().any(|v| v.id == *vid && !v.name.starts_with("__"));
        if is_user_vault {
            tracing::info!("Storing session in user vault: {}", vid);
            vid.clone()
        } else {
            ensure_sessions_vault(&mut manager).await?
        }
    } else {
        ensure_sessions_vault(&mut manager).await?
    };

    let mut session = SessionConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        host,
        port,
        username,
        auth_method,
        kind,
        wsl_distro,
        telnet: telnet.filter(|_| kind == crate::state::SessionKind::Telnet),
        serial: serial.filter(|_| kind == crate::state::SessionKind::Serial),
        spice: spice.filter(|o| kind == crate::state::SessionKind::Spice && !o.is_empty()),
        domain: domain.filter(|d| !d.trim().is_empty()),
        share_path: share_path.filter(|d| !d.trim().is_empty()),
        rdp_clipboard: rdp_clipboard_setting(rdp_clipboard, kind),
        rdp_use_freerdp: kind == crate::state::SessionKind::Rdp && rdp_use_freerdp.unwrap_or(false),
        rdp_forward_lock_keys: rdp_forward_lock_keys.filter(|_| kind == crate::state::SessionKind::Rdp),
        rdp: rdp.filter(|_| kind == crate::state::SessionKind::Rdp).and_then(crate::rdp::options::RdpOptions::tidy),
        via_session_id: via_session_id.filter(|s| !s.is_empty()),
        try_agent_keys: try_agent_keys.filter(|t| *t),
        folder_id,
        tags,
        // RDP cannot say what the machine is (xrdp claims to be Windows), so
        // the user says, Windows unless told otherwise; SSH sessions find
        // out on first connect.
        detected_os: detected_os
            .filter(|os| !os.is_empty())
            .or_else(|| (kind == crate::state::SessionKind::Rdp).then(|| "windows".to_string()))
            // WinBox only ever manages a MikroTik.
            .or_else(|| (kind == crate::state::SessionKind::Winbox).then(|| "mikrotik".to_string())),
        os_manual: os_manual.unwrap_or(false),
        vault_id: if storage_vault_id != ensure_sessions_vault(&mut manager).await.unwrap_or_default() {
            Some(storage_vault_id.clone())
        } else {
            None
        },
        wireguard_id: wireguard_id.filter(|w| !w.trim().is_empty()),
        jump_chain,
        proxy,
        shell,
        ssh_options: ssh_options.filter(|o| !o.is_empty()),
        credential_id: credential_id.filter(|c| !c.is_empty()),
        favorite: false,
        order: None,
        credential: None,
        notes,
        custom_fields,
        terminal_theme,
        cursor_style,
        cursor_blink,
        monitoring,
        history,
        tmux,
        x11_desktop,
        terminal,
        credential_ref_approvals: Default::default(),
        // Trusted by connecting to it, never by what a form sends.
        host_keys: None,
        host_keys_removed: None,
        host_label,
        alt_addresses,
        connect_mode,
        address_check,
        password_history: None,
        // Certificates are trusted by answering the prompt, never by a form.
        web: web.filter(|_| kind == crate::state::SessionKind::Web).map(|mut w| {
            w.certs.clear();
            w
        }),
        wol,
        files: files.filter(|_| kind == crate::state::SessionKind::Ssh),
        // Certificates likewise.
        ftp: ftp
            .filter(|_| kind == crate::state::SessionKind::Ftp)
            .map(|mut f| {
                f.certs.clear();
                f
            })
            .filter(|f| !f.is_empty()),
        // Its VNC server as the editor sent it; no certificates from the
        // window: only the prompt trusts one.
        bmc: bmc
            .filter(|_| kind == crate::state::SessionKind::Bmc)
            .map(|mut b| {
                b.certs.clear();
                b
            })
            .filter(|b| !b.is_empty()),
        // Its realm and token as the editor sent them; certificates likewise.
        proxmox: proxmox
            .filter(|_| kind == crate::state::SessionKind::Proxmox)
            .map(|mut p| {
                p.certs.clear();
                p
            })
            .filter(|p| !p.is_empty()),
    };
    session.tidy_documentation();

    // A saved credential's login is filled in, as it is when the session is
    // read; what is stored as its own is what the editor sent, unless that
    // is the credential's (then only its username: a credential's password
    // is not copied into the session).
    let logins = creds::logins(&manager).await;
    let vault = session.vault_id.clone();
    creds::check_links(&logins.creds, &session, vault.as_deref())?;
    if session.folder_id.is_some() {
        check_folder_vault(&read_folders(&manager).await?, session.folder_id.as_deref(), vault.as_deref())?;
    }
    let own = creds::fill_for_save(&mut session, vault.as_deref(), &logins.folders, &logins.creds, None);

    seal_approvals(&manager, &mut session, None).await;
    let json = serde_json::to_string(&to_store(&session, own)).map_err(|e| e.to_string())?;
    let plaintext = SecretBox::new(Box::new(json.into_bytes()));

    // O(1) insert with session.id as the secret_id
    manager
        .create_secret_with_id(
            &storage_vault_id,
            &session.id,
            &session.name,
            SecretCategory::Session,
            plaintext,
        )
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("Created session: {} in storage vault: {}", session.id, storage_vault_id);
    open_approvals(&manager, &mut session).await;
    Ok(session)
}

/// Update an existing session configuration. O(1) update.
#[tauri::command]
// The session carries its secrets; only its id is logged.
#[tracing::instrument(skip_all, fields(id = %session.id))]
pub async fn session_update(
    state: State<'_, AppState>,
    session: SessionConfig,
) -> Result<SessionConfig, String> {
    let manager = state.vault_manager.lock().await;

    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let (loc, stored) = prepare_update(&manager, session).await?;
    let json = serde_json::to_string(&stored.record).map_err(|e| e.to_string())?;
    let plaintext = SecretBox::new(Box::new(json.into_bytes()));

    // O(1) update by primary key
    manager
        .update_secret(&loc.vault_id, &loc.secret_id, plaintext)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("Updated session: {} in vault: {}", stored.session.id, loc.vault_id);
    let mut session = stored.session;
    open_approvals(&manager, &mut session).await;
    Ok(session)
}

/// A session about to be written: the record as stored, and the session
/// as the window sent it, with approvals sealed.
struct Prepared {
    record: SessionConfig,
    session: SessionConfig,
}

/// Everything `session_update` does before it writes: find the session,
/// check its credential links, keep the stored approvals and login as the
/// editor expects, and seal the user's approvals. Shared with the bulk
/// edit, so a session changed there is stored exactly as the editor would.
async fn prepare_update(
    manager: &crate::vault::VaultManager,
    mut session: SessionConfig,
) -> Result<(SessionLoc, Prepared), String> {
    session.tidy_documentation();
    session.rdp_clipboard = rdp_clipboard_setting(session.rdp_clipboard.take(), session.kind);
    if session.kind != crate::state::SessionKind::Rdp {
        session.rdp_use_freerdp = false;
        session.rdp_forward_lock_keys = None;
        session.rdp = None;
    }
    session.rdp = session.rdp.take().and_then(crate::rdp::options::RdpOptions::tidy);

    // Find which vault contains this session
    let loc = locate_session(manager, &session.id).await
        .ok_or_else(|| format!("Session not found: {}", session.id))?;

    let logins = creds::logins(manager).await;
    let vault = creds::vault_of(manager, &loc.vault_id);
    creds::check_links(&logins.creds, &session, vault.as_deref())?;

    // The record being replaced, as this user reads it: which password
    // manager references they had approved (see `seal_for`), and its own
    // login and the one a credential filled in, if one did.
    // Its own typed password and password history as stored, before a
    // credential fills anything in.
    let mut past: (Option<String>, Option<Vec<crate::state::PastPassword>>) = (None, None);
    let (stored, before) = match read_session(manager, &loc.vault_id, &loc.secret_id).await {
        Ok(mut s) => {
            past = (pwh::typed_password(&s.auth_method).map(str::to_string), s.password_history.take());
            let own = creds::SessionLogins::all(&s);
            let replaced = creds::fill(&mut s, vault.as_deref(), &logins.folders, &logins.creds);
            let filled = creds::SessionLogins::all(&s).only(&replaced);
            open_approvals(manager, &mut s).await;
            (Some(s), Some((own, filled)))
        }
        Err(_) => (None, None),
    };

    // Moving it to another folder: one of its own vault. A session an
    // older version filed across vaults can still be saved as it is.
    if session.folder_id.is_some() && stored.as_ref().map(|s| &s.folder_id) != Some(&session.folder_id) {
        check_folder_vault(&read_folders(manager).await?, session.folder_id.as_deref(), vault.as_deref())?;
    }

    // The place in the list is the stored one (another device may have
    // moved it since the editor opened); in another folder it starts last.
    session.order = match stored.as_ref() {
        Some(s) if s.folder_id == session.folder_id => s.order,
        _ => None,
    };

    // The approvals kept for a credential's entries are the stored ones,
    // never what the window sent.
    session.credential_ref_approvals = stored.as_ref().map(|s| s.credential_ref_approvals.clone()).unwrap_or_default();
    // The trusted host keys are the stored ones: a connect may have saved
    // one while the editor was open, and the editor never changes them.
    session.host_keys = stored.as_ref().and_then(|s| s.host_keys.clone());
    session.host_keys_removed = stored.as_ref().and_then(|s| s.host_keys_removed.clone());
    // The certificates a web session trusts likewise: answering the prompt
    // adds them, `web_session_cert_*` changes them, the editor never does.
    if let Some(w) = session.web.as_mut() {
        w.certs = stored.as_ref().and_then(|s| s.web.as_ref()).map(|w| w.certs.clone()).unwrap_or_default();
    }
    // An FTP session's the same way (see `ftp_session_cert_trust`).
    if session.kind == crate::state::SessionKind::Ftp {
        let certs = stored.as_ref().and_then(|s| s.ftp.as_ref()).map(|f| f.certs.clone()).unwrap_or_default();
        session.ftp.get_or_insert_with(Default::default).certs = certs;
        if session.ftp.as_ref().is_some_and(|f| f.is_empty()) {
            session.ftp = None;
        }
    } else {
        session.ftp = None;
    }
    if session.kind != crate::state::SessionKind::Ssh {
        session.files = None;
    }
    // A server management session's too (see `bmc_session_cert_trust`); its
    // VNC server is the editor's.
    session.bmc = if session.kind == crate::state::SessionKind::Bmc {
        let mut b = session.bmc.take().unwrap_or_default();
        b.certs = stored.as_ref().and_then(|s| s.bmc.as_ref()).map(|b| b.certs.clone()).unwrap_or_default();
        Some(b).filter(|b| !b.is_empty())
    } else {
        None
    };
    // A Proxmox session's realm and token are the editor's, its
    // certificates the stored ones (see `proxmox_session_cert_trust`).
    session.proxmox = if session.kind == crate::state::SessionKind::Proxmox {
        let mut p = session.proxmox.take().unwrap_or_default();
        p.certs = stored.as_ref().and_then(|s| s.proxmox.as_ref()).map(|p| p.certs.clone()).unwrap_or_default();
        Some(p).filter(|p| !p.is_empty())
    } else {
        None
    };

    // A login sent is the session's own, unless it is one a credential
    // filled in (the editor shows that one): then the stored own login stays.
    let own = creds::fill_for_save(
        &mut session,
        vault.as_deref(),
        &logins.folders,
        &logins.creds,
        before.as_ref().map(|(own, filled)| (own, filled)),
    );
    seal_approvals(manager, &mut session, stored.as_ref()).await;
    let mut record = to_store(&session, own);
    // The history is the stored one, with the stored password in front
    // when this save replaces it. Never what the window sent.
    record.password_history = pwh::carry(
        past.1.as_deref(),
        past.0.as_deref(),
        pwh::typed_password(&record.auth_method),
        pwh::limit(manager).await,
        pwh::now(),
    );
    session.password_history = None;
    Ok((loc, Prepared { record, session }))
}

#[path = "session_bulk.rs"]
mod bulk;
pub use bulk::*;

#[path = "session_hostkeys_cmds.rs"]
mod hostkeys;
#[path = "session_webcerts_cmds.rs"]
mod webcerts;
pub use webcerts::*;

#[path = "session_ftp_cmds.rs"]
mod ftp;
pub use ftp::*;

#[path = "session_bmc_cmds.rs"]
mod bmc;
pub use bmc::*;

#[path = "session_proxmox_cmds.rs"]
pub(crate) mod proxmox;
pub use proxmox::*;
pub use hostkeys::*;

#[path = "session_password_history_cmds.rs"]
mod password_history_cmds;
pub use password_history_cmds::*;

/// Move many sessions into a folder (`None`: out of any folder) in one
/// go. Only `folder_id` changes: each stored session is read and written
/// back with every other field as it was, including fields a newer Reach
/// may have added. Returns how many moved; a session that no longer
/// exists is skipped.
#[tauri::command]
#[tracing::instrument(skip(state, session_ids), fields(count = session_ids.len()))]
pub async fn session_move_to_folder(
    state: State<'_, AppState>,
    session_ids: Vec<String>,
    folder_id: Option<String>,
) -> Result<u32, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    move_to_folder(&manager, &session_ids, folder_id).await
}

async fn move_to_folder(
    manager: &crate::vault::VaultManager,
    session_ids: &[String],
    folder_id: Option<String>,
) -> Result<u32, String> {
    // Every session is checked before any is written, so a move that mixes
    // vaults leaves them all where they were.
    let folders = match folder_id {
        Some(_) => read_folders(manager).await?,
        None => Vec::new(),
    };
    let mut found = Vec::new();
    for id in session_ids {
        let Some(loc) = locate_session(manager, id).await else { continue };
        let vault = creds::vault_of(manager, &loc.vault_id);
        check_folder_vault(&folders, folder_id.as_deref(), vault.as_deref())?;
        found.push((id, loc));
    }
    let mut moved = 0u32;
    for (id, loc) in found {
        let plaintext = manager.read_secret(&loc.vault_id, &loc.secret_id).await.map_err(|e| e.to_string())?;
        use secrecy::ExposeSecret;
        let mut value: serde_json::Value =
            serde_json::from_slice(plaintext.expose_secret()).map_err(|e| format!("Invalid session data: {e}"))?;
        let Some(obj) = value.as_object_mut() else { return Err(format!("Invalid session data: {id}")) };
        let target = folder_id.clone().map_or(serde_json::Value::Null, serde_json::Value::String);
        if obj.get("folder_id") == Some(&target) {
            continue;
        }
        obj.insert("folder_id".into(), target);
        // Its place among the old folder's sessions means nothing in the
        // new one: it goes after those already ordered there.
        obj.remove("order");
        let json = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        manager
            .update_secret(&loc.vault_id, &loc.secret_id, SecretBox::new(Box::new(json)))
            .await
            .map_err(|e| e.to_string())?;
        moved += 1;
    }
    tracing::info!("Moved {moved} sessions to folder {folder_id:?}");
    Ok(moved)
}

/// Mark sessions as favorites, or unmark them, in one go. Like
/// `session_move_to_folder`, only `favorite` changes: each stored session
/// is written back with every other field as it was. Returns how many
/// changed; a session that no longer exists is skipped.
#[tauri::command]
#[tracing::instrument(skip(state, session_ids), fields(count = session_ids.len()))]
pub async fn session_set_favorite(
    state: State<'_, AppState>,
    session_ids: Vec<String>,
    favorite: bool,
) -> Result<u32, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let mut changed = 0u32;
    for id in &session_ids {
        let Some(loc) = locate_session(&manager, id).await else { continue };
        let plaintext = manager.read_secret(&loc.vault_id, &loc.secret_id).await.map_err(|e| e.to_string())?;
        use secrecy::ExposeSecret;
        let mut value: serde_json::Value =
            serde_json::from_slice(plaintext.expose_secret()).map_err(|e| format!("Invalid session data: {e}"))?;
        let Some(obj) = value.as_object_mut() else { return Err(format!("Invalid session data: {id}")) };
        let was = obj.get("favorite").and_then(serde_json::Value::as_bool).unwrap_or(false);
        if was == favorite {
            continue;
        }
        // Off is no field at all, as a session that was never a favorite.
        if favorite {
            obj.insert("favorite".into(), serde_json::Value::Bool(true));
        } else {
            obj.remove("favorite");
        }
        let json = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        manager
            .update_secret(&loc.vault_id, &loc.secret_id, SecretBox::new(Box::new(json)))
            .await
            .map_err(|e| e.to_string())?;
        changed += 1;
    }
    tracing::info!("Set favorite={favorite} on {changed} sessions");
    Ok(changed)
}

/// Put sessions in this order: each gets its index in `session_ids` as its
/// `order`. Meant for the sessions of one folder (or those outside any),
/// all of them, so the list reads the same on every device. Like
/// `session_set_favorite`, only that field changes and a session whose
/// place is already right is not written again. Returns how many changed;
/// a session that no longer exists is skipped.
#[tauri::command]
#[tracing::instrument(skip(state, session_ids), fields(count = session_ids.len()))]
pub async fn session_set_order(state: State<'_, AppState>, session_ids: Vec<String>) -> Result<u32, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    set_order(&manager, &session_ids).await
}

async fn set_order(manager: &crate::vault::VaultManager, session_ids: &[String]) -> Result<u32, String> {
    let mut changed = 0u32;
    for (index, id) in session_ids.iter().enumerate() {
        let Some(loc) = locate_session(manager, id).await else { continue };
        let plaintext = manager.read_secret(&loc.vault_id, &loc.secret_id).await.map_err(|e| e.to_string())?;
        use secrecy::ExposeSecret;
        let mut value: serde_json::Value =
            serde_json::from_slice(plaintext.expose_secret()).map_err(|e| format!("Invalid session data: {e}"))?;
        let Some(obj) = value.as_object_mut() else { return Err(format!("Invalid session data: {id}")) };
        let order = u32::try_from(index).map_err(|_| "Too many sessions".to_string())?;
        if set_order_field(obj, order) {
            let json = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
            manager
                .update_secret(&loc.vault_id, &loc.secret_id, SecretBox::new(Box::new(json)))
                .await
                .map_err(|e| e.to_string())?;
            changed += 1;
        }
    }
    tracing::info!("Reordered {} sessions, {changed} changed", session_ids.len());
    Ok(changed)
}

/// Set a stored session's `order`; false when it already had that one.
fn set_order_field(obj: &mut serde_json::Map<String, serde_json::Value>, order: u32) -> bool {
    let target = serde_json::Value::from(order);
    if obj.get("order") == Some(&target) {
        return false;
    }
    obj.insert("order".into(), target);
    true
}

/// Delete a session by ID. O(1) delete.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_delete(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    let manager = state.vault_manager.lock().await;

    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }

    // Find which vault contains this session
    let loc = match locate_session(&manager, &session_id).await {
        Some(loc) => loc,
        None => {
            // Already deleted, or never stored. Say so: a silent success left
            // the session on screen and looked like the click did nothing.
            tracing::warn!("session_delete: session {session_id} is not in any open vault");
            return Ok(());
        }
    };

    // O(1) delete by primary key
    manager
        .delete_secret(&loc.vault_id, &loc.secret_id)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("Deleted session: {} from vault: {}", session_id, loc.vault_id);
    Ok(())
}

/// List all session folders. O(n) where n = number of folders.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_list_folders(state: State<'_, AppState>) -> Result<Vec<Folder>, String> {
    let manager = state.vault_manager.lock().await;

    if manager.is_locked() {
        return Ok(Vec::new());
    }

    move_folders_to_their_vaults(&manager).await;
    read_folders(&manager).await
}

/// Create a new session folder. O(1) insert.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_create_folder(
    state: State<'_, AppState>,
    name: String,
    parent_id: Option<String>,
    vault_id: Option<String>,
) -> Result<Folder, String> {
    let mut manager = state.vault_manager.lock().await;

    if manager.is_locked() {
        return Err("Vault is locked. Set a master password first.".to_string());
    }

    create_folder(&mut manager, name, parent_id, vault_id).await
}

async fn create_folder(
    manager: &mut crate::vault::VaultManager,
    name: String,
    parent_id: Option<String>,
    vault_id: Option<String>,
) -> Result<Folder, String> {
    // A folder lives in the vault its sessions are in, so it syncs with
    // them; one for this device's own sessions stays in __folders__.
    let vault_id = match vault_id {
        Some(vid) if is_user_vault(manager, &vid).await? => Some(vid),
        _ => None,
    };
    let storage_vault_id = match vault_id {
        Some(ref vid) => vid.clone(),
        None => ensure_folders_vault(manager).await?,
    };
    if let Some(ref parent_id) = parent_id {
        let parent = read_folders(manager)
            .await?
            .into_iter()
            .find(|folder| folder.id == *parent_id)
            .ok_or_else(|| format!("Parent folder not found: {parent_id}"))?;
        if parent.vault_id != vault_id {
            return Err("Folders in different vaults cannot be nested".to_string());
        }
    }

    let folder = Folder {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.clone(),
        parent_id,
        vault_id,
        credential_id: None,
        tmux: None,
        icon: None,
        color: None,
    };

    let json = serde_json::to_string(&folder).map_err(|e| e.to_string())?;
    let plaintext = SecretBox::new(Box::new(json.into_bytes()));

    // O(1) insert
    manager
        .create_secret_with_id(
            &storage_vault_id,
            &folder.id,
            &name,
            SecretCategory::Folder,
            plaintext,
        )
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("Created folder: {}", folder.id);
    Ok(folder)
}

/// Move a folder under another folder, or to the root.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_update_folder_parent(
    state: State<'_, AppState>,
    folder_id: String,
    parent_id: Option<String>,
) -> Result<Folder, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }

    set_folder_parent(&manager, folder_id, parent_id).await
}

async fn set_folder_parent(
    manager: &crate::vault::VaultManager,
    folder_id: String,
    parent_id: Option<String>,
) -> Result<Folder, String> {
    let folders = read_folders(manager).await?;
    let by_id = folders
        .iter()
        .map(|folder| (folder.id.as_str(), folder))
        .collect::<std::collections::HashMap<_, _>>();
    let mut folder = by_id
        .get(folder_id.as_str())
        .cloned()
        .cloned()
        .ok_or_else(|| format!("Folder not found: {folder_id}"))?;

    if let Some(ref target_id) = parent_id {
        let target = by_id
            .get(target_id.as_str())
            .ok_or_else(|| format!("Parent folder not found: {target_id}"))?;
        if target.vault_id != folder.vault_id {
            return Err("Folders in different vaults cannot be nested".to_string());
        }

        let mut current = Some(target_id.as_str());
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = current {
            if id == folder_id {
                return Err("A folder cannot be moved into itself or its descendant".to_string());
            }
            if !seen.insert(id) {
                return Err("Folder hierarchy contains a cycle".to_string());
            }
            current = by_id.get(id).and_then(|item| item.parent_id.as_deref());
        }
    }

    folder.parent_id = parent_id;
    let json = serde_json::to_string(&folder).map_err(|e| e.to_string())?;
    let plaintext = SecretBox::new(Box::new(json.into_bytes()));
    let vault_id = find_folder_vault(manager, &folder.id)
        .await
        .ok_or_else(|| format!("Folder not found: {}", folder.id))?;
    manager
        .update_secret(&vault_id, &folder.id, plaintext)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!("Moved folder: {} under {:?}", folder.id, folder.parent_id);
    Ok(folder)
}

/// Delete a session folder by ID. O(1) delete.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_delete_folder(
    state: State<'_, AppState>,
    folder_id: String,
) -> Result<(), String> {
    let manager = state.vault_manager.lock().await;

    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }

    delete_folder(&manager, &folder_id).await
}

/// Removes the folder from its vault and any copy left in __folders__ by a
/// move cut short, which would otherwise be moved back on the next listing.
async fn delete_folder(manager: &crate::vault::VaultManager, folder_id: &str) -> Result<(), String> {
    let mut holders: Vec<String> = find_in_user_vaults(manager, folder_id).await.into_iter().collect();
    if let Some(local) = get_folders_vault_id_if_exists(manager) {
        if manager.secret_exists(&local, folder_id).await {
            holders.push(local);
        }
    }
    for vault_id in holders {
        manager
            .delete_secret(&vault_id, folder_id)
            .await
            .map_err(|e| e.to_string())?;
        tracing::info!("Deleted folder: {} from vault: {}", folder_id, vault_id);
    }
    Ok(())
}

/// Every folder: this device's own from __folders__, and each user vault's
/// from that vault. A folder found in both (a move cut short) is the user
/// vault's.
async fn read_folders(manager: &crate::vault::VaultManager) -> Result<Vec<Folder>, String> {
    let mut folders: Vec<Folder> = Vec::new();
    if let Some(vault_id) = get_folders_vault_id_if_exists(manager) {
        folders.extend(folders_in(manager, &vault_id).await?);
    }
    for vault in manager.list_vaults().await.unwrap_or_default() {
        if vault.name.starts_with("__") {
            continue;
        }
        // A vault that is not open here (unreachable, not yet unlocked)
        // shows its folders once it is.
        let Ok(found) = folders_in(manager, &vault.id).await else { continue };
        for mut folder in found {
            // A vault has its own id on each device; the stored folder
            // names the one it was made on.
            folder.vault_id = Some(vault.id.clone());
            folders.retain(|f| f.id != folder.id);
            folders.push(folder);
        }
    }
    Ok(folders)
}

/// A session goes only into a folder of its own vault (`None`: this
/// device's Private sessions and folders), so each vault's folders are its
/// own and a folder never shows up under another vault. A folder not found
/// (its vault is not open here) is let through.
fn check_folder_vault(folders: &[Folder], folder_id: Option<&str>, session_vault: Option<&str>) -> Result<(), String> {
    let Some(id) = folder_id else { return Ok(()) };
    match folders.iter().find(|f| f.id == id) {
        Some(folder) if folder.vault_id.as_deref() != session_vault => {
            Err("A session can only go into a folder in its own vault".to_string())
        }
        _ => Ok(()),
    }
}

async fn folders_in(manager: &crate::vault::VaultManager, vault_id: &str) -> Result<Vec<Folder>, String> {
    let secrets = manager
        .read_secrets_in(vault_id, &["folder"])
        .await
        .map_err(|e| e.to_string())?;
    let mut folders = Vec::new();
    for (_, plaintext) in secrets {
        if let Ok(plaintext) = plaintext {
            use secrecy::ExposeSecret;
            if let Ok(folder) = serde_json::from_slice::<Folder>(plaintext.expose_secret()) {
                folders.push(folder);
            }
        }
    }
    Ok(folders)
}

/// Before 0.8.2 every folder was kept in __folders__, which never leaves
/// the device, so a shared vault's folders reached no other device or
/// member. Each one is copied into its vault and only then removed here;
/// one whose vault is not open now is moved on a later listing.
async fn move_folders_to_their_vaults(manager: &crate::vault::VaultManager) {
    let Some(local) = get_folders_vault_id_if_exists(manager) else { return };
    let Ok(secrets) = manager.read_secrets_in(&local, &["folder"]).await else { return };
    let user_vaults: Vec<String> = manager
        .list_vaults()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|v| !v.name.starts_with("__"))
        .map(|v| v.id)
        .collect();
    for (meta, plaintext) in secrets {
        let Ok(plaintext) = plaintext else { continue };
        use secrecy::ExposeSecret;
        let Ok(folder) = serde_json::from_slice::<Folder>(plaintext.expose_secret()) else { continue };
        let Some(target) = folder.vault_id.as_ref().filter(|id| user_vaults.contains(id)) else { continue };
        let copy = SecretBox::new(Box::new(plaintext.expose_secret().clone()));
        if let Err(e) = manager
            .create_secret_with_id(target, &folder.id, &meta.name, SecretCategory::Folder, copy)
            .await
        {
            tracing::warn!("Folder {} stays on this device for now: {}", folder.id, e);
            continue;
        }
        match manager.delete_secret(&local, &folder.id).await {
            Ok(()) => tracing::info!("Moved folder {} into vault {}", folder.id, target),
            Err(e) => tracing::warn!("Folder {} copied to vault {}, local copy left: {}", folder.id, target, e),
        }
    }
}

// --- Helper functions (all O(1)) ---

/// Ensure the sessions vault exists. O(1).
async fn ensure_sessions_vault(
    manager: &mut crate::vault::VaultManager,
) -> Result<String, String> {
    if let Some(vault_id) = manager.get_vault_id_by_name(SESSIONS_VAULT_NAME) {
        let _ = manager.open_vault(&vault_id, None, None).await;
        manager
            .unlock_vault(&vault_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(vault_id)
    } else {
        let vault = manager
            .create_vault(SESSIONS_VAULT_NAME, crate::vault::types::VaultType::Private, None, None)
            .await
            .map_err(|e| e.to_string())?;
        Ok(vault.id)
    }
}

/// Ensure the folders vault exists. O(1).
async fn ensure_folders_vault(
    manager: &mut crate::vault::VaultManager,
) -> Result<String, String> {
    if let Some(vault_id) = manager.get_vault_id_by_name(FOLDERS_VAULT_NAME) {
        let _ = manager.open_vault(&vault_id, None, None).await;
        manager
            .unlock_vault(&vault_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(vault_id)
    } else {
        let vault = manager
            .create_vault(FOLDERS_VAULT_NAME, crate::vault::types::VaultType::Private, None, None)
            .await
            .map_err(|e| e.to_string())?;
        Ok(vault.id)
    }
}

/// Get sessions vault ID if exists. O(1).
fn get_sessions_vault_id_if_exists(manager: &crate::vault::VaultManager) -> Option<String> {
    manager.get_vault_id_by_name(SESSIONS_VAULT_NAME)
}

/// Get folders vault ID if exists. O(1).
fn get_folders_vault_id_if_exists(manager: &crate::vault::VaultManager) -> Option<String> {
    manager.get_vault_id_by_name(FOLDERS_VAULT_NAME)
}

/// Where a session's ciphertext lives. `secret_id` is the secrets-table row,
/// which is the session id except for a row copied into the unified personal
/// vault (see [`crate::vault::manager::unified_secret_id`]).
struct SessionLoc {
    vault_id: String,
    secret_id: String,
}

/// Find which vault contains a session, and the row to read or write.
/// Checks __sessions__ first (its own id, then the personal-sync copy), then
/// all user vaults.
async fn locate_session(manager: &crate::vault::VaultManager, session_id: &str) -> Option<SessionLoc> {
    if let Some(vault_id) = get_sessions_vault_id_if_exists(manager) {
        if let Some(secret_id) = session_row_id(manager, &vault_id, session_id).await {
            return Some(SessionLoc { vault_id, secret_id });
        }
    }
    let vault_id = find_in_user_vaults(manager, session_id).await?;
    Some(SessionLoc { vault_id, secret_id: session_id.to_string() })
}

/// The secrets-table id for a session in one vault.
///
/// Personal sync copies every internal vault into one database and stores
/// each row as `{vault}_{original_id}` so the ids cannot collide. The
/// session JSON still has the original id, which is what the list shows and
/// what edit and delete send. Looking up only that id misses the copy: delete
/// then reports success and the session stays, and saving it (including a
/// move into a folder) returns "Session not found".
async fn session_row_id(manager: &crate::vault::VaultManager, vault_id: &str, session_id: &str) -> Option<String> {
    if manager.secret_exists(vault_id, session_id).await {
        return Some(session_id.to_string());
    }
    let migrated = crate::vault::manager::unified_secret_id(SESSIONS_VAULT_NAME, session_id);
    if migrated != session_id && manager.secret_exists(vault_id, &migrated).await {
        return Some(migrated);
    }
    None
}

/// Find which vault contains a folder by ID: the user vaults first, as
/// `read_folders` prefers them, then __folders__.
async fn find_folder_vault(manager: &crate::vault::VaultManager, folder_id: &str) -> Option<String> {
    if let Some(vault_id) = find_in_user_vaults(manager, folder_id).await {
        return Some(vault_id);
    }
    let local = get_folders_vault_id_if_exists(manager)?;
    manager.secret_exists(&local, folder_id).await.then_some(local)
}

/// The user vault holding a secret, if any.
async fn find_in_user_vaults(manager: &crate::vault::VaultManager, secret_id: &str) -> Option<String> {
    if let Ok(vaults) = manager.list_vaults().await {
        for vault in vaults {
            if vault.name.starts_with("__") {
                continue;
            }
            if manager.secret_exists(&vault.id, secret_id).await {
                return Some(vault.id);
            }
        }
    }

    None
}

/// Whether an id names one of the user's own or joined vaults, not an internal one.
async fn is_user_vault(manager: &crate::vault::VaultManager, vault_id: &str) -> Result<bool, String> {
    let vaults = manager.list_vaults().await.map_err(|e| e.to_string())?;
    Ok(vaults.iter().any(|v| v.id == vault_id && !v.name.starts_with("__")))
}

/// Set or clear the saved credential a folder's sessions log in with. It
/// must be in the folder's vault.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_set_folder_credential(
    state: State<'_, AppState>,
    folder_id: String,
    credential_id: Option<String>,
) -> Result<Folder, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let mut folder = read_folders(&manager)
        .await?
        .into_iter()
        .find(|f| f.id == folder_id)
        .ok_or_else(|| format!("Folder not found: {folder_id}"))?;
    let credential_id = credential_id.filter(|c| !c.is_empty());
    let all = creds::read_credentials(&manager).await;
    creds::check_link(&all, credential_id.as_deref(), folder.vault_id.as_deref())?;
    folder.credential_id = credential_id;
    let vault_id = find_folder_vault(&manager, &folder.id)
        .await
        .ok_or_else(|| format!("Folder not found: {}", folder.id))?;
    let json = serde_json::to_vec(&folder).map_err(|e| e.to_string())?;
    manager
        .update_secret(&vault_id, &folder.id, SecretBox::new(Box::new(json)))
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("Folder {} now uses credential {:?}", folder.id, folder.credential_id);
    Ok(folder)
}

/// Turn tmux on or off for a folder's sessions (`None`: as the folder
/// above says).
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_set_folder_tmux(
    state: State<'_, AppState>,
    folder_id: String,
    tmux: Option<bool>,
) -> Result<Folder, String> {
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let mut folder = read_folders(&manager)
        .await?
        .into_iter()
        .find(|f| f.id == folder_id)
        .ok_or_else(|| format!("Folder not found: {folder_id}"))?;
    folder.tmux = tmux;
    let vault_id = find_folder_vault(&manager, &folder.id)
        .await
        .ok_or_else(|| format!("Folder not found: {}", folder.id))?;
    let json = serde_json::to_vec(&folder).map_err(|e| e.to_string())?;
    manager
        .update_secret(&vault_id, &folder.id, SecretBox::new(Box::new(json)))
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("Folder {} tmux: {:?}", folder.id, folder.tmux);
    Ok(folder)
}

/// The longest icon or colour name a folder keeps; anything longer is not
/// one of ours.
const FOLDER_ICON_MAX: usize = 64;

/// An icon or colour name as stored: trimmed, `None` when empty, refused
/// when it is too long or holds anything but letters, digits, `-`, `_`
/// and one `:`.
pub(crate) fn folder_icon_value(value: Option<String>, what: &str) -> Result<Option<String>, String> {
    let Some(value) = value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let ok = value.len() <= FOLDER_ICON_MAX
        && value.matches(':').count() <= 1
        && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'));
    if ok { Ok(Some(value)) } else { Err(format!("Not a folder {what}: {value}")) }
}

/// Set or clear a folder's icon and its colour. Both are part of the
/// folder's record, so they sync with it.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_set_folder_icon(
    state: State<'_, AppState>,
    folder_id: String,
    icon: Option<String>,
    color: Option<String>,
) -> Result<Folder, String> {
    let icon = folder_icon_value(icon, "icon")?;
    let color = folder_icon_value(color, "colour")?;
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let mut folder = read_folders(&manager)
        .await?
        .into_iter()
        .find(|f| f.id == folder_id)
        .ok_or_else(|| format!("Folder not found: {folder_id}"))?;
    folder.icon = icon;
    folder.color = color;
    let vault_id = find_folder_vault(&manager, &folder.id)
        .await
        .ok_or_else(|| format!("Folder not found: {}", folder.id))?;
    let json = serde_json::to_vec(&folder).map_err(|e| e.to_string())?;
    manager
        .update_secret(&vault_id, &folder.id, SecretBox::new(Box::new(json)))
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("Folder {} icon {:?} colour {:?}", folder.id, folder.icon, folder.color);
    Ok(folder)
}

/// Rename a folder. Its sessions and subfolders stay where they are.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn session_rename_folder(
    state: State<'_, AppState>,
    folder_id: String,
    name: String,
) -> Result<Folder, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Folder name cannot be empty".to_string());
    }
    let manager = state.vault_manager.lock().await;
    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }
    let mut folder = read_folders(&manager)
        .await?
        .into_iter()
        .find(|f| f.id == folder_id)
        .ok_or_else(|| format!("Folder not found: {folder_id}"))?;
    folder.name = name;
    let vault_id = find_folder_vault(&manager, &folder.id)
        .await
        .ok_or_else(|| format!("Folder not found: {}", folder.id))?;
    let json = serde_json::to_vec(&folder).map_err(|e| e.to_string())?;
    manager
        .update_secret(&vault_id, &folder.id, SecretBox::new(Box::new(json)))
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("Folder {} renamed", folder.id);
    Ok(folder)
}

// --- For saved_credential_commands ---

pub(crate) fn sessions_vault_id(manager: &crate::vault::VaultManager) -> Option<String> {
    get_sessions_vault_id_if_exists(manager)
}

pub(crate) async fn sessions_vault(manager: &mut crate::vault::VaultManager) -> Result<String, String> {
    ensure_sessions_vault(manager).await
}

pub(crate) async fn row_id_in(manager: &crate::vault::VaultManager, vault_id: &str, id: &str) -> Option<String> {
    session_row_id(manager, vault_id, id).await
}

pub(crate) async fn user_vault_holding(manager: &crate::vault::VaultManager, id: &str) -> Option<String> {
    find_in_user_vaults(manager, id).await
}

pub(crate) async fn user_vault(manager: &crate::vault::VaultManager, vault_id: &str) -> Result<bool, String> {
    is_user_vault(manager, vault_id).await
}

/// Every folder, or none when they cannot be read (a session is then read
/// as if its folders named no credential).
pub(crate) async fn all_folders(manager: &crate::vault::VaultManager) -> Vec<Folder> {
    read_folders(manager).await.unwrap_or_default()
}

/// Every session and folder that names a credential stops naming it.
/// Each record is edited as stored, so fields a newer Reach added stay.
/// Returns how many changed.
pub(crate) async fn unlink_credential(manager: &crate::vault::VaultManager, credential_id: &str) -> Result<u32, String> {
    let mut vaults: Vec<(String, &[&str])> = Vec::new();
    if let Some(v) = get_sessions_vault_id_if_exists(manager) {
        vaults.push((v, &["session", "custom:session"]));
    }
    if let Some(v) = get_folders_vault_id_if_exists(manager) {
        vaults.push((v, &["folder"]));
    }
    for v in manager.list_vaults().await.unwrap_or_default() {
        if !v.name.starts_with("__") {
            vaults.push((v.id, &["session", "custom:session", "folder"]));
        }
    }
    let mut changed = 0u32;
    for (vault_id, categories) in vaults {
        let Ok(secrets) = manager.read_secrets_in(&vault_id, categories).await else { continue };
        for (meta, plaintext) in secrets {
            let Ok(plaintext) = plaintext else { continue };
            use secrecy::ExposeSecret;
            let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(plaintext.expose_secret()) else { continue };
            let Some(obj) = value.as_object_mut() else { continue };
            if obj.get("credential_id").and_then(|v| v.as_str()) != Some(credential_id) {
                continue;
            }
            obj.remove("credential_id");
            let json = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
            manager
                .update_secret(&vault_id, &meta.id, SecretBox::new(Box::new(json)))
                .await
                .map_err(|e| e.to_string())?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// Share a session with another user via X25519 key re-wrap.
#[tauri::command(rename_all = "snake_case")]
#[tracing::instrument(skip(state, recipient_public_key))]
pub async fn session_share(
    state: State<'_, AppState>,
    session_id: String,
    recipient_uuid: String,
    recipient_public_key: String,
    expires_in_hours: Option<u64>,
) -> Result<crate::vault::types::ShareItemResult, String> {
    let manager = state.vault_manager.lock().await;

    if manager.is_locked() {
        return Err("Vault is locked".to_string());
    }

    let loc = locate_session(&manager, &session_id)
        .await
        .ok_or_else(|| format!("Session not found: {}", session_id))?;

    // Decode recipient public key from base64
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    let pk_bytes = BASE64
        .decode(&recipient_public_key)
        .map_err(|e| format!("Invalid public key base64: {}", e))?;

    if pk_bytes.len() != 32 {
        return Err(format!("Invalid public key length: expected 32, got {}", pk_bytes.len()));
    }

    let mut pk_array = [0u8; 32];
    pk_array.copy_from_slice(&pk_bytes);

    manager
        .share_item(&loc.vault_id, &loc.secret_id, &recipient_uuid, &pk_array, expires_in_hours)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod unique_tests {
    use super::*;

    fn session(id: &str, vault: &str) -> SessionConfig {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "host": "h", "port": 22, "username": "u",
            "auth_method": { "type": "Agent" }, "folder_id": null, "tags": [],
            "vault_id": vault,
        }))
        .unwrap()
    }

    #[test]
    fn a_personal_sync_copy_is_stored_under_the_vault_prefix() {
        // `__sessions__` plus the separator plus the original id. Edit and
        // delete must look up this row, not only the id inside the JSON.
        assert_eq!(
            crate::vault::manager::unified_secret_id(SESSIONS_VAULT_NAME, "ab8dc7a4-4d90-4c04-bb80-fb0ac8ed3afa"),
            "__sessions___ab8dc7a4-4d90-4c04-bb80-fb0ac8ed3afa"
        );
    }

    #[test]
    fn favorite_is_stored_only_when_on() {
        let mut s = session("a", "v1");
        assert!(!s.favorite);
        assert!(serde_json::to_value(&s).unwrap().get("favorite").is_none());
        s.favorite = true;
        let back: SessionConfig = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert!(back.favorite);
    }

    #[test]
    fn a_session_in_two_vaults_is_listed_once() {
        let listed = unique_sessions(vec![session("a", "v1"), session("b", "v1"), session("a", "v2")]);
        let ids: Vec<_> = listed.iter().map(|s| (s.id.as_str(), s.vault_id.as_deref())).collect();
        assert_eq!(ids, [("a", Some("v1")), ("b", Some("v1"))]);
    }
}

#[cfg(test)]
mod approval_tests {
    use super::*;

    fn session() -> SessionConfig {
        serde_json::from_value(serde_json::json!({
            "id": "s1", "name": "web", "host": "web.example", "port": 22, "username": "u",
            "auth_method": { "type": "Agent" }, "folder_id": null, "tags": [],
            "ssh_options": { "lines": ["StrictHostKeyChecking no"] },
        }))
        .unwrap()
    }

    fn mine_after_reading(s: &SessionConfig, key: &[u8; 32]) -> Vec<String> {
        let mut s = s.clone();
        let context = approval_context(&s);
        let o = s.ssh_options.as_mut().unwrap();
        o.open_for(Some(key), Some("alice"), &context);
        o.my_accepted_weakenings.clone().unwrap()
    }

    #[test]
    fn an_approval_does_not_follow_a_changed_server() {
        let key = [3u8; 32];
        let mut s = session();
        s.ssh_options.as_mut().unwrap().my_accepted_weakenings = Some(vec!["StrictHostKeyChecking no".into()]);
        let context = approval_context(&s);
        s.ssh_options.as_mut().unwrap().seal_for(Some(&key), Some("alice"), &context);
        assert_eq!(mine_after_reading(&s, &key), vec!["StrictHostKeyChecking no"]);

        // Someone points the session at their own server: the approval stays
        // in the record but counts for nothing.
        let mut moved = s.clone();
        moved.host = "attacker.example".into();
        assert!(mine_after_reading(&moved, &key).is_empty());
        let mut lines = s.clone();
        lines.ssh_options.as_mut().unwrap().lines.push("ProxyJump evil".into());
        assert!(mine_after_reading(&lines, &key).is_empty());

        // Saving a password, a name or a folder changes none of that.
        let mut renamed = s.clone();
        renamed.name = "web (prod)".into();
        renamed.folder_id = Some("f".into());
        renamed.auth_method = AuthMethod::Password { password: Some("pw".into()), password_ref: None };
        assert_eq!(mine_after_reading(&renamed, &key), vec!["StrictHostKeyChecking no"]);
    }

    #[test]
    fn a_proxy_password_does_not_cost_the_approvals() {
        let key = [3u8; 32];
        let mut s = session();
        s.proxy = Some(serde_json::from_value(serde_json::json!({ "proxy_type": "socks5", "host": "p.example", "port": 1080 })).unwrap());
        s.ssh_options.as_mut().unwrap().my_accepted_weakenings = Some(vec!["StrictHostKeyChecking no".into()]);
        let context = approval_context(&s);
        s.ssh_options.as_mut().unwrap().seal_for(Some(&key), Some("alice"), &context);

        let mut with_password = s.clone();
        let p = with_password.proxy.as_mut().unwrap();
        p.password = Some("secret".into());
        p.password_ref = Some(entry("e1"));
        assert_eq!(approval_context(&with_password), context);
        assert_eq!(mine_after_reading(&with_password, &key), vec!["StrictHostKeyChecking no"]);

        // Where the proxy is still counts.
        let mut moved = s.clone();
        moved.proxy.as_mut().unwrap().host = "attacker.example".into();
        assert!(mine_after_reading(&moved, &key).is_empty());
    }

    // --- Password manager references -------------------------------------

    const ALICE: [u8; 32] = [7; 32];
    const BOB: [u8; 32] = [8; 32];

    fn entry(id: &str) -> crate::pwmanager::SecretRef {
        crate::pwmanager::SecretRef { provider: "passbolt".into(), entry_id: id.into(), ..Default::default() }
    }

    fn picked(id: &str) -> crate::pwmanager::SecretRef {
        crate::pwmanager::SecretRef { confirm: true, ..entry(id) }
    }

    /// A session with a reference in every slot: its password, a jump host's
    /// password and the proxy's.
    fn with_refs() -> SessionConfig {
        let mut s = session();
        s.auth_method = AuthMethod::Password { password: None, password_ref: Some(entry("e-auth")) };
        s.jump_chain = Some(vec![JumpHostConfig {
            host: "bastion.example".into(),
            port: 22,
            username: "jump".into(),
            auth_method: AuthMethod::Key { path: None, passphrase: None, key_content: None, key_id: Some("k".into()), passphrase_ref: Some(entry("e-jump")) },
            credential_id: None,
            credential: None,
        }]);
        s.proxy = Some(serde_json::from_value(serde_json::json!({
            "proxy_type": "http", "host": "proxy.example", "port": 3128, "username": "pu",
        }))
        .unwrap());
        s.proxy.as_mut().unwrap().password_ref = Some(entry("e-proxy"));
        s
    }

    /// Mark every reference as picked in the editor.
    fn pick_all(s: &mut SessionConfig) {
        for (_, r) in ref_slots_mut(s) {
            r.confirm = true;
        }
    }

    /// What `who` sees after the session is stored and read back.
    fn read_as(s: &SessionConfig, key: &[u8; 32], who: &str) -> SessionConfig {
        let stored: SessionConfig = serde_json::from_str(&serde_json::to_string(s).unwrap()).unwrap();
        let mut s = stored;
        open_for(Some(key), Some(who), &mut s, None);
        s
    }

    fn trusted(s: &SessionConfig) -> Vec<(String, bool)> {
        ref_slots(s).into_iter().map(|(slot, r)| (slot, r.trusted)).collect()
    }

    fn only_the_password() -> Vec<(String, bool)> {
        vec![("password".into(), true), ("jump:0:passphrase".into(), false), ("proxy".into(), false)]
    }

    /// Saved by alice with every reference picked.
    fn sealed_by_alice() -> SessionConfig {
        let mut s = with_refs();
        pick_all(&mut s);
        seal_for(Some(&ALICE), Some("alice"), &mut s, None, None);
        s
    }

    #[test]
    fn a_reference_counts_only_for_who_approved_it() {
        let s = sealed_by_alice();
        let slots: Vec<String> = ref_slots(&s).into_iter().map(|(slot, _)| slot).collect();
        assert_eq!(slots, ["password", "jump:0:passphrase", "proxy"]);
        assert!(trusted(&read_as(&s, &ALICE, "alice")).iter().all(|(_, t)| *t));
        // Bob shares the vault: alice's approvals are nothing to him.
        assert!(trusted(&read_as(&s, &BOB, "bob")).iter().all(|(_, t)| !*t));
        // Nor can he write one in her name with his key.
        let mut forged = with_refs();
        pick_all(&mut forged);
        seal_for(Some(&BOB), Some("alice"), &mut forged, None, None);
        assert!(trusted(&read_as(&forged, &ALICE, "alice")).iter().all(|(_, t)| !*t));
        // The flags never go to disk, and confirm never comes back out.
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("trusted") && !json.contains("confirm"), "{json}");
        let read = serde_json::to_string(&read_as(&s, &ALICE, "alice")).unwrap();
        assert!(read.contains("\"trusted\":true") && !read.contains("confirm"), "{read}");
    }

    #[test]
    fn a_reference_does_not_follow_a_changed_session() {
        let s = sealed_by_alice();
        let all_untrusted = |s: &SessionConfig| trusted(&read_as(s, &ALICE, "alice")).iter().all(|(_, t)| !*t);
        type Change = Box<dyn Fn(&mut SessionConfig)>;
        let changes: Vec<(&str, Change)> = vec![
            ("host", Box::new(|s| s.host = "attacker.example".into())),
            ("port", Box::new(|s| s.port = 2222)),
            ("user", Box::new(|s| s.username = "other".into())),
            ("kind", Box::new(|s| s.kind = crate::state::SessionKind::Rdp)),
            ("via", Box::new(|s| s.via_session_id = Some("evil".into()))),
            ("jump host", Box::new(|s| s.jump_chain.as_mut().unwrap()[0].host = "attacker.example".into())),
            ("jump added", Box::new(|s| {
                let hop = s.jump_chain.as_ref().unwrap()[0].clone();
                s.jump_chain.as_mut().unwrap().push(JumpHostConfig { host: "attacker.example".into(), ..hop });
            })),
            ("proxy host", Box::new(|s| s.proxy.as_mut().unwrap().host = "attacker.example".into())),
            ("proxy port", Box::new(|s| s.proxy.as_mut().unwrap().port = 8080)),
            ("ssh_config", Box::new(|s| s.ssh_options.as_mut().unwrap().lines.push("HostName attacker.example".into()))),
        ];
        for (what, change) in changes {
            let mut changed = s.clone();
            change(&mut changed);
            assert!(all_untrusted(&changed), "{what} kept an approval");
        }

        // Another entry in the same place, or the same entry in another place.
        let mut swapped = s.clone();
        if let AuthMethod::Password { password_ref: Some(r), .. } = &mut swapped.auth_method {
            r.entry_id = "e-victim".into();
        }
        assert_eq!(trusted(&read_as(&swapped, &ALICE, "alice"))[0], ("password".into(), false));
        let mut moved = s.clone();
        let jump_ref = match &moved.jump_chain.as_ref().unwrap()[0].auth_method {
            AuthMethod::Key { passphrase_ref: Some(r), .. } => r.clone(),
            _ => unreachable!(),
        };
        moved.auth_method = AuthMethod::Password { password: None, password_ref: Some(jump_ref) };
        assert_eq!(trusted(&read_as(&moved, &ALICE, "alice"))[0], ("password".into(), false));
        let mut as_passphrase = s.clone();
        let auth_ref = match &as_passphrase.auth_method {
            AuthMethod::Password { password_ref: Some(r), .. } => r.clone(),
            _ => unreachable!(),
        };
        as_passphrase.auth_method = AuthMethod::Key { path: Some("k".into()), passphrase: None, key_content: None, key_id: None, passphrase_ref: Some(auth_ref) };
        assert_eq!(trusted(&read_as(&as_passphrase, &ALICE, "alice"))[0], ("passphrase".into(), false));

        // A name, a folder, tags or the proxy's credentials change none of it.
        let mut renamed = s.clone();
        renamed.name = "web (prod)".into();
        renamed.folder_id = Some("f".into());
        renamed.tags = vec!["prod".into()];
        renamed.proxy.as_mut().unwrap().password = Some("typed".into());
        assert!(trusted(&read_as(&renamed, &ALICE, "alice")).iter().all(|(_, t)| *t));
    }

    #[test]
    fn trusted_from_the_webview_is_ignored() {
        let mut v = serde_json::to_value(with_refs()).unwrap();
        v["auth_method"]["password_ref"]["trusted"] = true.into();
        let s: SessionConfig = serde_json::from_value(v).unwrap();
        let r = ref_at(&s, "password").unwrap();
        assert!(!r.trusted);
        assert!(crate::pwmanager::check_trusted(r).is_err());
        // Nor does it survive a save.
        let mut saved = s.clone();
        seal_for(Some(&ALICE), Some("alice"), &mut saved, None, None);
        assert_eq!(trusted(&read_as(&saved, &ALICE, "alice"))[0], ("password".into(), false));
    }

    #[test]
    fn saving_again_does_not_approve_what_someone_else_put_there() {
        // Mallory, a member of the shared vault, adds her reference.
        let mut s = with_refs();
        pick_all(&mut s);
        seal_for(Some(&BOB), Some("mallory"), &mut s, None, None);

        // Alice opens it in the editor, renames it and saves: the reference
        // goes back as it came, unconfirmed.
        let stored = read_as(&s, &ALICE, "alice");
        let mut edited = stored.clone();
        edited.name = "renamed".into();
        seal_for(Some(&ALICE), Some("alice"), &mut edited, Some(&stored), None);
        assert!(trusted(&read_as(&edited, &ALICE, "alice")).iter().all(|(_, t)| !*t));
        // Mallory's own approvals are left alone.
        assert!(trusted(&read_as(&edited, &BOB, "mallory")).iter().all(|(_, t)| *t));

        // Picking it in the editor is what approves it.
        let mut repicked = read_as(&edited, &ALICE, "alice");
        let before = repicked.clone();
        if let AuthMethod::Password { password_ref: Some(r), .. } = &mut repicked.auth_method {
            r.confirm = true;
        }
        seal_for(Some(&ALICE), Some("alice"), &mut repicked, Some(&before), None);
        assert_eq!(
            trusted(&read_as(&repicked, &ALICE, "alice")),
            only_the_password()
        );
    }

    #[test]
    fn the_owner_editing_keeps_their_approvals() {
        let s = sealed_by_alice();
        // Alice renames it: nothing to confirm again.
        let stored = read_as(&s, &ALICE, "alice");
        let mut renamed = stored.clone();
        renamed.name = "renamed".into();
        seal_for(Some(&ALICE), Some("alice"), &mut renamed, Some(&stored), None);
        assert!(trusted(&read_as(&renamed, &ALICE, "alice")).iter().all(|(_, t)| *t));
        // She moves it to another server herself, in the editor, with the
        // entries in view: they stay hers, for the new server.
        let mut moved = stored.clone();
        moved.host = "new.example".into();
        seal_for(Some(&ALICE), Some("alice"), &mut moved, Some(&stored), None);
        assert!(trusted(&read_as(&moved, &ALICE, "alice")).iter().all(|(_, t)| *t));
        // But a different entry in a slot is not carried over.
        let mut swapped = stored.clone();
        if let AuthMethod::Password { password_ref: Some(r), .. } = &mut swapped.auth_method {
            *r = entry("e-other");
        }
        seal_for(Some(&ALICE), Some("alice"), &mut swapped, Some(&stored), None);
        assert_eq!(trusted(&read_as(&swapped, &ALICE, "alice"))[0], ("password".into(), false));
    }

    #[test]
    fn a_new_session_signs_only_what_was_picked() {
        let mut s = with_refs();
        if let AuthMethod::Password { password_ref: Some(r), .. } = &mut s.auth_method {
            *r = picked("e-auth");
        }
        seal_for(Some(&ALICE), Some("alice"), &mut s, None, None);
        assert_eq!(
            trusted(&read_as(&s, &ALICE, "alice")),
            only_the_password()
        );
    }

    #[test]
    fn untrusted_references_are_refused_with_what_to_ask() {
        let mut s = with_refs();
        s.jump_chain = None;
        open_for(Some(&ALICE), Some("alice"), &mut s, None);
        let err = require_trusted_refs(&s, None, &slots_in_use(&s)).unwrap_err();
        let json = err.strip_prefix(UNTRUSTED_PREFIX).expect(&err);
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(v["session_id"], "s1");
        assert_eq!(v["host"], "web.example");
        assert_eq!(v["proxy"], "http pu@proxy.example:3128");
        assert_eq!(v["context"], approval_context(&s));
        assert_eq!(v["refs"][0], serde_json::json!({ "slot": "password", "provider": "passbolt", "entry_id": "e-auth" }));
        assert_eq!(v["refs"][1]["slot"], "proxy");
        // A slot the connect does not use is not asked about.
        assert!(require_trusted_refs(&s, None, &[]).is_ok());
        // The proxy is not used behind jump hosts.
        let with_jump = with_refs();
        assert_eq!(slots_in_use(&with_jump), ["password", "jump:0:passphrase"]);
    }

    #[test]
    fn approving_signs_only_what_the_dialog_showed() {
        let mut s = with_refs();
        open_for(Some(&ALICE), Some("alice"), &mut s, None);
        let asked = untrusted_refs(&s, None, &slots_in_use(&s)).unwrap().unwrap();
        let first = vec![asked.refs[0].clone()];

        // The session changed after the dialog opened: nothing is approved.
        let mut moved = s.clone();
        moved.host = "attacker.example".into();
        assert!(approve_refs(&mut moved, None, &asked.context, &first).is_err());
        let mut swapped = s.clone();
        if let AuthMethod::Password { password_ref: Some(r), .. } = &mut swapped.auth_method {
            r.entry_id = "e-victim".into();
        }
        assert!(approve_refs(&mut swapped, None, &asked.context, &first).is_err());

        // As shown: that one is signed, the others are not.
        let before = s.clone();
        approve_refs(&mut s, None, &asked.context, &first).unwrap();
        seal_for(Some(&ALICE), Some("alice"), &mut s, Some(&before), None);
        assert_eq!(
            trusted(&read_as(&s, &ALICE, "alice")),
            only_the_password()
        );
    }

    #[test]
    fn a_connect_with_references_must_name_its_session() {
        use crate::ipc::ssh_commands::{wanted_slots, JumpHostConnectParams};
        assert!(require_session_id(None).is_err());
        assert!(require_session_id(Some("")).is_err());
        assert_eq!(require_session_id(Some("s1")), Ok("s1"));
        let hop: JumpHostConnectParams = serde_json::from_value(serde_json::json!({
            "host": "b", "port": 22, "username": "j", "authMethod": "key",
            "keyPassphraseRef": { "provider": "passbolt", "entry_id": "e" },
        }))
        .unwrap();
        assert_eq!(wanted_slots("password", true, false, Some(std::slice::from_ref(&hop)), None), ["password", "jump:0:passphrase"]);
        // A reference for a secret the login does not use asks for nothing.
        assert!(wanted_slots("agent", true, true, None, None).is_empty());
    }

    #[test]
    fn a_connect_must_go_where_the_session_goes() {
        use crate::ipc::ssh_commands::check_matches_session;
        let s = with_refs();
        let hops: Vec<crate::ipc::ssh_commands::JumpHostConnectParams> = vec![serde_json::from_value(serde_json::json!({
            "host": "bastion.example", "port": 22, "username": "jump", "authMethod": "key",
        }))
        .unwrap()];
        let proxy = s.proxy.clone();
        assert!(check_matches_session(&s, None, "web.example", 22, "u", Some(&hops), proxy.as_ref()).is_ok());
        assert!(check_matches_session(&s, None, "attacker.example", 22, "u", Some(&hops), proxy.as_ref()).is_err());
        assert!(check_matches_session(&s, None, "web.example", 22, "root", Some(&hops), proxy.as_ref()).is_err());
        assert!(check_matches_session(&s, None, "web.example", 22, "u", None, proxy.as_ref()).is_err());
        let mut elsewhere = proxy.clone();
        elsewhere.as_mut().unwrap().host = "attacker.example".into();
        assert!(check_matches_session(&s, None, "web.example", 22, "u", Some(&hops), elsewhere.as_ref()).is_err());
        // The proxy's credentials are not where it goes.
        let mut typed = proxy.clone();
        typed.as_mut().unwrap().password = Some("pw".into());
        typed.as_mut().unwrap().password_ref = None;
        assert!(check_matches_session(&s, None, "web.example", 22, "u", Some(&hops), typed.as_ref()).is_ok());
    }

    #[test]
    fn a_connect_may_use_another_of_the_sessions_addresses() {
        use crate::ipc::ssh_commands::check_matches_session;
        let mut s = with_refs();
        s.alt_addresses = Some(vec![
            crate::state::SessionAddress { label: "Office".into(), host: "10.0.0.5".into(), port: None, direct: true, via_session_id: None, via_wireguard: None },
            crate::state::SessionAddress { label: "Internet".into(), host: "server.example.com".into(), port: Some(2222), direct: false, via_session_id: None, via_wireguard: None },
        ]);
        let hops: Vec<crate::ipc::ssh_commands::JumpHostConnectParams> = vec![serde_json::from_value(serde_json::json!({
            "host": "bastion.example", "port": 22, "username": "jump", "authMethod": "key",
        }))
        .unwrap()];
        let proxy = s.proxy.clone();
        // A direct address goes straight, and only straight.
        assert!(check_matches_session(&s, None, "10.0.0.5", 22, "u", None, None).is_ok());
        assert!(check_matches_session(&s, None, "10.0.0.5", 22, "u", Some(&hops), proxy.as_ref()).is_err());
        assert!(check_matches_session(&s, None, "10.0.0.5", 2222, "u", None, None).is_err());
        // Another address goes the session's way, on its own port.
        assert!(check_matches_session(&s, None, "server.example.com", 2222, "u", Some(&hops), proxy.as_ref()).is_ok());
        assert!(check_matches_session(&s, None, "server.example.com", 2222, "u", None, None).is_err());
        assert!(check_matches_session(&s, None, "server.example.com", 22, "u", Some(&hops), proxy.as_ref()).is_err());
        // The main address still never goes straight around the session's route.
        assert!(check_matches_session(&s, None, "web.example", 22, "u", None, None).is_err());
    }

    #[test]
    fn an_address_through_another_session_goes_its_way_only() {
        use crate::ipc::ssh_commands::check_matches_session;
        let mut s = with_refs();
        s.alt_addresses = Some(vec![crate::state::SessionAddress {
            label: "Branch".into(),
            host: "10.0.1.5".into(),
            port: None,
            direct: false,
            via_session_id: Some("gw".into()),
            via_wireguard: None,
        }]);
        let mut gw = session();
        gw.id = "gw".into();
        gw.host = "gw2.example.com".into();
        gw.port = 22;
        gw.username = "jump".into();
        gw.jump_chain = None;
        let hop = |h: &str, u: &str| -> crate::ipc::ssh_commands::JumpHostConnectParams {
            serde_json::from_value(serde_json::json!({ "host": h, "port": 22, "username": u, "authMethod": "key" })).unwrap()
        };
        let through = vec![hop("gw2.example.com", "jump")];
        assert!(check_matches_session(&s, Some(&gw), "10.0.1.5", 22, "u", Some(&through), None).is_ok());
        // Not the session's own route, nor straight, nor with a proxy.
        let session_route = vec![hop("bastion.example", "jump")];
        assert!(check_matches_session(&s, Some(&gw), "10.0.1.5", 22, "u", Some(&session_route), None).is_err());
        assert!(check_matches_session(&s, Some(&gw), "10.0.1.5", 22, "u", None, None).is_err());
        assert!(check_matches_session(&s, Some(&gw), "10.0.1.5", 22, "u", Some(&through), s.proxy.as_ref()).is_err());
        // The jump session gone or another one: refused.
        assert!(check_matches_session(&s, None, "10.0.1.5", 22, "u", Some(&through), None).is_err());
        gw.id = "other".into();
        assert!(check_matches_session(&s, Some(&gw), "10.0.1.5", 22, "u", Some(&through), None).is_err());
    }

    #[test]
    fn wireguard_routes_are_the_sessions_own() {
        use crate::ipc::ssh_commands::{check_matches_session, wireguard_route};
        let mut s = with_refs();
        let before = approval_context(&s);
        s.wireguard_id = Some("office".into());
        assert_ne!(approval_context(&s), before, "the tunnel is part of what was approved");
        s.alt_addresses = Some(vec![crate::state::SessionAddress {
            label: "Lab".into(),
            host: "10.9.0.5".into(),
            port: None,
            direct: false,
            via_session_id: None,
            via_wireguard: Some("lab".into()),
        }]);
        assert_eq!(wireguard_route(&s, &s.host.clone(), s.port), Some("office"));
        assert_eq!(wireguard_route(&s, "10.9.0.5", s.port), Some("lab"));
        assert_eq!(wireguard_route(&s, "10.9.0.6", s.port), None);
        // Inside its tunnel the address goes straight: no jump hosts, no proxy.
        assert!(check_matches_session(&s, None, "10.9.0.5", s.port, "u", None, None).is_ok());
        assert!(check_matches_session(&s, None, "10.9.0.5", s.port, "u", None, s.proxy.as_ref()).is_err());
        // Tidying drops an empty choice and lets the tunnel win over a session.
        s.alt_addresses.as_mut().unwrap()[0].via_session_id = Some("gw".into());
        s.wireguard_id = Some("  ".into());
        s.tidy_addresses();
        let a = &s.alt_addresses.as_ref().unwrap()[0];
        assert!(a.via_session_id.is_none() && a.via_wireguard.as_deref() == Some("lab"));
        assert!(s.wireguard_id.is_none());

        // A server management session's text console (SSH, another port)
        // goes through its tunnel too; another session's other port does not.
        s.wireguard_id = Some("office".into());
        let host = s.host.clone();
        assert_eq!(wireguard_route(&s, &host, 2222), None);
        s.kind = crate::state::SessionKind::Bmc;
        assert_eq!(wireguard_route(&s, &host, 22), Some("office"));
    }

    #[test]
    fn an_rdp_file_manager_goes_to_the_same_machine_only() {
        use crate::ipc::ssh_commands::{check_matches_session, rdp_files_username};
        let mut s = with_refs();
        s.kind = crate::state::SessionKind::Rdp;
        s.jump_chain = None;
        s.proxy = None;
        s.port = 3389;
        assert_eq!(rdp_files_username(&s), "u");
        assert!(check_matches_session(&s, None, "web.example", 22, "u", None, None).is_ok());
        // Not the RDP port, another host or user, nor through anything.
        assert!(check_matches_session(&s, None, "web.example", 3389, "u", None, None).is_err());
        assert!(check_matches_session(&s, None, "attacker.example", 22, "u", None, None).is_err());
        assert!(check_matches_session(&s, None, "web.example", 22, "root", None, None).is_err());
        let hop: crate::ipc::ssh_commands::JumpHostConnectParams =
            serde_json::from_value(serde_json::json!({ "host": "b", "port": 22, "username": "j", "authMethod": "key" })).unwrap();
        assert!(check_matches_session(&s, None, "web.example", 22, "u", Some(std::slice::from_ref(&hop)), None).is_err());
        // Its own files port, and the domain in front of the user.
        s.rdp = Some(crate::rdp::options::RdpOptions { files_port: Some(2222), ..Default::default() });
        s.domain = Some("CORP".into());
        assert_eq!(rdp_files_username(&s), "CORP\\u");
        assert!(check_matches_session(&s, None, "web.example", 2222, "CORP\\u", None, None).is_ok());
        assert!(check_matches_session(&s, None, "web.example", 22, "CORP\\u", None, None).is_err());
        assert!(check_matches_session(&s, None, "web.example", 2222, "u", None, None).is_err());
        s.username = "u@corp.example".into();
        assert_eq!(rdp_files_username(&s), "u@corp.example");
    }

    #[test]
    fn a_bmc_text_console_goes_to_the_same_controller_only() {
        use crate::ipc::ssh_commands::check_matches_session;
        let mut s = with_refs();
        s.kind = crate::state::SessionKind::Bmc;
        s.jump_chain = None;
        s.proxy = None;
        s.port = 443;
        assert!(check_matches_session(&s, None, "web.example", 22, "u", None, None).is_ok());
        assert!(check_matches_session(&s, None, "web.example", 2200, "u", None, None).is_ok());
        assert!(check_matches_session(&s, None, "attacker.example", 22, "u", None, None).is_err());
        assert!(check_matches_session(&s, None, "web.example", 22, "root", None, None).is_err());
        let hop: crate::ipc::ssh_commands::JumpHostConnectParams =
            serde_json::from_value(serde_json::json!({ "host": "b", "port": 22, "username": "j", "authMethod": "key" })).unwrap();
        assert!(check_matches_session(&s, None, "web.example", 22, "u", Some(std::slice::from_ref(&hop)), None).is_err());
    }

    #[test]
    fn other_addresses_are_part_of_what_is_approved() {
        let mut s = with_refs();
        let before = approval_context(&s);
        s.alt_addresses = Some(vec![crate::state::SessionAddress { label: String::new(), host: "10.0.0.5".into(), port: None, direct: true, via_session_id: None, via_wireguard: None }]);
        assert_ne!(approval_context(&s), before);
        s.alt_addresses = None;
        assert_eq!(approval_context(&s), before);
    }

    // --- A VNC session through an SSH session ------------------------------

    /// The SSH session a VNC session goes through.
    fn via_ssh() -> SessionConfig {
        let mut s = session();
        s.id = "s-ssh".into();
        s.name = "bastion".into();
        s.host = "bastion.example".into();
        s.username = "jump".into();
        s.ssh_options = None;
        s
    }

    /// A VNC session through [`via_ssh`], with its password from the manager.
    fn vnc() -> SessionConfig {
        serde_json::from_value(serde_json::json!({
            "id": "v1", "name": "desk", "host": "10.0.0.5", "port": 5900, "username": "",
            "auth_method": { "type": "Password", "password_ref": { "provider": "passbolt", "entry_id": "e-vnc" } },
            "folder_id": null, "tags": [], "kind": "vnc", "via_session_id": "s-ssh",
        }))
        .unwrap()
    }

    fn read_via(s: &SessionConfig, via: Option<&SessionConfig>) -> SessionConfig {
        let mut s: SessionConfig = serde_json::from_str(&serde_json::to_string(s).unwrap()).unwrap();
        open_for(Some(&ALICE), Some("alice"), &mut s, via);
        s
    }

    fn vnc_trusted(s: &SessionConfig, via: Option<&SessionConfig>) -> bool {
        ref_at(&read_via(s, via), "password").unwrap().trusted
    }

    /// Picked by alice in the editor, with `via` as it is then.
    fn vnc_sealed(via: &SessionConfig) -> SessionConfig {
        let mut v = vnc();
        pick_all(&mut v);
        seal_for(Some(&ALICE), Some("alice"), &mut v, None, Some(via));
        v
    }

    #[test]
    fn a_vnc_reference_does_not_follow_a_re_pointed_via_session() {
        let s = via_ssh();
        let v = vnc_sealed(&s);
        assert!(vnc_trusted(&v, Some(&s)));
        type Change = Box<dyn Fn(&mut SessionConfig)>;
        let changes: Vec<(&str, Change)> = vec![
            ("host", Box::new(|s| s.host = "attacker.example".into())),
            ("port", Box::new(|s| s.port = 2222)),
            ("user", Box::new(|s| s.username = "other".into())),
            ("jumps", Box::new(|s| {
                s.jump_chain = Some(vec![JumpHostConfig { host: "attacker.example".into(), port: 22, username: "j".into(), auth_method: AuthMethod::Agent, credential_id: None, credential: None }])
            })),
            ("proxy", Box::new(|s| {
                s.proxy = Some(serde_json::from_value(serde_json::json!({ "proxy_type": "socks5", "host": "attacker.example", "port": 1080 })).unwrap())
            })),
            ("ssh_config", Box::new(|s| {
                s.ssh_options = Some(serde_json::from_value(serde_json::json!({ "lines": ["ProxyJump attacker.example"] })).unwrap())
            })),
        ];
        for (what, change) in changes {
            let mut moved = s.clone();
            change(&mut moved);
            assert!(!vnc_trusted(&v, Some(&moved)), "via {what} kept the approval");
        }
        // Renaming the via session, or saving its password, changes nothing.
        let mut renamed = s.clone();
        renamed.name = "bastion (prod)".into();
        renamed.folder_id = Some("f".into());
        renamed.auth_method = AuthMethod::Password { password: Some("pw".into()), password_ref: None };
        assert!(vnc_trusted(&v, Some(&renamed)));
        // Another session than the one named is not it.
        let mut other = s.clone();
        other.id = "s-other".into();
        assert!(!vnc_trusted(&v, Some(&other)));
    }

    #[test]
    fn a_vnc_reference_through_a_deleted_session_counts_for_nothing() {
        let s = via_ssh();
        let v = vnc_sealed(&s);
        assert!(!vnc_trusted(&v, None));
        // Nothing to ask either: there is no way in to show.
        let read = read_via(&v, None);
        assert!(untrusted_refs(&read, None, &["password".into()]).is_err());
        assert!(require_trusted_refs(&read, None, &["password".into()]).is_err());
        // Saving while it is gone signs nothing and keeps what was there: it
        // counts again only once the session is back as it was.
        let mut saved = read.clone();
        pick_all(&mut saved);
        seal_for(Some(&ALICE), Some("alice"), &mut saved, Some(&read), None);
        assert_eq!(ref_at(&saved, "password").unwrap().approvals, ref_at(&v, "password").unwrap().approvals);
        assert!(vnc_trusted(&saved, Some(&s)));
        // A session that is not VNC does not go through another, whatever
        // the field says.
        let mut ssh = v.clone();
        ssh.kind = crate::state::SessionKind::Ssh;
        assert!(ref_context(&ssh, None).is_some());
    }

    #[test]
    fn the_via_session_does_not_change_the_ssh_config_approvals() {
        let s = via_ssh();
        let mut moved = s.clone();
        moved.host = "attacker.example".into();
        let mut v = vnc();
        v.ssh_options = Some(serde_json::from_value(serde_json::json!({ "lines": ["StrictHostKeyChecking no"] })).unwrap());
        v.ssh_options.as_mut().unwrap().my_accepted_weakenings = Some(vec!["StrictHostKeyChecking no".into()]);
        pick_all(&mut v);
        seal_for(Some(&ALICE), Some("alice"), &mut v, None, Some(&s));
        // The ssh_config approvals are signed over the session's own
        // context, whatever its via session is; only the references' gain
        // the via part.
        for via in [Some(&s), Some(&moved), None] {
            let read = read_via(&v, via);
            assert_eq!(read.ssh_options.unwrap().my_accepted_weakenings.unwrap(), ["StrictHostKeyChecking no"]);
        }
        assert!(vnc_trusted(&v, Some(&s)) && !vnc_trusted(&v, Some(&moved)));
        assert_ne!(ref_context(&v, Some(&s)), ref_context(&v, Some(&moved)));
        assert!(ref_context(&v, Some(&s)).unwrap().starts_with(&approval_context(&v)));
    }

    #[test]
    fn approving_a_vnc_reference_checks_the_via_session_too() {
        let s = via_ssh();
        let mut v = read_via(&vnc(), Some(&s));
        let asked = untrusted_refs(&v, Some(&s), &["password".into()]).unwrap().unwrap();
        // The dialog shows the way in.
        let via = asked.via.clone().unwrap();
        assert_eq!((via.session_name.as_str(), via.username.as_str(), via.host.as_str(), via.port), ("bastion", "jump", "bastion.example", 22));
        assert_eq!(asked.context, ref_context(&v, Some(&s)).unwrap());

        // The via session is re-pointed while the dialog is open: refused.
        let mut moved = s.clone();
        moved.host = "attacker.example".into();
        let mut changed = v.clone();
        assert!(approve_refs(&mut changed, Some(&moved), &asked.context, &asked.refs).is_err());
        let mut gone = v.clone();
        assert!(approve_refs(&mut gone, None, &asked.context, &asked.refs).is_err());

        // As shown: approved, for that way in only.
        let before = v.clone();
        approve_refs(&mut v, Some(&s), &asked.context, &asked.refs).unwrap();
        seal_for(Some(&ALICE), Some("alice"), &mut v, Some(&before), Some(&s));
        assert!(vnc_trusted(&v, Some(&s)));
        assert!(!vnc_trusted(&v, Some(&moved)));
    }

    // --- Entries a saved credential brings ---------------------------------

    trait PwTrusted {
        fn pw_trusted(&self) -> bool;
    }
    impl PwTrusted for SessionConfig {
        fn pw_trusted(&self) -> bool {
            ref_at(self, "password").is_some_and(|r| r.trusted)
        }
    }

    fn cred_with(id: &str) -> crate::state::SavedCredential {
        crate::state::SavedCredential {
            id: "c1".into(),
            name: "Admin".into(),
            username: "admin".into(),
            domain: None,
            auth_method: AuthMethod::Password { password: None, password_ref: Some(entry(id)) },
            vault_id: None,
            password_history: None,
            folder_id: None,
        }
    }

    /// A stored record as `who` reads it, with the credential filled in.
    fn read_filled(stored: &SessionConfig, c: &crate::state::SavedCredential, key: &[u8; 32], who: &str) -> (SessionConfig, creds::SessionLogins) {
        let mut s: SessionConfig = serde_json::from_str(&serde_json::to_string(stored).unwrap()).unwrap();
        let own = creds::fill(&mut s, None, &[], std::slice::from_ref(c));
        open_for(Some(key), Some(who), &mut s, None);
        (s, own)
    }

    /// `who` says yes in the connect dialog; returns the record as stored.
    fn approve_as(stored: &SessionConfig, c: &crate::state::SavedCredential, key: &[u8; 32], who: &str) -> SessionConfig {
        let (mut s, own) = read_filled(stored, c, key, who);
        let context = ref_context(&s, None).unwrap();
        let want = RefSlot { slot: "password".into(), provider: "passbolt".into(), entry_id: ref_at(&s, "password").unwrap().entry_id.clone() };
        approve_refs(&mut s, None, &context, &[want]).unwrap();
        let before = s.clone();
        seal_for(Some(key), Some(who), &mut s, Some(&before), None);
        to_store(&s, own)
    }

    #[test]
    fn a_credentials_entry_is_approved_per_session_and_person() {
        let c = cred_with("e-cred");
        let mut stored = session();
        stored.credential_id = Some("c1".into());
        let (s, _) = read_filled(&stored, &c, &ALICE, "alice");
        assert_eq!(trusted(&s), [("password".to_string(), false)], "asked the first time");

        let stored = approve_as(&stored, &c, &ALICE, "alice");
        assert!(matches!(stored.auth_method, AuthMethod::Agent), "the credential's login is not stored in the session");
        assert!(stored.credential_ref_approvals.contains_key("password"));
        assert!(read_filled(&stored, &c, &ALICE, "alice").0.pw_trusted());
        assert!(!read_filled(&stored, &c, &BOB, "bob").0.pw_trusted(), "alice's yes is not bob's");

        // Bob approves too; alice's approval stays.
        let stored = approve_as(&stored, &c, &BOB, "bob");
        assert!(read_filled(&stored, &c, &ALICE, "alice").0.pw_trusted());
        assert!(read_filled(&stored, &c, &BOB, "bob").0.pw_trusted());

        // A save that changes nothing about where it connects keeps it.
        let (read, own) = read_filled(&stored, &c, &ALICE, "alice");
        let mut renamed = read.clone();
        renamed.name = "web (prod)".into();
        seal_for(Some(&ALICE), Some("alice"), &mut renamed, Some(&read), None);
        let renamed = to_store(&renamed, own);
        assert!(read_filled(&renamed, &c, &ALICE, "alice").0.pw_trusted());

        // The credential pointed at another entry, or given another
        // username, or the session at another server: asked again.
        assert!(!read_filled(&stored, &cred_with("e-other"), &ALICE, "alice").0.pw_trusted());
        let mut root = c.clone();
        root.username = "root".into();
        assert!(!read_filled(&stored, &root, &ALICE, "alice").0.pw_trusted());
        let mut moved = stored.clone();
        moved.host = "attacker.example".into();
        assert!(!read_filled(&moved, &c, &ALICE, "alice").0.pw_trusted());

        // Another session using the same credential, given this one's
        // approvals by someone who can write to the vault: not approved.
        let mut other = session();
        other.id = "s2".into();
        other.credential_id = Some("c1".into());
        other.credential_ref_approvals = stored.credential_ref_approvals.clone();
        assert!(!read_filled(&other, &c, &ALICE, "alice").0.pw_trusted());
    }

    #[test]
    fn a_credentials_entry_for_a_jump_host_is_kept_by_its_slot() {
        let c = cred_with("e-cred");
        let mut stored = session();
        stored.jump_chain = Some(vec![JumpHostConfig {
            host: "bastion.example".into(),
            port: 22,
            username: "hop-own".into(),
            auth_method: AuthMethod::Agent,
            credential_id: Some("c1".into()),
            credential: None,
        }]);
        let (mut s, own) = read_filled(&stored, &c, &ALICE, "alice");
        assert_eq!(trusted(&s), [("jump:0:password".to_string(), false)]);
        pick_all(&mut s);
        seal_for(Some(&ALICE), Some("alice"), &mut s, None, None);
        let stored = to_store(&s, own);
        let hop = &stored.jump_chain.as_ref().unwrap()[0];
        assert_eq!(hop.username, "hop-own");
        assert!(matches!(hop.auth_method, AuthMethod::Agent));
        assert_eq!(stored.credential_ref_approvals.keys().collect::<Vec<_>>(), ["jump:0:password"]);
        assert_eq!(trusted(&read_filled(&stored, &c, &ALICE, "alice").0), [("jump:0:password".to_string(), true)]);
    }

    #[test]
    fn the_dialog_shows_where_ssh_config_sends_the_login() {
        let mut s = with_refs();
        s.jump_chain = None;
        s.ssh_options = Some(serde_json::from_value(serde_json::json!({
            "lines": ["HostName real.example", "ProxyJump hop.example", "Compression yes", "proxycommand nc %h %p"],
        }))
        .unwrap());
        open_for(Some(&ALICE), Some("alice"), &mut s, None);
        let asked = untrusted_refs(&s, None, &slots_in_use(&s)).unwrap().unwrap();
        assert_eq!(asked.route_lines, ["HostName real.example", "ProxyJump hop.example", "proxycommand nc %h %p"]);
        assert!(asked.via.is_none());
        // And the via session's, for a VNC session.
        let mut via = via_ssh();
        via.ssh_options = Some(serde_json::from_value(serde_json::json!({ "lines": ["HostName inner.example"] })).unwrap());
        let v = read_via(&vnc(), Some(&via));
        let asked = untrusted_refs(&v, Some(&via), &["password".into()]).unwrap().unwrap();
        assert_eq!(asked.via.unwrap().route_lines, ["HostName inner.example"]);
    }
}

/// The WSL distributions a person can open, on Windows; elsewhere none.
#[tauri::command]
pub async fn wsl_list_distros() -> Result<Vec<String>, String> {
    if !cfg!(windows) {
        return Ok(Vec::new());
    }
    // No WSL installed, or not answering: nothing to offer, not an error.
    Ok(crate::wsl::run(&["-l", "-q"], crate::wsl::LIST_WITHIN)
        .await
        .map(|listing| crate::wsl::distros(&listing))
        .unwrap_or_default())
}

/// Folders of a shared vault, end to end between two devices over a real
/// libsql server: an old folder kept on the device moves into its vault,
/// new ones are made there, and another member sees, nests and deletes
/// them with no copy coming back.
///
/// Needs a libsql server, so it is skipped unless one is named, e.g.
/// `docker run -d -p 58080:8080 ghcr.io/tursodatabase/libsql-server` and
/// `REACH_TEST_SQLD=http://127.0.0.1:58080`.
#[cfg(test)]
#[path = "session_commands_folder_sync_tests.rs"]
mod folder_sync_tests;

#[cfg(test)]
mod documentation_tests {
    use super::*;
    use crate::state::CustomField;

    fn session(extra: serde_json::Value) -> SessionConfig {
        let mut v = serde_json::json!({
            "id": "s1", "name": "web", "host": "web.example", "port": 22, "username": "u",
            "auth_method": { "type": "Agent" }, "folder_id": null, "tags": [],
        });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_session_saved_before_notes_reads_without_them_and_stores_none() {
        let s = session(serde_json::json!({}));
        assert!(s.notes.is_none() && s.custom_fields.is_none());
        let stored = serde_json::to_value(&s).unwrap();
        assert!(stored.get("notes").is_none() && stored.get("custom_fields").is_none());
    }

    #[test]
    fn notes_and_fields_are_kept_in_the_session_record() {
        let s = session(serde_json::json!({
            "notes": "Rack 4, ask Jana before rebooting",
            "custom_fields": [{ "name": "Asset", "value": "A-113" }, { "name": "PIN", "value": "1234", "hidden": true }],
        }));
        let stored = serde_json::to_value(&s).unwrap();
        assert_eq!(stored["notes"], "Rack 4, ask Jana before rebooting");
        // A visible field stores no `hidden` at all.
        assert_eq!(stored["custom_fields"][0], serde_json::json!({ "name": "Asset", "value": "A-113" }));
        assert_eq!(stored["custom_fields"][1]["hidden"], true);
        let back: SessionConfig = serde_json::from_value(stored).unwrap();
        assert_eq!(back.custom_fields, s.custom_fields);
    }

    #[test]
    fn empty_notes_and_blank_fields_are_dropped() {
        let mut s = session(serde_json::json!({
            "notes": "  \n ",
            "custom_fields": [{ "name": " ", "value": "" }, { "name": " Owner ", "value": "ops" }, { "name": "", "value": "orphan" }],
        }));
        s.tidy_documentation();
        assert!(s.notes.is_none());
        assert_eq!(
            s.custom_fields.unwrap(),
            vec![
                CustomField { name: "Owner".into(), value: "ops".into(), hidden: false },
                CustomField { name: "".into(), value: "orphan".into(), hidden: false },
            ]
        );
        let mut empty = session(serde_json::json!({ "custom_fields": [{ "name": "", "value": " " }] }));
        empty.tidy_documentation();
        assert!(empty.custom_fields.is_none());
    }

    #[test]
    fn a_terminal_theme_is_kept_and_a_blank_one_dropped() {
        let s = session(serde_json::json!({ "terminal_theme": "scheme:Solarized Dark" }));
        let stored = serde_json::to_value(&s).unwrap();
        assert_eq!(stored["terminal_theme"], "scheme:Solarized Dark");
        assert!(serde_json::to_value(session(serde_json::json!({}))).unwrap().get("terminal_theme").is_none());
        let mut blank = session(serde_json::json!({ "terminal_theme": " " }));
        blank.tidy_documentation();
        assert!(blank.terminal_theme.is_none());
    }

    #[test]
    fn a_cursor_choice_is_kept_and_an_unknown_shape_dropped() {
        let mut s = session(serde_json::json!({ "cursor_style": "block", "cursor_blink": false }));
        s.tidy_documentation();
        let stored = serde_json::to_value(&s).unwrap();
        assert_eq!(stored["cursor_style"], "block");
        assert_eq!(stored["cursor_blink"], false);
        let plain = serde_json::to_value(session(serde_json::json!({}))).unwrap();
        assert!(plain.get("cursor_style").is_none() && plain.get("cursor_blink").is_none());
        let mut odd = session(serde_json::json!({ "cursor_style": "beam" }));
        odd.tidy_documentation();
        assert!(odd.cursor_style.is_none());
    }

    #[test]
    fn a_hand_picked_os_is_marked_and_automatic_left_out() {
        let picked = serde_json::to_value(session(serde_json::json!({ "detected_os": "debian", "os_manual": true }))).unwrap();
        assert_eq!(picked["os_manual"], true);
        let found = serde_json::to_value(session(serde_json::json!({ "detected_os": "debian" }))).unwrap();
        assert!(found.get("os_manual").is_none());
    }

    #[test]
    fn a_monitoring_override_is_kept_and_unset_left_out() {
        let off = serde_json::to_value(session(serde_json::json!({ "monitoring": false }))).unwrap();
        assert_eq!(off["monitoring"], false);
        let on = serde_json::to_value(session(serde_json::json!({ "monitoring": true }))).unwrap();
        assert_eq!(on["monitoring"], true);
        assert!(serde_json::to_value(session(serde_json::json!({}))).unwrap().get("monitoring").is_none());
    }
}

#[cfg(test)]
mod folder_vault_tests {
    use super::*;
    use crate::vault::types::VaultType;

    fn folder(id: &str, vault: Option<&str>) -> Folder {
        Folder { id: id.into(), name: id.into(), parent_id: None, vault_id: vault.map(Into::into), credential_id: None, tmux: None, icon: None, color: None }
    }

    #[test]
    fn folder_icon_values_are_checked() {
        assert_eq!(folder_icon_value(None, "icon").unwrap(), None);
        assert_eq!(folder_icon_value(Some("  ".into()), "icon").unwrap(), None);
        assert_eq!(folder_icon_value(Some(" fa:server ".into()), "icon").unwrap().as_deref(), Some("fa:server"));
        assert_eq!(folder_icon_value(Some("os:mikrotik".into()), "icon").unwrap().as_deref(), Some("os:mikrotik"));
        assert_eq!(folder_icon_value(Some("red".into()), "colour").unwrap().as_deref(), Some("red"));
        assert!(folder_icon_value(Some("fa:a:b".into()), "icon").is_err());
        assert!(folder_icon_value(Some("<svg onload=x>".into()), "icon").is_err());
        assert!(folder_icon_value(Some("x".repeat(65)), "icon").is_err());
    }

    #[test]
    fn a_folder_without_an_icon_reads_and_writes_as_before() {
        let old: Folder = serde_json::from_str(r#"{"id":"f","name":"F","parent_id":null}"#).unwrap();
        assert_eq!(old.icon, None);
        assert_eq!(old.color, None);
        let json = serde_json::to_string(&old).unwrap();
        assert!(!json.contains("icon") && !json.contains("color"), "{json}");
        let mut with = old.clone();
        with.icon = Some("fa:server".into());
        with.color = Some("blue".into());
        let back: Folder = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
        assert_eq!(back.icon.as_deref(), Some("fa:server"));
        assert_eq!(back.color.as_deref(), Some("blue"));
    }

    #[test]
    fn a_session_goes_only_into_its_own_vaults_folders() {
        let folders = [folder("private", None), folder("team", Some("t"))];
        assert!(check_folder_vault(&folders, None, Some("t")).is_ok(), "out of any folder");
        assert!(check_folder_vault(&folders, Some("private"), None).is_ok());
        assert!(check_folder_vault(&folders, Some("team"), Some("t")).is_ok());
        assert!(check_folder_vault(&folders, Some("team"), None).is_err(), "a Private session into a vault's folder");
        assert!(check_folder_vault(&folders, Some("private"), Some("t")).is_err(), "a vault's session into a Private folder");
        assert!(check_folder_vault(&folders, Some("gone"), Some("t")).is_ok(), "a folder not open here");
    }

    fn put_session(id: &str) -> SecretBox<Vec<u8>> {
        let json = serde_json::json!({
            "id": id, "name": id, "host": "h", "port": 22, "username": "u",
            "auth_method": { "type": "Agent" }, "folder_id": null, "tags": []
        });
        SecretBox::new(Box::new(serde_json::to_vec(&json).unwrap()))
    }

    #[tokio::test]
    async fn moving_sessions_across_vaults_is_refused_and_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("reach-folder-vault-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = crate::vault::VaultManager::new(dir.clone());
        m.init_identity("folder-vault").await.unwrap();
        let team = m.create_vault("Team", VaultType::Private, None, None).await.unwrap().id;
        let private = ensure_sessions_vault(&mut m).await.unwrap();
        m.create_secret_with_id(&private, "mine", "mine", SecretCategory::Session, put_session("mine")).await.unwrap();
        m.create_secret_with_id(&team, "ours", "ours", SecretCategory::Session, put_session("ours")).await.unwrap();
        let home = create_folder(&mut m, "Home".into(), None, None).await.unwrap();
        let servers = create_folder(&mut m, "Servers".into(), None, Some(team.clone())).await.unwrap();

        let both = ["mine".to_string(), "ours".to_string()];
        assert!(move_to_folder(&m, &both, Some(servers.id.clone())).await.is_err());
        assert!(move_to_folder(&m, &both, Some(home.id.clone())).await.is_err());
        let folder_of = |s: SessionConfig| s.folder_id;
        assert_eq!(folder_of(read_session(&m, &private, "mine").await.unwrap()), None, "nothing moved");

        assert_eq!(move_to_folder(&m, &["ours".to_string()], Some(servers.id.clone())).await.unwrap(), 1);
        assert_eq!(move_to_folder(&m, &["mine".to_string()], Some(home.id.clone())).await.unwrap(), 1);
        assert_eq!(folder_of(read_session(&m, &team, "ours").await.unwrap()), Some(servers.id.clone()));
        assert_eq!(move_to_folder(&m, &both, None).await.unwrap(), 2, "out of folders works across vaults");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_order_is_stored_on_each_session_and_a_move_clears_it() {
        let dir = std::env::temp_dir().join(format!("reach-session-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = crate::vault::VaultManager::new(dir.clone());
        m.init_identity("session-order").await.unwrap();
        let private = ensure_sessions_vault(&mut m).await.unwrap();
        for id in ["a", "b", "c"] {
            m.create_secret_with_id(&private, id, id, SecretCategory::Session, put_session(id)).await.unwrap();
        }
        let order_of = |s: SessionConfig| s.order;
        assert_eq!(order_of(read_session(&m, &private, "a").await.unwrap()), None, "none before it is set");

        let ids = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(set_order(&m, &ids(&["c", "a", "gone", "b"])).await.unwrap(), 3, "a missing one is skipped");
        assert_eq!(order_of(read_session(&m, &private, "c").await.unwrap()), Some(0));
        assert_eq!(order_of(read_session(&m, &private, "b").await.unwrap()), Some(3));
        assert_eq!(set_order(&m, &ids(&["c", "a", "gone", "b"])).await.unwrap(), 0, "the same order writes nothing");
        assert_eq!(set_order(&m, &ids(&["a", "c"])).await.unwrap(), 2);

        let home = create_folder(&mut m, "Home".into(), None, None).await.unwrap();
        move_to_folder(&m, &ids(&["a"]), Some(home.id.clone())).await.unwrap();
        assert_eq!(order_of(read_session(&m, &private, "a").await.unwrap()), None, "it starts last in the new folder");
        assert_eq!(order_of(read_session(&m, &private, "c").await.unwrap()), Some(1), "the others keep theirs");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
