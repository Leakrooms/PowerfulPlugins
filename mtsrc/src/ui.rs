use std::sync::{Mutex, OnceLock};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde::Deserialize;

use crate::{
    astrobox::psys_host::{dialog, ui_v3 as ui},
    exports::astrobox::psys_plugin::event_v3 as event,
    resources::{
        self, UiAccountSnapshot, UiBinaryAsset, UiDeviceChoice, UiMyShareItem, UiUploadRequest,
    },
};

const EVENT_TAB_ACCOUNT: &str = "tab:account";
const EVENT_TAB_UPLOAD: &str = "tab:upload";
const EVENT_TAB_MANAGE: &str = "tab:manage";
const EVENT_DEVICE_SELECT: &str = "field:device";
const EVENT_LOGIN_USERNAME: &str = "field:login.username";
const EVENT_LOGIN_PASSWORD: &str = "field:login.password";
const EVENT_LOGIN_CODE: &str = "field:login.code";
const EVENT_MANUAL_OPENID: &str = "field:manual.openid";
const EVENT_MANUAL_TOKEN: &str = "field:manual.token";
const EVENT_MANUAL_NICKNAME: &str = "field:manual.nickname";
const EVENT_UPLOAD_NAME: &str = "field:upload.name";
const EVENT_UPLOAD_DESC: &str = "field:upload.desc";
const EVENT_UPLOAD_MITANTID: &str = "field:upload.mitantid";
const EVENT_UPLOAD_THREAD: &str = "toggle:upload.thread";
const EVENT_UPLOAD_STATIC_PNG: &str = "toggle:upload.static-png";
const EVENT_UPLOAD_AOD1: &str = "toggle:upload.aod1";
const EVENT_UPLOAD_AOD2: &str = "toggle:upload.aod2";
const EVENT_UPLOAD_AOD3: &str = "toggle:upload.aod3";
const EVENT_ACTION_LOGIN_PASSWORD: &str = "action:login.password";
const EVENT_ACTION_LOGIN_CODE: &str = "action:login.code";
const EVENT_ACTION_LOGIN_MANUAL: &str = "action:login.manual";
const EVENT_ACTION_LOGOUT: &str = "action:logout";
const EVENT_ACTION_OPEN_OAUTH: &str = "action:oauth.open";
const EVENT_ACTION_PICK_BINARY: &str = "action:upload.pick.binary";
const EVENT_ACTION_PICK_PREVIEW: &str = "action:upload.pick.preview";
const EVENT_ACTION_PICK_AOD1: &str = "action:upload.pick.aod1";
const EVENT_ACTION_PICK_AOD2: &str = "action:upload.pick.aod2";
const EVENT_ACTION_PICK_AOD3: &str = "action:upload.pick.aod3";
const EVENT_ACTION_CLEAR_BINARY: &str = "action:upload.clear.binary";
const EVENT_ACTION_CLEAR_PREVIEW: &str = "action:upload.clear.preview";
const EVENT_ACTION_CLEAR_AOD1: &str = "action:upload.clear.aod1";
const EVENT_ACTION_CLEAR_AOD2: &str = "action:upload.clear.aod2";
const EVENT_ACTION_CLEAR_AOD3: &str = "action:upload.clear.aod3";
const EVENT_ACTION_UPLOAD_SUBMIT: &str = "action:upload.submit";
const EVENT_ACTION_UPLOAD_RESET: &str = "action:upload.reset";
const EVENT_ACTION_MANAGE_REFRESH: &str = "action:manage.refresh";
const EVENT_ACTION_MANAGE_LOAD_MORE: &str = "action:manage.load-more";
const EVENT_ACTION_CLEAR_PLUGIN_CACHE: &str = "action:cache.clear";

const PICK_CACHE_BINARY_DIR: &str = "cache/picks/binary";
const PICK_CACHE_IMAGE_DIR: &str = "cache/picks/image";

const PAGE_SIZE: usize = 18;
const EMPTY_HINT: &str = "登录后可以上传资源、管理自己的社区内容，并和前端 provider 共用同一份会话。";

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ActivePanel {
    Account,
    Upload,
    Manage,
}

impl Default for ActivePanel {
    fn default() -> Self {
        Self::Account
    }
}

#[derive(Copy, Clone, Debug)]
enum NoticeTone {
    Info,
    Success,
    Error,
}

#[derive(Clone, Debug)]
struct NoticeState {
    tone: NoticeTone,
    message: String,
}

#[derive(Clone, Debug)]
struct ProgressState {
    label: String,
    value: f32,
}

#[derive(Clone, Debug)]
struct PickedAsset {
    name: String,
    data: Vec<u8>,
    preview_url: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct UploadEditState {
    upload_id: Option<String>,
    remote_preview_url: Option<String>,
    remote_preview_aod_url: Option<String>,
    remote_preview_aod2_url: Option<String>,
    remote_preview_aod3_url: Option<String>,
}

#[derive(Clone, Debug)]
struct UiState {
    root_element_id: Option<String>,
    initialized: bool,
    active_panel: ActivePanel,
    devices: Vec<UiDeviceChoice>,
    account: UiAccountSnapshot,
    notice: Option<NoticeState>,
    progress: Option<ProgressState>,
    busy: bool,
    login_username: String,
    login_password: String,
    login_code: String,
    manual_openid: String,
    manual_token: String,
    manual_nickname: String,
    upload_name: String,
    upload_desc: String,
    upload_mitantid: String,
    upload_use_thread_link: bool,
    upload_static_png: bool,
    upload_enable_aod1: bool,
    upload_enable_aod2: bool,
    upload_enable_aod3: bool,
    upload_binary: Option<PickedAsset>,
    upload_preview_main: Option<PickedAsset>,
    upload_preview_aod1: Option<PickedAsset>,
    upload_preview_aod2: Option<PickedAsset>,
    upload_preview_aod3: Option<PickedAsset>,
    upload_edit: UploadEditState,
    upload_tips: String,
    uploads: Vec<UiMyShareItem>,
    uploads_page: usize,
    uploads_has_more: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            root_element_id: None,
            initialized: false,
            active_panel: ActivePanel::Account,
            devices: Vec::new(),
            account: UiAccountSnapshot {
                logged_in: false,
                device_type: String::new(),
                device_name: String::new(),
                nickname: None,
                openid_masked: None,
                donation: "0".to_string(),
                last_error: None,
            },
            notice: None,
            progress: None,
            busy: false,
            login_username: String::new(),
            login_password: String::new(),
            login_code: String::new(),
            manual_openid: String::new(),
            manual_token: String::new(),
            manual_nickname: String::new(),
            upload_name: String::new(),
            upload_desc: String::new(),
            upload_mitantid: String::new(),
            upload_use_thread_link: true,
            upload_static_png: true,
            upload_enable_aod1: false,
            upload_enable_aod2: false,
            upload_enable_aod3: false,
            upload_binary: None,
            upload_preview_main: None,
            upload_preview_aod1: None,
            upload_preview_aod2: None,
            upload_preview_aod3: None,
            upload_edit: UploadEditState::default(),
            upload_tips: String::new(),
            uploads: Vec::new(),
            uploads_page: 1,
            uploads_has_more: false,
        }
    }
}

#[derive(Default, Deserialize)]
struct UiEventPayload {
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    checked: Option<bool>,
}

static UI_STATE: OnceLock<Mutex<UiState>> = OnceLock::new();

fn ui_state() -> &'static Mutex<UiState> {
    UI_STATE.get_or_init(|| Mutex::new(UiState::default()))
}

