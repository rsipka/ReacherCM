//! MQTT sessions (see `crate::mqtt`): connecting a saved one, the
//! certificates it trusts, and publishing and subscribing while it is open.
//! What arrives goes to the window as `mqtt-event-<connection id>`.

use super::*;
use base64::Engine;
use tauri::Emitter;

/// What the window is told to ask when a connect needs the user.
const PASSWORD_NEEDED: &str = "MQTT_PASSWORD_NEEDED";
const LOGIN_REFUSED: &str = "MQTT_LOGIN_REFUSED:";
const UNTRUSTED_CERT: &str = "MQTT_UNTRUSTED_CERT:";

fn conn(state: &AppState, id: &str) -> Result<std::sync::Arc<crate::mqtt::MqttConn>, String> {
    state.mqtt_manager.get(id).ok_or_else(|| "The MQTT session is closed; reconnect it".to_string())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MqttOpened {
    pub client_id: String,
    /// The topic filters subscribed to.
    pub filters: Vec<String>,
}

/// Connect a saved MQTT session under the connection id `id`. `password`,
/// typed in for this connect, wins over the saved one. Fails with
/// `MQTT_PASSWORD_NEEDED` when the broker refused a login without one,
/// `MQTT_LOGIN_REFUSED:<why>` when it refused the one given, and
/// `MQTT_UNTRUSTED_CERT:<fingerprint>` for a certificate neither the system
/// nor the session trusts. Listen to `mqtt-event-<id>` before calling.
#[tauri::command]
#[tracing::instrument(skip(state, app, password))]
pub async fn mqtt_connect(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    session_id: String,
    password: Option<String>,
) -> Result<MqttOpened, String> {
    let stored = stored_session(&state, &session_id).await?;
    if stored.kind != crate::state::SessionKind::Mqtt {
        return Err("Not an MQTT session".into());
    }
    let opts = stored.mqtt.clone().unwrap_or_default();
    let typed = password.as_ref().is_some_and(|p| !p.is_empty());
    let password = match password.filter(|p| !p.is_empty()) {
        Some(p) => p,
        None => match &stored.auth_method {
            AuthMethod::Password { password_ref: Some(_), .. } => {
                require_trusted_refs(&stored, None, &["password".to_string()])?;
                let reason = crate::pwmanager::Reason::Connect(stored.name.clone());
                let entry = desktop_ref(&stored)?;
                crate::pwmanager::with_prompt(&app, reason, crate::pwmanager::resolve_to_string(&state, entry)).await?
            }
            AuthMethod::Password { password: own, .. } => own.clone().unwrap_or_default(),
            _ => String::new(),
        },
    };
    let username = stored.username.trim().to_string();
    let client_id = opts.client_id.clone().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).unwrap_or_else(crate::mqtt::random_client_id);
    let filters = opts.filters();
    let client_auth = match (opts.client_cert.as_deref().map(str::trim), opts.client_key.as_deref().map(str::trim)) {
        (Some(c), Some(k)) if !c.is_empty() && !k.is_empty() => Some((c.to_string(), k.to_string())),
        _ => None,
    };
    let mut params = crate::mqtt::Params {
        host: stored.host.clone(),
        port: stored.port,
        transport: crate::mqtt::Transport::parse(opts.transport.as_deref()),
        ws_path: opts.ws_path.clone().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "/mqtt".into()),
        client_id: client_id.clone(),
        keep_alive: opts.keep_alive.filter(|k| *k > 0).unwrap_or(60),
        username: Some(username.clone()).filter(|u| !u.is_empty()),
        password: Some(password.clone()).filter(|p| !p.is_empty()),
        client_auth,
        trusted: opts.certs.clone(),
        wireguard: None,
        filters: filters.clone(),
    };
    // Through the session's WireGuard tunnel when the window turned it on.
    if let Some(wg) = stored.wireguard_id.clone().filter(|w| !w.is_empty()) {
        if crate::wireguard::manager::is_up(&wg).await {
            params.wireguard = Some(wg);
        }
    }
    let event = format!("mqtt-event-{id}");
    let emitter = app.clone();
    let emit: std::sync::Arc<dyn Fn(crate::mqtt::Event) + Send + Sync> = std::sync::Arc::new(move |e| {
        let _ = emitter.emit(&event, e);
    });
    let conn = crate::mqtt::connect(params, emit).await.map_err(|e| match e {
        crate::mqtt::MqttError::UntrustedCert(fp) => format!("{UNTRUSTED_CERT}{fp}"),
        crate::mqtt::MqttError::LoginRefused(_) if !typed && password.is_empty() => PASSWORD_NEEDED.into(),
        crate::mqtt::MqttError::LoginRefused(m) => format!("{LOGIN_REFUSED}{m}"),
        other => other.to_string(),
    })?;
    tracing::info!("MQTT session {} connected", stored.name);
    state.mqtt_manager.insert(id, conn);
    Ok(MqttOpened { client_id, filters })
}

/// Close an MQTT session.
#[tauri::command]
pub async fn mqtt_disconnect(state: State<'_, AppState>, connection_id: String) -> Result<(), String> {
    if let Some(conn) = state.mqtt_manager.remove(&connection_id) {
        conn.close().await;
    }
    Ok(())
}

/// Publish a message; `payload` is base64. Returns once the broker took it
/// (QoS 1 and 2), or once it was sent (QoS 0).
#[tauri::command]
pub async fn mqtt_publish(
    state: State<'_, AppState>,
    connection_id: String,
    topic: String,
    payload: String,
    qos: u8,
    retain: bool,
) -> Result<(), String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(payload.as_bytes()).map_err(|_| "The message is not base64".to_string())?;
    conn(&state, &connection_id)?.publish(topic, bytes, qos, retain).await
}

/// Subscribe to topic filters; the QoS the broker granted each, 128 for a
/// refused one.
#[tauri::command]
pub async fn mqtt_subscribe(state: State<'_, AppState>, connection_id: String, filters: Vec<String>, qos: u8) -> Result<Vec<u8>, String> {
    let filters = filters.into_iter().map(|f| f.trim().to_string()).filter(|f| !f.is_empty()).map(|f| (f, qos.min(2))).collect();
    conn(&state, &connection_id)?.subscribe(filters).await
}

#[tauri::command]
pub async fn mqtt_unsubscribe(state: State<'_, AppState>, connection_id: String, filters: Vec<String>) -> Result<(), String> {
    conn(&state, &connection_id)?.unsubscribe(filters).await
}

/// Trust a broker certificate for an MQTT session, by the fingerprint the
/// failed connect reported and the user agreed to.
#[tauri::command]
#[tracing::instrument(skip(state))]
pub async fn mqtt_session_cert_trust(state: State<'_, AppState>, session_id: String, fingerprint: String) -> Result<(), String> {
    super::ftp::trust_session_cert(&state, &session_id, &fingerprint, "mqtt").await
}
