#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod codex;
// The transport contract is intentionally compiled ahead of the LAN/mobile
// implementation, so future control surfaces do not reshape desktop state.
#[allow(dead_code)]
mod control;
mod domain;
mod history;
mod local_server;
mod overlay;
mod presets;

use crate::{
    codex::{CodexCliProvider, CodexModelOption},
    domain::{AnswerResult, DraftView, PromptPreset, QuestionDraft},
    history::HistoryStore,
    presets::built_in_presets,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::Cursor,
    path::PathBuf,
    process::Command,
    str::FromStr,
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Listener, Manager, State, WindowEvent,
};
use tauri::{PhysicalPosition, PhysicalSize, Position, Size};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

#[derive(Default)]
struct Inner {
    draft: Option<QuestionDraft>,
    answer: Option<AnswerResult>,
    answer_preset_id: Option<String>,
    status: String,
    message: Option<String>,
    protected_overlay: bool,
    /// True when the user explicitly hid the overlay; background actions must not pop it back.
    overlay_user_hidden: bool,
    active_pid: Option<u32>,
    stream_output: String,
    answer_page: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct HotkeyConfig {
    general: String,
    math: String,
    code: String,
    submit: String,
    clear: String,
    toggle: String,
    next: String,
    previous: String,
    opacity_up: String,
    opacity_down: String,
    font_up: String,
    font_down: String,
    move_left: String,
    move_right: String,
    move_up: String,
    move_down: String,
    port_check: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            general: "Ctrl+Alt+1".into(),
            math: "Ctrl+Alt+2".into(),
            code: "Ctrl+Alt+3".into(),
            submit: "Ctrl+Alt+Enter".into(),
            clear: "Ctrl+Alt+Backspace".into(),
            toggle: "Ctrl+Alt+H".into(),
            next: "Ctrl+Alt+N".into(),
            previous: "Ctrl+Alt+P".into(),
            opacity_up: "Ctrl+Alt+F11".into(),
            opacity_down: "Ctrl+Alt+F12".into(),
            font_up: "Ctrl+Alt+F9".into(),
            font_down: "Ctrl+Alt+F10".into(),
            move_left: "Ctrl+Alt+Left".into(),
            move_right: "Ctrl+Alt+Right".into(),
            move_up: "Ctrl+Alt+Up".into(),
            move_down: "Ctrl+Alt+Down".into(),
            port_check: "Ctrl+Alt+F8".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct AppConfig {
    provider: String,
    codex_path: String,
    overlay_opacity: f32,
    overlay_theme: String,
    overlay_font_size: u32,
    overlay_width: u32,
    overlay_height: u32,
    codex_timeout_seconds: u64,
    codex_model: String,
    prompt_addendum: String,
    lan_control_enabled: bool,
    lan_control_port: u16,
    mobile_auto_save_images: bool,
    hotkeys: HotkeyConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            provider: "codex-cli".into(),
            codex_path: String::new(),
            overlay_opacity: 0.70,
            overlay_theme: "follow".into(),
            overlay_font_size: 18,
            overlay_width: 560,
            overlay_height: 360,
            codex_timeout_seconds: 90,
            codex_model: String::new(),
            prompt_addendum: String::new(),
            lan_control_enabled: false,
            lan_control_port: 18765,
            mobile_auto_save_images: false,
            hotkeys: HotkeyConfig::default(),
        }
    }
}

pub(crate) struct AppState {
    inner: Mutex<Inner>,
    presets: Mutex<Vec<PromptPreset>>,
    presets_path: PathBuf,
    config: Mutex<AppConfig>,
    config_path: PathBuf,
    work_dir: PathBuf,
    history: HistoryStore,
    local_server: Mutex<Option<local_server::LocalServer>>,
    /// Encoded, downsampled previews are reused by snapshots and WebSocket clients.
    thumbnail_cache: Mutex<HashMap<PathBuf, (u128, String)>>,
}

#[derive(Serialize, Clone)]
struct Snapshot {
    draft: Option<DraftView>,
    answer_preset_id: Option<String>,
    presets: Vec<PromptPreset>,
    status: String,
    answer: Option<AnswerResult>,
    protected_overlay: bool,
    message: Option<String>,
    stream_output: String,
    answer_page: usize,
    config: AppConfig,
}

