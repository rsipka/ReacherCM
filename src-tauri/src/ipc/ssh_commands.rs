use crate::state::{AppState, AuthMethod};
use crate::ssh::client::{AuthParams, ConnectionInfo, HeadlessConnection, JumpHostParams, KeySource, SshManager, exec_on_connection_with_exit_code_timeout};
use crate::plugin::hooks;

/// Parameters for a jump host received from the frontend.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JumpHostConnectParams {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_method: String,
    pub password: Option<String>,
    pub key_path: Option<String>,
    pub key_passphrase: Option<String>,
    /// An imported key, instead of a path. See `ssh_key_import`.
    pub key_id: Option<String>,
    /// Use the saved session's password manager entry for this hop's
    /// password; wins over `password`. Only a signal: the entry itself is
    /// read from the stored session (see `ssh_connect`). Sent as
    /// `passwordRef` (`password_ref` is accepted too).
    #[serde(default, alias = "password_ref")]
    pub password_ref: Option<crate::pwmanager::SecretRef>,
    /// The same for the key passphrase; wins over `key_passphrase`. Sent as
    /// `keyPassphraseRef` (or `key_passphrase_ref`).
    #[serde(default, alias = "key_passphrase_ref")]
    pub key_passphrase_ref: Option<crate::pwmanager::SecretRef>,
}

/// Which secret of a login a password manager reference stands in for, if
/// the login method uses one: the session slot's field name (see
/// `session_commands::ref_slots`).
fn ref_field(auth_method: &str, has_password_ref: bool, has_passphrase_ref: bool) -> Option<&'static str> {
    match auth_method {
        "password" if has_password_ref => Some("password"),
        "key" if has_passphrase_ref => Some("passphrase"),
        _ => None,
    }
}

/// The slots of the saved session whose password manager entries this
/// connect asks for. The references the webview sends say only that:
/// which entry is used, and where it goes, is read from the stored session.
pub(crate) fn wanted_slots(
    auth_method: &str,
    has_password_ref: bool,
    has_passphrase_ref: bool,
    jump_chain: Option<&[JumpHostConnectParams]>,
    proxy: Option<&crate::state::ProxyConfig>,
) -> Vec<String> {
    let mut out: Vec<String> = ref_field(auth_method, has_password_ref, has_passphrase_ref).map(String::from).into_iter().collect();
    let jumps = jump_chain.unwrap_or_default();
    for (i, j) in jumps.iter().enumerate() {
        if let Some(field) = ref_field(&j.auth_method, j.password_ref.is_some(), j.key_passphrase_ref.is_some()) {
            out.push(format!("jump:{i}:{field}"));
        }
    }
    // The proxy is used only without jump hosts.
    if jumps.is_empty() && proxy.is_some_and(|p| p.password_ref.is_some()) {
        out.push("proxy".to_string());
    }
    out
}

/// Where a proxy goes, without its credentials.
fn proxy_endpoint(p: &crate::state::ProxyConfig) -> (&str, &str, u16, Option<&str>) {
    (p.proxy_type.as_str(), p.host.as_str(), p.port, p.username.as_deref().filter(|u| !u.is_empty()))
}

/// The connect must go where the saved session goes for its password
/// manager entries to be sent: the same server, user, jump hosts and proxy.
/// Their approvals were given for exactly that (`approval_context`).
pub(crate) fn check_matches_session(
    stored: &crate::state::SessionConfig,
    via: Option<&crate::state::SessionConfig>,
    host: &str,
    port: u16,
    username: &str,
    jump_chain: Option<&[JumpHostConnectParams]>,
    proxy: Option<&crate::state::ProxyConfig>,
) -> Result<(), String> {
    let jumps: Vec<(&str, u16, &str)> =
        jump_chain.unwrap_or_default().iter().map(|j| (j.host.as_str(), j.port, j.username.as_str())).collect();
    let stored_jumps: Vec<(&str, u16, &str)> =
        stored.jump_chain.iter().flatten().map(|j| (j.host.as_str(), j.port, j.username.as_str())).collect();
    // The main address goes the session's way; another of its addresses
    // goes that way too, straight when it is marked direct, or through the
    // saved SSH session it names (`via`, as stored now).
    let other = stored.host != host || stored.port != port;
    let (addr_ok, straight, through) = if other {
        match stored.alt_address(host, port) {
            Some(a) => (true, a.direct, a.via_session_id.as_deref()),
            None => (false, false, None),
        }
    } else {
        (true, false, None)
    };
    let route_ok = if let Some(via_id) = through {
        match via.filter(|v| v.id == via_id && v.kind == crate::state::SessionKind::Ssh) {
            Some(v) => {
                let mut want: Vec<(&str, u16, &str)> =
                    v.jump_chain.iter().flatten().map(|j| (j.host.as_str(), j.port, j.username.as_str())).collect();
                want.push((v.host.as_str(), v.port, v.username.as_str()));
                jumps == want && proxy.is_none()
            }
            None => false,
        }
    } else if straight {
        jumps.is_empty() && proxy.is_none()
    } else {
        jumps == stored_jumps && proxy.map(proxy_endpoint) == stored.proxy.as_ref().map(proxy_endpoint)
    };
    let same = stored.kind == crate::state::SessionKind::Ssh && addr_ok && stored.username == username && route_ok;
    if same {
        Ok(())
    } else {
        Err(format!(
            "This connection does not match the saved session '{}', so its password manager entries are not used. Reload the session list and connect again.",
            stored.name
        ))
    }
}

