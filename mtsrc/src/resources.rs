use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey, pkcs8::DecodePublicKey};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use waki::{
    Client,
    multipart::{Form, Part},
};

use crate::astrobox::psys_host::{os, provider_callback};

#[path = "resources/auth.rs"]
mod auth;
#[path = "resources/provider.rs"]
mod provider;
#[path = "resources/upload.rs"]
mod upload;

pub use self::auth::{
    apply_manual_session_for_ui, build_oauth_login_url_for_ui, get_account_snapshot_for_ui,
    list_supported_devices_for_ui, login_with_oauth_code_for_ui, login_with_password_for_ui,
    logout_for_ui, set_selected_device_for_ui,
};
pub use self::provider::handle_provider_action;
pub use self::upload::{
    delete_my_upload_for_ui, fetch_my_uploads_for_ui, fetch_upload_tips_for_ui,
    query_my_upload_reason_for_ui, submit_upload_for_ui, toggle_my_upload_share_for_ui,
    top_my_upload_for_ui,
};

pub const PROVIDER_NAME: &str = "givemefive-community";

const API_BASE_URL: &str = "https://www.mibandtool.club:9073/";
const API_JSON_BASE_URL: &str = "https://res.mibandtool.club:9073/";
const APP_CLIENT_VERSION: u32 = 3082;
const DEFAULT_DEVICE_TYPE: &str = "mi7";
const DEFAULT_MODEL: &str = "Xiaomi";
const DEFAULT_LANG: &str = "zh";
const EXTRACTED_MODEL2_CRC32: &str = "2249322543";
const EXTRACTED_SIGNATURE_SHA1: &str = "34aa96a18157f401eced9e045749ba63b0caf6b0";
const ACTIVITY_UTIL_PUBLIC_KEY_HEX: &str =
    "305C300D06092A864886F70D0101010500034B003048024100ADB8FA1B53DE4FB503463266CB78AC0D9C8565BE8A6A00223B89172B03D88F2404811A0191B044A4DD4A0BDA4A186826D1F10888AEDFF388A1DEE2A998CBA6110203010001";
const FALLBACK_ANDROID_ID_PLAINTEXT: &str = "62b36b9268c53666";
const REQUEST_TIMEOUT_SECS: u64 = 20;
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;
const SEARCH_SERVER_PAGE_SIZE: usize = 10;
const AGGREGATE_DEVICE_PAGE_SIZE: usize = 12;
const DEFAULT_CATEGORY_ID: i64 = 9999;
const DEVICE_CATEGORY_PREFIX: &str = "设备:";
const RESOURCE_CACHE_TTL_MS: u128 = 5 * 60 * 1000;
const SESSION_FILE: &str = "session.json";
const OAUTH_CLIENT_ID: &str = "6253518017122039";
const OAUTH_REDIRECT_URI: &str = "https://api.bandbbs.cn/wftools/bandbbs.html";
const OAUTH_SCOPE: &str = "user:read user:write resource_check:read resource:read";
const FALLBACK_TAGS: &[(i64, &str)] = &[
    (9999, "全部"),
    (1, "游戏"),
    (2, "软件"),
    (3, "工具"),
    (4, "动态表盘"),
    (5, "精选表盘"),
    (6, "表盘"),
    (7, "电子书"),
    (0, "未分类"),
];
const FALLBACK_DEVICES: &[(&str, &str)] = &[
    ("mi7", "小米手环7"),
    ("mi8", "小米手环8"),
    ("mi7pro", "小米手环7 Pro"),
    ("mi8pro", "小米手环8 Pro"),
    ("ws3", "小米手表S3/S4 Sport"),
    ("rw4", "红米手表4"),
    ("N66", "小米手环9"),
    ("N67", "小米手环9 Pro"),
    ("O62", "小米手表S4"),
    ("o65", "红米手表5"),
    ("o66", "小米手环10"),
    ("o67", "小米手环10 Pro"),
    ("p65", "红米手表6"),
    ("P62", "小米手表S5 46mm"),
    ("mi5", "小米手环5"),
    ("mi4", "小米手环4"),
    ("gtr47", "Amazfit GTR 47mm"),
    ("gvlite", "小米手表 Lite"),
];
const HIDDEN_DEVICE_MODELS: &[&str] = &[
    "mi7", "mi8", "mi7pro", "mi8pro", "rw4", "mi5", "mi4", "gtr47", "gvlite",
];
const HIDDEN_DEVICE_NAMES: &[&str] = &[
    "小米手环7",
    "小米手环8",
    "小米手环7 Pro",
    "小米手环8 Pro",
    "红米手表4",
    "小米手环5",
    "小米手环4",
    "Amazfit GTR 47mm",
    "小米手表 Lite",
];

#[derive(Default, Clone, Debug)]
struct ProviderState {
    config: ProviderConfig,
    auth: Option<AuthContext>,
    last_login_error: Option<String>,
}

#[derive(Default, Clone, Debug, Serialize)]
struct ProviderConfig {
    device_type: Option<String>,
    username: Option<String>,
    password: Option<String>,
    mitan_code: Option<String>,
    openid: Option<String>,
    valid_token: Option<String>,
    nickname: Option<String>,
    donation: Option<String>,
    limit_mac: Option<String>,
    use_donor_download: Option<bool>,
    did: Option<String>,
    model: Option<String>,
    model2: Option<String>,
    model3: Option<String>,
    lang: Option<String>,
    app_signature_sha1: Option<String>,
}

#[derive(Clone, Debug)]
struct AuthContext {
    openid: String,
    valid_token: String,
    nickname: Option<String>,
    donation: String,
}