impl AppState {
    pub(crate) fn snapshot(&self) -> Snapshot {
        let inner = self.inner.lock().expect("application state lock");
        let full_images = self
            .config
            .lock()
            .expect("config lock")
            .mobile_auto_save_images;
        let mut draft_view = inner.draft.as_ref().map(DraftView::from);
        if let Some(view) = draft_view.as_mut() {
            if let Some(draft) = inner.draft.as_ref() {
                let mut cache = self.thumbnail_cache.lock().expect("thumbnail cache lock");
                cache.retain(|path, _| path.exists());
                view.thumbnails = draft
                    .images
                    .iter()
                    .filter_map(|image| {
                        let metadata = fs::metadata(&image.path).ok()?;
                        let modified = metadata
                            .modified()
                            .ok()?
                            .duration_since(UNIX_EPOCH)
                            .ok()?
                            .as_nanos();
                        let stamp = modified
                            ^ u128::from(metadata.len())
                            ^ if full_images { u128::MAX } else { 0 };
                        if let Some((cached_stamp, value)) = cache.get(&image.path) {
                            if *cached_stamp == stamp {
                                return Some(value.clone());
                            }
                        }
                        let bytes = fs::read(&image.path).ok()?;
                        let value = if full_images {
                            format!("data:image/png;base64,{}", BASE64.encode(bytes))
                        } else {
                            let decoded = ::image::load_from_memory(&bytes).ok()?;
                            let preview = decoded.thumbnail(480, 360);
                            let mut encoded = Cursor::new(Vec::new());
                            preview
                                .write_to(&mut encoded, ::image::ImageFormat::Png)
                                .ok()?;
                            format!(
                                "data:image/png;base64,{}",
                                BASE64.encode(encoded.into_inner())
                            )
                        };
                        cache.insert(image.path.clone(), (stamp, value.clone()));
                        Some(value)
                    })
                    .collect();
            }
        }
        Snapshot {
            draft: draft_view,
            answer_preset_id: inner.answer_preset_id.clone(),
            presets: self.presets.lock().expect("preset lock").clone(),
            status: inner.status.clone(),
            answer: inner.answer.clone(),
            protected_overlay: inner.protected_overlay,
            message: inner.message.clone(),
            stream_output: inner.stream_output.clone(),
            answer_page: inner.answer_page,
            config: self.config.lock().expect("config lock").clone(),
        }
    }

    fn preset(&self, id: &str) -> Result<PromptPreset, String> {
        self.presets
            .lock()
            .expect("preset lock")
            .iter()
            .find(|preset| preset.id == id)
            .cloned()
            .ok_or_else(|| "unknown prompt preset".into())
    }

    fn persist_presets(&self) -> Result<(), String> {
        let saved: Vec<_> = self
            .presets
            .lock()
            .map_err(|_| "preset state unavailable")?
            .iter()
            .cloned()
            .collect();
        let content = serde_json::to_vec_pretty(&saved).map_err(|error| error.to_string())?;
        fs::write(&self.presets_path, content).map_err(|error| error.to_string())
    }
}

