use super::*;

#[derive(Clone, Debug)]
struct LoginResponse {
    openid: String,
    valid_token: String,
    nickname: Option<String>,
}

pub(crate) fn login_with_mitan(config: &ProviderConfig, payload: LoginPayload) -> Result<AuthContext> {
    let path = match payload.kind {
        LoginKind::UsernamePassword => "watchface/my/loginMitan",
        LoginKind::OauthCode => "watchface/my/loginByMitanTokenNew2",
    };
    tracing::info!(
        "login_with_mitan enter: kind={:?} path={} device_type={}",
        payload.kind,
        path,
        config.device_type()
    );

    let request = Client::new()
        .post(&api_url(path))
        .headers(base_headers(config, None))
        .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS));

    let request = match payload.kind {
        LoginKind::UsernamePassword => request.form([
            ("name", payload.name.unwrap_or_default()),
            ("password", payload.password.unwrap_or_default()),
        ]),
        LoginKind::OauthCode => request.form([
            ("name", String::new()),
            ("password", String::new()),
            ("code", payload.code.unwrap_or_default()),
        ]),
    };

    let login = parse_login_response(api_request("mitan-login", &api_url(path), request)?)?;
    tracing::info!(
        "login_with_mitan success: openid={} has_nickname={}",
        login.openid,
        login.nickname.is_some()
    );
    Ok(AuthContext {
        openid: login.openid,
        valid_token: login.valid_token,
        nickname: login.nickname,
        donation: config.donation.clone().unwrap_or_else(|| "0".to_string()),
    })
}

fn parse_login_response(value: Value) -> Result<LoginResponse> {
    let openid = pick_login_string(&value, &["openid", "openId"])
        .ok_or_else(|| anyhow!("login response missing openid"))?;
    let valid_token = pick_login_string(&value, &["valid_token", "validToken", "validtoken", "token"])
        .ok_or_else(|| anyhow!("login response missing valid_token"))?;
    let nickname = pick_login_string(&value, &["nickname", "nickName", "username", "name"]);

    Ok(LoginResponse {
        openid,
        valid_token,
        nickname,
    })
}

fn pick_login_string(value: &Value, keys: &[&str]) -> Option<String> {
    let root = value.as_object()?;
    if let Some(found) = pick_string(root, keys) {
        return Some(found);
    }

    for nested_key in ["user", "userinfo", "account", "profile", "result"] {
        if let Some(object) = value.get(nested_key).and_then(Value::as_object) {
            if let Some(found) = pick_string(object, keys) {
                return Some(found);
            }
        }
    }

    None
}

pub fn list_supported_devices_for_ui() -> Result<Vec<UiDeviceChoice>> {
    Ok(fetch_device_list()?
        .into_iter()
        .map(|device| UiDeviceChoice {
            model: device.model,
            name: device.name,
        })
        .collect())
}

pub fn get_account_snapshot_for_ui() -> UiAccountSnapshot {
    let devices =
        fetch_device_list().unwrap_or_else(|_| filter_supported_devices(fallback_device_list()));
    let guard = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let selected_device = normalize_ui_device_type(&guard.config.device_type.clone(), &devices);
    let device_name = devices
        .iter()
        .find(|device| device.model.eq_ignore_ascii_case(&selected_device))
        .map(|device| device.name.clone())
        .unwrap_or_else(|| selected_device.clone());

    UiAccountSnapshot {
        logged_in: guard.auth.is_some(),
        device_type: selected_device,
        device_name,
        nickname: guard
            .auth
            .as_ref()
            .and_then(|auth| auth.nickname.clone())
            .or_else(|| guard.config.nickname.clone()),
        openid_masked: guard
            .auth
            .as_ref()
            .map(|auth| mask_openid(&auth.openid))
            .or_else(|| guard.config.openid.as_deref().map(mask_openid)),
        donation: guard
            .auth
            .as_ref()
            .map(|auth| auth.donation.clone())
            .or_else(|| guard.config.donation.clone())
            .unwrap_or_else(|| "0".to_string()),
        last_error: guard.last_login_error.clone(),
    }
}