pub fn ui_event_processor(_evtype: event::Event, event_id: &str, event_payload: &str) {
    tracing::info!(
        "ui_event_processor_async enter: event_id={} payload_len={}",
        event_id,
        event_payload.len()
    );
    let payload = parse_event_payload(event_payload);
    let is_action = event_id.starts_with("action:");

    if is_action {
        let busy = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .busy;
        if busy {
            return;
        }
    }

    if let Some(id) = event_id.strip_prefix("action:manage.share:") {
        handle_toggle_share(id);
        return;
    }
    if let Some(id) = event_id.strip_prefix("action:manage.delete:") {
        handle_delete_upload(id);
        return;
    }
    if let Some(id) = event_id.strip_prefix("action:manage.top:") {
        handle_top_upload(id);
        return;
    }
    if let Some(id) = event_id.strip_prefix("action:manage.reason:") {
        handle_show_reason(id);
        return;
    }
    if let Some(id) = event_id.strip_prefix("action:manage.edit:") {
        handle_edit_upload(id);
        return;
    }

    match event_id {
        EVENT_TAB_ACCOUNT => {
            set_active_panel(ActivePanel::Account);
            rerender();
        }
        EVENT_TAB_UPLOAD => {
            set_active_panel(ActivePanel::Upload);
            rerender();
        }
        EVENT_TAB_MANAGE => {
            set_active_panel(ActivePanel::Manage);
            rerender();
        }
        EVENT_DEVICE_SELECT => {
            if let Some(value) = payload.value {
                handle_device_change(&value);
            }
        }
        EVENT_LOGIN_USERNAME => update_text_field(|state| state.login_username = payload.value.unwrap_or_default()),
        EVENT_LOGIN_PASSWORD => update_text_field(|state| state.login_password = payload.value.unwrap_or_default()),
        EVENT_LOGIN_CODE => update_text_field(|state| state.login_code = payload.value.unwrap_or_default()),
        EVENT_MANUAL_OPENID => update_text_field(|state| state.manual_openid = payload.value.unwrap_or_default()),
        EVENT_MANUAL_TOKEN => update_text_field(|state| state.manual_token = payload.value.unwrap_or_default()),
        EVENT_MANUAL_NICKNAME => update_text_field(|state| state.manual_nickname = payload.value.unwrap_or_default()),
        EVENT_UPLOAD_NAME => update_text_field(|state| state.upload_name = payload.value.unwrap_or_default()),
        EVENT_UPLOAD_DESC => update_text_field(|state| state.upload_desc = payload.value.unwrap_or_default()),
        EVENT_UPLOAD_MITANTID => update_text_field(|state| state.upload_mitantid = payload.value.unwrap_or_default()),
        EVENT_UPLOAD_THREAD => update_bool_field(payload.checked, |state, checked| state.upload_use_thread_link = checked),
        EVENT_UPLOAD_STATIC_PNG => update_bool_field(payload.checked, |state, checked| state.upload_static_png = checked),
        EVENT_UPLOAD_AOD1 => toggle_aod_slot(1, payload.checked),
        EVENT_UPLOAD_AOD2 => toggle_aod_slot(2, payload.checked),
        EVENT_UPLOAD_AOD3 => toggle_aod_slot(3, payload.checked),
        EVENT_ACTION_LOGIN_PASSWORD => handle_login_password(),
        EVENT_ACTION_LOGIN_CODE => handle_login_code(),
        EVENT_ACTION_LOGIN_MANUAL => handle_login_manual(),
        EVENT_ACTION_LOGOUT => handle_logout(),
        EVENT_ACTION_OPEN_OAUTH => handle_open_oauth(),
        EVENT_ACTION_PICK_BINARY => handle_pick_binary(),
        EVENT_ACTION_PICK_PREVIEW => handle_pick_preview(0),
        EVENT_ACTION_PICK_AOD1 => handle_pick_preview(1),
        EVENT_ACTION_PICK_AOD2 => handle_pick_preview(2),
        EVENT_ACTION_PICK_AOD3 => handle_pick_preview(3),
        EVENT_ACTION_CLEAR_BINARY => clear_binary(),
        EVENT_ACTION_CLEAR_PREVIEW => clear_preview(0),
        EVENT_ACTION_CLEAR_AOD1 => clear_preview(1),
        EVENT_ACTION_CLEAR_AOD2 => clear_preview(2),
        EVENT_ACTION_CLEAR_AOD3 => clear_preview(3),
        EVENT_ACTION_UPLOAD_SUBMIT => handle_submit_upload(),
        EVENT_ACTION_UPLOAD_RESET => {
            reset_upload_form(false);
            rerender();
        }
        EVENT_ACTION_MANAGE_REFRESH => load_uploads(true),
        EVENT_ACTION_MANAGE_LOAD_MORE => load_uploads(false),
        EVENT_ACTION_CLEAR_PLUGIN_CACHE => handle_clear_plugin_cache(),
        _ => {}
    }
}

pub fn ui_event_processor_v3(event_id: &str, _event_name: &str, event_payload: &str) {
    ui_event_processor(event::Event::Click, event_id, event_payload);
}

pub fn render_main_ui(element_id: &str) {
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.root_element_id = Some(element_id.to_string());
    }

    ensure_initialized();
    rerender();
}

fn ensure_initialized() {
    let already_initialized = ui_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .initialized;
    if already_initialized {
        return;
    }

    let devices = resources::list_supported_devices_for_ui().unwrap_or_default();
    let account = resources::get_account_snapshot_for_ui();
    let upload_tips = resources::fetch_upload_tips_for_ui().unwrap_or_default();
    let uploads = if account.logged_in {
        resources::fetch_my_uploads_for_ui(1, PAGE_SIZE).unwrap_or_default()
    } else {
        Vec::new()
    };
    let uploads_has_more = uploads.len() >= PAGE_SIZE;

    let mut state = ui_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.initialized = true;
    state.devices = devices;
    state.account = account;
    state.upload_tips = upload_tips;
    state.uploads = uploads;
    state.uploads_page = 1;
    state.uploads_has_more = uploads_has_more;
}

fn rerender() {
    let root_element_id = {
        ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .root_element_id
            .clone()
    };

    let Some(root_element_id) = root_element_id else {
        return;
    };

    let tree = {
        let state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        build_main_ui(&state)
    };

    ui::render(&root_element_id, tree);
}

fn build_main_ui(state: &UiState) -> ui::Element {
    let mut root = div()
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .width_full()
        .max_width(960)
        .padding(28)
        .gap(18)
        .align_start()
        .prop("data-theme", "dark");

    root = root.child(build_hero());

    if let Some(progress) = &state.progress {
        root = root.child(build_progress_block(progress));
    }

    if let Some(notice) = &state.notice {
        root = root.child(build_notice(notice));
    }

    root = root.child(build_tabs_shell(state));

    root
}

fn build_hero() -> ui::Element {
    div()
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(8)
        .width_full()
        .child(title("表盘自定义工具资源社区工作台", 30))
        .child(
            body(
                "复刻表盘自定义工具App的资源管理功能，但限制在AstroBox当前支持的新设备范围内。",
                15,
                0.76,
            ),
        )
}