fn load_config(path: &PathBuf) -> AppConfig {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn persist_config(state: &AppState) -> Result<(), String> {
    let config = state
        .config
        .lock()
        .map_err(|_| "config state unavailable")?
        .clone();
    fs::write(
        &state.config_path,
        serde_json::to_vec_pretty(&config).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn merge_json(base: &mut serde_json::Value, patch: &serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(base), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                merge_json(
                    base.entry(key.clone()).or_insert(serde_json::Value::Null),
                    value,
                );
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}
#[tauri::command]
fn update_config(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    config: serde_json::Value,
) -> Result<(), String> {
    if !config.is_object() {
        return Err("配置 patch 必须是 JSON 对象".into());
    }
    let mut merged = serde_json::to_value(
        state
            .config
            .lock()
            .map_err(|_| "config state unavailable")?
            .clone(),
    )
    .map_err(|error| error.to_string())?;
    merge_json(&mut merged, &config);
    let config: AppConfig =
        serde_json::from_value(merged).map_err(|error| format!("配置格式无效：{error}"))?;
    let previous_config = state
        .config
        .lock()
        .map_err(|_| "config state unavailable")?
        .clone();
    if !matches!(config.provider.as_str(), "codex-cli" | "claude-code-cli") {
        return Err("未知 Provider，请选择 Codex CLI 或 Claude Code CLI".into());
    }
    if !config.overlay_opacity.is_finite() || !(0.05..=0.95).contains(&config.overlay_opacity) {
        return Err("前台背景不透明度必须在 0.05 到 0.95 之间".into());
    }
    if !matches!(config.overlay_theme.as_str(), "day" | "night" | "follow") {
        return Err("前台模式必须是 day、night 或 follow".into());
    }
    if !(10..=48).contains(&config.overlay_font_size) {
        return Err("字体大小必须在 10 到 48 之间".into());
    }
    if !(240..=1400).contains(&config.overlay_width)
        || !(160..=1000).contains(&config.overlay_height)
    {
        return Err("前台窗口尺寸超出允许范围".into());
    }
    if !(5..=600).contains(&config.codex_timeout_seconds) {
        return Err("Codex 超时必须在 5 到 600 秒之间".into());
    }
    if !(1024..=65535).contains(&config.lan_control_port) {
        return Err("本地控制端口必须在 1024 到 65535 之间".into());
    }
    if previous_config.hotkeys != config.hotkeys {
        let values = [
            &config.hotkeys.general,
            &config.hotkeys.math,
            &config.hotkeys.code,
            &config.hotkeys.submit,
            &config.hotkeys.clear,
            &config.hotkeys.toggle,
            &config.hotkeys.next,
            &config.hotkeys.previous,
            &config.hotkeys.opacity_up,
            &config.hotkeys.opacity_down,
            &config.hotkeys.font_up,
            &config.hotkeys.font_down,
            &config.hotkeys.move_left,
            &config.hotkeys.move_right,
            &config.hotkeys.move_up,
            &config.hotkeys.move_down,
            &config.hotkeys.port_check,
        ];
        let mut parsed = Vec::new();
        for value in values {
            let shortcut =
                Shortcut::from_str(value.trim()).map_err(|_| format!("快捷键格式无效：{value}"))?;
            if parsed.iter().any(|item: &Shortcut| item == &shortcut) {
                return Err("快捷键不能重复绑定".into());
            }
            parsed.push(shortcut);
        }
    }
    *state
        .config
        .lock()
        .map_err(|_| "config state unavailable")? = config.clone();
    persist_config(&state)?;
    if previous_config.lan_control_enabled != config.lan_control_enabled
        || previous_config.lan_control_port != config.lan_control_port
    {
        {
            let current = state
                .local_server
                .lock()
                .map_err(|_| "local server state unavailable")?;
            if let Some(server) = current.as_ref() {
                server.stop();
            }
        }
        let app_state = state.inner().clone();
        let mut server_slot = state
            .local_server
            .lock()
            .map_err(|_| "local server state unavailable")?;
        *server_slot = if config.lan_control_enabled {
            Some(
                local_server::LocalServer::start(app.clone(), app_state, config.lan_control_port)
                    .map_err(|error| format!("启动本地控制端口失败：{error}"))?,
            )
        } else {
            None
        };
    }
    // Re-register shortcuts only when the shortcut map changed. Saving a
    // provider/display field must not disturb an already valid registration.
    if previous_config.hotkeys != config.hotkeys {
        app.global_shortcut()
            .unregister_all()
            .map_err(|error| error.to_string())?;
        register_shortcuts(&app).map_err(|error| error.to_string())?;
    }
    if previous_config.overlay_width != config.overlay_width
        || previous_config.overlay_height != config.overlay_height
    {
        if let Some(window) = app.get_webview_window("overlay") {
            window
                .set_size(Size::Physical(PhysicalSize::new(
                    config.overlay_width,
                    config.overlay_height,
                )))
                .map_err(|error| error.to_string())?;
        }
    }
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "application state unavailable")?;
    inner.message = Some("配置已保存；变更的快捷键已立即应用。".into());
    drop(inner);
    emit_snapshot(&app, &state);
    Ok(())
}

#[tauri::command]
fn adjust_overlay_font_size(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    delta: i32,
) -> Result<(), String> {
    let mut config = state
        .config
        .lock()
        .map_err(|_| "config state unavailable")?;
    let current = config.overlay_font_size as i32;
    config.overlay_font_size = (current + delta).clamp(10, 48) as u32;
    drop(config);
    persist_config(&state)?;
    emit_snapshot(&app, &state);
    Ok(())
}

#[tauri::command]
fn set_overlay_font_size(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    value: u32,
) -> Result<(), String> {
    if !(10..=48).contains(&value) {
        return Err("字体大小必须在 10 到 48 之间".into());
    }
    let mut config = state
        .config
        .lock()
        .map_err(|_| "config state unavailable")?;
    config.overlay_font_size = value;
    drop(config);
    persist_config(&state)?;
    emit_snapshot(&app, &state);
    Ok(())
}

#[tauri::command]
fn adjust_overlay_opacity(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    delta: f32,
) -> Result<(), String> {
    let mut config = state
        .config
        .lock()
        .map_err(|_| "config state unavailable")?;
    config.overlay_opacity = (config.overlay_opacity + delta).clamp(0.05, 0.95);
    drop(config);
    persist_config(&state)?;
    emit_snapshot(&app, &state);
    Ok(())
}

#[tauri::command]
fn set_overlay_opacity(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    value: f32,
) -> Result<(), String> {
    if !value.is_finite() || !(0.05..=0.95).contains(&value) {
        return Err("前台背景不透明度必须在 0.05 到 0.95 之间".into());
    }
    let mut config = state
        .config
        .lock()
        .map_err(|_| "config state unavailable")?;
    config.overlay_opacity = value;
    drop(config);
    persist_config(&state)?;
    emit_snapshot(&app, &state);
    Ok(())
}

#[tauri::command]
fn sample_overlay_background(app: AppHandle) -> Result<String, String> {
    let window = app
        .get_webview_window("overlay")
        .ok_or("overlay window is unavailable")?;
    let (red, green, blue) =
        overlay::sample_background(&window).map_err(|error| error.to_string())?;
    Ok(format!("{red} {green} {blue}"))
}

#[tauri::command]
fn list_codex_models(state: State<'_, Arc<AppState>>) -> Result<Vec<CodexModelOption>, String> {
    let config = state
        .config
        .lock()
        .map_err(|_| "config state unavailable")?
        .clone();
    if config.provider != "codex-cli" {
        return Err("当前 Provider 不是 Codex CLI，没有可读取的 Codex 模型目录".into());
    }
    CodexCliProvider::list_models(
        state.work_dir.clone(),
        config.codex_path,
        config.codex_timeout_seconds,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn dispatch_remote_command(
    app: AppHandle,
    id: String,
    command: String,
    preset_id: String,
) {
    let _ = app.emit(
        "command.accepted",
        serde_json::json!({"type":"command.accepted", "id":id, "action":command}),
    );
    let state = app.state::<Arc<AppState>>();
    let result = match command.as_str() {
        "capture" => capture_for_preset(app.clone(), state, preset_id),
        "submit" => submit_draft(app.clone(), state),
        "clear" => clear_draft(app.clone(), state),
        "cancel" => cancel_current_job(app.clone(), state),
        _ => Err("未知远程命令".into()),
    };
    if let Err(error) = result {
        let _ = app.emit(
            "command.failed",
            serde_json::json!({"type":"command.failed", "id":id, "error":error}),
        );
    } else if matches!(command.as_str(), "clear" | "cancel") {
        let _ = app.emit(
            "command.completed",
            serde_json::json!({"type":"command.completed", "id":id, "action":command}),
        );
    }
}
fn emit_snapshot(app: &AppHandle, state: &AppState) {
    let _ = app.emit("state-changed", state.snapshot());
}

fn remove_draft_images(draft: &QuestionDraft) {
    for image in &draft.images {
        let _ = fs::remove_file(&image.path);
    }
}

#[tauri::command]
fn get_snapshot(state: State<'_, Arc<AppState>>) -> Snapshot {
    state.snapshot()
}

#[tauri::command]
fn capture_for_preset(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    preset_id: String,
) -> Result<(), String> {
    state.preset(&preset_id)?;
    {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "application state unavailable")?;
        if inner.status == "capturing" {
            return Err("正在截取上一张图片，请稍候".into());
        }
        if let Some(draft) = inner.draft.as_ref() {
            if draft.preset_id != preset_id {
                return Err("当前草稿已绑定其他题型，请先提交或清空".into());
            }
            if draft.images.len() >= domain::MAX_IMAGES_PER_DRAFT {
                return Err(format!(
                    "单个草稿最多 {} 张截图",
                    domain::MAX_IMAGES_PER_DRAFT
                ));
            }
        }
        inner.status = "capturing".into();
        inner.message = Some("正在后台截取主屏".into());
    }
    emit_snapshot(&app, &state);
    let work_dir = state.work_dir.clone();
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let captured = capture::capture_primary(&work_dir);
        let mut inner = match state.inner.lock() {
            Ok(value) => value,
            Err(_) => return,
        };
        match captured {
            Ok(image) => {
                let push_result = match inner.draft.as_mut() {
                    Some(draft) => draft
                        .try_push(image.clone())
                        .map_err(|error| error.to_string()),
                    None => {
                        inner.draft = Some(QuestionDraft::new(preset_id.clone(), image.clone()));
                        Ok(())
                    }
                };
                if let Err(error) = push_result {
                    let _ = fs::remove_file(image.path);
                    inner.status = "failed".into();
                    inner.message = Some(format!("截图失败：{error}"));
                } else {
                    inner.answer = None;
                    inner.answer_preset_id = None;
                    inner.stream_output.clear();
                    inner.answer_page = 0;
                    inner.status = "drafting".into();
                    inner.message = Some("截图已添加到草稿".into());
                }
            }
            Err(error) => {
                inner.status = "failed".into();
                inner.message = Some(format!("截图失败：{error}"));
            }
        }
        drop(inner);
        emit_snapshot(&app, &state);
    });
    Ok(())
}
#[tauri::command]
fn clear_draft(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "application state unavailable")?;
    if let Some(draft) = inner.draft.take() {
        remove_draft_images(&draft);
    }
    inner.answer = None;
    inner.answer_preset_id = None;
    inner.stream_output.clear();
    inner.answer_page = 0;
    inner.status = "idle".into();
    inner.message = Some("草稿已清空".into());
    drop(inner);
    emit_snapshot(&app, &state);
    Ok(())
}

