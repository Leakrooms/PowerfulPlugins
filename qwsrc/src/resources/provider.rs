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
                "handle_provider_action success: response_len={}",
                response.len()
            );
            response
        }
        Err(error) => {
            tracing::error!("provider action failed: {error:#}");
            fallback_response(event_payload, &error.to_string())
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
        "provider callback sent: request_id={request_id} accepted={accepted} response_len={}",
        response.len()
    );
}

pub(super) fn notify_provider_action_progress(
    request_id: Option<&str>,
    progress: f32,
    status: &str,
) {
    let Some(request_id) = request_id.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    let accepted =
        provider_callback::report_provider_action_progress(request_id, progress, status);
    tracing::info!(
        "provider progress sent: request_id={request_id} accepted={accepted} progress={progress} status={status}"
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
        "provider action parsed: version={} provider={} action={}",
        envelope.version,
        envelope.provider,
        envelope.action
    );

    match envelope.action.as_str() {
        "refresh" => {
            let params: RefreshParams = serde_json::from_value(envelope.params)?;
            refresh_state(params)?;
            Ok("{}".to_string())
        }
        "get_categories" => {
            let categories = fetch_categories()?;
            Ok(serde_json::to_string(&categories)?)
        }
        "get_index" => {
            let params: IndexParams = serde_json::from_value(envelope.params)?;
            let items = get_index(params)?;
            Ok(serde_json::to_string(&items)?)
        }
        "get_manifest" => {
            let params: ManifestParams = serde_json::from_value(envelope.params)?;
            let manifest = get_manifest(&params.item_id)?;
            Ok(serde_json::to_string(&manifest)?)
        }
        "get_total" => {
            let total = get_total()?;
            Ok(serde_json::to_string(&total)?)
        }
        "download" => {
            let params: DownloadParams = serde_json::from_value(envelope.params)?;
            download_app(&params.item_id, params.device.as_deref(), request_id.as_deref())
        }
        other => Err(anyhow!("unsupported provider action: {other}")),
    }
}

fn fallback_response(event_payload: &str, error: &str) -> String {
    let action = serde_json::from_str::<ProviderActionEnvelope>(event_payload)
        .map(|payload| payload.action)
        .unwrap_or_default();

    match action.as_str() {
        "get_categories" | "get_index" => "[]".to_string(),
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