fn build_tabs_shell(state: &UiState) -> ui::Element {
    ui::Element::new(ui::ElementType::TabsRoot, None)
        .prop("value", active_panel_value(state.active_panel))
        .prop("activation-mode", "manual")
        .width_full()
        .child(
            ui::Element::new(ui::ElementType::TabsList, None)
                .prop("size", "2")
                .prop("color", "blue")
                .prop("justify", "start")
                .width_full()
                .child(tab_trigger("账号", "account", EVENT_TAB_ACCOUNT))
                .child(tab_trigger("上传", "upload", EVENT_TAB_UPLOAD))
                .child(tab_trigger("我的上传", "manage", EVENT_TAB_MANAGE)),
        )
        .child(
            ui::Element::new(ui::ElementType::TabsContent, None)
                .prop("value", "account")
                .padding_top(18)
                .child(build_account_panel(state)),
        )
        .child(
            ui::Element::new(ui::ElementType::TabsContent, None)
                .prop("value", "upload")
                .padding_top(18)
                .child(build_upload_panel(state)),
        )
        .child(
            ui::Element::new(ui::ElementType::TabsContent, None)
                .prop("value", "manage")
                .padding_top(18)
                .child(build_manage_panel(state)),
        )
}

fn active_panel_value(panel: ActivePanel) -> &'static str {
    match panel {
        ActivePanel::Account => "account",
        ActivePanel::Upload => "upload",
        ActivePanel::Manage => "manage",
    }
}

fn tab_trigger(label_text: &str, value: &str, event_id: &str) -> ui::Element {
    ui::Element::new(ui::ElementType::TabsTrigger, Some(label_text))
        .prop("value", value)
        .on(ui::Event::Click, event_id)
}

fn build_account_panel(state: &UiState) -> ui::Element {
    let mut panel = section("账号与会话", "这里的登录会直接复用给 provider 浏览、下载与上传管理。");

    panel = panel.child(build_device_select(state));
    panel = panel.child(build_account_status(state));
    panel = panel.child(separator());
    panel = panel.child(subsection_title("账号密码登录"));
    panel = panel.child(
        input_field(
            &state.login_username,
            "米坛账号",
            EVENT_LOGIN_USERNAME,
            "text",
        )
        .width_full(),
    );
    panel = panel.child(
        input_field(
            &state.login_password,
            "密码",
            EVENT_LOGIN_PASSWORD,
            "password",
        )
        .width_full(),
    );
    panel = panel.child(action_row(vec![
        action_button("登录", EVENT_ACTION_LOGIN_PASSWORD, "blue", false, state.busy),
    ]));

    panel = panel.child(separator());
    panel = panel.child(subsection_title("网页登录 / APP 授权码登录"));
    panel = panel.child(
        body(
            "AstroBox 里无法像原 app 那样直接接管 OAuth 回跳，因此需要先打开米坛授权页，再把回调链接里的 code 或整个链接粘回来。",
            13,
            0.64,
        ),
    );
    panel = panel.child(
        textarea_field(
            &state.login_code,
            "粘贴 code 或完整回调链接",
            EVENT_LOGIN_CODE,
        )
        .width_full()
        .min_height(92),
    );
    panel = panel.child(action_row(vec![
        action_button("打开授权页", EVENT_ACTION_OPEN_OAUTH, "gray", true, state.busy),
        action_button("使用 code 登录", EVENT_ACTION_LOGIN_CODE, "blue", false, state.busy),
    ]));

    panel = panel.child(separator());
    panel = panel.child(subsection_title("手动会话注入"));
    panel = panel.child(
        body(
            "如果你已经从别处拿到了 openid 和 valid_token，这里可以直接注入到插件会话里，不需要再次走登录。",
            13,
            0.64,
        ),
    );
    panel = panel.child(input_field(&state.manual_openid, "openid", EVENT_MANUAL_OPENID, "text"));
    panel = panel.child(
        input_field(
            &state.manual_token,
            "valid_token",
            EVENT_MANUAL_TOKEN,
            "password",
        )
        .width_full(),
    );
    panel = panel.child(
        input_field(
            &state.manual_nickname,
            "昵称（可选）",
            EVENT_MANUAL_NICKNAME,
            "text",
        )
        .width_full(),
    );
    panel = panel.child(action_row(vec![
        action_button("应用会话", EVENT_ACTION_LOGIN_MANUAL, "blue", false, state.busy),
        action_button("退出登录", EVENT_ACTION_LOGOUT, "red", true, state.busy),
        action_button("清理插件缓存", EVENT_ACTION_CLEAR_PLUGIN_CACHE, "gray", true, state.busy),
    ]));
    panel = panel.child(
        body(
            "上传与“我的上传”接口都依赖登录态。未登录时 provider 仍可浏览公开资源，但无法管理个人内容。",
            13,
            0.58,
        ),
    );

    panel
}

fn build_device_select(state: &UiState) -> ui::Element {
    let mut select = ui::Element::new(ui::ElementType::Select, None)
        .prop("value", state.account.device_type.as_str())
        .prop("size", "3")
        .prop("variant", "surface")
        .prop("color", "gray")
        .prop("radius", "large")
        .prop("position", "popper")
        .prop("content-variant", "solid")
        .prop("placeholder", "选择上传目标设备")
        .on(ui::Event::Change, EVENT_DEVICE_SELECT)
        .width_full();

    for device in &state.devices {
        select = select.child(
            ui::Element::new(ui::ElementType::Option, Some(device.name.as_str()))
                .prop("value", device.model.as_str()),
        );
    }

    div()
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(8)
        .width_full()
        .child(label("上传目标设备"))
        .child(select)
}

fn build_account_status(state: &UiState) -> ui::Element {
    let mut badges = div().flex().gap(10).width_full();
    badges = badges.child(badge(
        if state.account.logged_in { "已登录" } else { "未登录" },
        if state.account.logged_in { "green" } else { "gray" },
    ));
    badges = badges.child(badge(
        state.account.device_name.as_str(),
        "blue",
    ));
    if state.account.donation == "1" {
        badges = badges.child(badge("赞助用户", "amber"));
    }

    let mut container = div()
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(10)
        .width_full()
        .child(badges);

    if let Some(nickname) = state.account.nickname.as_deref() {
        container = container.child(body(
            format!(
                "当前会话：{} · {}",
                nickname,
                state.account.openid_masked.as_deref().unwrap_or("未提供 openid"),
            )
            .as_str(),
            14,
            0.82,
        ));
    } else {
        container = container.child(body(EMPTY_HINT, 14, 0.62));
    }

    if let Some(error) = state.account.last_error.as_deref() {
        container = container.child(
            body(format!("最近一次登录失败：{error}").as_str(), 13, 0.88)
                .text_color("#ff6b82"),
        );
    }

    container
}