pub fn set_selected_device_for_ui(device_type: &str) -> Result<()> {
    let devices =
        fetch_device_list().unwrap_or_else(|_| filter_supported_devices(fallback_device_list()));
    let normalized = normalize_ui_device_type(&Some(device_type.to_string()), &devices);
    let mut guard = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.config.device_type = Some(normalized);
    guard.config.normalize();
    save_session_locked(&guard);
    Ok(())
}

pub fn login_with_password_for_ui(
    device_type: &str,
    username: &str,
    password: &str,
) -> Result<UiAccountSnapshot> {
    let devices =
        fetch_device_list().unwrap_or_else(|_| filter_supported_devices(fallback_device_list()));
    let normalized_device = normalize_ui_device_type(&Some(device_type.to_string()), &devices);
    let mut config = current_config();
    config.device_type = Some(normalized_device);
    config.username = Some(username.trim().to_string());
    config.password = Some(password.to_string());
    config.normalize();
    let auth = login_with_mitan(
        &config,
        LoginPayload {
            kind: LoginKind::UsernamePassword,
            name: Some(username.trim().to_string()),
            password: Some(password.to_string()),
            code: None,
        },
    )?;

    let mut guard = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.config = config;
    guard.auth = Some(auth);
    guard.last_login_error = None;
    save_session_locked(&guard);
    drop(guard);
    Ok(get_account_snapshot_for_ui())
}

pub fn login_with_oauth_code_for_ui(
    device_type: &str,
    raw_code: &str,
) -> Result<UiAccountSnapshot> {
    let devices =
        fetch_device_list().unwrap_or_else(|_| filter_supported_devices(fallback_device_list()));
    let normalized_device = normalize_ui_device_type(&Some(device_type.to_string()), &devices);
    let code = extract_oauth_code(raw_code);
    if code.is_empty() {
        return Err(anyhow!("请输入授权完成后的 code 或回调链接"));
    }

    let mut config = current_config();
    config.device_type = Some(normalized_device);
    config.mitan_code = Some(code.clone());
    config.normalize();
    let auth = login_with_mitan(
        &config,
        LoginPayload {
            kind: LoginKind::OauthCode,
            name: None,
            password: None,
            code: Some(code),
        },
    )?;

    let mut guard = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.config = config;
    guard.auth = Some(auth);
    guard.last_login_error = None;
    save_session_locked(&guard);
    drop(guard);
    Ok(get_account_snapshot_for_ui())
}

pub fn apply_manual_session_for_ui(
    device_type: &str,
    openid: &str,
    valid_token: &str,
    nickname: Option<&str>,
) -> Result<UiAccountSnapshot> {
    let openid = openid.trim();
    let valid_token = valid_token.trim();
    if openid.is_empty() || valid_token.is_empty() {
        return Err(anyhow!("openid 和 valid_token 不能为空"));
    }

    let devices =
        fetch_device_list().unwrap_or_else(|_| filter_supported_devices(fallback_device_list()));
    let normalized_device = normalize_ui_device_type(&Some(device_type.to_string()), &devices);
    let nickname = nickname
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mut config = current_config();
    config.device_type = Some(normalized_device);
    config.openid = Some(openid.to_string());
    config.valid_token = Some(valid_token.to_string());
    config.nickname = nickname.clone();
    config.normalize();
    let auth = AuthContext {
        openid: openid.to_string(),
        valid_token: valid_token.to_string(),
        nickname,
        donation: config.donation.clone().unwrap_or_else(|| "0".to_string()),
    };

    let mut guard = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.config = config;
    guard.auth = Some(auth);
    guard.last_login_error = None;
    save_session_locked(&guard);
    drop(guard);
    Ok(get_account_snapshot_for_ui())
}

pub fn logout_for_ui() {
    let mut guard = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.auth = None;
    guard.last_login_error = None;
    guard.config.openid = None;
    guard.config.valid_token = None;
    guard.config.mitan_code = None;
    guard.config.password = None;
    guard.config.nickname = None;
    save_session_locked(&guard);
}

pub fn build_oauth_login_url_for_ui() -> String {
    let state = md5_hex(&(unix_time_ms().to_string() + PROVIDER_NAME));
    format!(
        "https://www.bandbbs.cn/oauth2/authorize?type=authorization_code&client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
        OAUTH_CLIENT_ID,
        url_encode(OAUTH_REDIRECT_URI),
        url_encode(OAUTH_SCOPE),
        state
    )
}
