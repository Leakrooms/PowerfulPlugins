use super::*;

pub fn handle_provider_action(event_payload: &str) -> String {
    let request_id = serde_json::from_str::<ProviderActionEnvelope>(event_payload)
        .ok()
        .and_then(|envelope| envelope.request_id);
    tracing::info!(
        "handle_provider_action enter: payload_len={}, preview={}",
        event_payload.len(),
        event_payload.chars().take(300).collect::<String>()
    );
    let response = match handle_provider_action_inner(event_payload) {
        Ok(response) => {
            tracing::info!(
                "handle_provider_action success: response_len={}, preview={}",
                response.len(),
                response.chars().take(300).collect::<String>()
            );
            response
        }
        Err(error) => {
            tracing::error!("provider action failed: {error:#}");
            let fallback = fallback_response(event_payload, &error.to_string());
            tracing::warn!(
                "handle_provider_action fallback: response_len={}, preview={}",
                fallback.len(),
                fallback.chars().take(300).collect::<String>()
            );
            fallback
        }
    };

    notify_provider_action_response(request_id.as_deref(), &response);
    response
}

fn notify_provider_action_response(request_id: Option<&str>, response: &str) {
    let Some(request_id) = request_id.filter(|value| !value.trim().is_empty()) else {
        tracing::debug!("provider callback skipped: request_id missing");
        return;
    };

    let accepted = provider_callback::resolve_provider_action(request_id, response);
    tracing::info!(
        "provider callback sent: request_id={} accepted={} response_len={}",
        request_id,
        accepted,
        response.len()
    );
}

pub(super) fn notify_provider_action_progress(request_id: Option<&str>, progress: f32, status: &str) {
    let Some(request_id) = request_id.filter(|value| !value.trim().is_empty()) else {
        return;
    };

    let accepted =
        provider_callback::report_provider_action_progress(request_id, progress, status);
    tracing::info!(
        "provider progress sent: request_id={} accepted={} progress={} status={}",
        request_id,
        accepted,
        progress,
        status
    );
}

fn handle_provider_action_inner(event_payload: &str) -> Result<String> {
    let envelope: ProviderActionEnvelope = serde_json::from_str(event_payload)
        .with_context(|| "failed to parse provider-action payload")?;
    let request_id = envelope.request_id.clone();

    if envelope.version != 1 {
        return Err(anyhow!(
            "unsupported provider payload version: {}",
            envelope.version
        ));
    }
    if envelope.provider != PROVIDER_NAME {
        return Err(anyhow!(
            "provider mismatch: expected {}, got {}",
            PROVIDER_NAME,
            envelope.provider
        ));
    }

    tracing::info!(
        "provider action parsed: version={}, provider={}, action={}",
        envelope.version,
        envelope.provider,
        envelope.action
    );

    match envelope.action.as_str() {
        "refresh" => {
            let params: RefreshParams = serde_json::from_value(envelope.params)?;
            tracing::info!("dispatch action=refresh");
            refresh_state(params)?;
            Ok("{}".to_string())
        }
        "get_categories" => {
            tracing::info!("dispatch action=get_categories");
            let categories = fetch_categories()?;
            Ok(serde_json::to_string(&categories)?)
        }
        "get_index" => {
            let params: IndexParams = serde_json::from_value(envelope.params)?;
            tracing::info!(
                "dispatch action=get_index page={} limit={} filter={:?} sort={:?} category={:?}",
                params.page,
                params.limit,
                params.search.filter,
                params.search.sort,
                params.search.category
            );
            let items = get_index(params)?;
            Ok(serde_json::to_string(&items)?)
        }
        "get_manifest" => {
            let params: ManifestParams = serde_json::from_value(envelope.params)?;
            tracing::info!("dispatch action=get_manifest item_id={}", params.item_id);
            let manifest = get_manifest(&params.item_id)?;
            Ok(serde_json::to_string(&manifest)?)
        }
        "get_total" => {
            tracing::info!("dispatch action=get_total");
            let total = get_total()?;
            Ok(serde_json::to_string(&total)?)
        }
        "download" => {
            let params: DownloadParams = serde_json::from_value(envelope.params)?;
            tracing::info!(
                "dispatch action=download item_id={} device={:?}",
                params.item_id,
                params.device
            );
            let result = download_watchface(
                &params.item_id,
                params.device.as_deref(),
                request_id.as_deref(),
            )?;
            Ok(result)
        }
        other => Err(anyhow!("unsupported provider action: {other}")),
    }
}

fn fallback_response(event_payload: &str, error: &str) -> String {
    let action = serde_json::from_str::<ProviderActionEnvelope>(event_payload)
        .map(|payload| payload.action)
        .unwrap_or_default();

    match action.as_str() {
        "get_categories" => "[]".to_string(),
        "get_index" => "[]".to_string(),
        "get_manifest" => json!({
            "item": {
                "id": "",
                "restype": "watchface",
                "name": "请求失败",
                "description": error,
                "preview": [],
                "icon": "",
                "cover": "",
                "author": []
            }
        })
        .to_string(),
        "get_total" => "0".to_string(),
        "download" => String::new(),
        _ => "{}".to_string(),
    }
}

fn refresh_state(params: RefreshParams) -> Result<()> {
    // Snapshot the current (possibly disk-restored) state up front so a refresh
    // that omits credentials/preferences preserves the persisted session instead
    // of resetting it.
    let existing = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();

    let mut config = ProviderConfig::default();
    config.device_type = existing.config.device_type.clone();
    config.use_donor_download = existing.config.use_donor_download;
    tracing::info!(
        "refresh_state enter: has_config_raw={} has_config_obj={}",
        params
            .config_raw
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        params.config.as_ref().is_some_and(|value| !value.is_null())
    );

    if let Some(config_value) = params.config.as_ref().filter(|value| !value.is_null()) {
        config.merge_json(config_value)?;
    }
    if let Some(raw) = params.config_raw.as_deref() {
        config.merge_raw(raw)?;
    }
    config.normalize();
    tracing::info!(
        "refresh_state normalized: device_type={} has_openid={} has_valid_token={} has_username={} has_password={} has_mitan_code={}",
        config.device_type(),
        config.openid.is_some(),
        config.valid_token.is_some(),
        config.username.is_some(),
        config.password.is_some(),
        config.mitan_code.is_some()
    );

    let auth = match config.explicit_auth() {
        Some(auth) => Some(auth),
        None => match config.login_payload() {
            Some(login) => match auth::login_with_mitan(&config, login) {
                Ok(auth) => Some(auth),
                Err(error) => {
                    let error_string = error.to_string();
                    tracing::warn!("provider login failed: {error_string}");
                    let mut guard = state()
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard.last_login_error = Some(error_string);
                    guard.config = config.clone();
                    guard.auth = None;
                    return Ok(());
                }
            },
            // Host supplied no credentials: keep the persisted login session.
            None => existing.auth.clone(),
        },
    };

    let mut guard = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.config = config;
    guard.auth = auth;
    guard.last_login_error = None;
    save_session_locked(&guard);
    tracing::info!("refresh_state completed");
    Ok(())
}