#[tauri::command]
fn submit_draft(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let provider = state
        .config
        .lock()
        .map_err(|_| "config state unavailable")?
        .provider
        .clone();
    if provider != "codex-cli" {
        return Err(format!("{provider} provider 尚未接入求解适配器"));
    }
    let (draft, preset) = {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "application state unavailable")?;
        if inner.status == "solving" {
            return Err("已有任务正在求解".into());
        }
        let draft = inner.draft.clone().ok_or("请先添加至少一张截图")?;
        let preset = state.preset(&draft.preset_id)?;
        inner.stream_output.clear();
        inner.status = "solving".into();
        inner.message = Some("正在通过本机 Codex CLI 求解".into());
        (draft, preset)
    };
    emit_snapshot(&app, &state);
    let state: Arc<AppState> = state.inner().clone();
    let config = state
        .config
        .lock()
        .map_err(|_| "config state unavailable")?
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        let process_state = state.clone();
        let pid_state = process_state.clone();
        let progress_state = process_state.clone();

        let result = CodexCliProvider::new(
            state.work_dir.clone(),
            config.codex_path.clone(),
            config.codex_model.clone(),
            config.codex_timeout_seconds,
            config.prompt_addendum.clone(),
        )
        .solve(
            &draft,
            &preset,
            move |pid| {
                pid_state
                    .inner
                    .lock()
                    .expect("application state lock")
                    .active_pid = Some(pid);
            },
            move |_line| {
                // The product intentionally renders only the final plain-text
                // answer. Avoid retaining/emitting every provider line so a
                // long solve cannot flood the UI or LAN connection.
                let mut inner = progress_state.inner.lock().expect("application state lock");
                if inner.status == "solving" {
                    inner.message = Some("正在处理 Codex 输出".into());
                }
            },
        );
        let mut inner = state.inner.lock().expect("application state lock");
        let was_cancelled = inner.status == "cancelled" && inner.active_pid.is_none();
        if was_cancelled {
            remove_draft_images(&draft);
            inner.draft = None;
            drop(inner);
            emit_snapshot(&app, &state);
            return;
        }
        match result {
            Ok(answer) => {
                let answer_preset_id = draft.preset_id.clone();
                if let Err(error) = state.history.save(&draft, &answer) {
                    inner.message = Some(format!("答案已生成，但加密历史保存失败：{error}"));
                }
                remove_draft_images(&draft);
                inner.answer = Some(answer);
                inner.answer_page = 0;
                inner.answer_preset_id = Some(answer_preset_id);
                inner.draft = None;
                inner.status = "displaying".into();
                inner.message = Some("答案已生成".into());
                if inner.protected_overlay && !inner.overlay_user_hidden {
                    if let Some(overlay_window) = app.get_webview_window("overlay") {
                        let _ = overlay::show(&overlay_window);
                    }
                } else {
                    inner.message = Some("答案已生成，但浮窗保护未确认，已拒绝展示".into());
                }
            }
            Err(error) => {
                remove_draft_images(&draft);
                inner.draft = None;
                inner.status = "failed".into();
                inner.message = Some(format!("求解失败：{error}"));
            }
        }
        inner.active_pid = None;
        drop(inner);
        emit_snapshot(&app, &state);
    });
    Ok(())
}

