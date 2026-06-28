//! 轻腕社区源 provider 业务逻辑：把轻腕加密网关的 apps/versions 接口
//! 映射为 AstroBox provider 的 refresh/get_categories/get_index/get_manifest/
//! get_total/download 动作。

use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};
use waki::Client;

use crate::astrobox::psys_host::provider_callback;

#[path = "resources/gateway.rs"]
mod gateway;
#[path = "resources/provider.rs"]
mod provider;

pub use self::provider::handle_provider_action;

pub const PROVIDER_NAME: &str = "轻腕社区";

const DEFAULT_DEVICE_TYPE: &str = "watch-square";
const IMAGE_BASE_URL: &str = "https://voss.omoi.online/";
const DEFAULT_PAGE_SIZE: usize = 24;
const MAX_APPS_LIST_PAGE_SIZE: usize = 100;
const MAX_VISIBLE_SCAN_PAGES: usize = 100;
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;

const FALLBACK_CATEGORIES: &[(i64, &str)] = &[
    (1, "表盘"),
    (2, "工具"),
    (3, "娱乐"),
    (4, "游戏"),
    (5, "运动健康"),
];
const ALL_CATEGORY_LABEL: &str = "全部";
const WATCHFACE_CATEGORY_ID: i64 = 1;
/// 设备筛选分类前缀。分类名形如 `设备:vivo(iQOO) WATCH 5`，在 get_index 时
/// 解析回 /api/v1/apps/list 的 `deviceIds` 过滤参数。
const DEVICE_CATEGORY_PREFIX: &str = "设备:";

// ----------------------------- provider 协议 ------------------------------

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

#[derive(Debug, Deserialize, Default)]
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

// ------------------------------- 全局状态 --------------------------------

#[derive(Default, Clone, Debug)]
struct ProviderConfig {
    device_type: Option<String>,
}

impl ProviderConfig {
    fn device_type(&self) -> String {
        self.device_type
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_DEVICE_TYPE.to_string())
    }

    fn merge_json(&mut self, value: &Value) {
        if let Some(object) = value.as_object() {
            if let Some(device) = ["deviceType", "device_type", "type", "device"]
                .iter()
                .find_map(|key| object.get(*key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                self.device_type = Some(device.to_string());
            }
        }
    }

    fn merge_raw(&mut self, raw: &str) {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            self.merge_json(&value);
        } else {
            self.device_type = Some(trimmed.to_string());
        }
    }
}

/// 远程设备列表（/api/v1/devices）的精简表示，仅保留筛选所需字段。
#[derive(Clone, Debug)]
struct DeviceInfo {
    id: String,
    name: String,
    model: String,
}

static STATE: OnceLock<Mutex<ProviderConfig>> = OnceLock::new();
static CATEGORY_INDEX: OnceLock<Mutex<Option<Vec<(i64, String)>>>> = OnceLock::new();
static DEVICE_INDEX: OnceLock<Mutex<Option<Vec<DeviceInfo>>>> = OnceLock::new();

fn state() -> &'static Mutex<ProviderConfig> {
    STATE.get_or_init(|| Mutex::new(ProviderConfig::default()))
}

/// 返回 (分类 id, 分类名) 列表，结果缓存。失败时回退到内置分类。
fn category_index() -> Vec<(i64, String)> {
    let cell = CATEGORY_INDEX.get_or_init(|| Mutex::new(None));
    {
        let guard = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = guard.as_ref() {
            return cached.clone();
        }
    }
    let fetched = fetch_category_index_remote().unwrap_or_else(|error| {
        tracing::warn!("category index fetch failed, using fallback: {error:#}");
        fallback_category_index()
    });
    let mut guard = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(fetched.clone());
    fetched
}

fn fetch_category_index_remote() -> Result<Vec<(i64, String)>> {
    let data = gateway::api_data("/api/v1/categories", "GET", None)?;
    let array = match data {
        Value::Array(items) => items,
        ref other => extract_records(other),
    };
    let mut out = Vec::new();
    for item in &array {
        if let (Some(id), Some(name)) = (record_u64(item, &["id"]), record_string(item, &["name"]))
        {
            out.push((id as i64, name));
        }
    }
    if out.is_empty() {
        return Err(anyhow!("categories response had no usable entries"));
    }
    Ok(out)
}