fn build_upload_panel(state: &UiState) -> ui::Element {
    let mut panel = section("上传资源", "支持原 app 里的预览图上传、表盘资源上传，以及对已上传条目的重新提交。");
    if let Some(upload_id) = state.upload_edit.upload_id.as_deref() {
        panel = panel.child(action_row(vec![
            badge(format!("编辑条目 #{upload_id}").as_str(), "amber"),
            action_button("退出编辑", EVENT_ACTION_UPLOAD_RESET, "gray", true, state.busy),
        ]));
    } else {
        panel = panel.child(body("新建上传默认会提交到当前选中的设备类型。", 13, 0.62));
    }

    panel = panel.child(build_device_select(state));
    panel = panel.child(input_field(&state.upload_name, "资源名称", EVENT_UPLOAD_NAME, "text"));
    panel = panel.child(
        textarea_field(&state.upload_desc, "资源简介 / 更新说明", EVENT_UPLOAD_DESC)
            .min_height(128)
            .width_full(),
    );
    panel = panel.child(input_field(
        &state.upload_mitantid,
        "关联米坛帖子/资源 ID（可选）",
        EVENT_UPLOAD_MITANTID,
        "text",
    ));

    panel = panel.child(build_switch_row(
        "米坛链接按帖子处理",
        "关闭后会按资源页处理。",
        EVENT_UPLOAD_THREAD,
        state.upload_use_thread_link,
    ));
    panel = panel.child(build_switch_row(
        "静态 PNG 预览",
        "和原 app 的 staticPng 参数保持一致。",
        EVENT_UPLOAD_STATIC_PNG,
        state.upload_static_png,
    ));
    panel = panel.child(build_switch_row(
        "启用 AOD 预览 1",
        "动态表盘可额外上传 Always-On 预览。",
        EVENT_UPLOAD_AOD1,
        state.upload_enable_aod1,
    ));
    if state.upload_enable_aod1 {
        panel = panel.child(build_file_picker_line(
            "AOD 预览 1",
            state.upload_preview_aod1.as_ref(),
            state.upload_edit.remote_preview_aod_url.as_deref(),
            EVENT_ACTION_PICK_AOD1,
            EVENT_ACTION_CLEAR_AOD1,
            true,
            state.busy,
        ));
    }
    panel = panel.child(build_switch_row(
        "启用 AOD 预览 2",
        "多状态表盘可继续追加。",
        EVENT_UPLOAD_AOD2,
        state.upload_enable_aod2,
    ));
    if state.upload_enable_aod2 {
        panel = panel.child(build_file_picker_line(
            "AOD 预览 2",
            state.upload_preview_aod2.as_ref(),
            state.upload_edit.remote_preview_aod2_url.as_deref(),
            EVENT_ACTION_PICK_AOD2,
            EVENT_ACTION_CLEAR_AOD2,
            true,
            state.busy,
        ));
    }
    panel = panel.child(build_switch_row(
        "启用 AOD 预览 3",
        "极少数动态表盘需要第三张。",
        EVENT_UPLOAD_AOD3,
        state.upload_enable_aod3,
    ));
    if state.upload_enable_aod3 {
        panel = panel.child(build_file_picker_line(
            "AOD 预览 3",
            state.upload_preview_aod3.as_ref(),
            state.upload_edit.remote_preview_aod3_url.as_deref(),
            EVENT_ACTION_PICK_AOD3,
            EVENT_ACTION_CLEAR_AOD3,
            true,
            state.busy,
        ));
    }

    panel = panel.child(separator());
    panel = panel.child(build_file_picker_line(
        "资源文件",
        state.upload_binary.as_ref(),
        None,
        EVENT_ACTION_PICK_BINARY,
        EVENT_ACTION_CLEAR_BINARY,
        false,
        state.busy,
    ));
    panel = panel.child(build_file_picker_line(
        "主预览图",
        state.upload_preview_main.as_ref(),
        state.upload_edit.remote_preview_url.as_deref(),
        EVENT_ACTION_PICK_PREVIEW,
        EVENT_ACTION_CLEAR_PREVIEW,
        true,
        state.busy,
    ));
    panel = panel.child(action_row(vec![
        action_button("提交上传", EVENT_ACTION_UPLOAD_SUBMIT, "blue", false, state.busy),
        action_button("清空表单", EVENT_ACTION_UPLOAD_RESET, "gray", true, state.busy),
    ]));

    if !state.upload_tips.trim().is_empty() {
        panel = panel.child(separator());
        panel = panel.child(subsection_title("社区上传须知"));
        panel = panel.child(
            ui::Element::new(ui::ElementType::ScrollArea, None)
                .width_full()
                .max_height(180)
                .child(
                    body(state.upload_tips.as_str(), 13, 0.60)
                        .width_full()
                        .padding_right(10),
                ),
        );
    }

    panel
}

fn build_manage_panel(state: &UiState) -> ui::Element {
    let mut panel = section("我的上传", "这里补齐了原 app 里的列表、上下架、删除、顶贴和驳回原因查看能力。");
    panel = panel.child(action_row(vec![
        action_button("刷新列表", EVENT_ACTION_MANAGE_REFRESH, "blue", true, state.busy),
        action_button(
            "加载更多",
            EVENT_ACTION_MANAGE_LOAD_MORE,
            "gray",
            true,
            state.busy || !state.uploads_has_more,
        ),
    ]));

    if !state.account.logged_in {
        return panel.child(body("请先登录米坛账号，然后这里才会显示你的上传内容。", 14, 0.62));
    }

    if state.uploads.is_empty() {
        return panel.child(body("还没有任何已上传资源。先去上传页提交一个新条目。", 14, 0.62));
    }

    let mut list = div().flex().flex_direction(ui::FlexDirection::Column).gap(18).width_full();
    for (index, item) in state.uploads.iter().enumerate() {
        list = list.child(build_manage_item(item, state.busy));
        if index + 1 != state.uploads.len() {
            list = list.child(separator());
        }
    }

    panel.child(
        ui::Element::new(ui::ElementType::ScrollArea, None)
            .width_full()
            .max_height(720)
            .child(list),
    )
}

fn build_manage_item(item: &UiMyShareItem, busy: bool) -> ui::Element {
    let mut row = div()
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(10)
        .width_full();

    row = row.child(
        title(item.display_name.as_str(), 18)
            .width_full(),
    );

    let mut meta = div().flex().gap(8).width_full();
    meta = meta.child(badge(item.device_name.as_str(), "blue"));
    meta = meta.child(badge(
        if item.is_share { "已公开" } else { "未公开" },
        if item.is_share { "green" } else { "gray" },
    ));
    meta = meta.child(match item.is_review {
        1 => badge("审核通过", "green"),
        0 => badge("未通过", "red"),
        _ => badge("审核中", "amber"),
    });
    row = row.child(meta);

    if !item.description.trim().is_empty() {
        row = row.child(body(item.description.as_str(), 14, 0.68));
    }

    row = row.child(
        body(
            format!("下载 {} 次", item.download_times).as_str(),
            13,
            0.52,
        ),
    );

    if !item.preview_url.trim().is_empty() {
        row = row.child(
            ui::Element::new(ui::ElementType::Image, Some(item.preview_url.as_str()))
                .width(160)
                .height(160)
                .radius(18)
                .opacity(0.96),
        );
    }

    row.child(action_row(vec![
        action_button(
            if item.is_share { "下架" } else { "公开" },
            format!("action:manage.share:{}", item.id).as_str(),
            if item.is_share { "red" } else { "green" },
            true,
            busy,
        ),
        action_button(
            "顶贴",
            format!("action:manage.top:{}", item.id).as_str(),
            "blue",
            true,
            busy,
        ),
        action_button(
            "载入表单",
            format!("action:manage.edit:{}", item.id).as_str(),
            "gray",
            true,
            busy,
        ),
        action_button(
            "驳回原因",
            format!("action:manage.reason:{}", item.id).as_str(),
            "gray",
            true,
            busy || item.is_review != 0,
        ),
        action_button(
            "删除",
            format!("action:manage.delete:{}", item.id).as_str(),
            "red",
            true,
            busy,
        ),
    ]))
}

fn build_progress_block(progress: &ProgressState) -> ui::Element {
    div()
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(8)
        .width_full()
        .child(body(progress.label.as_str(), 13, 0.76))
        .child(
            ui::Element::new(ui::ElementType::Progress, None)
                .prop("value", format!("{:.0}", progress.value).as_str())
                .prop("color", "blue")
                .width_full(),
        )
}