#[tauri::command]
fn cancel_current_job(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let pid = {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "application state unavailable")?;
        inner.status = "cancelled".into();
        inner.message = Some("已取消任务".into());
        inner.active_pid.take()
    };
    if let Some(pid) = pid {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    emit_snapshot(&app, &state);
    Ok(())
}

#[tauri::command]
fn toggle_overlay(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let inner = state
        .inner
        .lock()
        .map_err(|_| "application state unavailable")?;
    if !inner.protected_overlay {
        return Err("浮窗保护未确认，拒绝显示答案".into());
    }
    drop(inner);
    let window = app
        .get_webview_window("overlay")
        .ok_or("overlay window is unavailable")?;
    let visible = window.is_visible().map_err(|error| error.to_string())?;
    if visible {
        overlay::hide(&window).map_err(|error| error.to_string())?;
    } else {
        overlay::show(&window).map_err(|error| error.to_string())?;
    }
    if let Ok(mut inner) = state.inner.lock() {
        inner.overlay_user_hidden = visible;
    }
    emit_snapshot(&app, &state);
    Ok(())
}

#[tauri::command]
fn copy_preset(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    preset_id: String,
) -> Result<(), String> {
    let source = state.preset(&preset_id)?;
    let copy = PromptPreset {
        id: format!("custom-{}", uuid::Uuid::new_v4()),
        name: format!("{} 副本", source.name),
        built_in: false,
        hotkey: "未设置".into(),
        task_template: source.task_template,
        version: 1,
    };
    state
        .presets
        .lock()
        .map_err(|_| "preset state unavailable")?
        .push(copy);
    state.persist_presets()?;
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "application state unavailable")?;
    inner.message = Some(format!("已复制“{}”，现在可编辑该自定义预设。", source.name));
    drop(inner);
    emit_snapshot(&app, &state);
    Ok(())
}