fn fallback_category_index() -> Vec<(i64, String)> {
    FALLBACK_CATEGORIES
        .iter()
        .map(|(id, name)| (*id, (*name).to_string()))
        .collect()
}

/// 返回设备列表，结果缓存。拉取失败时返回空列表（不展示设备筛选）。
fn device_index() -> Vec<DeviceInfo> {
    let cell = DEVICE_INDEX.get_or_init(|| Mutex::new(None));
    {
        let guard = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = guard.as_ref() {
            return cached.clone();
        }
    }
    let fetched = fetch_device_index_remote().unwrap_or_else(|error| {
        tracing::warn!("device index fetch failed, skipping device filter: {error:#}");
        Vec::new()
    });
    let mut guard = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(fetched.clone());
    fetched
}

fn fetch_device_index_remote() -> Result<Vec<DeviceInfo>> {
    let data = gateway::api_data("/api/v1/devices", "GET", None)?;
    let array = match data {
        Value::Array(items) => items,
        ref other => extract_records(other),
    };
    let mut out = Vec::new();
    for item in &array {
        if let (Some(id), Some(name)) =
            (record_string(item, &["id"]), record_string(item, &["name"]))
        {
            let model = record_string(item, &["model"]).unwrap_or_default();
            out.push(DeviceInfo { id, name, model });
        }
    }
    if out.is_empty() {
        return Err(anyhow!("devices response had no usable entries"));
    }
    Ok(out)
}