/// The reference in one slot of the stored session.
fn stored_ref<'a>(stored: &'a crate::state::SessionConfig, slot: &str) -> Result<&'a crate::pwmanager::SecretRef, String> {
    crate::ipc::session_commands::ref_at(stored, slot).ok_or_else(|| {
        format!("The saved session '{}' has no password manager entry for this login any more. Reload the session list and connect again.", stored.name)
    })
}

/// What a login may try, and nothing else. A key session offers its key, a
/// password session its password, an agent session the agent's keys. The
/// agent's keys are offered to a key session only when the user turned that
/// on for it (`try_agent_keys`).
///
/// Every refused offer counts against the server: OpenSSH's MaxAuthTries
/// (6) and PerSourcePenalties, fail2ban and firewalls block the address
/// after a few, and every key offered tells the server who you are. Offering
/// all the agent's keys behind a refused session key was guessing, and it
/// also hid a session whose own key no longer worked (it then failed on a
/// phone, which has no agent).
pub(crate) fn build_auth(
    auth_method: &str,
    password: Option<String>,
    key_source: Option<KeySource>,
    key_passphrase: Option<String>,
    try_agent_keys: bool,
) -> Result<AuthParams, String> {
    let mut auth = AuthParams { allow_agent: auth_method == "agent", ..Default::default() };
    match auth_method {
        "password" => {
            auth.password = Some(password.ok_or("Password required for password auth")?);
        }
        "key" => {
            auth.key = Some(crate::ssh::client::KeyAuth {
                source: key_source
                    .ok_or("Key auth needs either a key file or an imported key")?,
                passphrase: key_passphrase,
            });
            auth.allow_agent = try_agent_keys;
        }
        "agent" => {}
        _ => return Err(format!("Unknown auth method: {}", auth_method)),
    }
    Ok(auth)
}

/// Work out where a key comes from. An imported key wins over a path: it is
/// the one the user picked most recently, and it is the one that works on a
/// machine with no key files.
pub(crate) async fn resolve_key_source(
    state: &AppState,
    key_id: Option<String>,
    key_path: Option<String>,
) -> Result<(Option<KeySource>, Option<String>), String> {
    if let Some(id) = key_id.filter(|i| !i.is_empty()) {
        let resolved = crate::ipc::sshkey_commands::resolve(state, &id).await?;
        return Ok((
            Some(KeySource::Material(resolved.private_key)),
            resolved.passphrase,
        ));
    }
    Ok((
        key_path
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .map(KeySource::Path),
        None,
    ))
}

/// Only an SSH session can carry a route: an RDP or VNC session's host and
/// login are not an SSH server's.
pub(crate) fn require_ssh_session(s: &crate::state::SessionConfig) -> Result<(), String> {
    if s.kind == crate::state::SessionKind::Ssh {
        Ok(())
    } else {
        Err(format!("'{}' is not an SSH session, so nothing can go through it. Choose an SSH session to go through.", s.name))
    }
}

/// Who a login is for, to ask for a password that is not saved.
struct LoginFor<'a> {
    app: &'a tauri::AppHandle,
    host: &'a str,
    port: u16,
    username: &'a str,
}

async fn auth_for(state: &AppState, method: &AuthMethod, try_agent_keys: bool, who: LoginFor<'_>) -> Result<AuthParams, String> {
    use crate::ssh::prompt::{ask, Field, Kind};
    // A password or passphrase held by a password manager, fetched now.
    let method = crate::pwmanager::fill_auth_method(state, method).await?;
    match &method {
        AuthMethod::Password { password, .. } => {
            // Not saving a password is the safer choice. Ask for it, as a
            // terminal tab does, instead of failing with "Password required"
            // and a Try again that could never work.
            let password = match password.clone().filter(|p| !p.is_empty()) {
                Some(p) => p,
                None => ask(
                    Some(who.app),
                    who.host,
                    who.port,
                    Kind::Password,
                    &format!("Password for {}@{}", who.username, who.host),
                    "",
                    vec![Field { text: "Password".into(), echo: false }],
                )
                .await
                .and_then(|a| a.into_iter().next())
                .filter(|p| !p.is_empty())
                .ok_or("No password was given")?,
            };
            build_auth("password", Some(password), None, None, false)
        }
        AuthMethod::Key { path, passphrase, key_content, key_id, .. } => {
            let (source, stored) = match key_content.clone().filter(|k| !k.trim().is_empty()) {
                Some(k) => (Some(KeySource::Material(k)), None),
                None => resolve_key_source(state, key_id.clone(), path.clone()).await?,
            };
            build_auth("key", None, source, passphrase.clone().filter(|p| !p.is_empty()).or(stored), try_agent_keys)
        }
        AuthMethod::Agent => build_auth("agent", None, None, None, false),
    }
}