fn build_notice(notice: &NoticeState) -> ui::Element {
    body(notice.message.as_str(), 14, 0.92).text_color(match notice.tone {
        NoticeTone::Info => "#95a0ba",
        NoticeTone::Success => "#68d391",
        NoticeTone::Error => "#ff6b82",
    })
}

fn build_switch_row(
    title_text: &str,
    detail_text: &str,
    event_id: &str,
    checked: bool,
) -> ui::Element {
    div()
        .flex()
        .width_full()
        .gap(12)
        .align_center()
        .child(
            ui::Element::new(ui::ElementType::Switch, None)
                .prop("checked", bool_str(checked))
                .prop("color", "blue")
                .on(ui::Event::Change, event_id),
        )
        .child(
            div()
                .flex()
                .flex_direction(ui::FlexDirection::Column)
                .gap(4)
                .child(title(title_text, 15))
                .child(body(detail_text, 12, 0.52)),
        )
}

fn build_file_picker_line(
    label_text: &str,
    local_asset: Option<&PickedAsset>,
    remote_preview_url: Option<&str>,
    pick_event: &str,
    clear_event: &str,
    is_image: bool,
    busy: bool,
) -> ui::Element {
    let mut line = div()
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(8)
        .width_full()
        .child(title(label_text, 15));

    let mut actions = vec![
        action_button("选择文件", pick_event, "blue", true, busy),
    ];
    if local_asset.is_some() || remote_preview_url.is_some() {
        actions.push(action_button("清空", clear_event, "gray", true, busy));
    }
    line = line.child(action_row(actions));

    if let Some(asset) = local_asset {
        line = line.child(
            div()
                .flex()
                .gap(8)
                .align_center()
                .width_full()
                .child(badge("已选择", "green"))
                .child(
                    body(
                        format!(
                            "{} · {}",
                            asset.name,
                            format_file_size(asset.data.len())
                        )
                        .as_str(),
                        13,
                        0.76,
                    )
                    .width_full(),
                ),
        );
    } else if let Some(remote_preview_url) = remote_preview_url.filter(|value| !value.trim().is_empty()) {
        line = line.child(body("当前线上资源将作为回显参考。", 13, 0.58));
        if is_image {
            line = line.child(
                ui::Element::new(ui::ElementType::Image, Some(remote_preview_url))
                    .width(144)
                    .height(144)
                    .radius(16)
                    .opacity(0.94),
            );
        }
    }

    if is_image {
        if let Some(local_asset) = local_asset {
            if let Some(preview_url) = local_asset.preview_url.as_deref() {
                line = line.child(
                    ui::Element::new(ui::ElementType::Image, Some(preview_url))
                        .width(144)
                        .height(144)
                        .radius(16)
                        .opacity(0.96),
                );
            }
        }
    }

    line
}

fn parse_event_payload(raw: &str) -> UiEventPayload {
    serde_json::from_str(raw).unwrap_or_default()
}

fn update_text_field(mutator: impl FnOnce(&mut UiState)) {
    let mut state = ui_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    mutator(&mut state);
}

fn update_bool_field(value: Option<bool>, mutator: impl FnOnce(&mut UiState, bool)) {
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        mutator(&mut state, value.unwrap_or(false));
    }
    rerender();
}

fn toggle_aod_slot(index: usize, checked: Option<bool>) {
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let checked = checked.unwrap_or(false);
        match index {
            1 => {
                state.upload_enable_aod1 = checked;
                if !checked {
                    state.upload_preview_aod1 = None;
                    state.upload_edit.remote_preview_aod_url = None;
                }
            }
            2 => {
                state.upload_enable_aod2 = checked;
                if !checked {
                    state.upload_preview_aod2 = None;
                    state.upload_edit.remote_preview_aod2_url = None;
                }
            }
            3 => {
                state.upload_enable_aod3 = checked;
                if !checked {
                    state.upload_preview_aod3 = None;
                    state.upload_edit.remote_preview_aod3_url = None;
                }
            }
            _ => {}
        }
    }
    rerender();
}

fn set_active_panel(panel: ActivePanel) {
    let mut state = ui_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.active_panel = panel;
}

fn handle_device_change(value: &str) {
    let result = resources::set_selected_device_for_ui(value);
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match result {
            Ok(()) => {
                if let Some(device) = state
                    .devices
                    .iter()
                    .find(|device| device.model.eq_ignore_ascii_case(value))
                    .cloned()
                {
                    state.account.device_type = device.model;
                    state.account.device_name = device.name;
                }
                clear_notice_locked(&mut state);
            }
            Err(error) => set_notice_locked(&mut state, NoticeTone::Error, error.to_string()),
        }
    }
    rerender();
}

fn handle_login_password() {
    let (device_type, username, password) = {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.busy = true;
        state.progress = Some(ProgressState {
            label: "正在登录米坛账号…".to_string(),
            value: 30.0,
        });
        clear_notice_locked(&mut state);
        (
            state.account.device_type.clone(),
            state.login_username.clone(),
            state.login_password.clone(),
        )
    };
    rerender();

    let result = resources::login_with_password_for_ui(&device_type, &username, &password);
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.busy = false;
        state.progress = None;
        match result {
            Ok(snapshot) => {
                state.account = snapshot;
                state.login_password.clear();
                set_notice_locked(&mut state, NoticeTone::Success, "登录成功，后续 provider 浏览与上传会直接复用该会话。");
                if state.active_panel == ActivePanel::Manage {
                    drop(state);
                    load_uploads(true);
                    return;
                }
            }
            Err(error) => set_notice_locked(&mut state, NoticeTone::Error, error.to_string()),
        }
    }
    rerender();
}

fn handle_login_code() {
    let (device_type, code) = {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.busy = true;
        state.progress = Some(ProgressState {
            label: "正在用 OAuth code 登录…".to_string(),
            value: 35.0,
        });
        clear_notice_locked(&mut state);
        (state.account.device_type.clone(), state.login_code.clone())
    };
    rerender();

    let result = resources::login_with_oauth_code_for_ui(&device_type, &code);
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.busy = false;
        state.progress = None;
        match result {
            Ok(snapshot) => {
                state.account = snapshot;
                state.login_code.clear();
                set_notice_locked(&mut state, NoticeTone::Success, "网页登录会话已导入。");
                if state.active_panel == ActivePanel::Manage {
                    drop(state);
                    load_uploads(true);
                    return;
                }
            }
            Err(error) => set_notice_locked(&mut state, NoticeTone::Error, error.to_string()),
        }
    }
    rerender();
}

fn handle_login_manual() {
    let (device_type, openid, token, nickname) = {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.busy = true;
        state.progress = Some(ProgressState {
            label: "正在写入会话…".to_string(),
            value: 22.0,
        });
        clear_notice_locked(&mut state);
        (
            state.account.device_type.clone(),
            state.manual_openid.clone(),
            state.manual_token.clone(),
            state.manual_nickname.clone(),
        )
    };
    rerender();

    let result = resources::apply_manual_session_for_ui(
        &device_type,
        &openid,
        &token,
        Some(&nickname),
    );
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.busy = false;
        state.progress = None;
        match result {
            Ok(snapshot) => {
                state.account = snapshot;
                set_notice_locked(&mut state, NoticeTone::Success, "会话已注入，可以直接使用上传与管理接口。");
            }
            Err(error) => set_notice_locked(&mut state, NoticeTone::Error, error.to_string()),
        }
    }
    rerender();
}

