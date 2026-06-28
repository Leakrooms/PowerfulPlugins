use super::*;

pub fn fetch_upload_tips_for_ui() -> Result<String> {
    let value = api_request(
        "upload-tips",
        &api_json_url("config/json/uploadtips"),
        Client::new()
            .get(&api_json_url("config/json/uploadtips"))
            .headers(base_headers(&current_config(), None))
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?;

    if let Some(message) = value
        .get("msg")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(message.to_string());
    }

    if let Some(message) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(message.to_string());
    }

    Ok(serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()))
}

pub fn fetch_my_uploads_for_ui(page: usize, page_size: usize) -> Result<Vec<UiMyShareItem>> {
    let (config, auth) = require_auth_context_for_ui()?;
    let page = page.max(1);
    let records: Vec<MyShareItemRecord> = serde_json::from_value(api_request(
        "my-share-list",
        &api_url(&format!("watchface/my/share/list/{page}/{page_size}")),
        Client::new()
            .get(&api_url(&format!("watchface/my/share/list/{page}/{page_size}")))
            .headers(base_headers(&config, Some(&auth)))
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?)?;

    let devices =
        fetch_device_list().unwrap_or_else(|_| filter_supported_devices(fallback_device_list()));
    let device_name_map = build_device_name_map(&devices);
    Ok(records
        .into_iter()
        .filter(|item| !is_hidden_device_model(item.r#type.as_deref().unwrap_or_default()))
        .map(|item| map_my_share_record(item, &device_name_map))
        .collect())
}

pub fn toggle_my_upload_share_for_ui(id: i64, current_is_share: bool) -> Result<()> {
    let (config, auth) = require_auth_context_for_ui()?;
    api_request(
        "my-share-set",
        &api_url("watchface/my/share/set"),
        Client::new()
            .post(&api_url("watchface/my/share/set"))
            .headers(base_headers(&config, Some(&auth)))
            .form([
                ("id", id.to_string()),
                ("isShare", if current_is_share { "1" } else { "0" }.to_string()),
            ])
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?;
    Ok(())
}

pub fn delete_my_upload_for_ui(id: i64) -> Result<()> {
    let (config, auth) = require_auth_context_for_ui()?;
    api_request(
        "my-share-delete",
        &api_url("watchface/my/share/delete"),
        Client::new()
            .post(&api_url("watchface/my/share/delete"))
            .headers(base_headers(&config, Some(&auth)))
            .form([("id", id.to_string())])
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?;
    Ok(())
}

pub fn top_my_upload_for_ui(id: i64) -> Result<()> {
    let (config, auth) = require_auth_context_for_ui()?;
    api_request(
        "my-share-top",
        &api_url("watchface/my/share/top"),
        Client::new()
            .post(&api_url("watchface/my/share/top"))
            .headers(base_headers(&config, Some(&auth)))
            .form([("id", id.to_string())])
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?;
    Ok(())
}

pub fn query_my_upload_reason_for_ui(id: i64) -> Result<Option<String>> {
    let (config, auth) = require_auth_context_for_ui()?;
    let value = api_request(
        "my-share-reason",
        &api_url(&format!("watchface/work/queryreason/{id}")),
        Client::new()
            .get(&api_url(&format!("watchface/work/queryreason/{id}")))
            .headers(base_headers(&config, Some(&auth)))
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?;

    if let Some(first_reason) = value
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item.get("tag"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(first_reason.to_string()));
    }

    Ok(None)
}

pub fn submit_upload_for_ui(request: &UiUploadRequest) -> Result<()> {
    let watchface_file = request
        .watchface_file
        .clone()
        .ok_or_else(|| anyhow!("请选择要上传的资源文件"))?;
    let (config, auth) = require_auth_context_for_ui()?;
    let devices =
        fetch_device_list().unwrap_or_else(|_| filter_supported_devices(fallback_device_list()));
    let device_type = normalize_ui_device_type(&Some(request.device_type.clone()), &devices);
    let preview_main = upload_preview_asset_for_ui(&device_type, request.preview_main.as_ref())?;
    let preview_aod = upload_preview_asset_for_ui(&device_type, request.preview_aod.as_ref())?;
    let preview_aod2 = upload_preview_asset_for_ui(&device_type, request.preview_aod2.as_ref())?;
    let preview_aod3 = upload_preview_asset_for_ui(&device_type, request.preview_aod3.as_ref())?;

    let mut form = Form::new()
        .text("name", request.name.trim())
        .text("desc", request.description.trim())
        .text("type", device_type.clone())
        .text("staticPng", request.static_png.to_string())
        .text("previewImg", preview_main.unwrap_or_default())
        .text("previewImgAod", preview_aod.unwrap_or_default())
        .text("previewImgAod2", preview_aod2.unwrap_or_default())
        .text("previewImgAod3", preview_aod3.unwrap_or_default())
        .text("mitantid", request.mitantid.as_deref().unwrap_or_default().trim())
        .text("mitantype", request.mitantype.trim())
        .text("updateId", request.update_id.as_deref().unwrap_or_default().trim());

    let file_part = Part::new("file", watchface_file.data)
        .filename(watchface_file.name)
        .mime_str("application/octet-stream")?;
    form = form.part(file_part);

    api_multipart_request(
        "upload-bin-self",
        &api_url("watchface/uploadBinSelfMi7"),
        Client::new()
            .post(&api_url("watchface/uploadBinSelfMi7"))
            .headers(build_upload_headers(&config, &auth, &device_type))
            .multipart(form)
            .connect_timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS)),
    )?;
    Ok(())
}

fn require_auth_context_for_ui() -> Result<(ProviderConfig, AuthContext)> {
    let guard = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let auth = guard
        .auth
        .clone()
        .or_else(|| guard.config.explicit_auth())
        .ok_or_else(|| anyhow!("请先登录米坛账号"))?;
    Ok((guard.config.clone(), auth))
}

fn upload_preview_asset_for_ui(
    device_type: &str,
    asset: Option<&UiBinaryAsset>,
) -> Result<Option<String>> {
    let Some(asset) = asset else {
        return Ok(None);
    };

    let part = Part::new("file", asset.data.clone())
        .filename(asset.name.clone())
        .mime_str(guess_image_mime(&asset.name))?;
    let value = api_multipart_request(
        "upload-preview",
        &api_url("watchface/uploadPreviewImgMi7"),
        Client::new()
            .post(&api_url("watchface/uploadPreviewImgMi7"))
            .headers(vec![
                ("Accept", "application/json".to_string()),
                ("type", device_type.to_string()),
            ])
            .multipart(Form::new().part(part))
            .connect_timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS)),
    )?;

    let token = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("上传预览图成功，但服务端没有返回预览标识"))?;
    Ok(Some(token.to_string()))
}

fn build_upload_headers(
    config: &ProviderConfig,
    auth: &AuthContext,
    device_type: &str,
) -> Vec<(&'static str, String)> {
    let mut headers = base_headers_for_type(config, device_type, Some(auth));
    headers.push(("Accept", "application/json".to_string()));
    headers.push(("token", auth.valid_token.clone()));
    headers
}

fn api_multipart_request(label: &str, url: &str, builder: waki::RequestBuilder) -> Result<Value> {
    tracing::info!("api_multipart_request start: label={label} url={url}");
    let started_at = Instant::now();
    let response = builder.send().with_context(|| "request failed")?;
    let status = response.status_code();
    let body = response
        .body()
        .with_context(|| "failed to read response body")?;
    let body_str = String::from_utf8_lossy(&body).to_string();
    tracing::info!(
        "api_multipart_request response: label={label} status={} elapsed_ms={} body_len={} body_preview={}",
        status,
        started_at.elapsed().as_millis(),
        body.len(),
        body_str.chars().take(400).collect::<String>()
    );

    if status >= 400 {
        return Err(anyhow!("http {status}: {body_str}"));
    }

    let value: Value = serde_json::from_slice(&body)
        .with_context(|| format!("failed to decode response body as JSON: {body_str}"))?;

    if let Some(code) = value.get("code").and_then(Value::as_i64) {
        let message = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("unknown api error");
        return match code {
            0 => Ok(value.get("data").cloned().unwrap_or(Value::Null)),
            -1 | 9991 | 9999 => Err(anyhow!(message.to_string())),
            other => Err(anyhow!("api error {other}: {message}")),
        };
    }

    Ok(value)
}