#[tauri::command]
fn update_custom_preset(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    preset_id: String,
    name: String,
    hotkey: String,
    task_template: String,
) -> Result<(), String> {
    if name.trim().is_empty() || task_template.trim().is_empty() {
        return Err("名称和提示词不能为空".into());
    }
    let mut presets = state
        .presets
        .lock()
        .map_err(|_| "preset state unavailable")?;
    let position = presets
        .iter()
        .position(|preset| preset.id == preset_id)
        .ok_or("unknown prompt preset")?;
    let previous_hotkey = presets[position].hotkey.clone();
    let hotkey_changed = !previous_hotkey.eq_ignore_ascii_case(hotkey.trim());
    if hotkey_changed
        && hotkey != "未设置"
        && presets
            .iter()
            .enumerate()
            .any(|(index, preset)| index != position && preset.hotkey.eq_ignore_ascii_case(&hotkey))
    {
        return Err("快捷键已被其他预设使用".into());
    }
    if hotkey_changed && hotkey != "未设置" && Shortcut::from_str(hotkey.trim()).is_err() {
        return Err("快捷键格式无效，例如 Ctrl+Alt+4".into());
    }
    let preset = &mut presets[position];
    preset.name = name.trim().to_owned();
    preset.hotkey = hotkey.trim().to_owned();
    preset.task_template = task_template.trim().to_owned();
    preset.version += 1;
    drop(presets);
    state.persist_presets()?;
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "application state unavailable")?;
    inner.message = Some("预设已保存；快捷键将在重启应用后生效。".into());
    drop(inner);
    emit_snapshot(&app, &state);
    Ok(())
}