fn handle_logout() {
    resources::logout_for_ui();
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.account = resources::get_account_snapshot_for_ui();
        state.uploads.clear();
        state.uploads_page = 1;
        state.uploads_has_more = false;
        set_notice_locked(&mut state, NoticeTone::Info, "已退出插件内的米坛会话。");
    }
    rerender();
}

fn handle_open_oauth() {
    dialog::open_url(resources::build_oauth_login_url_for_ui().as_str());
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set_notice_locked(
            &mut state,
            NoticeTone::Info,
            "浏览器已打开。授权完成后，把回调地址里的 code 或整个回调链接贴回输入框即可。",
        );
    }
    rerender();
}

fn handle_pick_binary() {
    match pick_binary_asset() {
        Ok(Some(asset)) => {
            let notice = format!(
                "已选择资源文件：{}（{}）",
                asset.name,
                format_file_size(asset.data.len())
            );
            let mut state = ui_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.upload_binary = Some(asset);
            set_notice_locked(&mut state, NoticeTone::Success, notice);
            drop(state);
            rerender();
        }
        Ok(None) => {
            tracing::info!("handle_pick_binary: no file selected");
        }
        Err(error) => {
            tracing::error!("handle_pick_binary failed: {error:#}");
            let mut state = ui_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            set_notice_locked(&mut state, NoticeTone::Error, error.to_string());
            drop(state);
            rerender();
        }
    }
}

fn handle_pick_preview(slot: usize) {
    match pick_image_asset() {
        Ok(Some(asset)) => {
            let notice = format!(
                "已选择{}：{}（{}）",
                match slot {
                    0 => "主预览图",
                    1 => "AOD 预览 1",
                    2 => "AOD 预览 2",
                    3 => "AOD 预览 3",
                    _ => "图片文件",
                },
                asset.name,
                format_file_size(asset.data.len())
            );
            let mut state = ui_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match slot {
                0 => {
                    state.upload_preview_main = Some(asset);
                    state.upload_edit.remote_preview_url = None;
                }
                1 => {
                    state.upload_preview_aod1 = Some(asset);
                    state.upload_edit.remote_preview_aod_url = None;
                }
                2 => {
                    state.upload_preview_aod2 = Some(asset);
                    state.upload_edit.remote_preview_aod2_url = None;
                }
                3 => {
                    state.upload_preview_aod3 = Some(asset);
                    state.upload_edit.remote_preview_aod3_url = None;
                }
                _ => {}
            }
            set_notice_locked(&mut state, NoticeTone::Success, notice);
            drop(state);
            rerender();
        }
        Ok(None) => {
            tracing::info!("handle_pick_preview: no file selected for slot={slot}");
        }
        Err(error) => {
            tracing::error!("handle_pick_preview failed: slot={slot} error={error:#}");
            let mut state = ui_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            set_notice_locked(&mut state, NoticeTone::Error, error.to_string());
            drop(state);
            rerender();
        }
    }
}

fn clear_binary() {
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_binary = None;
    }
    rerender();
}

fn clear_preview(slot: usize) {
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match slot {
            0 => {
                state.upload_preview_main = None;
                state.upload_edit.remote_preview_url = None;
            }
            1 => {
                state.upload_preview_aod1 = None;
                state.upload_edit.remote_preview_aod_url = None;
            }
            2 => {
                state.upload_preview_aod2 = None;
                state.upload_edit.remote_preview_aod2_url = None;
            }
            3 => {
                state.upload_preview_aod3 = None;
                state.upload_edit.remote_preview_aod3_url = None;
            }
            _ => {}
        }
    }
    rerender();
}

fn handle_clear_plugin_cache() {
    let result = resources::clear_plugin_cache_for_ui();
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.upload_binary = None;
        state.upload_preview_main = None;
        state.upload_preview_aod1 = None;
        state.upload_preview_aod2 = None;
        state.upload_preview_aod3 = None;
        match result {
            Ok(()) => set_notice_locked(
                &mut state,
                NoticeTone::Success,
                "插件缓存已清理，已选择的本地文件也已重置。",
            ),
            Err(error) => set_notice_locked(&mut state, NoticeTone::Error, error.to_string()),
        }
    }
    rerender();
}

fn handle_submit_upload() {
    let request = {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.account.logged_in {
            set_notice_locked(&mut state, NoticeTone::Error, "请先登录米坛账号，再执行上传。");
            drop(state);
            rerender();
            return;
        }
        if state.upload_name.trim().is_empty() {
            set_notice_locked(&mut state, NoticeTone::Error, "资源名称不能为空。");
            drop(state);
            rerender();
            return;
        }
        if state.upload_name.contains('\'') {
            set_notice_locked(&mut state, NoticeTone::Error, "资源名称不能包含单引号。");
            drop(state);
            rerender();
            return;
        }
        if state.upload_binary.is_none() {
            set_notice_locked(&mut state, NoticeTone::Error, "请先选择资源文件。");
            drop(state);
            rerender();
            return;
        }
        if state.upload_edit.upload_id.is_none()
            && state.upload_preview_main.is_none()
            && state.upload_edit.remote_preview_url.is_none()
        {
            set_notice_locked(&mut state, NoticeTone::Error, "新建上传必须提供主预览图。");
            drop(state);
            rerender();
            return;
        }

        state.busy = true;
        state.progress = Some(ProgressState {
            label: "正在上传预览图与资源文件…".to_string(),
            value: 18.0,
        });
        clear_notice_locked(&mut state);

        UiUploadRequest {
            device_type: state.account.device_type.clone(),
            name: state.upload_name.clone(),
            description: state.upload_desc.clone(),
            static_png: state.upload_static_png,
            update_id: state.upload_edit.upload_id.clone(),
            mitantid: Some(state.upload_mitantid.clone()).filter(|value| !value.trim().is_empty()),
            mitantype: if state.upload_use_thread_link { "t" } else { "r" }.to_string(),
            preview_main: state.upload_preview_main.clone().map(as_ui_asset),
            preview_aod: if state.upload_enable_aod1 {
                state.upload_preview_aod1.clone().map(as_ui_asset)
            } else {
                None
            },
            preview_aod2: if state.upload_enable_aod2 {
                state.upload_preview_aod2.clone().map(as_ui_asset)
            } else {
                None
            },
            preview_aod3: if state.upload_enable_aod3 {
                state.upload_preview_aod3.clone().map(as_ui_asset)
            } else {
                None
            },
            watchface_file: state.upload_binary.clone().map(as_ui_asset),
        }
    };
    rerender();

    let result = resources::submit_upload_for_ui(&request);
    let succeeded = result.is_ok();
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.busy = false;
        state.progress = None;
        match result {
            Ok(()) => {
                reset_upload_form_locked(&mut state, true);
                state.active_panel = ActivePanel::Manage;
                set_notice_locked(
                    &mut state,
                    NoticeTone::Success,
                    "资源已提交。社区侧仍可能有审核过程，具体状态请看“我的上传”。",
                );
            }
            Err(ref error) => set_notice_locked(&mut state, NoticeTone::Error, error.to_string()),
        }
    }
    rerender();

    if succeeded {
        load_uploads(true);
    }
}