/// Login credentials + user preferences persisted to disk so that a successful
/// login survives plugin reloads. Stored as JSON in [`SESSION_FILE`] within the
/// plugin working directory (the process current directory).
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
struct PersistedSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    openid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    valid_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nickname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    donation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    use_donor_download: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ProviderActionEnvelope {
    version: u64,
    provider: String,
    action: String,
    #[serde(default, rename = "requestId")]
    request_id: Option<String>,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshParams {
    #[serde(default)]
    config_raw: Option<String>,
    #[serde(default)]
    config: Option<Value>,
}

#[derive(Debug, Deserialize, Default)]
struct IndexParams {
    #[serde(default)]
    page: usize,
    #[serde(default)]
    limit: usize,
    #[serde(default)]
    search: SearchParams,
}

#[derive(Debug, Deserialize, Default)]
struct SearchParams {
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    category: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestParams {
    item_id: String,
}

#[derive(Debug, Deserialize)]
struct DownloadParams {
    #[serde(rename = "itemId")]
    item_id: String,
    #[serde(default)]
    device: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WatchfaceItem {
    id: i64,
    #[serde(default)]
    nickname: Option<String>,
    name: String,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    download_times: Option<u64>,
    #[serde(default)]
    preview: Option<String>,
    #[serde(default)]
    preview_aod: Option<String>,
    #[serde(default)]
    created_at: Option<i64>,
    #[serde(default)]
    updated_at: Option<i64>,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    views: Option<u64>,
    #[serde(default)]
    is_recommend: Option<i64>,
    #[serde(default)]
    donation: Option<String>,
    #[serde(default)]
    is_tag: Option<i64>,
    #[serde(default)]
    filesize: Option<u64>,
    #[serde(default)]
    mitantid: Option<String>,
    #[serde(default)]
    mitantype: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct CommunityConfigResponse {
    #[serde(default)]
    tags: Vec<TagInfo>,
    #[serde(default, rename = "deviceList")]
    device_list: Vec<DeviceInfo>,
}

#[derive(Clone, Debug, Deserialize)]
struct TagInfo {
    id: i64,
    name: String,
}

#[derive(Clone, Debug, Deserialize)]
struct DeviceInfo {
    model: String,
    name: String,
}

#[derive(Clone, Debug)]
pub struct UiDeviceChoice {
    pub model: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct UiAccountSnapshot {
    pub logged_in: bool,
    pub device_type: String,
    pub device_name: String,
    pub nickname: Option<String>,
    pub openid_masked: Option<String>,
    pub donation: String,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct UiBinaryAsset {
    pub name: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct UiUploadRequest {
    pub device_type: String,
    pub name: String,
    pub description: String,
    pub static_png: bool,
    pub update_id: Option<String>,
    pub mitantid: Option<String>,
    pub mitantype: String,
    pub preview_main: Option<UiBinaryAsset>,
    pub preview_aod: Option<UiBinaryAsset>,
    pub preview_aod2: Option<UiBinaryAsset>,
    pub preview_aod3: Option<UiBinaryAsset>,
    pub watchface_file: Option<UiBinaryAsset>,
}

#[derive(Clone, Debug)]
pub struct UiMyShareItem {
    pub id: i64,
    pub display_name: String,
    pub name: String,
    pub description: String,
    pub device_type: String,
    pub device_name: String,
    pub preview_url: String,
    pub preview_aod_url: Option<String>,
    pub preview_aod2_url: Option<String>,
    pub preview_aod3_url: Option<String>,
    pub is_share: bool,
    pub is_review: i64,
    pub download_times: u64,
    pub updated_at: Option<i64>,
    pub created_at: Option<i64>,
    pub mitantid: Option<String>,
    pub mitantype: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MyShareItemRecord {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default, alias = "description")]
    desc: String,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default, alias = "previewImg")]
    preview: Option<String>,
    #[serde(default)]
    preview_aod: Option<String>,
    #[serde(default)]
    preview_aod2: Option<String>,
    #[serde(default)]
    preview_aod3: Option<String>,
    #[serde(default)]
    download_times: Option<u64>,
    #[serde(default)]
    updated_at: Option<i64>,
    #[serde(default)]
    created_at: Option<i64>,
    #[serde(default)]
    is_share: Option<i64>,
    #[serde(default)]
    is_review: Option<i64>,
    #[serde(default)]
    mitantid: Option<String>,
    #[serde(default)]
    mitantype: Option<String>,
}

static STATE: OnceLock<Mutex<ProviderState>> = OnceLock::new();
static RESOURCE_CACHE: OnceLock<Mutex<ResourceCacheState>> = OnceLock::new();
static CLIENT_IDENTITY: OnceLock<ClientIdentity> = OnceLock::new();

fn state() -> &'static Mutex<ProviderState> {
    STATE.get_or_init(|| Mutex::new(build_initial_state()))
}

/// Build the in-memory state on first access, restoring any persisted login
/// session from disk so credentials survive plugin reloads.
fn build_initial_state() -> ProviderState {
    let mut state = ProviderState::default();
    if let Some(session) = load_persisted_session() {
        apply_persisted_session(&mut state, session);
    }
    state.config.normalize();
    state
}

fn load_persisted_session() -> Option<PersistedSession> {
    match std::fs::read_to_string(SESSION_FILE) {
        Ok(content) => match serde_json::from_str::<PersistedSession>(&content) {
            Ok(session) => {
                tracing::info!(
                    "persisted session loaded: device_type={:?} has_token={}",
                    session.device_type,
                    session.valid_token.is_some()
                );
                Some(session)
            }
            Err(err) => {
                tracing::warn!("failed to parse persisted session, ignoring: {err}");
                None
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            tracing::warn!("failed to read persisted session: {err}");
            None
        }
    }
}

fn apply_persisted_session(state: &mut ProviderState, session: PersistedSession) {
    if let Some(device_type) = session.device_type {
        state.config.device_type = Some(device_type);
    }
    state.config.use_donor_download = session.use_donor_download;

    if let (Some(openid), Some(valid_token)) = (session.openid, session.valid_token) {
        let donation = session.donation.unwrap_or_else(|| "0".to_string());
        let nickname = session.nickname;
        state.config.openid = Some(openid.clone());
        state.config.valid_token = Some(valid_token.clone());
        state.config.nickname = nickname.clone();
        state.config.donation = Some(donation.clone());
        state.auth = Some(AuthContext {
            openid,
            valid_token,
            nickname,
            donation,
        });
        tracing::info!("restored persisted login session");
    }
}

/// Persist the credential-bearing parts of the current state to disk. The caller
/// must hold the state lock. Failures are logged but never propagated, so a
/// read-only filesystem degrades to in-memory-only behaviour.
fn save_session_locked(state: &ProviderState) {
    let auth = state.auth.as_ref();
    let session = PersistedSession {
        device_type: state.config.device_type.clone(),
        openid: auth.map(|auth| auth.openid.clone()),
        valid_token: auth.map(|auth| auth.valid_token.clone()),
        nickname: auth.and_then(|auth| auth.nickname.clone()),
        donation: auth.map(|auth| auth.donation.clone()),
        use_donor_download: state.config.use_donor_download,
    };

    let json = match serde_json::to_string_pretty(&session) {
        Ok(json) => json,
        Err(err) => {
            tracing::warn!("failed to serialize session for persistence: {err}");
            return;
        }
    };

    match std::fs::write(SESSION_FILE, json) {
        Ok(()) => tracing::info!("session persisted: logged_in={}", auth.is_some()),
        Err(err) => tracing::warn!("failed to persist session: {err}"),
    }
}

fn resource_cache() -> &'static Mutex<ResourceCacheState> {
    RESOURCE_CACHE.get_or_init(|| Mutex::new(ResourceCacheState::default()))
}

fn client_identity() -> &'static ClientIdentity {
    CLIENT_IDENTITY.get_or_init(build_client_identity)
}

pub fn clear_plugin_cache_for_ui() -> Result<()> {
    {
        let mut cache = resource_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.community_configs.clear();
        cache.aggregate_indexes.clear();
    }

    match std::fs::remove_dir_all("cache") {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(anyhow!("failed to remove plugin cache directory: {err}"));
        }
    }

    tracing::info!("plugin cache cleared");
    Ok(())
}

#[derive(Default)]
struct ResourceCacheState {
    community_configs: HashMap<String, CachedEntry<CommunityConfigResponse>>,
    aggregate_indexes: HashMap<AggregateIndexCacheKey, CachedEntry<AggregateIndexState>>,
}

#[derive(Clone)]
struct CachedEntry<T> {
    expires_at_ms: u128,
    value: T,
}

#[derive(Clone, Debug)]
struct ClientIdentity {
    did: String,
    model: String,
    model2: String,
    model3: String,
    app_signature_sha1: String,
}

#[derive(Clone, Debug)]
struct AggregateIndexState {
    items: Vec<WatchfaceItem>,
    next_page_by_device: HashMap<String, usize>,
    exhausted_devices: HashSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct AggregateIndexCacheKey {
    tag_id: i64,
    sort: ListMode,
    filter: Option<String>,
    device_types: Vec<String>,
}

fn now_ms() -> u128 {
    unix_time_ms()
}

fn build_client_identity() -> ClientIdentity {
    let app_signature_sha1 = EXTRACTED_SIGNATURE_SHA1.to_string();
    let model = derive_default_model();
    let raw_android_id = derive_android_id_plaintext();
    let did =
        build_default_client_did(&raw_android_id).unwrap_or_else(|| raw_android_id.to_ascii_uppercase());

    ClientIdentity {
        did,
        model,
        model2: EXTRACTED_MODEL2_CRC32.to_string(),
        model3: md5_hex(&(app_signature_sha1.clone() + "VVV")),
        app_signature_sha1,
    }
}

fn derive_default_model() -> String {
    for value in [
        read_host_string(|| os::hostname().into_future()),
        read_host_string(|| os::platform().into_future()),
        read_host_string(|| os::arch().into_future()),
    ] {
        if let Some(value) = value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .filter(|value| !contains_astrobox(Some(value.as_str())))
        {
            return value;
        }
    }
    DEFAULT_MODEL.to_string()
}

fn derive_android_id_plaintext() -> String {
    let seed_source = [
        read_host_string(|| os::hostname().into_future()),
        read_host_string(|| os::platform().into_future()),
        read_host_string(|| os::version().into_future()),
        read_host_string(|| os::arch().into_future()),
    ]
    .into_iter()
    .flatten()
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join("|");

    if seed_source.is_empty() {
        return FALLBACK_ANDROID_ID_PLAINTEXT.to_string();
    }

    let digest = md5::compute(seed_source.as_bytes());
    format!("{:x}", digest).chars().take(16).collect()
}

fn read_host_string<F, Fut>(future_factory: F) -> Option<String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = String>,
{
    let value = wit_bindgen::block_on(future_factory());
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn build_default_client_did(raw_android_id: &str) -> Option<String> {
    let public_key_der = hex_string_to_bytes(ACTIVITY_UTIL_PUBLIC_KEY_HEX)?;
    let public_key = RsaPublicKey::from_public_key_der(&public_key_der).ok()?;

    let left_seed = md5::compute(raw_android_id.as_bytes());
    let right_seed =
        md5::compute((raw_android_id.to_string() + EXTRACTED_SIGNATURE_SHA1).as_bytes());
    let mut seed = [0u8; 32];
    seed[..16].copy_from_slice(&left_seed.0);
    seed[16..].copy_from_slice(&right_seed.0);
    let mut rng = ChaCha20Rng::from_seed(seed);

    let encrypted = public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, raw_android_id.as_bytes())
        .ok()?;
    Some(bytes_to_upper_hex(&encrypted))
}

fn hex_string_to_bytes(input: &str) -> Option<Vec<u8>> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() % 2 != 0 {
        return None;
    }