fn current_config() -> ProviderConfig {
    state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

// ------------------------------- 动作实现 --------------------------------

fn refresh_state(params: RefreshParams) -> Result<()> {
    let mut config = ProviderConfig::default();
    if let Some(value) = params.config.as_ref().filter(|value| !value.is_null()) {
        config.merge_json(value);
    }
    if let Some(raw) = params.config_raw.as_deref() {
        config.merge_raw(raw);
    }
    let mut guard = state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = config;
    tracing::info!(
        "refresh_state completed: device_type={}",
        guard.device_type()
    );
    Ok(())
}

fn fetch_categories() -> Result<Vec<String>> {
    tracing::info!("fetch_categories enter");
    // 分类列表由两组拼接：
    //   1. /api/v1/categories 的顶级资源分类（表盘/工具/娱乐/游戏/运动健康）。
    //   2. /api/v1/devices 的设备，以 `设备:` 前缀区分，用于按设备机型筛选。
    // watchface-tags 是表盘内的主题标签，不属于资源分类，不并入。
    let mut categories = vec![ALL_CATEGORY_LABEL.to_string()];
    for (_id, name) in category_index() {
        let trimmed = name.trim();
        if !trimmed.is_empty() && !categories.iter().any(|existing| existing == trimmed) {
            categories.push(trimmed.to_string());
        }
    }
    for device in device_index() {
        let label = format!("{DEVICE_CATEGORY_PREFIX}{}", device.name.trim());
        if !device.name.trim().is_empty() && !categories.iter().any(|existing| existing == &label) {
            categories.push(label);
        }
    }
    tracing::info!("fetch_categories success: count={}", categories.len());
    Ok(categories)
}

fn get_index(params: IndexParams) -> Result<Vec<Value>> {
    let limit = params.limit.max(1).min(100);
    let config = current_config();
    let filter = params
        .search
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let category_id = resolve_category_id(&params.search.category);
    let device_ids = resolve_device_ids(&params.search.category);
    let sort = resolve_sort(params.search.sort.as_deref());

    // 统一走 /api/v1/apps/list：该接口同时支持 keyword 搜索、category 分类与
    // deviceIds 设备筛选，三者可叠加。（旧的 /apps/search 不支持设备过滤且已失效。）
    tracing::info!(
        "get_index list: page={} size={limit} sort={sort} keyword={filter:?} category={category_id:?} device_ids={device_ids:?}",
        params.page
    );
    let page_start = params.page.saturating_mul(limit);
    let required_visible = page_start.saturating_add(limit);
    let records = fetch_visible_app_records(
        required_visible,
        sort,
        filter,
        category_id,
        device_ids.as_deref(),
    )?;
    let page_records = records
        .into_iter()
        .skip(page_start)
        .take(limit)
        .collect::<Vec<_>>();
    tracing::info!(
        "get_index visible page: offset={page_start} returned={}",
        page_records.len()
    );
    Ok(build_visible_manifest_items(&page_records, &config))
}

fn get_manifest(item_id: &str) -> Result<Value> {
    tracing::info!("get_manifest enter: item_id={item_id}");
    let config = current_config();
    let record = fetch_app_detail(item_id)?;
    let versions = fetch_versions(item_id).unwrap_or_default();
    tracing::info!(
        "get_manifest loaded: item_id={item_id} versions={}",
        versions.len()
    );
    Ok(build_manifest(&record, &versions, &config))
}

fn get_total() -> Result<u64> {
    tracing::info!("get_total enter");
    let total = count_visible_app_records("latest", None, None, None)?;
    tracing::info!("get_total success: total={total}");
    Ok(total)
}

fn download_app(item_id: &str, device: Option<&str>, request_id: Option<&str>) -> Result<String> {
    tracing::info!("download_app enter: item_id={item_id} device={device:?}");
    provider::notify_provider_action_progress(request_id, 0.0, "preparing");

    let file_url = resolve_download_file_url(item_id, device)?;
    tracing::info!("download_app url acquired");
    provider::notify_provider_action_progress(request_id, 0.1, "got-download-url");

    let started_at = Instant::now();
    let response = Client::new()
        .get(&file_url)
        .connect_timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .send()
        .with_context(|| format!("failed to fetch binary from {file_url}"))?;
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
            || total.map(|len| downloaded >= len).unwrap_or(false);
        if should_emit {
            let progress = match total {
                Some(len) if len > 0 => {
                    0.1 + ((downloaded as f32 / len as f32).clamp(0.0, 1.0) * 0.8)
                }
                _ => 0.5,
            };
            provider::notify_provider_action_progress(request_id, progress, "downloading");
            last_emit = Instant::now();
        }
    }
    tracing::info!(
        "download_app fetched {} bytes in {} ms",
        bytes.len(),
        started_at.elapsed().as_millis()
    );
    provider::notify_provider_action_progress(request_id, 0.92, "encoding");

    let file_name = build_download_file_name(item_id, &file_url);
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

// ----------------------------- 网关数据访问 ------------------------------

fn fetch_app_detail(item_id: &str) -> Result<Value> {
    // 优先用专用详情接口；失败则回退到列表里匹配同 id 的记录。
    if let Ok(data) = gateway::api_data(&format!("/api/v1/apps/{item_id}"), "GET", None) {
        if let Some(record) = single_record(&data) {
            return Ok(record);
        }
    }

    let data = gateway::api_data(
        "/api/v1/apps/list",
        "POST",
        Some(json!({ "current": 1, "size": DEFAULT_PAGE_SIZE, "sort": "latest" })),
    )?;
    extract_records(&data)
        .into_iter()
        .find(|record| record_id(record).as_deref() == Some(item_id))
        .ok_or_else(|| anyhow!("app {item_id} not found"))
}

fn fetch_versions(item_id: &str) -> Result<Vec<Value>> {
    let data = gateway::api_data(&format!("/api/v1/apps/{item_id}/versions"), "GET", None)?;
    let versions = match &data {
        Value::Array(items) => items.clone(),
        Value::Object(_) => extract_records(&data),
        _ => Vec::new(),
    };
    Ok(versions)
}

fn fetch_apps_list_page(
    current: usize,
    size: usize,
    sort: &str,
    filter: Option<&str>,
    category_id: Option<i64>,
    device_ids: Option<&[i64]>,
) -> Result<Value> {
    let mut body = json!({
        "current": current,
        "size": size,
        "sort": sort,
    });
    if let Some(keyword) = filter {
        body["keyword"] = json!(keyword);
    }
    if let Some(category_id) = category_id {
        body["category"] = json!(category_id);
    }
    if let Some(device_ids) = device_ids.filter(|ids| !ids.is_empty()) {
        body["deviceIds"] = json!(device_ids);
    }
    gateway::api_data("/api/v1/apps/list", "POST", Some(body))
}

fn fetch_visible_app_records(
    required_visible: usize,
    sort: &str,
    filter: Option<&str>,
    category_id: Option<i64>,
    device_ids: Option<&[i64]>,
) -> Result<Vec<Value>> {
    if required_visible == 0 {
        return Ok(Vec::new());
    }

    let mut visible = Vec::new();
    let mut current = 1usize;
    while visible.len() < required_visible && current <= MAX_VISIBLE_SCAN_PAGES {
        let data = fetch_apps_list_page(
            current,
            MAX_APPS_LIST_PAGE_SIZE,
            sort,
            filter,
            category_id,
            device_ids,
        )?;
        let pages = record_u64(&data, &["pages", "totalPages"]).map(|value| value as usize);
        let records = extract_records(&data);
        let fetched = records.len();
        let paid = records
            .iter()
            .filter(|record| is_paid_resource(record))
            .count();
        for record in records
            .into_iter()
            .filter(|record| !is_paid_resource(record))
        {
            visible.push(record);
            if visible.len() >= required_visible {
                break;
            }
        }
        tracing::info!(
            "get_index scanned remote page={current} fetched={fetched} hidden_paid={paid} visible_collected={}",
            visible.len()
        );

        if fetched == 0
            || fetched < MAX_APPS_LIST_PAGE_SIZE
            || pages.map(|last| current >= last).unwrap_or(false)
        {
            break;
        }
        current += 1;
    }

    if visible.len() < required_visible && current > MAX_VISIBLE_SCAN_PAGES {
        tracing::warn!(
            "get_index visible scan capped: required={required_visible} collected={}",
            visible.len()
        );
    }
    Ok(visible)
}

fn count_visible_app_records(
    sort: &str,
    filter: Option<&str>,
    category_id: Option<i64>,
    device_ids: Option<&[i64]>,
) -> Result<u64> {
    let mut total = 0u64;
    let mut current = 1usize;
    while current <= MAX_VISIBLE_SCAN_PAGES {
        let data = fetch_apps_list_page(
            current,
            MAX_APPS_LIST_PAGE_SIZE,
            sort,
            filter,
            category_id,
            device_ids,
        )?;
        let pages = record_u64(&data, &["pages", "totalPages"]).map(|value| value as usize);
        let records = extract_records(&data);
        let fetched = records.len();
        let paid = records
            .iter()
            .filter(|record| is_paid_resource(record))
            .count();
        total += (fetched.saturating_sub(paid)) as u64;
        tracing::info!(
            "get_total scanned remote page={current} fetched={fetched} hidden_paid={paid} visible_total={total}"
        );

        if fetched == 0
            || fetched < MAX_APPS_LIST_PAGE_SIZE
            || pages.map(|last| current >= last).unwrap_or(false)
        {
            break;
        }
        current += 1;
    }

    if current > MAX_VISIBLE_SCAN_PAGES {
        tracing::warn!("get_total visible scan capped at {MAX_VISIBLE_SCAN_PAGES} pages");
    }
    Ok(total)
}

/// 解析可下载的文件链接。
///
/// 轻腕的 app 分为“父级 app”与其下多个“variant”（对应具体设备）。
/// 父级 app 的 `/api/v1/apps/{id}/versions` 返回空，真正的 fileUrl 在各
/// variant 的 `latestVersion` 上（详情的顶级 `latestVersion` 指向选中/推荐的 variant）。
fn resolve_download_file_url(item_id: &str, device: Option<&str>) -> Result<String> {
    let detail = fetch_app_detail(item_id).ok();

    if let Some(detail) = detail.as_ref() {
        // 父级 app：从 variants 中按设备/选中/推荐/首个的顺序选取。
        if let Some(variants) = detail.get("variants").and_then(Value::as_array) {
            if let Some(url) = pick_variant_file_url(detail, variants, device) {
                return Ok(url);
            }
        }
        // 叶子 app 或选中 variant：详情顶级 latestVersion 直接携带 fileUrl。
        if let Some(url) = detail.get("latestVersion").and_then(version_file_url) {
            return Ok(url);
        }
    }

    // 兑底：走 versions 接口（适用于本身就是 variant id 的资源）。
    if let Some(url) = fetch_versions(item_id)?.first().and_then(version_file_url) {
        return Ok(url);
    }

    Err(anyhow!("no downloadable file resolved for app {item_id}"))
}

/// 从 variant 列表中按优先级选取 fileUrl：
/// 1. 与下载 key（device）匹配的 variant；2. selectedVariantId；3. recommended；4. 首个可用。
fn pick_variant_file_url(
    detail: &Value,
    variants: &[Value],
    device: Option<&str>,
) -> Option<String> {
    if let Some(want) = device.map(str::trim).filter(|value| !value.is_empty()) {
        if let Some(url) = variants
            .iter()
            .find(|variant| variant_matches_device(variant, want))
            .and_then(|variant| variant.get("latestVersion"))
            .and_then(version_file_url)
        {
            return Some(url);
        }
    }
    if let Some(selected) = record_string(detail, &["selectedVariantId"]) {
        if let Some(url) = variants
            .iter()
            .find(|variant| record_id(variant).as_deref() == Some(selected.as_str()))
            .and_then(|variant| variant.get("latestVersion"))
            .and_then(version_file_url)
        {
            return Some(url);
        }
    }
    if let Some(url) = variants
        .iter()
        .find(|variant| {
            variant
                .get("recommended")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .and_then(|variant| variant.get("latestVersion"))
        .and_then(version_file_url)
    {
        return Some(url);
    }
    variants
        .iter()
        .find_map(|variant| variant.get("latestVersion").and_then(version_file_url))
}

/// 判断 variant 是否匹配下载 key（可能是 variant id、variantLabel、设备 id/名/型号）。
fn variant_matches_device(variant: &Value, want: &str) -> bool {
    if record_id(variant).as_deref() == Some(want) {
        return true;
    }
    if record_string(variant, &["variantLabel"]).as_deref() == Some(want) {
        return true;
    }
    if let Some(devices) = variant.get("compatibleDevices").and_then(Value::as_array) {
        for device in devices {
            if record_id(device).as_deref() == Some(want)
                || record_string(device, &["name"]).as_deref() == Some(want)
                || record_string(device, &["model"])
                    .map(|model| model.eq_ignore_ascii_case(want))
                    .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

/// 从版本对象（latestVersion / versions 元素）中提取合法的 http(s) 下载链接。
fn version_file_url(version: &Value) -> Option<String> {
    record_string(version, &["fileUrl", "url", "downloadUrl"])
        .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
}

/// 取记录的主版本对象（用于构建 manifest 的版本号/文件名）。
/// 优先详情顶层 `latestVersion`；其次按选中/推荐/首个 variant 的 `latestVersion`。
fn resolve_primary_version(record: &Value) -> Option<Value> {
    if let Some(version) = record
        .get("latestVersion")
        .filter(|value| value.is_object())
    {
        if version_file_url(version).is_some() {
            return Some(version.clone());
        }
    }
    let variants = record.get("variants").and_then(Value::as_array)?;
    let selected = record_string(record, &["selectedVariantId"]);
    let pick = variants
        .iter()
        .find(|variant| {
            selected
                .as_deref()
                .map(|id| record_id(variant).as_deref() == Some(id))
                .unwrap_or(false)
        })
        .or_else(|| {
            variants.iter().find(|variant| {
                variant
                    .get("recommended")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
        })
        .or_else(|| variants.first())?;
    pick.get("latestVersion").cloned()
}

fn extract_records(data: &Value) -> Vec<Value> {
    match data {
        Value::Array(items) => items.clone(),
        Value::Object(_) => data
            .get("records")
            .or_else(|| data.get("list"))
            .or_else(|| data.get("items"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn single_record(data: &Value) -> Option<Value> {
    match data {
        Value::Object(_) if data.get("records").is_none() && data.get("list").is_none() => {
            Some(data.clone())
        }
        _ => extract_records(data).into_iter().next(),
    }
}

// ------------------------------- 映射构建 --------------------------------

fn build_manifest_item(record: &Value, config: &ProviderConfig) -> Value {
    let id = record_id(record).unwrap_or_default();
    let name = record_string(record, &["name", "title", "appName"]).unwrap_or_default();
    let description =
        record_string(record, &["description", "desc", "summary", "intro"]).unwrap_or_default();
    let preview = preview_urls(record);
    let cover = preview.first().cloned().unwrap_or_default();
    let authors = build_authors(record);

    json!({
        "id": id,
        "name": name,
        "description": description,
        "preview": preview,
        "icon": cover,
        "cover": cover,
        "paid_type": resolve_paid_type(record),
        "restype": resolve_restype(record),
        "category": record_string(record, &["categoryName"]),
        "author": authors,
        "ext": build_ext(record, config)
    })
}

fn build_manifest(record: &Value, versions: &[Value], config: &ProviderConfig) -> Value {
    let id = record_id(record).unwrap_or_default();
    let name = record_string(record, &["name", "title", "appName"]).unwrap_or_default();
    let description =
        record_string(record, &["description", "desc", "summary", "intro"]).unwrap_or_default();
    let preview = preview_urls(record);
    let cover = preview.first().cloned().unwrap_or_default();
    let authors = build_authors(record);
    let restype = resolve_restype(record);
    let downloads = build_downloads(record, versions, config);

    json!({
        "item": {
            "id": id,
            "restype": restype,
            "name": name,
            "description": description,
            "preview": preview,
            "icon": cover,
            "cover": cover,
            "paid_type": resolve_paid_type(record),
            "category": record_string(record, &["categoryName"]),
            "author": authors
        },
        "downloads": downloads,
        "links": build_links(record),
        "ext": build_ext(record, config)
    })
}

fn build_visible_manifest_items(records: &[Value], config: &ProviderConfig) -> Vec<Value> {
    records
        .iter()
        .filter(|record| !is_paid_resource(record))
        .map(|record| build_manifest_item(record, config))
        .collect()
}

/// 构建 manifest 的 `downloads` map。
///
/// 轻腕的父级 app 下挂多个 variant（对应不同设备机型）。为了让宿主的
/// “更多设备” 下拉能列出每个机型，这里给**每个带有效下载链接的 variant**
/// 生成一个 download 条目：键为 variant id（下载时宿主会原样回传，
/// `resolve_download_file_url` 据此精确匹配），`display_name` 为机型名。
/// 没有 variant 信息时回退为单条目（键为 config.device_type()）。
fn build_downloads(record: &Value, versions: &[Value], config: &ProviderConfig) -> Value {
    let id = record_id(record).unwrap_or_default();
    let mut map = serde_json::Map::new();

    if let Some(variants) = record.get("variants").and_then(Value::as_array) {
        for variant in variants {
            let Some(variant_id) = record_id(variant) else {
                continue;
            };
            let latest = variant.get("latestVersion");
            let Some(url) = latest.and_then(version_file_url) else {
                continue;
            };
            let version = latest
                .and_then(|v| record_string(v, &["versionName", "versionId", "version", "id"]))
                .or_else(|| record_string(variant, &["updateTime", "createTime"]))
                .unwrap_or_default();
            let display_name = variant_display_name(variant);
            let file_name = build_download_file_name(&variant_id, &url);
            map.insert(
                variant_id,
                json!({
                    "version": version,
                    "file_name": file_name,
                    "display_name": display_name
                }),
            );
        }
    }

    if !map.is_empty() {
        return Value::Object(map);
    }

    // 回退：无 variant 信息时构建单条目。
    let device_type = config.device_type();
    let primary_version = resolve_primary_version(record).or_else(|| versions.first().cloned());
    let version = primary_version
        .as_ref()
        .and_then(|version| record_string(version, &["versionName", "versionId", "version", "id"]))
        .or_else(|| record_string(record, &["updateTime", "createTime"]))
        .unwrap_or_default();
    let version_url = primary_version
        .as_ref()
        .and_then(version_file_url)
        .unwrap_or_default();
    let file_name = build_download_file_name(&id, &version_url);
    map.insert(
        device_type,
        json!({
            "version": version,
            "file_name": file_name
        }),
    );
    Value::Object(map)
}

/// variant 的展示名：优先 variantLabel，其次首个 compatibleDevices 的名字/型号。
fn variant_display_name(variant: &Value) -> String {
    if let Some(label) = record_string(variant, &["variantLabel", "name"]) {
        return label;
    }
    if let Some(devices) = variant.get("compatibleDevices").and_then(Value::as_array) {
        for device in devices {
            if let Some(name) = record_string(device, &["name", "model"]) {
                return name;
            }
        }
    }
    record_id(variant).unwrap_or_default()
}

fn build_ext(record: &Value, config: &ProviderConfig) -> Value {
    json!({
        "provider": PROVIDER_NAME,
        "sourceId": record_id(record),
        "deviceType": config.device_type(),
        "category": record_u64(record, &["category"]),
        "categoryName": record_string(record, &["categoryName"]),
        "downloads": record_u64(record, &["downloadCount", "downloads", "downloadTimes"]).unwrap_or(0),
        "starglow": record_u64(record, &["starglowCount", "likeCount", "likes"]).unwrap_or(0),
        "rating": record.get("ratingScore").cloned().unwrap_or(Value::Null),
        "ratingCount": record_u64(record, &["ratingCount"]).unwrap_or(0),
        "price": record_u64(record, &["price"]).unwrap_or(0),
        "isFeatured": record.get("isFeatured").and_then(Value::as_bool).unwrap_or(false),
        "createdAt": record_string(record, &["createTime", "createdAt"]),
        "updatedAt": record_string(record, &["updateTime", "updatedAt"])
    })
}

fn build_links(record: &Value) -> Vec<Value> {
    let mut links = Vec::new();
    if let Some(url) = record_string(record, &["afdianUrl"])
        .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
    {
        links.push(json!({
            "title": "作者/赞助链接",
            "url": url,
            "icon": "link"
        }));
    }
    links
}

/// 表盘(category 1)映射为 watchface，其余 Vivo 快应用映射为 quickapp。
fn resolve_restype(record: &Value) -> &'static str {
    let category = record_u64(record, &["category"]).map(|id| id as i64);
    let category_name = record_string(record, &["categoryName"]);
    if category == Some(WATCHFACE_CATEGORY_ID) || category_name.as_deref() == Some("表盘") {
        "watchface"
    } else {
        "quick_app"
    }
}

fn resolve_category_id(categories: &Option<Vec<String>>) -> Option<i64> {
    let list = categories.as_ref()?;
    let index = category_index();
    for raw in list {
        let value = raw.trim();
        // 跳过空项、"全部" 以及设备筛选项（由 resolve_device_ids 处理）。
        if value.is_empty()
            || value == ALL_CATEGORY_LABEL
            || value.starts_with(DEVICE_CATEGORY_PREFIX)
        {
            continue;
        }
        if let Ok(id) = value.parse::<i64>() {
            return Some(id);
        }
        if let Some((id, _)) = index.iter().find(|(_, name)| name == value) {
            return Some(*id);
        }
    }
    None
}

/// 将选中的 `设备:` 分类解析为 /api/v1/apps/list 的 `deviceIds` 数组。
/// 支持多选（取并集）。未选设备时返回 None（不附加 deviceIds 过滤）。
fn resolve_device_ids(categories: &Option<Vec<String>>) -> Option<Vec<i64>> {
    let list = categories.as_ref()?;
    let devices = device_index();
    let mut ids = Vec::new();
    for raw in list {
        let value = raw.trim();
        let Some(candidate) = value.strip_prefix(DEVICE_CATEGORY_PREFIX).map(str::trim) else {
            continue;
        };
        if candidate.is_empty() {
            continue;
        }
        let matched = devices.iter().find(|device| {
            device.name.trim() == candidate
                || device.model.eq_ignore_ascii_case(candidate)
                || device.id == candidate
        });
        if let Some(device) = matched {
            if let Ok(id) = device.id.trim().parse::<i64>() {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }
    if ids.is_empty() {
        None
    } else {
        Some(ids)
    }
}

fn build_authors(record: &Value) -> Vec<Value> {
    let name = record_string(record, &["authorName", "nickname", "username"])
        .or_else(|| {
            record
                .get("developer")
                .map(|developer| record_string(developer, &["username", "nickname", "name"]))
                .flatten()
        })
        .or_else(|| {
            record
                .get("author")
                .map(|author| record_string(author, &["nickname", "username", "name"]))
                .flatten()
        });
    let avatar = record
        .get("developer")
        .and_then(|developer| record_string(developer, &["avatar"]));

    name.as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|name| {
            vec![json!({
                "name": name,
                "avatar": avatar,
                "bindABAccount": false
            })]
        })
        .unwrap_or_default()
}

fn resolve_paid_type(record: &Value) -> Option<&'static str> {
    let price = record_f64(record, &["price", "coin", "cost"]).unwrap_or(0.0);
    // priceType: 0 免费, 1 付费, >=2 视为强制付费。
    let price_type = record_u64(record, &["priceType"]).unwrap_or(0);
    if price_type >= 2 {
        Some("force_paid")
    } else if price_type == 1 || price > 0.0 {
        Some("paid")
    } else {
        None
    }
}

fn is_paid_resource(record: &Value) -> bool {
    resolve_paid_type(record).is_some()
}

fn preview_urls(record: &Value) -> Vec<String> {
    let mut urls = Vec::new();
    let mut push = |value: Option<String>| {
        if let Some(url) = value
            .map(|value| normalize_image_url(&value))
            .filter(|url| !url.is_empty() && !urls.contains(url))
        {
            urls.push(url);
        }
    };

    push(record_string(
        record,
        &["coverImage", "cover", "previewImage", "preview"],
    ));
    push(record_string(record, &["posterImage", "poster"]));

    for key in ["images", "previews", "previewImages", "screenshots"] {
        if let Some(array) = record.get(key).and_then(Value::as_array) {
            for item in array {
                if let Some(url) = item.as_str() {
                    push(Some(url.to_string()));
                }
            }
        }
    }
    urls
}

fn normalize_image_url(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return trimmed.to_string();
    }
    format!("{}{}", IMAGE_BASE_URL, trimmed.trim_start_matches('/'))
}

fn build_download_file_name(item_id: &str, file_url: &str) -> String {
    let extension = guess_extension(file_url).unwrap_or_else(|| "rpk".to_string());
    format!("qingwear-{}.{}", sanitize_ascii(item_id), extension)
}

fn guess_extension(file_url: &str) -> Option<String> {
    let path = file_url.split('?').next().unwrap_or_default();
    let candidate = path.rsplit('/').next()?.rsplit('.').next()?;
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

fn resolve_sort(sort: Option<&str>) -> &'static str {
    match sort.unwrap_or("latest").to_ascii_lowercase().as_str() {
        "hot" | "popular" | "download" => "hot",
        "recommend" | "recommended" | "featured" => "featured",
        _ => "latest",
    }
}

// ------------------------------- 值提取助手 ------------------------------

fn record_id(record: &Value) -> Option<String> {
    let object = record.as_object()?;
    for key in ["id", "appId", "_id", "uuid"] {
        if let Some(value) = object.get(key) {
            match value {
                Value::String(string) if !string.trim().is_empty() => return Some(string.clone()),
                Value::Number(number) => return Some(number.to_string()),
                _ => {}
            }
        }
    }
    None
}

fn record_string(record: &Value, keys: &[&str]) -> Option<String> {
    let object = record.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| match value {
            Value::String(string) => Some(string.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn record_u64(record: &Value, keys: &[&str]) -> Option<u64> {
    let object = record.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| match value {
            Value::Number(number) => number.as_u64(),
            Value::String(string) => string.trim().parse::<u64>().ok(),
            _ => None,
        })
}

fn record_f64(record: &Value, keys: &[&str]) -> Option<f64> {
    let object = record.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| match value {
            Value::Number(number) => number.as_f64(),
            Value::String(string) => string.trim().parse::<f64>().ok(),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_detection_handles_price_type_and_decimal_price() {
        assert!(is_paid_resource(&json!({ "priceType": 1, "price": 0 })));
        assert!(is_paid_resource(&json!({ "priceType": 2, "price": 0 })));
        assert!(is_paid_resource(&json!({ "priceType": 0, "price": 2.15 })));
        assert!(is_paid_resource(&json!({ "priceType": 0, "price": "0.5" })));
        assert!(!is_paid_resource(&json!({ "priceType": 0, "price": 0 })));
    }

    #[test]
    fn visible_manifest_items_filter_paid_records() {
        let records = vec![
            json!({ "id": "free", "name": "Free", "priceType": 0, "price": 0 }),
            json!({ "id": "paid-type", "name": "Paid Type", "priceType": 1, "price": 0 }),
            json!({ "id": "paid-price", "name": "Paid Price", "priceType": 0, "price": 1.25 }),
            json!({ "id": "force-paid", "name": "Force Paid", "priceType": 2, "price": 0 }),
        ];

        let items = build_visible_manifest_items(&records, &ProviderConfig::default());
        let ids = items
            .iter()
            .map(|item| item["id"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["free"]);
        assert!(items[0]["paid_type"].is_null());
    }
}
