import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { createRoot } from "react-dom/client";
import React from "react";
import "./style.css";

type Preset = { id: string; name: string; built_in: boolean; hotkey: string; task_template: string; version: number };
type Answer = { text: string };
type HotkeyName = "general" | "math" | "code" | "submit" | "clear" | "toggle" | "next" | "previous" | "opacity_up" | "opacity_down" | "font_up" | "font_down" | "move_left" | "move_right" | "move_up" | "move_down";
type Config = { provider: string; codex_path: string; overlay_opacity: number; overlay_theme: "day" | "night" | "follow"; overlay_font_size: number; overlay_width: number; overlay_height: number; codex_timeout_seconds: number; codex_model: string; prompt_addendum: string; lan_control_enabled: boolean; lan_control_port: number; mobile_auto_save_images: boolean; hotkeys: Record<HotkeyName, string> };
type Draft = { id: string; preset_id: string; image_count: number; thumbnails: string[]; created_at: string };
type Snapshot = { draft?: Draft; answer_preset_id?: string; presets: Preset[]; status: string; answer?: Answer; answer_page: number; protected_overlay: boolean; message?: string; stream_output: string; config: Config };
type CodexModel = { id: string; model: string; displayName: string; description: string; isDefault: boolean; hidden: boolean };

// React owns the window root; the existing feature renderer writes into the
// stable child container so state/event wiring remains shared across windows.
const root = createRoot(document.querySelector<HTMLDivElement>("#app")!);
root.render(React.createElement(React.Fragment, null));
// Keep a synchronous stable reference; React commits its child asynchronously.
// Legacy window renderers update this root after bootstrap without racing mount.
const app = document.querySelector<HTMLDivElement>("#app")!;
const windowLabel = getCurrentWindow().label;
let snapshot: Snapshot | undefined;
let codexModels: CodexModel[] = [];
let modelListMessage = "尚未读取 Codex CLI 模型列表";
const labels: Record<HotkeyName, string> = { general: "通用截图", math: "数学截图", code: "代码截图", submit: "提交", clear: "清空", toggle: "显隐浮窗", next: "下一页", previous: "上一页", opacity_up: "提高透明度", opacity_down: "降低透明度", font_up: "增大字体", font_down: "减小字体", move_left: "窗口左移", move_right: "窗口右移", move_up: "窗口上移", move_down: "窗口下移" };
const escapeHtml = (value: string) => value.replace(/[&<>'"]/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", "\"": "&quot;" }[character] ?? character));
async function refresh() { snapshot = await invoke<Snapshot>("get_snapshot"); render(); }
async function action(command: string, args: Record<string, unknown> = {}) { try { await invoke(command, args); await refresh(); } catch (error) { alert(String(error)); } }
async function loadCodexModels() {
  if (windowLabel === "overlay") return;
  if (snapshot?.config.provider !== "codex-cli") {
    codexModels = [];
    modelListMessage = "当前 Provider 不支持 Codex 模型目录";
    render();
    return;
  }
  modelListMessage = "正在从 Codex CLI 读取模型列表…";
  render();
  try {
    codexModels = await invoke<CodexModel[]>("list_codex_models");
    modelListMessage = codexModels.length ? `已读取 ${codexModels.length} 个可用模型` : "Codex CLI 返回了空模型列表";
    const configuredModel = snapshot?.config.codex_model.trim();
    if (configuredModel && codexModels.length && !codexModels.some((model) => model.id === configuredModel || model.model === configuredModel)) {
      modelListMessage += `；当前配置的 ${configuredModel} 不在列表中`;
    }
  } catch (error) {
    modelListMessage = `读取失败：${String(error)}`;
  }
  render();
}
async function sampleFollowBackground() {
  if (windowLabel !== "overlay" || snapshot?.config.overlay_theme !== "follow") return;
  try {
    const rgb = await invoke<string>("sample_overlay_background");
    document.documentElement.style.setProperty("--follow-rgb", rgb);
    const channels = rgb.split(/\s+/).map(Number);
    if (channels.length === 3 && channels.every(Number.isFinite)) {
      const [red, green, blue] = channels.map((channel) => channel / 255);
      const linear = (channel: number) => channel <= 0.03928 ? channel / 12.92 : Math.pow((channel + 0.055) / 1.055, 2.4);
      const luminance = 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue);
      document.documentElement.style.setProperty("--follow-fg", luminance > 0.52 ? "18 30 52" : "239 245 255");
      document.documentElement.style.setProperty("--follow-muted", luminance > 0.52 ? "58 76 105" : "190 207 235");
    }
  } catch {
    // A transparent window can briefly have no desktop DC during startup.
  }
}
function presetName(id?: string) { return snapshot?.presets.find((preset) => preset.id === id)?.name ?? "未选择题型"; }
function thumbnails(draft?: Draft) { return draft?.thumbnails.map((src, index) => `<div class="thumb"><img src="${src}" alt="第 ${index + 1} 张截图"><span>${index + 1}</span></div>`).join("") ?? ""; }

function renderOverlay() {
  const current = snapshot; const answer = current?.answer; const status = current?.status ?? "idle";
  document.documentElement.classList.add("overlay-document");
  const overlayAlpha = current?.config.overlay_opacity ?? 0.70;
  document.documentElement.style.setProperty("--overlay-alpha", String(overlayAlpha));
  document.documentElement.style.setProperty("--overlay-text-alpha", String(Math.max(0.82, overlayAlpha)));
  app.className = `overlay-root theme-${current?.config.overlay_theme ?? "follow"}`;
  if (status === "capturing") { app.innerHTML = ""; return; }
  if (answer) {
    // Keep pages sized for the compact overlay viewport rather than waiting
    // for a very long response; this makes paging useful at normal font sizes.
    const fontSize = current?.config.overlay_font_size ?? 18;
    const pageSize = Math.max(100, Math.floor(5200 / fontSize));
    const pages = Array.from({ length: Math.max(1, Math.ceil(answer.text.length / pageSize)) }, (_, index) => answer.text.slice(index * pageSize, (index + 1) * pageSize));
    const page = Math.min(current?.answer_page ?? 0, pages.length - 1);
    app.innerHTML = `<section class="answer-card"><div class="answer-meta">${escapeHtml(presetName(current?.answer_preset_id))}<span>第 ${page + 1} / ${pages.length} 页</span></div><pre class="answer-text" style="--answer-font-size:${fontSize}px">${escapeHtml(pages[page])}</pre><footer>${escapeHtml(current?.config.hotkeys.previous ?? "上一页")} / ${escapeHtml(current?.config.hotkeys.next ?? "下一页")} 翻页 · ${escapeHtml(current?.config.hotkeys.font_down ?? "减小字体")} / ${escapeHtml(current?.config.hotkeys.font_up ?? "增大字体")} 调整字号</footer></section>`;
    return;
  }
  const draft = current?.draft; const solving = status === "solving";
  app.innerHTML = `<section class="overlay-input"><div class="overlay-title"><span class="dot"></span><strong>${solving ? "正在处理" : "宝宝巴士"}</strong><span>${draft ? `${escapeHtml(presetName(draft.preset_id))} · ${draft.image_count}/6` : "按题型快捷键截图"}</span></div><textarea class="question-box" readonly placeholder="截图后将在此处建立题目草稿"></textarea><div class="thumb-grid">${thumbnails(draft)}</div>${solving ? `<div class="stream">正在处理，请稍候…</div>` : `<div class="input-placeholder">截图会按顺序出现在这里<br><small>快捷键提交或清空</small></div>`}</section>`;
}

function recorderValue(event: KeyboardEvent): string | undefined {
  const modifiers = [event.ctrlKey ? "Ctrl" : "", event.altKey ? "Alt" : "", event.shiftKey ? "Shift" : "", event.metaKey ? "Cmd" : ""].filter(Boolean);
  if (!modifiers.length || ["Control", "Alt", "Shift", "Meta"].includes(event.key)) return undefined;
  const keyNames: Record<string, string> = { " ": "Space", Enter: "Enter", Backspace: "Backspace", Escape: "Escape", ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right", PageUp: "PageUp", PageDown: "PageDown", BracketLeft: "[", BracketRight: "]" };
  const key = keyNames[event.key] ?? (event.code.startsWith("Key") ? event.code.slice(3) : event.code.startsWith("Digit") ? event.code.slice(5) : event.code.startsWith("F") ? event.code : event.key.length === 1 ? event.key.toUpperCase() : event.key);
  return [...modifiers, key].join("+");
}

function editPreset(preset: Preset) {
  const dialog = document.createElement("dialog");
  dialog.className = "editor-dialog";
  dialog.innerHTML = `<form method="dialog" class="editor-form"><div class="dialog-heading"><div><p class="eyebrow">PROMPT PRESET</p><h2>编辑题型 Prompt</h2></div><button type="button" class="dialog-close" data-close>×</button></div><label>名称<input name="name" value="${escapeHtml(preset.name)}" required></label><label>快捷键<input name="hotkey" value="${escapeHtml(preset.hotkey)}" placeholder="未设置"></label><label>让模型如何解题<textarea name="taskTemplate" rows="12" required>${escapeHtml(preset.task_template)}</textarea></label><p class="hint">这里编辑的是该题型的具体解题指令；全局 Prompt 附加要求会在请求时自动追加。</p><div class="dialog-actions"><button type="button" data-close>取消</button><button class="primary" value="save">保存预设</button></div></form>`;
  document.body.append(dialog);
  const close = () => { dialog.close(); dialog.remove(); };
  dialog.querySelectorAll<HTMLElement>("[data-close]").forEach((button) => button.addEventListener("click", close));
  dialog.addEventListener("cancel", close, { once: true });
  dialog.querySelector("form")?.addEventListener("submit", async (event) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget as HTMLFormElement);
    const name = String(data.get("name") ?? "").trim();
    const hotkey = String(data.get("hotkey") ?? "").trim() || "未设置";
    const taskTemplate = String(data.get("taskTemplate") ?? "").trim();
    if (!name || !taskTemplate) return;
    close();
    await action("update_custom_preset", { presetId: preset.id, name, hotkey, taskTemplate });
  });
  dialog.showModal();
}