/// Authenticate with a saved session for services that only need SSH channels.
/// A route pin, when supplied by the caller, must match before any login.
pub(crate) async fn open_saved_headless(
    app: &tauri::AppHandle,
    state: &AppState,
    session_id: &str,
    pin: Option<&str>,
) -> Result<HeadlessConnection, String> {
    let mut s = crate::ipc::session_commands::stored_session(state, session_id).await?;
    require_ssh_session(&s)?;
    if pin.is_some_and(|p| p != crate::ipc::session_commands::approval_context(&s)) {
        return Err(format!("The SSH session '{}' changed while connecting. Connect again.", s.name));
    }
    // Only the approvals of the person connecting count (stored_session
    // has checked their signatures).
    s.ssh_options = s.ssh_options.map(|o| o.effective());
    // The same for its password manager entries: each must be one
    // this person approved for this session as it is. The error says
    // which, for the session list to ask. An SSH session goes
    // through no other session.
    crate::ipc::session_commands::require_trusted_refs(&s, None, &crate::ipc::session_commands::slots_in_use(&s))?;
    // Every entry of the session, its jump hosts and its proxy in one
    // operation: a password manager passphrase that must be asked
    // for is asked once (or not at all, inside a connect that already
    // has it).
    let trust = Some(crate::ssh::session_hostkeys::SessionTrust::new(s.id.clone(), s.host_keys.clone().unwrap_or_default()));
    let reason = crate::pwmanager::Reason::Connect(s.name.clone());
    let (auth, jumps, proxy) = crate::pwmanager::with_prompt(app, reason, async {
        let who = LoginFor { app, host: &s.host, port: s.port, username: &s.username };
        let auth = auth_for(state, &s.auth_method, s.try_agent_keys.unwrap_or(false), who).await?;
        let mut jumps = Vec::new();
        for j in s.jump_chain.clone().unwrap_or_default() {
            let mut opts: crate::ssh::client::HopOptions = crate::ssh::sshconf::session::plan_for(s.ssh_options.as_ref(), &j.host, j.port, &j.username, true).into();
            opts.trust = trust.clone();
            let who = LoginFor { app, host: &j.host, port: j.port, username: &j.username };
            let auth = auth_for(state, &j.auth_method, false, who).await?;
            jumps.push(JumpHostParams { auth, host: j.host, port: j.port, username: j.username, opts });
        }
        // The proxy is used only without jump hosts.
        let mut proxy = s.proxy.clone();
        if jumps.is_empty() {
            crate::pwmanager::fill_proxy(state, &mut proxy).await?;
        }
        Ok::<_, String>((auth, jumps, proxy))
    })
    .await?;
    let mut opts: crate::ssh::client::HopOptions = crate::ssh::sshconf::session::plan_for(s.ssh_options.as_ref(), &s.host, s.port, &s.username, false).into();
    opts.trust = trust;
    SshManager::open_headless(&s.host, s.port, &s.username, auth, jumps, proxy, app.clone(), opts)
        .await
        .map_err(|e| format!("SSH to {}: {e}", s.name))
}