    let mut bytes = Vec::with_capacity(trimmed.len() / 2);
    let raw = trimmed.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        bytes.push(decode_hex_pair(raw[index], raw[index + 1])?);
        index += 2;
    }
    Some(bytes)
}

fn bytes_to_upper_hex(input: &[u8]) -> String {
    let mut output = String::with_capacity(input.len() * 2);
    for byte in input {
        output.push_str(&format!("{byte:02X}"));
    }
    output
}

fn is_cache_entry_fresh<T>(entry: &CachedEntry<T>) -> bool {
    entry.expires_at_ms > now_ms()
}

fn get_index(params: IndexParams) -> Result<Vec<Value>> {
    let config = current_config();
    let requested_categories = params.search.category.as_deref().unwrap_or(&[]);
    let devices = fetch_device_list()?;
    let device_name_map = build_device_name_map(&devices);
    let selected_device_type = requested_categories
        .iter()
        .find_map(|category| match_device_category(category, &devices));
    let device_type = selected_device_type
        .clone()
        .unwrap_or_else(|| resolve_requested_device_type(&config, requested_categories, &devices));
    let categories = fetch_category_map_for(&device_type)?;
    let tag_id = resolve_category_id(&categories, requested_categories);
    let limit = params.limit.max(1);
    tracing::info!(
        "get_index resolved: device_type={} selected_device_type={:?} tag_id={} categories_count={} requested_categories={:?}",
        device_type,
        selected_device_type,
        tag_id,
        categories.len(),
        requested_categories
    );

    let items = if selected_device_type.is_some() {
        fetch_index_items_for_device(&params, &device_type, tag_id, limit)?
    } else {
        fetch_index_items_across_supported_devices(&params, &devices, tag_id, limit)?
    };
    tracing::info!("get_index fetched {} items", items.len());

    Ok(items
        .into_iter()
        .filter(|item| is_supported_watchface_item(item))
        .map(|item| build_manifest_item(&item, &categories, &device_name_map))
        .collect())
}

fn fetch_index_items_for_device(
    params: &IndexParams,
    device_type: &str,
    tag_id: i64,
    limit: usize,
) -> Result<Vec<WatchfaceItem>> {
    if let Some(filter) = params
        .search
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        search_watchfaces(filter, params.page, limit, device_type)
    } else {
        let mode = resolve_list_mode(params.search.sort.as_deref());
        fetch_watchface_page(mode, tag_id, params.page + 1, limit, device_type)
    }
}

fn fetch_index_items_across_supported_devices(
    params: &IndexParams,
    devices: &[DeviceInfo],
    tag_id: i64,
    limit: usize,
) -> Result<Vec<WatchfaceItem>> {
    let device_types = supported_device_types(devices);
    let sort_mode = resolve_list_mode(params.search.sort.as_deref());
    let filter = params
        .search
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let cache_key = AggregateIndexCacheKey {
        tag_id,
        sort: sort_mode,
        filter,
        device_types: device_types
            .iter()
            .map(|device_type| device_type.to_ascii_lowercase())
            .collect(),
    };
    let target_count = params.page.saturating_mul(limit).saturating_add(limit);

    let mut state = {
        let cache = resource_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache
            .aggregate_indexes
            .get(&cache_key)
            .filter(|entry| is_cache_entry_fresh(entry))
            .map(|entry| {
                tracing::info!(
                    "aggregate index cache hit: sort={:?} filter={:?} tag_id={} devices={} cached_items={}",
                    cache_key.sort,
                    cache_key.filter,
                    tag_id,
                    device_types.len(),
                    entry.value.items.len()
                );
                entry.value.clone()
            })
            .unwrap_or_else(|| {
                tracing::info!(
                    "aggregate index cache miss: sort={:?} filter={:?} tag_id={} devices={}",
                    cache_key.sort,
                    cache_key.filter,
                    tag_id,
                    device_types.len()
                );
                new_aggregate_index_state(&device_types)
            })
    };

    while state.items.len() < target_count
        && state.exhausted_devices.len() < device_types.len()
    {
        let before_len = state.items.len();
        extend_aggregate_index_state(&mut state, &cache_key, tag_id, &device_types)?;
        if state.items.len() == before_len {
            break;
        }
    }

    {
        let mut cache = resource_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.aggregate_indexes.insert(
            cache_key,
            CachedEntry {
                expires_at_ms: now_ms().saturating_add(RESOURCE_CACHE_TTL_MS),
                value: state.clone(),
            },
        );
    }

    let offset = params.page.saturating_mul(limit);
    Ok(state.items.into_iter().skip(offset).take(limit).collect())
}

fn new_aggregate_index_state(device_types: &[String]) -> AggregateIndexState {
    let next_page_by_device = device_types
        .iter()
        .map(|device_type| (device_type.to_ascii_lowercase(), 1usize))
        .collect();
    AggregateIndexState {
        items: Vec::new(),
        next_page_by_device,
        exhausted_devices: HashSet::new(),
    }
}

fn extend_aggregate_index_state(
    state: &mut AggregateIndexState,
    cache_key: &AggregateIndexCacheKey,
    tag_id: i64,
    device_types: &[String],
) -> Result<()> {
    let mut appended = false;

    for device_type in device_types {
        let normalized_device = device_type.to_ascii_lowercase();
        if state.exhausted_devices.contains(&normalized_device) {
            continue;
        }

        let page = state
            .next_page_by_device
            .get(&normalized_device)
            .copied()
            .unwrap_or(1);
        tracing::info!(
            "aggregate index fetching device page: device_type={} page={} filter={:?}",
            device_type,
            page,
            cache_key.filter
        );
        let mut fetched = if let Some(filter) = cache_key.filter.as_deref() {
            search_watchfaces_server_page(filter, page, device_type)?
        } else {
            fetch_watchface_page(
                cache_key.sort,
                tag_id,
                page,
                AGGREGATE_DEVICE_PAGE_SIZE,
                device_type,
            )?
        };

        if fetched.is_empty() {
            state.exhausted_devices.insert(normalized_device.clone());
            continue;
        }

        let expected_page_size = if cache_key.filter.is_some() {
            SEARCH_SERVER_PAGE_SIZE
        } else {
            AGGREGATE_DEVICE_PAGE_SIZE
        };
        if fetched.len() < expected_page_size {
            state.exhausted_devices.insert(normalized_device.clone());
        }

        state
            .next_page_by_device
            .insert(normalized_device, page.saturating_add(1));
        state.items.append(&mut fetched);
        appended = true;
    }

    if appended {
        dedupe_watchface_items(&mut state.items);
        sort_watchface_items(&mut state.items, Some(cache_key.sort.as_str()));
    }

    Ok(())
}