fn load_uploads(refresh: bool) {
    let page = {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.account.logged_in {
            set_notice_locked(&mut state, NoticeTone::Error, "请先登录，然后再查看“我的上传”。");
            drop(state);
            rerender();
            return;
        }
        state.busy = true;
        state.progress = Some(ProgressState {
            label: if refresh {
                "正在刷新上传列表…".to_string()
            } else {
                "正在加载更多上传内容…".to_string()
            },
            value: if refresh { 30.0 } else { 55.0 },
        });
        clear_notice_locked(&mut state);
        if refresh {
            1
        } else {
            state.uploads_page.saturating_add(1)
        }
    };
    rerender();

    let result = resources::fetch_my_uploads_for_ui(page, PAGE_SIZE);
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.busy = false;
        state.progress = None;
        match result {
            Ok(items) => {
                let fetched_len = items.len();
                if refresh {
                    state.uploads = items;
                } else {
                    state.uploads.extend(items.clone());
                }
                state.uploads_page = page;
                state.uploads_has_more = fetched_len >= PAGE_SIZE;
                clear_notice_locked(&mut state);
            }
            Err(error) => set_notice_locked(&mut state, NoticeTone::Error, error.to_string()),
        }
    }
    rerender();
}

fn handle_toggle_share(id: &str) {
    let Some(item) = find_upload_by_id(id) else {
        return;
    };
    let result = resources::toggle_my_upload_share_for_ui(item.id, item.is_share);
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match result {
            Ok(()) => {
                if let Some(target) = state.uploads.iter_mut().find(|upload| upload.id == item.id) {
                    target.is_share = !target.is_share;
                }
                set_notice_locked(
                    &mut state,
                    NoticeTone::Success,
                    if item.is_share {
                        "资源已下架。"
                    } else {
                        "资源已提交分享。"
                    },
                );
            }
            Err(error) => set_notice_locked(&mut state, NoticeTone::Error, error.to_string()),
        }
    }
    rerender();
}

fn handle_delete_upload(id: &str) {
    let Some(item) = find_upload_by_id(id) else {
        return;
    };
    if !confirm_dialog("删除资源", "该操作不可撤销，确定删除这个已上传资源吗？", "删除") {
        return;
    }

    let result = resources::delete_my_upload_for_ui(item.id);
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match result {
            Ok(()) => {
                state.uploads.retain(|upload| upload.id != item.id);
                set_notice_locked(&mut state, NoticeTone::Success, "资源已删除。");
            }
            Err(error) => set_notice_locked(&mut state, NoticeTone::Error, error.to_string()),
        }
    }
    rerender();
}

fn handle_top_upload(id: &str) {
    let Some(item) = find_upload_by_id(id) else {
        return;
    };
    if !confirm_dialog("顶贴", "确定对这个资源执行顶贴吗？", "顶贴") {
        return;
    }

    let result = resources::top_my_upload_for_ui(item.id);
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match result {
            Ok(()) => set_notice_locked(&mut state, NoticeTone::Success, "顶贴请求已提交。"),
            Err(error) => set_notice_locked(&mut state, NoticeTone::Error, error.to_string()),
        }
    }
    rerender();
}

fn handle_show_reason(id: &str) {
    let Some(item) = find_upload_by_id(id) else {
        return;
    };
    if item.is_review != 0 {
        return;
    }

    let dialog = match resources::query_my_upload_reason_for_ui(item.id) {
        Ok(Some(reason)) => ("未通过原因".to_string(), reason),
        Ok(None) => ("未通过原因".to_string(), "社区暂未返回具体原因。".to_string()),
        Err(error) => ("获取失败".to_string(), error.to_string()),
    };
    show_alert(&dialog.0, &dialog.1);
}

fn handle_edit_upload(id: &str) {
    let Some(item) = find_upload_by_id(id) else {
        return;
    };

    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active_panel = ActivePanel::Upload;
        state.account.device_type = item.device_type.clone();
        state.account.device_name = item.device_name.clone();
        state.upload_name = item.name.clone();
        state.upload_desc = item.description.clone();
        state.upload_mitantid = item.mitantid.clone().unwrap_or_default();
        state.upload_use_thread_link = item
            .mitantype
            .as_deref()
            .map(|value| !value.starts_with('r'))
            .unwrap_or(true);
        state.upload_binary = None;
        state.upload_preview_main = None;
        state.upload_preview_aod1 = None;
        state.upload_preview_aod2 = None;
        state.upload_preview_aod3 = None;
        state.upload_enable_aod1 = item.preview_aod_url.is_some();
        state.upload_enable_aod2 = item.preview_aod2_url.is_some();
        state.upload_enable_aod3 = item.preview_aod3_url.is_some();
        state.upload_edit = UploadEditState {
            upload_id: Some(item.id.to_string()),
            remote_preview_url: if item.preview_url.trim().is_empty() {
                None
            } else {
                Some(item.preview_url.clone())
            },
            remote_preview_aod_url: item.preview_aod_url.clone(),
            remote_preview_aod2_url: item.preview_aod2_url.clone(),
            remote_preview_aod3_url: item.preview_aod3_url.clone(),
        };
        set_notice_locked(
            &mut state,
            NoticeTone::Info,
            "已把条目载入上传表单。当前插件的更新模式要求你重新选择资源文件后再提交。",
        );
    }

    let _ = resources::set_selected_device_for_ui(&item.device_type);
    rerender();
}

fn find_upload_by_id(id: &str) -> Option<UiMyShareItem> {
    let parsed = id.parse::<i64>().ok()?;
    ui_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .uploads
        .iter()
        .find(|item| item.id == parsed)
        .cloned()
}

fn reset_upload_form(preserve_notice: bool) {
    {
        let mut state = ui_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        reset_upload_form_locked(&mut state, preserve_notice);
    }
    rerender();
}

fn reset_upload_form_locked(state: &mut UiState, preserve_notice: bool) {
    state.upload_name.clear();
    state.upload_desc.clear();
    state.upload_mitantid.clear();
    state.upload_use_thread_link = true;
    state.upload_static_png = true;
    state.upload_enable_aod1 = false;
    state.upload_enable_aod2 = false;
    state.upload_enable_aod3 = false;
    state.upload_binary = None;
    state.upload_preview_main = None;
    state.upload_preview_aod1 = None;
    state.upload_preview_aod2 = None;
    state.upload_preview_aod3 = None;
    state.upload_edit = UploadEditState::default();
    if !preserve_notice {
        clear_notice_locked(state);
    }
}

fn set_notice_locked(state: &mut UiState, tone: NoticeTone, message: impl Into<String>) {
    state.notice = Some(NoticeState {
        tone,
        message: message.into(),
    });
}

fn clear_notice_locked(state: &mut UiState) {
    state.notice = None;
}

fn pick_binary_asset() -> anyhow::Result<Option<PickedAsset>> {
    let picked = resolve_future(dialog::pick_file(
        &dialog::PickConfig {
            read: true,
            copy_to: Some(PICK_CACHE_BINARY_DIR.to_string()),
        },
        &dialog::FilterConfig {
            multiple: false,
            extensions: vec![],
            default_directory: String::new(),
            default_file_name: String::new(),
        },
    ));

    tracing::info!(
        "pick_binary_asset result: name={} data_len={}",
        picked.name,
        picked.data.len()
    );

    if picked.name.trim().is_empty() {
        tracing::info!("pick_binary_asset canceled or empty result");
        return Ok(None);
    }

    let cache_path = build_cached_pick_path(PICK_CACHE_BINARY_DIR, &picked.name);
    let data = match std::fs::read(&cache_path) {
        Ok(bytes) => {
            tracing::info!(
                "pick_binary_asset loaded cached file: path={} bytes={}",
                cache_path,
                bytes.len()
            );
            bytes
        }
        Err(err) => {
            tracing::warn!(
                "pick_binary_asset cache read failed: path={} err={} fallback_data_len={}",
                cache_path,
                err,
                picked.data.len()
            );
            picked.data
        }
    };

    if data.is_empty() {
        return Err(anyhow::anyhow!(
            "已选择文件“{}”，但插件既没有从宿主拿到文件数据，也没有从插件缓存目录读到副本。",
            picked.name
        ));
    }

    Ok(Some(PickedAsset {
        name: picked.name,
        data,
        preview_url: None,
    }))
}