#[tauri::command]
#[expect(clippy::too_many_arguments, reason = "a Tauri command takes each argument from the frontend's invoke by name")]
pub async fn ssh_connect(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
    host: String,
    port: u16,
    username: String,
    auth_method: String,
    password: Option<String>,
    key_path: Option<String>,
    key_passphrase: Option<String>,
    key_id: Option<String>,
    cols: u16,
    rows: u16,
    jump_chain: Option<Vec<JumpHostConnectParams>>,
    proxy: Option<crate::state::ProxyConfig>,
    shell: Option<String>,
    inject_colors: Option<bool>,
    show_login_message: Option<bool>,
    session_log: Option<crate::ssh::session_log::SessionLogConfig>,
    try_agent_keys: Option<bool>,
    ssh_options: Option<crate::ssh::sshconf::session::SshOptions>,
    password_ref: Option<crate::pwmanager::SecretRef>,
    key_passphrase_ref: Option<crate::pwmanager::SecretRef>,
    session_id: Option<String>,
    tmux: Option<crate::ssh::tmux::TmuxStart>,
    x11_desktop: Option<crate::ssh::x11_desktop::X11Desktop>,
    terminal: Option<crate::ssh::terminal::TerminalOptions>,
) -> Result<String, String> {
    // Boxed: Tauri builds a command's future on the main thread and passes it
    // by value before spawning it, and the main thread's stack is small
    // (1 MB on Windows). Only a pointer is left there this way.
    Box::pin(async move {
        // Only the approvals of the person connecting count: a session synced
        // from a vault others can write to cannot approve its own commands.
        let mut ssh_options = ssh_options.map(|o| o.effective());
        // X11 Desktop: the channel starts the desktop, never a shell or tmux.
        let tmux = if x11_desktop.is_some() { None } else { tmux };
        let shell = match &x11_desktop {
            Some(d) => Some(d.launcher()?),
            None => shell,
        };
        // Nothing is typed into the user's shell unless the caller asks for it
        // (Settings → Appearance): a caller that does not say gets no setup. The
        // login message is shown unless asked otherwise.
        // tmux: the channel runs the tmux start script instead of the login
        // shell (which becomes the shell of a new tmux window). Nothing is
        // typed into it: the init would land in a tmux window, again on
        // every reattach.
        // Ask mode: the shell stays as it is until the picker answers, once
        // logged in (see `ssh::tmux::Ask`); so does a name to settle
        // between the old and the current one (`TmuxStart::legacy`).
        let tmux_ask = tmux.as_ref().filter(|t| t.ask || t.legacy.is_some()).map(|t| crate::ssh::tmux::Ask {
            id: id.clone(),
            start: t.clone(),
            shell: shell.clone(),
            app: app.clone(),
            host: host.clone(),
            port,
        });
        let shell = match &tmux {
            Some(t) if !t.ask && t.legacy.is_none() => Some(crate::ssh::tmux::start_command(t, shell.as_deref())?),
            _ => shell,
        };
        let login = crate::ssh::client::LoginOptions {
            inject_colors: inject_colors.unwrap_or(false) && tmux.is_none() && x11_desktop.is_none(),
            show_login_message: show_login_message.unwrap_or(true),
            log: session_log,
            tmux_ask,
            terminal: terminal.and_then(|t| t.tidy()).unwrap_or_default(),
        };
        tracing::info!(
            "ssh_connect IPC: id={}, host={}, port={}, user={}, auth_method='{}', has_key_path={}, has_password={}, has_passphrase={}, has_password_ref={}, has_passphrase_ref={}, has_proxy={}, has_jump={}",
            id, host, port, username, auth_method,
            key_path.as_deref().map(|p| !p.is_empty()).unwrap_or(false),
            password.as_deref().map(|p| !p.is_empty()).unwrap_or(false),
            key_passphrase.as_deref().map(|p| !p.is_empty()).unwrap_or(false),
            password_ref.is_some(),
            key_passphrase_ref.is_some(),
            proxy.is_some(),
            jump_chain.as_ref().map(|c| c.len()).unwrap_or(0),
        );
        // Secrets held by a password manager are fetched now, for the target,
        // every jump host and the proxy, before anything connects. The proxy is
        // used only without a jump chain.
        //
        // Which entries, and where they may go, come from the saved session as
        // stored, not from this call: the references sent here only say "use
        // the session's entry". The connect must match the session, and each
        // entry must be one the connecting person approved for it; otherwise
        // the error says which, for the session list to ask (see
        // `session_commands::require_trusted_refs`).
        let mut password = password;
        let mut key_passphrase = key_passphrase;
        let mut jump_chain = jump_chain;
        let mut proxy = proxy;
        let wanted = wanted_slots(&auth_method, password_ref.is_some(), key_passphrase_ref.is_some(), jump_chain.as_deref(), proxy.as_ref());
        if !wanted.is_empty() {
            let sid = crate::ipc::session_commands::require_session_id(session_id.as_deref())?;
            let stored = crate::ipc::session_commands::stored_session(&state, sid).await?;
            // An address that goes through another saved session: that
            // session's route as stored now. Its hops' password manager
            // entries are that session's, not this one's, so none are used.
            let through = stored.alt_address(&host, port).and_then(|a| a.via_session_id.clone()).filter(|_| stored.host != host || stored.port != port);
            let via = match &through {
                Some(v) => {
                    if wanted.iter().any(|s| s.starts_with("jump:")) {
                        return Err(format!("'{}' goes through another saved session here, whose password manager entries are not used for it. Save the jump host's password in that session, or use a key or the agent.", stored.name));
                    }
                    Some(crate::ipc::session_commands::stored_session(&state, v).await?)
                }
                None => None,
            };
            check_matches_session(&stored, via.as_ref(), &host, port, &username, jump_chain.as_deref(), proxy.as_ref())?;
            crate::ipc::session_commands::require_trusted_refs(&stored, None, &wanted)?;
            // The ssh_config settings decide where the login really goes
            // (HostName, ProxyJump): the stored session's, which the approvals
            // were given for.
            ssh_options = stored.ssh_options.clone().map(|o| o.effective());
            // One operation: if the manager's passphrase must be asked for, it
            // is asked once for all of these, not once per entry.
            let reason = crate::pwmanager::Reason::Connect(stored.name.clone());
            crate::pwmanager::with_prompt(&app, reason, async {
                if let Some(field) = ref_field(&auth_method, password_ref.is_some(), key_passphrase_ref.is_some()) {
                    let secret = crate::pwmanager::resolve_to_string(&state, stored_ref(&stored, field)?).await?;
                    if field == "password" { password = Some(secret) } else { key_passphrase = Some(secret) }
                }
                for (i, j) in jump_chain.iter_mut().flatten().enumerate() {
                    if let Some(field) = ref_field(&j.auth_method, j.password_ref.is_some(), j.key_passphrase_ref.is_some()) {
                        let secret = crate::pwmanager::resolve_to_string(&state, stored_ref(&stored, &format!("jump:{i}:{field}"))?).await?;
                        if field == "password" { j.password = Some(secret) } else { j.key_passphrase = Some(secret) }
                    }
                }
                if wanted.iter().any(|s| s == "proxy") {
                    if let Some(p) = proxy.as_mut() {
                        p.password = Some(crate::pwmanager::resolve_to_string(&state, stored_ref(&stored, "proxy")?).await?);
                    }
                }
                Ok::<(), String>(())
            })
            .await?;
        }
        if let Some(t) = &tmux {
            tracing::info!("ssh_connect: in tmux session '{}'", t.name);
        } else if let Some(s) = shell.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            tracing::info!("ssh_connect: per-session login shell override = '{}'", s);
        }
        // A key stored in the vault brings its own passphrase; one typed into the
        // connect dialog still wins, so a saved passphrase that went stale can be
        // corrected without editing the key.
        let (key_source, stored_passphrase) =
            resolve_key_source(&state, key_id, key_path).await?;
        let key_passphrase = key_passphrase.filter(|p| !p.is_empty()).or(stored_passphrase);
        let auth = build_auth(&auth_method, password, key_source, key_passphrase, try_agent_keys.unwrap_or(false))?;

        // Establish the connection WITHOUT holding the global ssh_manager lock. The
        // handshake/auth/shell setup can take up to the connect timeout (longer if a
        // host hangs); holding the lock across it would block ssh_send / ssh_resize /
        // ssh_disconnect on every other live connection. We lock only afterwards,
        // briefly, to register the finished connection (a single HashMap insert).
        // ssh_config settings for the target: resolved here, before connecting.
        let mut target_opts: crate::ssh::client::HopOptions =
            crate::ssh::sshconf::session::plan_for(ssh_options.as_ref(), &host, port, &username, false).into();
        // LogLevel/LogVerbose: everything logged while connecting is collected
        // for the terminal, and shown with the error if the connection fails.
        // A saved session keeps the host keys it trusts, for every hop.
        let trust = crate::ipc::session_commands::session_trust(&state, session_id.as_deref()).await;
        target_opts.trust = trust.clone();
        if x11_desktop.is_some() {
            crate::ssh::x11_desktop::apply(&mut target_opts, &host, &format!("{username}@{host}")).await?;
        }
        let connlog = target_opts.log.as_ref().map(|(level, verbose)| crate::ssh::connlog::ConnLog::new(level.as_deref(), verbose));
        target_opts.connlog = connlog.clone();
        let span = connlog.as_ref().map_or_else(tracing::Span::none, |l| l.span());
        let connected: Result<_, String> = tracing::Instrument::instrument(async {
        let conn = if let Some(chain) = jump_chain {
            if chain.is_empty() {
                // No jump hosts, connect directly
                SshManager::connect(&id, &host, port, &username, auth, cols, rows, app.clone(), proxy, shell, login, target_opts)
                    .await
                    .map_err(|e| e.to_string())?
            } else {
                // Resolve every jump host's key before the closure below, which
                // cannot await.
                let mut jump_key_sources: Vec<Option<(Option<KeySource>, Option<String>)>> =
                    Vec::with_capacity(chain.len());
                for j in &chain {
                    jump_key_sources.push(Some(
                        resolve_key_source(&state, j.key_id.clone(), j.key_path.clone()).await?,
                    ));
                }

                // Build jump host params
                let jump_params: Result<Vec<JumpHostParams>, String> = chain
                    .into_iter()
                    .map(|j| {
                        let (jsource, jstored) = jump_key_sources
                            .remove(0)
                            .ok_or_else(|| "Jump host key could not be resolved".to_string())?;
                        let jauth = build_auth(
                            &j.auth_method,
                            j.password,
                            jsource,
                            j.key_passphrase.filter(|p| !p.is_empty()).or(jstored),
                            false,
                        )?;
                        let mut opts: crate::ssh::client::HopOptions = crate::ssh::sshconf::session::plan_for(ssh_options.as_ref(), &j.host, j.port, &j.username, true).into();
                        opts.trust = trust.clone();
                        Ok(JumpHostParams {
                            host: j.host,
                            port: j.port,
                            username: j.username,
                            auth: jauth,
                            opts,
                        })
                    })
                    .collect();

                SshManager::connect_via_jump(
                    &id,
                    &host,
                    port,
                    &username,
                    auth,
                    jump_params?,
                    cols,
                    rows,
                    app.clone(),
                    shell,
                    login,
                    target_opts,
                )
                .await
                .map_err(|e| e.to_string())?
            }
        } else {
            SshManager::connect(&id, &host, port, &username, auth, cols, rows, app.clone(), proxy, shell, login, target_opts)
                .await
                .map_err(|e| e.to_string())?
        };
        Ok(conn)
        }, span).await;
        let conn = match connected {
            Ok(c) => c,
            Err(e) => {
                let Some(cl) = connlog else { return Err(e) };
                crate::ssh::connlog::close(&cl);
                let lines = cl.take();
                return Err(if lines.is_empty() { e } else { format!("{e}\n\n{}", lines.join("\n")) });
            }
        };

        // Register the finished connection under a brief lock, released immediately
        // (before plugin hooks, which may themselves need the lock).
        let info = state.ssh_manager.lock().await.register(conn);

        let connection_id = info.id.clone();

        // Fire-and-forget: a slow or hung plugin hook must not block the IPC
        // return. dispatch_hook applies a per-hook timeout internally.
        let hook = hooks::session_connected(&connection_id, &host, &username);
        let plugin_mgr = state.plugin_manager.clone();
        let app_for_hook = app.clone();
        tokio::spawn(async move {
            let mut mgr = plugin_mgr.lock().await;
            mgr.dispatch_hook(&hook, Some(&app_for_hook)).await;
        });

        Ok(connection_id)
    })
    .await
}