fn dedupe_watchface_items(items: &mut Vec<WatchfaceItem>) {
    let mut seen = HashSet::new();
    items.retain(|item| {
        let key = format!(
            "{}:{}",
            item.id,
            item.r#type.as_deref().unwrap_or(DEFAULT_DEVICE_TYPE)
        );
        seen.insert(key)
    });
}

fn get_manifest(item_id: &str) -> Result<Value> {
    tracing::info!("get_manifest enter: item_id={item_id}");
    let item = fetch_watchface_detail(item_id)?;
    ensure_supported_watchface_item(&item)?;
    let devices = fetch_device_list()?;
    let device_name_map = build_device_name_map(&devices);
    let categories = fetch_category_map_for(item.r#type.as_deref().unwrap_or(DEFAULT_DEVICE_TYPE))?;
    let aod_preview = fetch_aod_preview(item.id).ok().flatten();
    tracing::info!(
        "get_manifest loaded: item_id={} name={} categories_count={} has_aod={}",
        item.id,
        item.name,
        categories.len(),
        aod_preview.is_some()
    );
    Ok(build_manifest(&item, &categories, &device_name_map, aod_preview))
}

fn get_total() -> Result<u64> {
    tracing::info!("get_total enter");
    let value = api_request(
        "watchface-total-generated",
        &api_url("watchface/total/genrated"),
        Client::new()
            .post(&api_url("watchface/total/genrated"))
            .headers(base_headers(&current_config(), None))
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?;

    let total = value_to_u64(&value).ok_or_else(|| anyhow!("unexpected total payload: {value}"))?;
    tracing::info!("get_total success: total={total}");
    Ok(total)
}

fn download_watchface(
    item_id: &str,
    _device: Option<&str>,
    request_id: Option<&str>,
) -> Result<String> {
    tracing::info!("download_watchface enter: item_id={item_id}");
    provider::notify_provider_action_progress(request_id, 0.0, "preparing");
    let item = fetch_watchface_detail(item_id)?;
    ensure_supported_watchface_item(&item)?;
    let guard = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let endpoint = if guard
        .config
        .use_donor_download
        .unwrap_or_else(|| guard.auth.as_ref().is_some_and(|auth| auth.donation == "1"))
    {
        "watchface/downloadUsr"
    } else {
        "watchface/downloadCom"
    };
    let headers = base_headers(&guard.config, guard.auth.as_ref());
    drop(guard);
    tracing::info!("download_watchface endpoint selected: {endpoint}");

    let download_url_value = api_request(
        "watchface-download-url",
        &api_url(endpoint),
        Client::new()
            .post(&api_url(endpoint))
            .headers(headers)
            .form([("id", item.id.to_string())])
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?;
    let download_url = download_url_value
        .as_str()
        .ok_or_else(|| anyhow!("download endpoint did not return a string"))?;
    tracing::info!("download_watchface url acquired: {}", download_url);
    provider::notify_provider_action_progress(request_id, 0.1, "got-download-url");

    let started_at = Instant::now();
    let response = Client::new()
        .get(download_url)
        .connect_timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .send()
        .with_context(|| format!("failed to fetch binary from {download_url}"))?;
    let total = response
        .header("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let mut bytes = Vec::with_capacity(total.unwrap_or_default() as usize);
    let mut downloaded = 0u64;
    let mut last_emit = Instant::now();

    while let Some(chunk) = response
        .chunk(256 * 1024)
        .with_context(|| "failed to read binary response chunk")?
    {
        downloaded += chunk.len() as u64;
        bytes.extend_from_slice(&chunk);

        let should_emit = last_emit.elapsed() >= Duration::from_millis(120)
            || total.map(|total_len| downloaded >= total_len).unwrap_or(false);
        if should_emit {
            let progress = match total {
                Some(total_len) if total_len > 0 => {
                    0.1 + ((downloaded as f32 / total_len as f32).clamp(0.0, 1.0) * 0.8)
                }
                _ => 0.5,
            };
            provider::notify_provider_action_progress(request_id, progress, "downloading");
            last_emit = Instant::now();
        }
    }
    tracing::info!(
        "download_watchface binary fetched: {} bytes in {} ms",
        bytes.len(),
        started_at.elapsed().as_millis()
    );
    provider::notify_provider_action_progress(request_id, 0.92, "encoding");

    let file_name = build_download_file_name(&item, download_url);
    let encoded = BASE64_STANDARD.encode(bytes);
    let payload = json!({
        "kind": "base64",
        "fileName": file_name,
        "data": encoded,
    })
    .to_string();
    provider::notify_provider_action_progress(request_id, 1.0, "finished");
    Ok(payload)
}

fn fetch_watchface_page(
    mode: ListMode,
    tag_id: i64,
    page: usize,
    limit: usize,
    device_type: &str,
) -> Result<Vec<WatchfaceItem>> {
    let config = current_config();
    let path = match mode {
        ListMode::Latest => format!("watchface/listbytag/0/{page}/{limit}/{tag_id}"),
        ListMode::Hot => format!("watchface/listbytag/1/{page}/{limit}/{tag_id}"),
        ListMode::Recommend => format!("watchface/list/recommendsbytag/{page}/{limit}/{tag_id}"),
    };
    tracing::info!(
        "fetch_watchface_page: mode={mode:?} page={} limit={} tag_id={} device_type={} path={}",
        page,
        limit,
        tag_id,
        device_type,
        path
    );

    let items: Vec<WatchfaceItem> = serde_json::from_value(api_request(
        "watchface-page",
        &api_url(&path),
        Client::new()
            .get(&api_url(&path))
            .headers(base_headers_for_type(&config, device_type, None))
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?)?;
    tracing::info!("fetch_watchface_page success: {} items", items.len());
    Ok(items)
}

fn search_watchfaces(
    keyword: &str,
    page: usize,
    limit: usize,
    device_type: &str,
) -> Result<Vec<WatchfaceItem>> {
    let offset = page.saturating_mul(limit);
    let start_server_page = offset / SEARCH_SERVER_PAGE_SIZE + 1;
    let required_end = offset + limit;
    let end_server_page = required_end
        .div_ceil(SEARCH_SERVER_PAGE_SIZE)
        .max(start_server_page);
    tracing::info!(
        "search_watchfaces enter: keyword={keyword:?} page={} limit={} device_type={} start_server_page={} end_server_page={}",
        page,
        limit,
        device_type,
        start_server_page,
        end_server_page
    );

    let mut all_items = Vec::new();
    for server_page in start_server_page..=end_server_page {
        tracing::info!("search_watchfaces requesting server_page={server_page}");
        let mut page_items =
            search_watchfaces_server_page(keyword, server_page, device_type)?;
        let reached_end = page_items.len() < SEARCH_SERVER_PAGE_SIZE;
        tracing::info!(
            "search_watchfaces server_page={} returned {} items reached_end={}",
            server_page,
            page_items.len(),
            reached_end
        );
        all_items.append(&mut page_items);
        if reached_end {
            break;
        }
    }

    let slice_start = offset % SEARCH_SERVER_PAGE_SIZE;
    let result: Vec<_> = all_items
        .into_iter()
        .skip(slice_start)
        .take(limit)
        .collect();
    tracing::info!("search_watchfaces final sliced count={}", result.len());
    Ok(result)
}

fn search_watchfaces_server_page(
    keyword: &str,
    server_page: usize,
    device_type: &str,
) -> Result<Vec<WatchfaceItem>> {
    let config = current_config();
    let items: Vec<WatchfaceItem> = serde_json::from_value(api_request(
        "watchface-search",
        &api_url("watchface/searchForPage"),
        Client::new()
            .post(&api_url("watchface/searchForPage"))
            .headers(base_headers_for_type(&config, device_type, None))
            .form([
                ("keyword", keyword.to_string()),
                ("page", server_page.to_string()),
            ])
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?)?;
    Ok(items)
}

fn fetch_watchface_detail(item_id: &str) -> Result<WatchfaceItem> {
    tracing::info!("fetch_watchface_detail enter: item_id={item_id}");
    let item: WatchfaceItem = serde_json::from_value(api_request(
        "watchface-detail",
        &api_url("watchface/get/by/id"),
        Client::new()
            .post(&api_url("watchface/get/by/id"))
            .headers(base_headers(&current_config(), None))
            .form([("id", item_id.to_string())])
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?)?;
    tracing::info!(
        "fetch_watchface_detail success: id={} name={} type={:?}",
        item.id,
        item.name,
        item.r#type
    );
    Ok(item)
}

fn fetch_aod_preview(item_id: i64) -> Result<Option<String>> {
    tracing::info!("fetch_aod_preview enter: item_id={item_id}");
    let value = api_request(
        "watchface-aod-preview",
        &api_url(&format!("watchface/work/getAodImgPath/{item_id}")),
        Client::new()
            .get(&api_url(&format!("watchface/work/getAodImgPath/{item_id}")))
            .headers(base_headers(&current_config(), None))
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?;

    let result = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    tracing::info!("fetch_aod_preview success: has_value={}", result.is_some());
    Ok(result)
}

fn fetch_categories() -> Result<Vec<String>> {
    tracing::info!("fetch_categories enter");
    let config = current_config();
    let devices = fetch_device_list()?;
    let tags = fetch_tag_list_for(&config.device_type())?;
    let mut categories = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for device in devices {
        let label = format!("{DEVICE_CATEGORY_PREFIX}{}", device.name.trim());
        if !device.name.trim().is_empty() && seen.insert(label.clone()) {
            categories.push(label);
        }
    }

    for tag in tags {
        let name = tag.name.trim();
        if !name.is_empty() && seen.insert(name.to_string()) {
            categories.push(name.to_string());
        }
    }
    tracing::info!("fetch_categories success: count={}", categories.len());
    Ok(categories)
}

fn fetch_category_map_for(device_type: &str) -> Result<HashMap<i64, String>> {
    tracing::info!("fetch_category_map enter: device_type={device_type}");
    let tags = fetch_tag_list_for(device_type)?;
    let map: HashMap<_, _> = tags.into_iter().map(|tag| (tag.id, tag.name)).collect();
    tracing::info!(
        "fetch_category_map success: device_type={} count={}",
        device_type,
        map.len()
    );
    Ok(map)
}

fn fetch_tag_list_for(device_type: &str) -> Result<Vec<TagInfo>> {
    match fetch_community_config(device_type) {
        Ok(config) if !config.tags.is_empty() => {
            tracing::info!(
                "fetch_tag_list remote success: device_type={} count={}",
                device_type,
                config.tags.len()
            );
            Ok(config.tags)
        }
        Ok(_) => {
            tracing::warn!(
                "fetch_tag_list remote config missing tags, using fallback: device_type={}",
                device_type
            );
            Ok(fallback_tag_list())
        }
        Err(error) => {
            tracing::warn!(
                "fetch_tag_list remote failed, using fallback: device_type={} error={error:#}",
                device_type
            );
            Ok(fallback_tag_list())
        }
    }
}

fn fetch_device_list() -> Result<Vec<DeviceInfo>> {
    match fetch_community_config(DEFAULT_DEVICE_TYPE) {
        Ok(config) if !config.device_list.is_empty() => {
            let mut devices = config.device_list;
            append_missing_fallback_devices(&mut devices);
            let devices = filter_supported_devices(devices);
            tracing::info!("fetch_device_list remote success: count={}", devices.len());
            Ok(devices)
        }
        Ok(_) => {
            tracing::warn!("fetch_device_list remote config missing device list, using fallback");
            Ok(filter_supported_devices(fallback_device_list()))
        }
        Err(error) => {
            tracing::warn!("fetch_device_list remote failed, using fallback: {error:#}");
            Ok(filter_supported_devices(fallback_device_list()))
        }
    }
}

fn fetch_community_config(device_type: &str) -> Result<CommunityConfigResponse> {
    let normalized_device_type = device_type.to_ascii_lowercase();
    if let Some(cached) = resource_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .community_configs
        .get(&normalized_device_type)
        .filter(|entry| is_cache_entry_fresh(entry))
        .map(|entry| entry.value.clone())
    {
        tracing::debug!(
            "fetch_community_config cache hit: device_type={} tags={} devices={}",
            device_type,
            cached.tags.len(),
            cached.device_list.len()
        );
        return Ok(cached);
    }

    let path = format!("config/json/configs_{device_type}");
    let url = api_json_url(&path);
    let config: CommunityConfigResponse = serde_json::from_value(api_request(
        "community-configs",
        &url,
        Client::new()
            .get(&url)
            .headers(base_headers_for_type(&current_config(), device_type, None))
            .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS)),
    )?)?;
    tracing::info!(
        "fetch_community_config success: device_type={} tags={} devices={}",
        device_type,
        config.tags.len(),
        config.device_list.len()
    );
    resource_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .community_configs
        .insert(
            normalized_device_type,
            CachedEntry {
                expires_at_ms: now_ms().saturating_add(RESOURCE_CACHE_TTL_MS),
                value: config.clone(),
            },
        );
    Ok(config)
}

fn fallback_tag_list() -> Vec<TagInfo> {
    FALLBACK_TAGS
        .iter()
        .map(|(id, name)| TagInfo {
            id: *id,
            name: (*name).to_string(),
        })
        .collect()
}

fn fallback_device_list() -> Vec<DeviceInfo> {
    FALLBACK_DEVICES
        .iter()
        .map(|(model, name)| DeviceInfo {
            model: (*model).to_string(),
            name: (*name).to_string(),
        })
        .collect()
}

fn append_missing_fallback_devices(devices: &mut Vec<DeviceInfo>) {
    for (model, name) in FALLBACK_DEVICES {
        let exists = devices
            .iter()
            .any(|item| item.model.eq_ignore_ascii_case(model));
        if !exists {
            devices.push(DeviceInfo {
                model: (*model).to_string(),
                name: (*name).to_string(),
            });
        }
    }
}

fn filter_supported_devices(devices: Vec<DeviceInfo>) -> Vec<DeviceInfo> {
    devices
        .into_iter()
        .filter(|device| !is_hidden_device_info(device))
        .collect()
}

fn supported_device_types(devices: &[DeviceInfo]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for device in devices {
        let model = device.model.trim();
        if model.is_empty() {
            continue;
        }
        let normalized = model.to_ascii_lowercase();
        if seen.insert(normalized) {
            result.push(model.to_string());
        }
    }
    result
}

fn resolve_requested_device_type(
    config: &ProviderConfig,
    categories: &[String],
    devices: &[DeviceInfo],
) -> String {
    for category in categories {
        if let Some(device_type) = match_device_category(category, devices) {
            tracing::info!(
                "resolve_requested_device_type matched category={:?} -> {}",
                category,
                device_type
            );
            return device_type;
        }
    }
    let configured = config.device_type();
    if devices
        .iter()
        .any(|device| device.model.eq_ignore_ascii_case(&configured))
    {
        configured
    } else {
        devices
            .first()
            .map(|device| device.model.clone())
            .unwrap_or(configured)
    }
}

fn match_device_category(category: &str, devices: &[DeviceInfo]) -> Option<String> {
    let trimmed = category.trim();
    if trimmed.is_empty() {
        return None;
    }

    let candidate = trimmed
        .strip_prefix(DEVICE_CATEGORY_PREFIX)
        .map(str::trim)
        .unwrap_or(trimmed);

    devices.iter().find_map(|device| {
        if device.model.eq_ignore_ascii_case(candidate) || device.name == candidate {
            Some(device.model.clone())
        } else {
            None
        }
    })
}

fn normalize_ui_device_type(selected: &Option<String>, devices: &[DeviceInfo]) -> String {
    let requested = selected
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(requested) = requested {
        if devices
            .iter()
            .any(|device| device.model.eq_ignore_ascii_case(requested))
        {
            return requested.to_string();
        }
    }

    devices
        .first()
        .map(|device| device.model.clone())
        .unwrap_or_else(|| DEFAULT_DEVICE_TYPE.to_string())
}

fn extract_oauth_code(raw_code: &str) -> String {
    let trimmed = raw_code.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(fragment) = trimmed.split('?').nth(1) {
        for pair in fragment.split('&') {
            if let Some(value) = pair.strip_prefix("code=") {
                return percent_decode(value);
            }
        }
    }
    if let Some(value) = trimmed.split("code=").nth(1) {
        return percent_decode(value.split('&').next().unwrap_or_default());
    }
    trimmed.to_string()
}

fn guess_image_mime(file_name: &str) -> &'static str {
    match file_name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

fn map_my_share_record(
    item: MyShareItemRecord,
    device_name_map: &HashMap<String, String>,
) -> UiMyShareItem {
    let watchface_like = WatchfaceItem {
        id: item.id,
        nickname: None,
        name: item.name.clone(),
        desc: Some(item.desc.clone()),
        download_times: item.download_times,
        preview: item.preview.clone(),
        preview_aod: item.preview_aod.clone(),
        created_at: item.created_at,
        updated_at: item.updated_at,
        r#type: item.r#type.clone(),
        views: None,
        is_recommend: None,
        donation: None,
        is_tag: None,
        filesize: None,
        mitantid: item.mitantid.clone(),
        mitantype: item.mitantype.clone(),
    };
    let device_type = item
        .r#type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DEVICE_TYPE)
        .to_string();
    let device_name = device_name_map
        .get(&device_type.to_ascii_lowercase())
        .cloned()
        .unwrap_or_else(|| device_type.clone());

    UiMyShareItem {
        id: item.id,
        display_name: build_display_name(&watchface_like, device_name_map),
        name: item.name,
        description: item.desc,
        device_type,
        device_name,
        preview_url: normalize_preview_reference(item.preview.as_deref()),
        preview_aod_url: normalize_preview_option(item.preview_aod.as_deref()),
        preview_aod2_url: normalize_preview_option(item.preview_aod2.as_deref()),
        preview_aod3_url: normalize_preview_option(item.preview_aod3.as_deref()),
        is_share: item.is_share.unwrap_or(0) == 1,
        is_review: item.is_review.unwrap_or(2),
        download_times: item.download_times.unwrap_or(0),
        updated_at: item.updated_at,
        created_at: item.created_at,
        mitantid: item.mitantid,
        mitantype: item.mitantype,
    }
}

fn normalize_preview_option(value: Option<&str>) -> Option<String> {
    let normalized = normalize_preview_reference(value);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn normalize_preview_reference(value: Option<&str>) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };
    if value.starts_with("http://") || value.starts_with("https://") {
        return value.to_string();
    }
    if value.starts_with("watchface/") {
        return format!("{}{}", API_BASE_URL, value);
    }
    format!("{}watchface/getPreviewImgMi7/{}", API_BASE_URL, value)
}

fn mask_openid(openid: &str) -> String {
    let trimmed = openid.trim();
    if trimmed.len() <= 8 {
        return trimmed.to_string();
    }
    let prefix = &trimmed[..4];
    let suffix = &trimmed[trimmed.len() - 4..];
    format!("{prefix}…{suffix}")
}

fn api_request(label: &str, url: &str, builder: waki::RequestBuilder) -> Result<Value> {
    tracing::info!("api_request start: label={label} url={url}");
    let started_at = Instant::now();
    let response = builder.send().with_context(|| "request failed")?;
    let status = response.status_code();
    let body = response
        .body()
        .with_context(|| "failed to read response body")?;
    let body_str = String::from_utf8_lossy(&body).to_string();
    tracing::info!(
        "api_request response: label={label} status={} elapsed_ms={} body_len={} body_preview={}",
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
        tracing::info!(
            "api_request envelope: label={label} code={} msg={}",
            code,
            message
        );
        return match code {
            0 => Ok(value.get("data").cloned().unwrap_or(Value::Null)),
            -1 | 9991 | 9999 => Err(anyhow!(message.to_string())),
            other => Err(anyhow!("api error {other}: {message}")),
        };
    }

    Ok(value)
}

fn build_manifest_item(
    item: &WatchfaceItem,
    category_map: &HashMap<i64, String>,
    device_name_map: &HashMap<String, String>,
) -> Value {
    let preview = item.preview.clone().unwrap_or_default();
    let previews = if preview.trim().is_empty() {
        Vec::<String>::new()
    } else {
        vec![preview.clone()]
    };
    let authors = build_authors(item.nickname.as_deref());
    let display_name = build_display_name(item, device_name_map);

    json!({
        "id": item.id.to_string(),
        "name": display_name,
        "description": build_index_description(item.desc.as_deref()),
        "preview": previews,
        "icon": preview,
        "cover": preview,
        "paid_type": resolve_paid_type(item),
        "restype": "watchface",
        "author": authors,
        "ext": build_index_ext(item, category_map)
    })
}

fn build_manifest(
    item: &WatchfaceItem,
    category_map: &HashMap<i64, String>,
    device_name_map: &HashMap<String, String>,
    aod_preview: Option<String>,
) -> Value {
    let mut previews = Vec::new();
    if let Some(preview) = item
        .preview
        .clone()
        .filter(|value| !value.trim().is_empty())
    {
        previews.push(preview.clone());
    }
    if let Some(preview) = item
        .preview_aod
        .clone()
        .filter(|value| !value.trim().is_empty())
        .filter(|preview| !previews.contains(preview))
    {
        previews.push(preview);
    }
    if let Some(preview) = aod_preview
        .filter(|value| !value.trim().is_empty())
        .filter(|preview| !previews.contains(preview))
    {
        previews.push(preview);
    }

    let preview = previews.first().cloned().unwrap_or_default();
    let mut links = Vec::new();
    if let Some(url) = build_mitan_url(item) {
        links.push(json!({
            "title": "米坛帖子",
            "url": url,
            "icon": "forum"
        }));
    }

    let authors = build_authors(item.nickname.as_deref());
    let device_type = item
        .r#type
        .clone()
        .unwrap_or_else(|| DEFAULT_DEVICE_TYPE.to_string());
    let display_name = build_display_name(item, device_name_map);
    let file_name = build_download_file_name(item, "");
    let tag_name = item
        .is_tag
        .and_then(|tag_id| category_map.get(&tag_id).cloned())
        .unwrap_or_else(|| "全部".to_string());

    json!({
        "item": {
            "id": item.id.to_string(),
            "restype": "watchface",
            "name": display_name,
            "description": item.desc.clone().unwrap_or_default(),
            "preview": previews,
            "icon": preview,
            "cover": preview,
            "paid_type": resolve_paid_type(item),
            "author": authors
        },
        "downloads": {
            device_type: {
                "version": item.updated_at.unwrap_or_default().to_string(),
                "file_name": file_name
            }
        },
        "links": links,
        "ext": {
            "provider": PROVIDER_NAME,
            "community": {
                "id": item.id,
                "deviceType": item.r#type,
                "tagId": item.is_tag,
                "tagName": tag_name,
                "downloads": item.download_times.unwrap_or(0),
                "views": item.views.unwrap_or(0),
                "filesize": item.filesize.unwrap_or(0),
                "createdAt": item.created_at,
                "updatedAt": item.updated_at,
                "isRecommend": item.is_recommend.unwrap_or(0) == 1,
                "donation": item.donation.clone().unwrap_or_default()
            }
        }
    })
}

fn build_authors(nickname: Option<&str>) -> Vec<Value> {
    nickname
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|name| {
            vec![json!({
                "name": name,
                "bindABAccount": false
            })]
        })
        .unwrap_or_default()
}

fn resolve_paid_type(item: &WatchfaceItem) -> Option<&'static str> {
    if item.donation.as_deref() == Some("2") {
        Some("force_paid")
    } else if item.donation.as_deref() == Some("1") {
        Some("paid")
    } else {
        None
    }
}

fn build_index_ext(item: &WatchfaceItem, category_map: &HashMap<i64, String>) -> Value {
    json!({
        "provider": PROVIDER_NAME,
        "sourceId": item.id,
        "deviceType": item.r#type,
        "tagId": item.is_tag,
        "tagName": category_map
            .get(&item.is_tag.unwrap_or(DEFAULT_CATEGORY_ID))
            .cloned()
            .unwrap_or_else(|| "全部".to_string()),
        "downloads": item.download_times.unwrap_or(0),
        "views": item.views.unwrap_or(0),
        "filesize": item.filesize.unwrap_or(0),
        "rawUpdatedAt": item.updated_at,
        "rawCreatedAt": item.created_at
    })
}

fn build_index_description(_description: Option<&str>) -> String {
    String::new()
}

fn sort_watchface_items(items: &mut [WatchfaceItem], sort: Option<&str>) {
    match resolve_list_mode(sort) {
        ListMode::Hot => items.sort_by(|left, right| {
            right
                .download_times
                .unwrap_or(0)
                .cmp(&left.download_times.unwrap_or(0))
                .then_with(|| right.views.unwrap_or(0).cmp(&left.views.unwrap_or(0)))
                .then_with(|| right.updated_at.unwrap_or(0).cmp(&left.updated_at.unwrap_or(0)))
                .then_with(|| right.id.cmp(&left.id))
        }),
        ListMode::Recommend => items.sort_by(|left, right| {
            right
                .is_recommend
                .unwrap_or(0)
                .cmp(&left.is_recommend.unwrap_or(0))
                .then_with(|| right.updated_at.unwrap_or(0).cmp(&left.updated_at.unwrap_or(0)))
                .then_with(|| right.id.cmp(&left.id))
        }),
        ListMode::Latest => items.sort_by(|left, right| {
            right
                .updated_at
                .unwrap_or(0)
                .cmp(&left.updated_at.unwrap_or(0))
                .then_with(|| right.created_at.unwrap_or(0).cmp(&left.created_at.unwrap_or(0)))
                .then_with(|| right.id.cmp(&left.id))
        }),
    }
}

fn is_hidden_device_model(model: &str) -> bool {
    HIDDEN_DEVICE_MODELS
        .iter()
        .any(|hidden| hidden.eq_ignore_ascii_case(model.trim()))
}

fn is_hidden_device_name(name: &str) -> bool {
    HIDDEN_DEVICE_NAMES.iter().any(|hidden| *hidden == name.trim())
}

fn is_hidden_device_info(device: &DeviceInfo) -> bool {
    is_hidden_device_model(&device.model) || is_hidden_device_name(&device.name)
}

fn is_supported_watchface_item(item: &WatchfaceItem) -> bool {
    item.r#type
        .as_deref()
        .map(|device_type| !is_hidden_device_model(device_type))
        .unwrap_or(true)
}

fn ensure_supported_watchface_item(item: &WatchfaceItem) -> Result<()> {
    if is_supported_watchface_item(item) {
        Ok(())
    } else {
        Err(anyhow!(
            "unsupported device resource filtered: {}",
            item.r#type.as_deref().unwrap_or_default()
        ))
    }
}

fn build_device_name_map(devices: &[DeviceInfo]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for device in devices {
        let model = device.model.trim();
        let name = device.name.trim();
        if !model.is_empty() && !name.is_empty() {
            map.insert(model.to_ascii_lowercase(), name.to_string());
        }
    }
    for (model, name) in FALLBACK_DEVICES {
        if !is_hidden_device_model(model) && !is_hidden_device_name(name) {
            map.entry(model.to_ascii_lowercase())
                .or_insert_with(|| (*name).to_string());
        }
    }
    map
}

fn resolve_device_display_name(
    item: &WatchfaceItem,
    device_name_map: &HashMap<String, String>,
) -> String {
    let device_type = item
        .r#type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DEVICE_TYPE);
    device_name_map
        .get(&device_type.to_ascii_lowercase())
        .cloned()
        .unwrap_or_else(|| device_type.to_string())
}

fn build_display_name(item: &WatchfaceItem, device_name_map: &HashMap<String, String>) -> String {
    let device_name = resolve_device_display_name(item, device_name_map);
    format!("{}（{}）", item.name.trim(), device_name)
}

fn build_mitan_url(item: &WatchfaceItem) -> Option<String> {
    let tid = item
        .mitantid
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let item_type = item.mitantype.as_deref().unwrap_or_default();
    if item_type.starts_with('r') {
        Some(format!("https://www.bandbbs.cn/resources/{tid}/"))
    } else {
        Some(format!("https://www.bandbbs.cn/threads/{tid}/"))
    }
}

fn resolve_category_id(category_map: &HashMap<i64, String>, categories: &[String]) -> i64 {
    for category in categories {
        let trimmed = category.trim();
        if trimmed.is_empty() || trimmed.starts_with(DEVICE_CATEGORY_PREFIX) {
            continue;
        }
        if let Ok(id) = trimmed.parse::<i64>() {
            return id;
        }
        if let Some((id, _)) = category_map
            .iter()
            .find(|(_, name)| name.as_str() == trimmed)
        {
            return *id;
        }
    }
    DEFAULT_CATEGORY_ID
}

fn resolve_list_mode(sort: Option<&str>) -> ListMode {
    match sort.unwrap_or("time").to_ascii_lowercase().as_str() {
        "hot" | "popular" | "download" => ListMode::Hot,
        "recommend" | "recommended" => ListMode::Recommend,
        _ => ListMode::Latest,
    }
}

fn base_headers(
    config: &ProviderConfig,
    auth: Option<&AuthContext>,
) -> Vec<(&'static str, String)> {
    base_headers_for_type(config, &config.device_type(), auth)
}

fn base_headers_for_type(
    config: &ProviderConfig,
    device_type: &str,
    auth: Option<&AuthContext>,
) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("version", APP_CLIENT_VERSION.to_string()),
        ("did", config.did()),
        ("model", config.model()),
        ("model2", config.model2()),
        ("model3", config.model3()),
        ("lang", config.lang()),
        ("type", device_type.to_string()),
        ("plat", "normal".to_string()),
    ];

    if let Some(auth) = auth {
        let donation = auth.donation.clone();
        let time_stamp = unix_time_ms().to_string();
        let nick_name = auth.nickname.clone().unwrap_or_default();
        headers.push(("openId", auth.openid.clone()));
        headers.push(("nickName", url_encode(&nick_name)));
        headers.push(("validtoken", auth.valid_token.clone()));
        headers.push(("isDonation", donation.clone()));
        headers.push((
            "token",
            md5_hex(&(time_stamp.clone() + "Aa90_89123jn!jkna90+ak90Aa113asgj")),
        ));
        headers.push(("time_stamp", time_stamp.clone()));
        headers.push((
            "token2",
            md5_hex(
                &(time_stamp.clone()
                    + "Aa901123_89123jn!jkna90+ak90Aa113asgj"
                    + &auth.openid
                    + "_"
                    + &donation
                    + "_"
                    + &auth.valid_token),
            ),
        ));
        headers.push(("time_stamp2", time_stamp.clone()));
        headers.push((
            "token3",
            md5_hex(
                &(time_stamp
                    + "Aa901123_89123jn!jkna90+ak90Aa113asgj"
                    + &auth.openid
                    + "_"
                    + &donation
                    + "_"
                    + &auth.valid_token
                    + "_"
                    + &APP_CLIENT_VERSION.to_string()
                    + "_"
                    + &config.model2()),
            ),
        ));
    }

    headers
}

fn current_config() -> ProviderConfig {
    state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .config
        .clone()
}

fn api_url(path: &str) -> String {
    format!("{}{}", API_BASE_URL, path.trim_start_matches('/'))
}

fn api_json_url(path: &str) -> String {
    format!("{}{}", API_JSON_BASE_URL, path.trim_start_matches('/'))
}

fn build_download_file_name(item: &WatchfaceItem, download_url: &str) -> String {
    let extension = guess_extension(download_url).unwrap_or_else(|| "bin".to_string());
    format!(
        "givemefive-{}-{}.{}",
        item.id,
        sanitize_ascii(item.r#type.as_deref().unwrap_or(DEFAULT_DEVICE_TYPE)),
        extension
    )
}

fn guess_extension(download_url: &str) -> Option<String> {
    let decoded = percent_decode(download_url.split('?').next().unwrap_or_default());
    let candidate = decoded
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .rsplit('.')
        .next()
        .unwrap_or_default();
    if candidate.is_empty() || candidate.len() > 8 {
        return None;
    }
    if candidate.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn sanitize_ascii(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn contains_astrobox(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .is_some_and(|value| value.to_ascii_lowercase().contains("astrobox"))
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(string) => Some(string.clone()),
        Value::Bool(boolean) => Some(if *boolean { "1" } else { "0" }.to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn value_to_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(boolean) => Some(*boolean),
        Value::String(string) => match string.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        },
        Value::Number(number) => number.as_i64().map(|value| value != 0),
        _ => None,
    }
}

fn value_to_u64(value: &Value) -> Option<u64> {
    match value {
        Value::String(string) => string.parse().ok(),
        Value::Number(number) => number.as_u64(),
        _ => None,
    }
}

fn md5_hex(value: &str) -> String {
    format!("{:x}", md5::compute(value.as_bytes()))
}

fn percent_decode(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hi = bytes[index + 1];
                let lo = bytes[index + 2];
                let decoded = decode_hex_pair(hi, lo).unwrap_or(bytes[index]);
                output.push(decoded as char);
                index += 3;
            }
            b'+' => {
                output.push(' ');
                index += 1;
            }
            byte => {
                output.push(byte as char);
                index += 1;
            }
        }
    }
    output
}