fn pick_image_asset() -> anyhow::Result<Option<PickedAsset>> {
    let picked = resolve_future(dialog::pick_file(
        &dialog::PickConfig {
            read: true,
            copy_to: Some(PICK_CACHE_IMAGE_DIR.to_string()),
        },
        &dialog::FilterConfig {
            multiple: false,
            extensions: vec![
                "png".to_string(),
                "jpg".to_string(),
                "jpeg".to_string(),
                "bmp".to_string(),
                "webp".to_string(),
            ],
            default_directory: String::new(),
            default_file_name: String::new(),
        },
    ));

    tracing::info!(
        "pick_image_asset result: name={} data_len={}",
        picked.name,
        picked.data.len()
    );

    if picked.name.trim().is_empty() {
        tracing::info!("pick_image_asset canceled or empty result");
        return Ok(None);
    }

    let cache_path = build_cached_pick_path(PICK_CACHE_IMAGE_DIR, &picked.name);
    let data = match std::fs::read(&cache_path) {
        Ok(bytes) => {
            tracing::info!(
                "pick_image_asset loaded cached file: path={} bytes={}",
                cache_path,
                bytes.len()
            );
            bytes
        }
        Err(err) => {
            tracing::warn!(
                "pick_image_asset cache read failed: path={} err={} fallback_data_len={}",
                cache_path,
                err,
                picked.data.len()
            );
            picked.data
        }
    };

    if data.is_empty() {
        return Err(anyhow::anyhow!(
            "已选择图片“{}”，但插件既没有从宿主拿到文件数据，也没有从插件缓存目录读到副本。",
            picked.name
        ));
    }

    let mime = match picked
        .name
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
    };
    let data_url = format!(
        "data:{};base64,{}",
        mime,
        BASE64_STANDARD.encode(&data)
    );

    Ok(Some(PickedAsset {
        name: picked.name,
        data,
        preview_url: Some(data_url),
    }))
}

fn confirm_dialog(title: &str, content: &str, confirm_text: &str) -> bool {
    let result = resolve_future(dialog::show_dialog(
        dialog::DialogType::Alert,
        dialog::DialogStyle::Website,
        &dialog::DialogInfo {
            title: title.to_string(),
            content: content.to_string(),
            buttons: vec![
                dialog::DialogButton {
                    id: "cancel".to_string(),
                    primary: false,
                    content: "取消".to_string(),
                },
                dialog::DialogButton {
                    id: "confirm".to_string(),
                    primary: true,
                    content: confirm_text.to_string(),
                },
            ],
        },
    ));

    result.clicked_btn_id == "confirm"
}

fn show_alert(title: &str, content: &str) {
    let _ = resolve_future(dialog::show_dialog(
        dialog::DialogType::Alert,
        dialog::DialogStyle::Website,
        &dialog::DialogInfo {
            title: title.to_string(),
            content: content.to_string(),
            buttons: vec![dialog::DialogButton {
                id: "ok".to_string(),
                primary: true,
                content: "确定".to_string(),
            }],
        },
    ));
}

fn resolve_future<T>(future: wit_bindgen::FutureReader<T>) -> T {
    wit_bindgen::block_on(future.into_future())
}

fn as_ui_asset(asset: PickedAsset) -> UiBinaryAsset {
    UiBinaryAsset {
        name: asset.name,
        data: asset.data,
    }
}

fn div() -> ui::Element {
    ui::Element::new(ui::ElementType::Div, None)
}

fn title(content: &str, size: u32) -> ui::Element {
    ui::Element::new(ui::ElementType::P, Some(content))
        .size(size)
        .text_color("#f7f8fc")
}

fn label(content: &str) -> ui::Element {
    ui::Element::new(ui::ElementType::Span, Some(content))
        .size(12)
        .text_color("#8d97b0")
        .opacity(0.82)
}

fn body(content: &str, size: u32, opacity: f32) -> ui::Element {
    ui::Element::new(ui::ElementType::P, Some(content))
        .size(size)
        .text_color("#c7cfde")
        .opacity(opacity)
}

fn badge(content: &str, color: &str) -> ui::Element {
    ui::Element::new(ui::ElementType::Badge, Some(content))
        .prop("variant", "soft")
        .prop("color", color)
        .prop("radius", "full")
}

fn separator() -> ui::Element {
    ui::Element::new(ui::ElementType::Separator, None)
        .prop("color", "gray")
        .width_full()
        .opacity(0.28)
}

fn section(title_text: &str, detail_text: &str) -> ui::Element {
    div()
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .gap(14)
        .width_full()
        .child(title(title_text, 22))
        .child(body(detail_text, 14, 0.68))
}

fn subsection_title(content: &str) -> ui::Element {
    title(content, 16).opacity(0.96)
}

fn input_field(value: &str, placeholder: &str, event_id: &str, input_type: &str) -> ui::Element {
    ui::Element::new(ui::ElementType::Input, None)
        .prop("default-value", value)
        .prop("placeholder", placeholder)
        .prop("size", "3")
        .prop("variant", "surface")
        .prop("color", "gray")
        .prop("radius", "large")
        .prop("type", input_type)
        .on(ui::Event::Input, event_id)
        .width_full()
}

fn textarea_field(value: &str, placeholder: &str, event_id: &str) -> ui::Element {
    ui::Element::new(ui::ElementType::Textarea, None)
        .prop("default-value", value)
        .prop("placeholder", placeholder)
        .prop("size", "3")
        .prop("variant", "surface")
        .prop("color", "gray")
        .prop("radius", "large")
        .on(ui::Event::Input, event_id)
        .width_full()
}

fn action_button(
    label_text: &str,
    event_id: &str,
    color: &str,
    subtle: bool,
    disabled: bool,
) -> ui::Element {
    let mut button = ui::Element::new(ui::ElementType::Button, Some(label_text))
        .prop("variant", if subtle { "ghost" } else { "soft" })
        .prop("color", color)
        .prop("radius", "full")
        .on(ui::Event::Click, event_id);
    if disabled {
        button = button.disabled();
    }
    button
}

fn action_row(buttons: Vec<ui::Element>) -> ui::Element {
    let mut row = div().flex().gap(10).width_full().align_center();
    for button in buttons {
        row = row.child(button);
    }
    row
}

fn bool_str(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn format_file_size(bytes: usize) -> String {
    const KB: f32 = 1024.0;
    const MB: f32 = 1024.0 * 1024.0;
    let size = bytes as f32;
    if size >= MB {
        format!("{:.2} MB", size / MB)
    } else if size >= KB {
        format!("{:.1} KB", size / KB)
    } else {
        format!("{bytes} B")
    }
}

fn build_cached_pick_path(dir: &str, file_name: &str) -> String {
    std::path::Path::new(dir)
        .join(file_name)
        .to_string_lossy()
        .into_owned()
}