/// One address to check, as `ssh_probe_addresses` takes it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProbeTarget {
    pub host: String,
    pub port: u16,
}

/// Which of a session's addresses answer from here: a plain TCP connect to
/// each, all at once, closed again at once. For each, the milliseconds it
/// took, or `None` when it did not answer within `timeout_ms` (1500 unless
/// said). Nothing is sent, so no login or key goes anywhere.
#[tauri::command]
pub async fn ssh_probe_addresses(targets: Vec<ProbeTarget>, timeout_ms: Option<u64>) -> Vec<Option<u64>> {
    let limit = std::time::Duration::from_millis(timeout_ms.unwrap_or(1500).clamp(100, 10_000));
    let probes = targets.into_iter().map(|t| async move {
        // Not to be checked (one reached through a jump host).
        if t.host.trim().is_empty() || t.port == 0 {
            return None;
        }
        let start = std::time::Instant::now();
        match tokio::time::timeout(limit, tokio::net::TcpStream::connect((t.host.as_str(), t.port))).await {
            Ok(Ok(_)) => Some(start.elapsed().as_millis() as u64),
            Ok(Err(e)) => {
                tracing::debug!("probe {}:{}: {e}", t.host, t.port);
                None
            }
            Err(_) => None,
        }
    });
    futures_util::future::join_all(probes).await
}

#[tauri::command]
pub async fn ssh_send(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    // A ZMODEM transfer holds the line.
    if crate::zmodem::intercept_input(&connection_id, &data) {
        return Ok(());
    }
    crate::history::input(&connection_id, &data);
    let manager = state.ssh_manager.lock().await;
    manager.send_data(&connection_id, &data).map_err(|e| e.to_string())
}