fn load_presets(path: &PathBuf) -> Vec<PromptPreset> {
    let mut presets = built_in_presets();
    if let Ok(contents) = fs::read(path) {
        if let Ok(saved) = serde_json::from_slice::<Vec<PromptPreset>>(&contents) {
            for saved_preset in saved
                .into_iter()
                .filter(|preset| !preset.id.trim().is_empty())
            {
                // Built-in ids remain built-in even if an older file omitted
                // the flag. This keeps edited built-ins from becoming a second
                // hidden custom entry after restart.
                if let Some(existing) = presets.iter_mut().find(|item| item.id == saved_preset.id) {
                    if existing.built_in {
                        existing.name = saved_preset.name;
                        existing.hotkey = saved_preset.hotkey;
                        existing.task_template = saved_preset.task_template;
                        existing.version = saved_preset.version.max(existing.version);
                    }
                } else if !saved_preset.built_in {
                    presets.push(saved_preset);
                }
            }
        }
    }
    presets
}
fn register_shortcuts(app: &AppHandle) -> tauri::Result<()> {
    let config = app
        .state::<Arc<AppState>>()
        .config
        .lock()
        .expect("config lock")
        .clone();
    let parse =
        |value: &str| Shortcut::from_str(value).map_err(|error| anyhow::anyhow!(error.to_string()));
    let general = parse(&config.hotkeys.general)?;
    let math = parse(&config.hotkeys.math)?;
    let code = parse(&config.hotkeys.code)?;
    let submit = parse(&config.hotkeys.submit)?;
    let clear = parse(&config.hotkeys.clear)?;
    let toggle = parse(&config.hotkeys.toggle)?;
    let opacity_up = parse(&config.hotkeys.opacity_up)?;
    let opacity_down = parse(&config.hotkeys.opacity_down)?;
    let font_up = parse(&config.hotkeys.font_up)?;
    let font_down = parse(&config.hotkeys.font_down)?;
    let next = parse(&config.hotkeys.next)?;
    let previous = parse(&config.hotkeys.previous)?;
    let move_left = parse(&config.hotkeys.move_left)?;
    let move_right = parse(&config.hotkeys.move_right)?;
    let move_up = parse(&config.hotkeys.move_up)?;
    let move_down = parse(&config.hotkeys.move_down)?;
    let custom_shortcuts: Vec<(Shortcut, String)> = app
        .state::<Arc<AppState>>()
        .presets
        .lock()
        .expect("preset lock")
        .iter()
        .filter(|preset| !preset.built_in && preset.hotkey != "未设置")
        .filter_map(|preset| {
            Shortcut::from_str(&preset.hotkey)
                .ok()
                .map(|shortcut| (shortcut, preset.id.clone()))
        })
        .collect();
    let mut shortcuts = vec![
        general.clone(),
        math.clone(),
        code.clone(),
        submit.clone(),
        clear.clone(),
        toggle.clone(),
        opacity_up.clone(),
        opacity_down.clone(),
        font_up.clone(),
        font_down.clone(),
        next.clone(),
        previous.clone(),
        move_left.clone(),
        move_right.clone(),
        move_up.clone(),
        move_down.clone(),
    ];
    shortcuts.extend(
        custom_shortcuts
            .iter()
            .map(|(shortcut, _)| shortcut.clone()),
    );
    app.global_shortcut()
        .on_shortcuts(shortcuts, move |app, shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            let state = app.state::<Arc<AppState>>();
            let _ = match shortcut {
                item if *item == general => {
                    capture_for_preset(app.clone(), state, "general".into())
                }
                item if *item == math => capture_for_preset(app.clone(), state, "math".into()),
                item if *item == code => capture_for_preset(app.clone(), state, "code".into()),
                item if *item == submit => submit_draft(app.clone(), state),
                item if *item == clear => clear_draft(app.clone(), state),
                item if *item == toggle => toggle_overlay(app.clone(), state),
                item if *item == opacity_up => adjust_overlay_opacity(app.clone(), state, 0.05),
                item if *item == opacity_down => adjust_overlay_opacity(app.clone(), state, -0.05),
                item if *item == font_up => adjust_overlay_font_size(app.clone(), state, 1),
                item if *item == font_down => adjust_overlay_font_size(app.clone(), state, -1),
                item if *item == next => change_answer_page(app.clone(), state, 1),
                item if *item == previous => change_answer_page(app.clone(), state, -1),
                item if *item == move_left => move_overlay(app.clone(), -10, 0),
                item if *item == move_right => move_overlay(app.clone(), 10, 0),
                item if *item == move_up => move_overlay(app.clone(), 0, -10),
                item if *item == move_down => move_overlay(app.clone(), 0, 10),
                item => custom_shortcuts
                    .iter()
                    .find(|(candidate, _)| item == candidate)
                    .map(|(_, preset_id)| capture_for_preset(app.clone(), state, preset_id.clone()))
                    .unwrap_or(Ok(())),
            };
        })
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(())
}