function renderMain() {
  const current = snapshot!; const draft = current.draft; const config = current.config;
  const hotkeyRows = (Object.entries(config.hotkeys) as [HotkeyName, string][]).map(([key, value]) => `<label><span>${labels[key]}</span><input data-hotkey="${key}" value="${escapeHtml(value)}" readonly title="点击后按下组合键"></label>`).join("");
  const modelOptions = codexModels.map((model) => `<option value="${escapeHtml(model.id)}">${escapeHtml(model.displayName || model.id)}</option>`).join("");
  const effectiveModel = config.codex_model.trim() || codexModels.find((model) => model.isDefault)?.id || "Codex CLI 默认";
  app.className = "main-root";
  const providerCard = config.provider === "codex-cli" ? `<label class="wide-field"><span>Codex 路径</span><input data-config="codex_path" value="${escapeHtml(config.codex_path)}" placeholder="留空：自动搜索 PATH"></label><label class="wide-field"><span>超时（秒）</span><input data-config="codex_timeout_seconds" type="number" min="5" max="600" value="${config.codex_timeout_seconds}"></label><label class="wide-field"><span>模型</span><input list="codex-model-candidates" data-config="codex_model" value="${escapeHtml(config.codex_model)}" placeholder="模型会在启动时自动读取"><datalist id="codex-model-candidates">${modelOptions}</datalist></label><p class="current-model">当前生效模型：<strong>${escapeHtml(effectiveModel)}</strong>（每次请求通过 --model 传入；留空时使用 CLI 默认）</p><p class="hint model-status">${escapeHtml(modelListMessage)}</p>` : `<label class="wide-field"><span>Claude Code 路径</span><input data-config="codex_path" value="${escapeHtml(config.codex_path)}" placeholder="例如 claude 或 claude.cmd"></label><label class="wide-field"><span>超时（秒）</span><input data-config="codex_timeout_seconds" type="number" min="5" max="600" value="${config.codex_timeout_seconds}"></label><label class="wide-field"><span>模型（可选）</span><input data-config="codex_model" value="${escapeHtml(config.codex_model)}" placeholder="由 Claude Code CLI 决定"></label><p class="hint">Claude Code CLI 卡片已预留；当前版本会在适配器接入后启用求解。</p>`;
  app.innerHTML = `<main><header><div><p class="eyebrow">SOLVE LENS / CONTROL CENTER</p><h1>做题小助手</h1><p>后台配置、任务进度与运行诊断</p></div><span class="badge ${current.protected_overlay ? "ok" : "danger"}">${current.protected_overlay ? "前台保护已启用" : "保护失败：拒绝展示"}</span></header><section class="panel task-panel"><div class="panel-heading"><div><h2>当前任务</h2><p>${draft ? `${escapeHtml(presetName(draft.preset_id))} · ${draft.image_count}/6 张截图` : "尚未创建草稿"}</p></div><span class="status-pill">${escapeHtml(current.status)}</span></div><div class="thumb-grid main-thumbs">${thumbnails(draft)}</div><div class="actions">${current.presets.map((preset) => `<button data-capture="${preset.id}">截图：${escapeHtml(preset.name)} <kbd>${escapeHtml(preset.hotkey)}</kbd></button>`).join("")}</div><div class="actions secondary"><button data-action="submit" ${draft ? "" : "disabled"}>提交 <kbd>${escapeHtml(config.hotkeys.submit)}</kbd></button><button data-action="clear" ${draft ? "" : "disabled"}>清空 <kbd>${escapeHtml(config.hotkeys.clear)}</kbd></button><button data-action="cancel" ${current.status === "solving" ? "" : "disabled"}>取消</button></div>${current.stream_output ? `<pre class="stream main-stream">${escapeHtml(current.stream_output)}</pre>` : ""}<p class="message">${escapeHtml(current.message ?? "等待快捷键或操作")}</p></section><section class="panel"><h2>前台显示</h2><label class="wide-field"><span>前台背景不透明度 <output id="opacity-value">${Math.round(config.overlay_opacity * 100)}%</output></span><input data-config="overlay_opacity" type="range" min="0.05" max="0.95" step="0.05" value="${config.overlay_opacity}"></label><label class="wide-field"><span>颜色模式</span><select data-config="overlay_theme"><option value="follow" ${config.overlay_theme === "follow" ? "selected" : ""}>实时模仿窗口后方颜色</option><option value="day" ${config.overlay_theme === "day" ? "selected" : ""}>日间</option><option value="night" ${config.overlay_theme === "night" ? "selected" : ""}>夜间</select></label><p class="hint">透明度拖动时立即作用于前台浮窗；保存按钮用于持久化其它配置。</p></section><section class="panel provider-card"><div class="panel-heading"><div><h2>${config.provider === "codex-cli" ? "Codex CLI" : "Claude Code CLI"}</h2><p class="hint">Provider 决定使用哪一种解题程序。</p></div><label class="provider-switch"><span>Provider</span><select data-config="provider"><option value="codex-cli" ${config.provider === "codex-cli" ? "selected" : ""}>Codex CLI（本机登录）</option><option value="claude-code-cli" ${config.provider === "claude-code-cli" ? "selected" : ""}>Claude Code CLI（适配中）</option></select></label></div>${providerCard}</section><section class="panel"><h2>Prompt 配置</h2><label class="wide-field column-field"><span>全局 Prompt 附加要求</span><textarea data-config="prompt_addendum" rows="4" placeholder="例如：用中文回答，结论优先">${escapeHtml(config.prompt_addendum)}</textarea></label><p class="hint">安全规则始终保留；下面的内置题型模板也可以直接编辑并保存。</p></section><section class="panel"><h2>快捷键</h2><p class="hint">点击输入框后直接按下组合键，松开后自动填入；保存后重启生效。</p><div class="hotkeys">${hotkeyRows}</div><button data-action="save-config">保存全部配置</button></section><section class="panel"><h2>题型预设</h2><ul>${current.presets.map((preset) => `<li><strong>${escapeHtml(preset.name)}</strong><span>${preset.built_in ? "内置（可编辑）" : "自定义"} · ${escapeHtml(preset.hotkey)}</span><button data-edit="${preset.id}">编辑</button>${preset.built_in ? `<button data-copy="${preset.id}">复制</button>` : ""}</li>`).join("")}</ul></section><section class="panel muted"><p>前台窗口使用原生 click-through、置顶、无焦点和 Windows 捕获排除。透明度快捷键：${escapeHtml(config.hotkeys.opacity_up)} / ${escapeHtml(config.hotkeys.opacity_down)}。</p></section></main>`;
  app.innerHTML = app.innerHTML.replaceAll("SOLVE LENS", "宝宝巴士").replaceAll("Solve Lens", "宝宝巴士");
  const providerPanel = document.querySelector<HTMLElement>(".provider-card");
  if (providerPanel && !document.querySelector("[data-config=lan_control_enabled]")) {
    providerPanel.insertAdjacentHTML("afterend", `<section class="panel local-port-card"><div class="panel-heading"><div><h2>局域网控制端口</h2><p class="hint">开启后绑定所有网卡，手机可通过电脑局域网 IP 访问；默认关闭。</p></div><label class="toggle-field"><input data-config="lan_control_enabled" type="checkbox" ${config.lan_control_enabled ? "checked" : ""}><span>启用</span></label></div><label class="wide-field"><span>端口号</span><input data-config="lan_control_port" type="number" min="1024" max="65535" value="${config.lan_control_port}" ${config.lan_control_enabled ? "" : "disabled"}></label><label class="toggle-field mobile-save-toggle"><input data-config="mobile_auto_save_images" type="checkbox" ${config.mobile_auto_save_images ? "checked" : ""}><span>自动保存完整截图到手机</span></label><p class="hint port-status">${config.lan_control_enabled ? `局域网监听端口 ${config.lan_control_port}，手机访问 http://电脑IP:${config.lan_control_port}` : "已关闭，不会占用本地端口"}</p></section>`);
  }
  const portToggle = document.querySelector<HTMLInputElement>("[data-config=lan_control_enabled]");
  const portInput = document.querySelector<HTMLInputElement>("[data-config=lan_control_port]");
  const mobileSaveToggle = document.querySelector<HTMLInputElement>("[data-config=mobile_auto_save_images]");
  mobileSaveToggle?.addEventListener("change", () => { config.mobile_auto_save_images = mobileSaveToggle.checked; });
  portToggle?.addEventListener("change", () => { config.lan_control_enabled = portToggle.checked; if (portInput) portInput.disabled = !portToggle.checked; const status = document.querySelector<HTMLElement>(".port-status"); if (status) status.textContent = portToggle.checked ? `局域网监听端口 ${portInput?.value ?? config.lan_control_port}，手机访问电脑 IP 对应端口` : "已关闭，不会占用本地端口"; });
  portInput?.addEventListener("input", () => { config.lan_control_port = Number(portInput.value); const status = document.querySelector<HTMLElement>(".port-status"); if (status && portToggle?.checked) status.textContent = `配置端口 ${portInput.value}（当前版本尚未启动监听）`; });
  const themeField = document.querySelector<HTMLSelectElement>("[data-config=overlay_theme]")?.closest("label");
  if (themeField && !document.querySelector("[data-config=overlay_font_size]")) {
    themeField.insertAdjacentHTML("beforebegin", `<label class="wide-field"><span>字体大小 <output id="font-size-value">${config.overlay_font_size}px</output></span><input data-config="overlay_font_size" type="range" min="10" max="48" step="1" value="${config.overlay_font_size}"></label>`);
  }
  if (themeField && !document.querySelector("[data-config=overlay_width]")) {
    themeField.insertAdjacentHTML("beforebegin", `<div class="size-fields"><label class="wide-field"><span>窗口宽度</span><input data-config="overlay_width" type="number" min="240" max="1400" value="${config.overlay_width}"></label><label class="wide-field"><span>窗口高度</span><input data-config="overlay_height" type="number" min="160" max="1000" value="${config.overlay_height}"></label></div>`);
  }
  document.querySelector<HTMLInputElement>("[data-config=overlay_font_size]")?.addEventListener("input", (event) => { const value = Number((event.target as HTMLInputElement).value); document.querySelector("#font-size-value")!.textContent = `${value}px`; if (snapshot) snapshot.config.overlay_font_size = value; void invoke("set_overlay_font_size", { value }).catch((error) => console.warn(error)); });
  document.querySelectorAll<HTMLInputElement>("[data-config=overlay_width], [data-config=overlay_height]").forEach((input) => input.addEventListener("input", () => { const key = input.dataset.config as "overlay_width" | "overlay_height"; config[key] = Number(input.value); }));
  document.querySelectorAll<HTMLButtonElement>("[data-capture]").forEach((button) => button.addEventListener("click", () => action("capture_for_preset", { presetId: button.dataset.capture })));
  document.querySelector<HTMLButtonElement>("[data-action=submit]")?.addEventListener("click", () => action("submit_draft")); document.querySelector<HTMLButtonElement>("[data-action=clear]")?.addEventListener("click", () => action("clear_draft")); document.querySelector<HTMLButtonElement>("[data-action=cancel]")?.addEventListener("click", () => action("cancel_current_job"));
  document.querySelectorAll<HTMLInputElement>("[data-hotkey]").forEach((input) => input.addEventListener("keydown", (event) => { const value = recorderValue(event); if (!value) return; event.preventDefault(); input.value = value; }));
  document.querySelectorAll<HTMLButtonElement>("[data-copy]").forEach((button) => button.addEventListener("click", () => action("copy_preset", { presetId: button.dataset.copy })));
  document.querySelectorAll<HTMLButtonElement>("[data-edit]").forEach((button) => button.addEventListener("click", () => { const preset = snapshot?.presets.find((item) => item.id === button.dataset.edit); if (preset) editPreset(preset); }));
  document.querySelector<HTMLSelectElement>("[data-config=provider]")?.addEventListener("change", (event) => { const provider = (event.target as HTMLSelectElement).value; if (snapshot) snapshot.config.provider = provider; codexModels = provider === "codex-cli" ? codexModels : []; modelListMessage = provider === "codex-cli" ? "正在从 Codex CLI 读取模型列表…" : "Claude Code CLI 暂不提供模型目录"; render(); if (provider === "codex-cli") void loadCodexModels(); });
  document.querySelector<HTMLInputElement>("[data-config=overlay_opacity]")?.addEventListener("input", (event) => { const value = Number((event.target as HTMLInputElement).value); document.querySelector("#opacity-value")!.textContent = `${Math.round(value * 100)}%`; void invoke("set_overlay_opacity", { value }).catch((error) => console.warn(error)); });
  document.querySelector<HTMLButtonElement>("[data-action=refresh-models]")?.addEventListener("click", () => { void loadCodexModels(); });
  document.querySelector<HTMLButtonElement>("[data-action=save-config]")?.addEventListener("click", () => { const next: Config = { ...config, provider: document.querySelector<HTMLSelectElement>("[data-config=provider]")!.value, codex_path: document.querySelector<HTMLInputElement>("[data-config=codex_path]")!.value, overlay_opacity: Number(document.querySelector<HTMLInputElement>("[data-config=overlay_opacity]")!.value), overlay_theme: document.querySelector<HTMLSelectElement>("[data-config=overlay_theme]")!.value as Config["overlay_theme"], codex_timeout_seconds: Number(document.querySelector<HTMLInputElement>("[data-config=codex_timeout_seconds]")?.value ?? config.codex_timeout_seconds), codex_model: document.querySelector<HTMLInputElement>("[data-config=codex_model]")?.value ?? config.codex_model, prompt_addendum: document.querySelector<HTMLTextAreaElement>("[data-config=prompt_addendum]")!.value, hotkeys: { ...config.hotkeys } }; document.querySelectorAll<HTMLInputElement>("[data-hotkey]").forEach((input) => { next.hotkeys[input.dataset.hotkey as HotkeyName] = input.value; input.closest("label")?.querySelector(".hotkey-error")?.remove(); input.classList.remove("hotkey-conflict"); }); const seen = new Map<string, HTMLInputElement>(); let conflict = false; document.querySelectorAll<HTMLInputElement>("[data-hotkey]").forEach((input) => { const value = input.value.trim().toLowerCase(); if (seen.has(value) && value) { conflict = true; input.classList.add("hotkey-conflict"); seen.get(value)!.classList.add("hotkey-conflict"); [input, seen.get(value)!].forEach((item) => item.closest("label")?.insertAdjacentHTML("beforeend", `<small class="hotkey-error">与“${labels[item.dataset.hotkey as HotkeyName]}”冲突</small>`)); } else if (value) seen.set(value, input); }); if (conflict) return; void action("update_config", { config: next }).then(() => { if (next.provider === "codex-cli") return loadCodexModels(); }); });
}
function render() { if (windowLabel === "overlay") renderOverlay(); else { document.documentElement.classList.remove("overlay-document"); renderMain(); } }
async function bootstrap() {
  await listen<Snapshot>("state-changed", (event) => { snapshot = event.payload; render(); });
  if (windowLabel !== "overlay") {
    await listen<{ command: string; presetId?: string }>("remote-command", (event) => {
      const command = event.payload.command;
      if (command === "capture") void action("capture_for_preset", { presetId: event.payload.presetId ?? "general" });
      else if (command === "submit") void action("submit_draft");
      else if (command === "clear") void action("clear_draft");
      else if (command === "cancel") void action("cancel_current_job");
    });
  }
  await refresh();
  if (windowLabel === "overlay") {
    await sampleFollowBackground();
    window.setInterval(() => { void sampleFollowBackground(); }, 450);
  } else {
    await loadCodexModels();
  }
}
void bootstrap();