/// Called by the terminal once its data listener is attached: flushes any
/// buffered remote output (motd/banner) and switches to live streaming.
#[tauri::command]
pub async fn ssh_ready(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> Result<(), String> {
    let manager = state.ssh_manager.lock().await;
    manager.mark_ready(&connection_id).map_err(|e| e.to_string())
}

/// Where session logs go when Settings leaves the folder empty, shown there
/// as the placeholder.
#[tauri::command]
pub fn session_log_default_dir() -> String {
    crate::ssh::session_log::default_dir().display().to_string()
}

/// The file log of a terminal stream, if it is writing or paused.
#[tauri::command]
pub fn session_log_status(stream: String) -> Option<crate::ssh::session_log::LogStatus> {
    crate::ssh::session_log::log_status(&stream)
}

/// Pauses or resumes a terminal's file log from the bottom bar.
#[tauri::command]
pub fn session_log_pause(stream: String, paused: bool) -> Option<crate::ssh::session_log::LogStatus> {
    crate::ssh::session_log::set_log_paused(&stream, paused)
}

/// Stops a terminal's file log and closes the file.
#[tauri::command]
pub fn session_log_stop(stream: String) -> bool {
    crate::ssh::session_log::stop_log(&stream)
}

/// The frontend's host-key verification dialog reports the user's decision,
/// un-parking the SSH handshake that's waiting on `check_server_key`.
/// `save`: false for Accept once; absent means save, as Accept always did.
#[tauri::command]
pub fn ssh_hostkey_response(prompt_id: String, accept: bool, save: Option<bool>) {
    crate::ssh::client::resolve_hostkey_prompt(&prompt_id, crate::ssh::session_hostkeys::Answer::from_reply(accept, save));
}

/// The answer to a login question (password, passphrase, keyboard-interactive,
/// yes/no); `None` when the user cancelled.
#[tauri::command]
pub fn ssh_auth_prompt_response(prompt_id: String, answers: Option<Vec<String>>) {
    crate::ssh::prompt::resolve(&prompt_id, answers);
}

#[tauri::command]
pub async fn ssh_resize(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let manager = state.ssh_manager.lock().await;
    manager.resize(&connection_id, cols, rows).map_err(|e| e.to_string())
}

/// Make a connection's files go over SCP: an SCP session's, whose server
/// may have no SFTP subsystem (see `sftp::scp`).
#[tauri::command]
pub async fn ssh_files_scp(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> Result<(), String> {
    let manager = state.ssh_manager.lock().await;
    manager.set_scp(&connection_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ssh_disconnect(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> Result<(), String> {
    let mut manager = state.ssh_manager.lock().await;
    manager.disconnect(&connection_id).map_err(|e| e.to_string())?;
    drop(manager);
    // Transfers on this connection end with it, rather than being left
    // waiting on a channel that is gone (#161).
    crate::ipc::sftp_commands::cancel_transfers_on(&state, &connection_id);

    // Fire-and-forget hook dispatch (see ssh_connect for rationale).
    let hook = hooks::session_disconnected(&connection_id);
    let plugin_mgr = state.plugin_manager.clone();
    let app_for_hook = app.clone();
    tokio::spawn(async move {
        let mut mgr = plugin_mgr.lock().await;
        mgr.dispatch_hook(&hook, Some(&app_for_hook)).await;
    });

    Ok(())
}

#[tauri::command]
pub async fn ssh_list_connections(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ConnectionInfo>, String> {
    let manager = state.ssh_manager.lock().await;
    Ok(manager.list_connections())
}

/// The algorithms the connection's key exchange settled on, for the tab's
/// tooltip; `None` before the first one or on a shared connection.
#[tauri::command]
pub async fn ssh_connection_algorithms(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> Result<Option<crate::ssh::client::NegotiatedAlgorithms>, String> {
    let manager = state.ssh_manager.lock().await;
    manager.algorithms(&connection_id).map_err(|e| e.to_string())
}

/// OS probes answer at once or not at all.
const OS_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Detect the remote operating system. Probes, in order:
/// 1. `/etc/os-release` (Linux; MSYS2/Cygwin on Windows ship one too)
/// 2. `uname -s` (macOS, BSDs; Git Bash on Windows answers `MINGW64_NT-10.0`)
/// 3. `cmd /c ver` (Windows, whatever its default shell)
/// Returns a lowercase distro ID (e.g. "debian", "ubuntu", "alpine",
/// "raspbian" for Raspberry Pi OS),
/// "windows", or "unknown" when nothing answered: a router or a restricted
/// shell is not Linux, and guessing so put a penguin on it for good.
/// (Ported from the ReachQ fork by CPTProgrammer.)
#[tauri::command]
pub async fn ssh_detect_os(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> Result<String, String> {
    let handle = {
        let manager = state.ssh_manager.lock().await;
        manager.get_handle(&connection_id).map_err(|e| e.to_string())?
    };

    // Exit codes matter: cmd.exe on Windows answers `cat` with an error on
    // stderr, and a shell that prints a banner must not pass for an answer.
    // MikroTik RouterOS runs exec commands in its own CLI, which answers
    // `cat` with "bad command name", so look at the text before the code.
    let release = exec_on_connection_with_exit_code_timeout(&handle, "cat /etc/os-release", OS_PROBE_TIMEOUT).await;
    if let Ok((out, err, _)) = &release {
        if is_routeros_reply(out) || is_routeros_reply(err) {
            return Ok("mikrotik".to_string());
        }
    }
    let unix = match release {
        Ok((out, _, 0)) => Some(os_from_os_release(&out)),
        _ => match exec_on_connection_with_exit_code_timeout(&handle, "uname -s", OS_PROBE_TIMEOUT).await {
            Ok((out, err, _)) if is_routeros_reply(&out) || is_routeros_reply(&err) => return Ok("mikrotik".to_string()),
            Ok((out, _, 0)) => os_from_uname(&out),
            _ => None,
        },
    };
    if let Some(os) = unix {
        // UniFi consoles run Debian and UniFi access points and switches a
        // BusyBox Linux; both carry Ubiquiti's board files.
        if os != "windows" && os != "darwin" {
            if let Ok((_, _, 0)) = exec_on_connection_with_exit_code_timeout(&handle, UNIFI_PROBE, OS_PROBE_TIMEOUT).await {
                return Ok("unifi".to_string());
            }
        }
        // Raspberry Pi OS (64-bit, and 32-bit since Bookworm) says ID=debian;
        // only the old 32-bit Raspbian says ID=raspbian. Ubuntu or another
        // distro on a Pi keeps its own name.
        if os == "debian" || os == "linux" {
            if let Ok((_, _, 0)) = exec_on_connection_with_exit_code_timeout(&handle, RASPBERRY_PI_PROBE, OS_PROBE_TIMEOUT).await {
                return Ok("raspbian".to_string());
            }
        }
        return Ok(os);
    }
    if let Ok((out, _, 0)) = exec_on_connection_with_exit_code_timeout(&handle, "cmd /c ver", OS_PROBE_TIMEOUT).await {
        if out.to_lowercase().contains("windows") {
            return Ok("windows".to_string());
        }
    }
    // A RouterOS that answered `cat` with nothing at all still knows this.
    if let Ok((out, _, _)) = exec_on_connection_with_exit_code_timeout(&handle, "/system resource print", OS_PROBE_TIMEOUT).await {
        if is_routeros_resource(&out) {
            return Ok("mikrotik".to_string());
        }
    }
    Ok("unknown".to_string())
}

/// Succeeds only on a Ubiquiti device: UniFi OS consoles ship
/// `ubnt-device-info`, access points and switches `/etc/board.info` and
/// `mca-cli-op`.
const UNIFI_PROBE: &str = "test -e /etc/board.info || test -e /usr/bin/ubnt-device-info || test -e /sbin/ubnt-device-info || command -v mca-cli-op >/dev/null 2>&1";

/// Succeeds only on a Raspberry Pi running Raspberry Pi OS, which ships
/// `/etc/rpi-issue`; the board's device-tree model names it too.
const RASPBERRY_PI_PROBE: &str = "test -e /etc/rpi-issue || grep -qa 'Raspberry Pi' /proc/device-tree/model 2>/dev/null";

/// The RouterOS CLI's answer to a command it does not have, such as
/// `bad command name cat (line 1 column 1)`.
fn is_routeros_reply(text: &str) -> bool {
    let text = text.to_lowercase();
    text.contains("bad command name") || text.contains("expected end of command")
}

/// `/system resource print` on RouterOS lists `platform: MikroTik`.
fn is_routeros_resource(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim().to_lowercase();
        line.starts_with("platform:") && line.contains("mikrotik")
    })
}

/// The `ID=` of an os-release file (not `ID_LIKE=`). MSYS2, MinGW and Cygwin
/// ship one on a Windows host; WSL's is its real distro, which is right.
fn os_from_os_release(text: &str) -> String {
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("ID=") {
            let id = rest.trim().trim_matches(|c| c == '"' || c == '\'').to_lowercase();
            if id.is_empty() {
                break;
            }
            if id.contains("msys") || id.contains("mingw") || id.contains("cygwin") {
                return "windows".to_string();
            }
            return id;
        }
    }
    // An os-release without an ID is still a Linux system.
    "linux".to_string()
}

/// `uname -s`, lowercased. Git Bash, MSYS and Cygwin on Windows answer
/// `MINGW64_NT-10.0-19045`, `MSYS_NT-…` or `CYGWIN_NT-…`.
fn os_from_uname(text: &str) -> Option<String> {
    let os = text.lines().next()?.trim().to_lowercase();
    if os.is_empty() {
        return None;
    }
    if os.contains("_nt") {
        return Some("windows".to_string());
    }
    Some(os)
}

#[cfg(test)]
mod detect_os_tests {
    use super::*;

    #[test]
    fn os_release_gives_its_id() {
        let ubuntu = "NAME=\"Ubuntu\"\nID_LIKE=debian\nID=ubuntu\nVERSION_ID=\"24.04\"\n";
        assert_eq!(os_from_os_release(ubuntu), "ubuntu");
        assert_eq!(os_from_os_release("ID='alpine'\n"), "alpine");
        assert_eq!(os_from_os_release("NAME=Something\n"), "linux");
    }

    #[test]
    fn msys_and_cygwin_are_windows() {
        assert_eq!(os_from_os_release("ID=msys2\n"), "windows");
        assert_eq!(os_from_os_release("ID=cygwin\n"), "windows");
        assert_eq!(os_from_uname("MINGW64_NT-10.0-19045\n").as_deref(), Some("windows"));
        assert_eq!(os_from_uname("CYGWIN_NT-10.0\n").as_deref(), Some("windows"));
    }

    #[test]
    fn uname_gives_the_kernel() {
        assert_eq!(os_from_uname("Darwin\n").as_deref(), Some("darwin"));
        assert_eq!(os_from_uname("FreeBSD\n").as_deref(), Some("freebsd"));
        assert_eq!(os_from_uname("  \n"), None);
    }

    #[test]
    fn routeros_answers_are_mikrotik() {
        assert!(is_routeros_reply("bad command name cat (line 1 column 1)\n"));
        assert!(is_routeros_reply("expected end of command (line 1 column 5)"));
        assert!(!is_routeros_reply("cat: /etc/os-release: No such file or directory"));
        let resource = "                   uptime: 3d2h\n                  version: 7.16 (stable)\n              board-name: hAP ax2\n                 platform: MikroTik\n";
        assert!(is_routeros_resource(resource));
        assert!(!is_routeros_resource("sh: /system: not found\n"));
    }
}

/// The tmux start script for a WSL tab, which runs it with
/// `wsl.exe -e sh -c <script>` (see `ssh::tmux::start_script`).
#[tauri::command]
pub fn tmux_start_script(start: crate::ssh::tmux::TmuxStart) -> Result<String, String> {
    crate::ssh::tmux::start_script(&start, None)
}

/// What the tmux picker chose while connection `connection_id` opened,
/// once: `{ "name": "main" }`, `{ "name": null }` for no tmux, or null
/// when nothing was asked.
#[tauri::command]
pub fn ssh_tmux_chosen(connection_id: String) -> Option<serde_json::Value> {
    crate::ssh::tmux::take_chosen(&connection_id).map(|name| serde_json::json!({ "name": name }))
}

/// The server's tmux sessions, for a tab's "Switch tmux session".
#[tauri::command]
pub async fn ssh_tmux_list(
    state: tauri::State<'_, AppState>,
    connection_id: String,
) -> Result<Vec<crate::ssh::tmux::Listed>, String> {
    let handle = {
        let manager = state.ssh_manager.lock().await;
        manager.get_handle(&connection_id).map_err(|e| e.to_string())?
    };
    let out = crate::ssh::client::exec_on_connection(&handle, crate::ssh::tmux::LIST_COMMAND).await.map_err(|e| e.to_string())?;
    Ok(crate::ssh::tmux::parse_list(&out))
}

/// The windows of a tab's tmux session, as its view (`{name}-{view}`) sees them.
#[tauri::command]
pub async fn ssh_tmux_windows(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    name: String,
    view: String,
) -> Result<Vec<crate::ssh::tmux::Window>, String> {
    let view = crate::ssh::tmux::view_name(&name, &view)?;
    let handle = {
        let manager = state.ssh_manager.lock().await;
        manager.get_handle(&connection_id).map_err(|e| e.to_string())?
    };
    let out = crate::ssh::client::exec_on_connection(&handle, &crate::ssh::tmux::windows_command(&view)).await.map_err(|e| e.to_string())?;
    Ok(crate::ssh::tmux::parse_windows(&out))
}

/// Switch, rename, close or open a window in a tab's tmux view.
#[tauri::command]
pub async fn ssh_tmux_window(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    name: String,
    view: String,
    op: crate::ssh::tmux::WindowOp,
    shell: Option<String>,
) -> Result<(), String> {
    let view = crate::ssh::tmux::view_name(&name, &view)?;
    let command = crate::ssh::tmux::window_command(&view, &op, shell.as_deref())?;
    let handle = {
        let manager = state.ssh_manager.lock().await;
        manager.get_handle(&connection_id).map_err(|e| e.to_string())?
    };
    let (_, err, code) = crate::ssh::client::exec_on_connection_with_exit_code_timeout(&handle, &command, std::time::Duration::from_secs(15))
        .await
        .map_err(|e| e.to_string())?;
    if code != 0 {
        let err = err.trim();
        return Err(if err.is_empty() { format!("tmux exited with {code}") } else { err.to_string() });
    }
    Ok(())
}

/// Ends a tab's tmux session for good (the tab's own connection runs it),
/// with the sessions other tabs joined it as.
#[tauri::command]
pub async fn ssh_tmux_kill(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    name: String,
) -> Result<(), String> {
    let command = crate::ssh::tmux::kill_command(&name)?;
    let handle = {
        let manager = state.ssh_manager.lock().await;
        manager.get_handle(&connection_id).map_err(|e| e.to_string())?
    };
    crate::ssh::client::exec_on_connection(&handle, &command).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Inspect a key-file path so the UI can warn when the user picked the wrong
/// file (e.g. an OpenSSH public key) and suggest the right private key from the
/// same folder. Pure filesystem read — no connection or state required.
#[tauri::command]
pub fn inspect_key_file(path: String) -> crate::ssh::keyfile::KeyFileInfo {
    crate::ssh::keyfile::classify_path(&path)
}

#[cfg(test)]
mod auth_choice_tests {
    use super::*;

    fn key() -> Option<KeySource> {
        Some(KeySource::Material("k".into()))
    }

    /// Each kind of session offers what it names and nothing else, as PuTTY
    /// (userauth2-client.c filters Pageant down to the configured key) and
    /// OpenSSH with IdentitiesOnly do. No other key, no password on the side.
    #[test]
    fn a_session_offers_only_what_it_names() {
        let k = build_auth("key", Some("pw".into()), key(), None, false).unwrap();
        assert!(k.key.is_some() && !k.allow_agent && k.password.is_none());

        let p = build_auth("password", Some("pw".into()), None, None, true).unwrap();
        assert!(p.password.is_some() && !p.allow_agent && p.key.is_none());

        let a = build_auth("agent", None, None, None, false).unwrap();
        assert!(a.allow_agent && a.key.is_none() && a.password.is_none());
    }

    #[test]
    fn the_agent_backs_a_key_only_when_the_session_asks() {
        assert!(build_auth("key", None, key(), None, true).unwrap().allow_agent);
    }
}