#[tauri::command]
fn change_answer_page(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    delta: i32,
) -> Result<(), String> {
    // The next/previous shortcuts are keyboard-only because the overlay is
    // click-through. Scroll the single answer container instead of replacing
    // the answer with artificial pages.
    {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "application state unavailable")?;
        inner.answer_page = 0;
    }
    app.emit(
        "overlay-scroll",
        serde_json::json!({"delta": if delta >= 0 { 1 } else { -1 }}),
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}
fn move_overlay(app: AppHandle, dx: i32, dy: i32) -> Result<(), String> {
    let window = app
        .get_webview_window("overlay")
        .ok_or("overlay window is unavailable")?;
    let position = window.outer_position().map_err(|error| error.to_string())?;
    window
        .set_position(Position::Physical(PhysicalPosition::new(
            position.x + dx,
            position.y + dy,
        )))
        .map_err(|error| error.to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            let work_dir = data_dir.join("runtime");
            fs::create_dir_all(&work_dir)?;
            let history_root = data_dir.join("history");
            let history = match HistoryStore::new(history_root.clone()) {
                Ok(store) => store,
                Err(error) => {
                    eprintln!("local encrypted history disabled: {error}");
                    HistoryStore::unavailable(history_root)
                }
            };
            let config_path = data_dir.join("config.json");
            let state = Arc::new(AppState {
                inner: Mutex::new(Inner {
                    status: "idle".into(),
                    ..Default::default()
                }),
                presets: Mutex::new(load_presets(&data_dir.join("presets.json"))),
                presets_path: data_dir.join("presets.json"),
                config: Mutex::new(load_config(&config_path)),
                config_path,
                work_dir,
                history,
                local_server: Mutex::new(None),
                thumbnail_cache: Mutex::new(HashMap::new()),
            });
            let overlay_window = app
                .get_webview_window("overlay")
                .ok_or("overlay window missing")?;
            let tray_menu = Menu::with_items(
                app,
                &[
                    &MenuItem::with_id(app, "show", "打开配置窗口", true, None::<&str>)?,
                    &MenuItem::with_id(app, "quit", "退出宝宝巴士", true, None::<&str>)?,
                ],
            )?;
            TrayIconBuilder::new()
                .menu(&tray_menu)
                .tooltip("宝宝巴士")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            let protected = overlay::secure_overlay(&overlay_window).is_ok();
            state
                .inner
                .lock()
                .expect("application state lock")
                .protected_overlay = protected;
            app.manage(state);
            // Keep the app-level event bridge for future control transports.
            // LocalServer dispatches directly, while other transports can still
            // emit the same event without duplicating business logic.
            let remote_app = app.handle().clone();
            app.listen("remote-command", move |event| {
                let Ok(payload) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
                    return;
                };
                let command = payload
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if !matches!(command, "capture" | "submit" | "clear" | "cancel") {
                    return;
                }
                crate::dispatch_remote_command(
                    remote_app.clone(),
                    payload
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    command.to_owned(),
                    payload
                        .get("presetId")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("general")
                        .to_owned(),
                );
            }); // LAN commands are dispatched directly by LocalServer into the same business functions.
            let app_state = app.state::<Arc<AppState>>();
            let initial_config = app_state.config.lock().expect("config lock").clone();
            if initial_config.lan_control_enabled {
                if let Ok(server) = local_server::LocalServer::start(
                    app.handle().clone(),
                    app_state.inner().clone(),
                    initial_config.lan_control_port,
                ) {
                    *app_state.local_server.lock().expect("server lock") = Some(server);
                }
            }
            let config = app
                .state::<Arc<AppState>>()
                .config
                .lock()
                .expect("config lock")
                .clone();
            let _ = overlay_window.set_size(Size::Physical(PhysicalSize::new(
                config.overlay_width,
                config.overlay_height,
            )));
            if protected {
                let _ = overlay::show(&overlay_window);
            } else {
                let _ = overlay::hide(&overlay_window);
            }
            if let Err(error) = register_shortcuts(app.handle()) {
                let app_state = app.state::<Arc<AppState>>();
                let mut inner = app_state.inner.lock().expect("application state lock");
                inner.message = Some(format!(
                    "部分全局快捷键注册失败：{error}；请在配置窗口重新录制，鼠标操作仍可用。"
                ));
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            capture_for_preset,
            clear_draft,
            submit_draft,
            cancel_current_job,
            toggle_overlay,
            copy_preset,
            update_custom_preset,
            update_config,
            adjust_overlay_opacity,
            adjust_overlay_font_size,
            set_overlay_font_size,
            change_answer_page,
            set_overlay_opacity,
            sample_overlay_background,
            list_codex_models
        ])
        .run(tauri::generate_context!())
        .expect("宝宝巴士启动失败");
}