fn decode_hex_pair(hi: u8, lo: u8) -> Option<u8> {
    let high = (hi as char).to_digit(16)? as u8;
    let low = (lo as char).to_digit(16)? as u8;
    Some((high << 4) | low)
}

fn url_encode(input: &str) -> String {
    let mut encoded = String::new();
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            b' ' => encoded.push_str("%20"),
            byte => encoded.push_str(&format!("%{:02X}", byte)),
        }
    }
    encoded
}

impl ProviderConfig {
    fn merge_raw(&mut self, raw: &str) -> Result<()> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return self.merge_json(&value);
        }

        if trimmed.contains('=') || trimmed.contains(':') {
            let mut object = Map::new();
            for line in trimmed
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
            {
                if let Some((key, value)) = line
                    .split_once('=')
                    .or_else(|| line.split_once(':'))
                    .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
                {
                    object.insert(key, Value::String(value));
                }
            }
            if !object.is_empty() {
                return self.merge_json(&Value::Object(object));
            }
        }

        self.device_type = Some(trimmed.to_string());
        Ok(())
    }

    fn merge_json(&mut self, value: &Value) -> Result<()> {
        let object = value
            .as_object()
            .ok_or_else(|| anyhow!("provider config must be a JSON object"))?;

        self.device_type = pick_string(object, &["deviceType", "device_type", "type", "device"])
            .or(self.device_type.take());
        self.username = pick_string(object, &["username", "name", "account", "mitanUsername"])
            .or(self.username.take());
        self.password =
            pick_string(object, &["password", "passwd", "mitanPassword"]).or(self.password.take());
        self.mitan_code =
            pick_string(object, &["mitanCode", "code", "oauthCode"]).or(self.mitan_code.take());
        self.openid =
            pick_string(object, &["openid", "openId", "mitanOpenid"]).or(self.openid.take());
        self.valid_token = pick_string(object, &["validToken", "valid_token", "token"])
            .or(self.valid_token.take());
        self.nickname = pick_string(object, &["nickname", "nickName"]).or(self.nickname.take());
        self.donation = pick_string(object, &["donation", "isDonation"]).or(self.donation.take());
        self.limit_mac = pick_string(object, &["limitMac", "limit_mac"]).or(self.limit_mac.take());
        self.use_donor_download = pick_bool(object, &["useDonorDownload", "use_donor_download"])
            .or(self.use_donor_download.take());
        self.did = pick_string(object, &["did"]).or(self.did.take());
        self.model = pick_string(object, &["androidModel", "model"]).or(self.model.take());
        self.model2 = pick_string(object, &["model2"]).or(self.model2.take());
        self.model3 = pick_string(object, &["model3"]).or(self.model3.take());
        self.lang = pick_string(object, &["lang", "language"]).or(self.lang.take());
        self.app_signature_sha1 = pick_string(object, &["appSignatureSha1", "signatureSha1"])
            .or(self.app_signature_sha1.take());

        Ok(())
    }

    fn normalize(&mut self) {
        if self
            .device_type
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            self.device_type = Some(DEFAULT_DEVICE_TYPE.to_string());
        }
        if self
            .did
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
            || contains_astrobox(self.did.as_deref())
        {
            self.did = Some(client_identity().did.clone());
        }
        if self
            .model
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
            || contains_astrobox(self.model.as_deref())
        {
            self.model = Some(client_identity().model.clone());
        }
        if self
            .lang
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            self.lang = Some(DEFAULT_LANG.to_string());
        }
        if self
            .model2
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            self.model2 = Some(client_identity().model2.clone());
        }
        if self
            .app_signature_sha1
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            self.app_signature_sha1 = Some(client_identity().app_signature_sha1.clone());
        }
        if self
            .model3
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            self.model3 = Some(client_identity().model3.clone());
        }
    }

    fn device_type(&self) -> String {
        self.device_type
            .clone()
            .unwrap_or_else(|| DEFAULT_DEVICE_TYPE.to_string())
    }

    fn did(&self) -> String {
        self.did
            .clone()
            .unwrap_or_else(|| client_identity().did.clone())
    }

    fn model(&self) -> String {
        self.model
            .clone()
            .unwrap_or_else(|| client_identity().model.clone())
    }

    fn model2(&self) -> String {
        self.model2
            .clone()
            .unwrap_or_else(|| client_identity().model2.clone())
    }

    fn model3(&self) -> String {
        self.model3
            .clone()
            .unwrap_or_else(|| client_identity().model3.clone())
    }

    fn lang(&self) -> String {
        self.lang
            .clone()
            .unwrap_or_else(|| DEFAULT_LANG.to_string())
    }

    fn explicit_auth(&self) -> Option<AuthContext> {
        Some(AuthContext {
            openid: self.openid.clone()?,
            valid_token: self.valid_token.clone()?,
            nickname: self.nickname.clone(),
            donation: self.donation.clone().unwrap_or_else(|| "0".to_string()),
        })
    }

    fn login_payload(&self) -> Option<LoginPayload> {
        if let Some(code) = self
            .mitan_code
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(LoginPayload {
                kind: LoginKind::OauthCode,
                name: None,
                password: None,
                code: Some(code.to_string()),
            });
        }

        let username = self
            .username
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        let password = self
            .password
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        Some(LoginPayload {
            kind: LoginKind::UsernamePassword,
            name: Some(username.to_string()),
            password: Some(password.to_string()),
            code: None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ListMode {
    Latest,
    Hot,
    Recommend,
}

impl ListMode {
    fn as_str(self) -> &'static str {
        match self {
            ListMode::Latest => "time",
            ListMode::Hot => "hot",
            ListMode::Recommend => "recommend",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum LoginKind {
    UsernamePassword,
    OauthCode,
}

#[derive(Clone, Debug)]
struct LoginPayload {
    kind: LoginKind,
    name: Option<String>,
    password: Option<String>,
    code: Option<String>,
}

fn pick_string(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(value_to_string)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn pick_bool(object: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(value_to_bool)
}
