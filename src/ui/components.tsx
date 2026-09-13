import React, { useEffect, useRef, useState } from "react";
import {
  Alert, Button as ArcoButton, Card, Divider, Grid, Input, InputNumber, Modal,
  Select, Slider, Space, Switch, Tag, Typography,
} from "@arco-design/web-react";
import { IconCheckCircle, IconClose, IconCopy, IconDelete, IconEdit, IconLoading, IconPause, IconPlayArrow, IconRefresh, IconSave, IconSettings, IconStop, IconUpload, IconPlus } from "@arco-design/web-react/icon";
import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event";
import type { CodexModelOption, CodexThreadOption, Config, Preset, Snapshot } from "../ui_types";
import type { PendingCommand } from "./context";
import { useAppState } from "./context";

const { Row, Col } = Grid;
const { Title, Text, Paragraph } = Typography;

function formatThreadDate(value?: string) {
  if (!value) return "";
  const numeric = Number(value);
  const date = Number.isFinite(numeric)
    ? new Date(numeric < 1e12 ? numeric * 1000 : numeric)
    : new Date(value);
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleString();
}

export function ToastView({ toast }: { toast?: { level: "info" | "success" | "error"; text: string } }) {
  if (!toast) return null;
  const icon = toast.level === "success" ? <IconCheckCircle /> : toast.level === "error" ? <IconClose /> : <IconLoading />;
  return <div className={`toast toast-${toast.level}`} role="status"><span className="toast-icon">{icon}</span><span>{toast.text}</span></div>;
}

function ActionButton({ children, danger, ...props }: React.ComponentProps<typeof ArcoButton> & { danger?: boolean }) {
  return <ArcoButton {...props} type={props.type ?? "secondary"} status={danger ? "danger" : props.status} className={`${props.className ?? ""} pressable`.trim()}>{children}</ArcoButton>;
}

export function ShortcutInput({ value, onChange, error }: { value: string; onChange: (value: string) => void; error?: string }) {
  const [focused, setFocused] = useState(false);
  const keyName = (event: React.KeyboardEvent<HTMLInputElement>) => {
    const mods: string[] = [];
    if (event.ctrlKey) mods.push("Ctrl");
    if (event.altKey) mods.push("Alt");
    if (event.shiftKey) mods.push("Shift");
    if (event.metaKey) mods.push("Super");
    const ignored = ["Control", "Alt", "Shift", "Meta"];
    if (ignored.includes(event.key)) return;
    const map: Record<string, string> = {
      " ": "Space", Escape: "Esc", Backspace: "Backspace", Enter: "Enter", Tab: "Tab",
      ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right",
      PageUp: "PageUp", PageDown: "PageDown", Home: "Home", End: "End", Insert: "Insert", Delete: "Delete",
    };
    const key = map[event.key] ?? (event.key.startsWith("F") && /^F\d+$/.test(event.key) ? event.key : event.code?.startsWith("Key") ? event.code.slice(3) : event.key.length === 1 ? event.key.toUpperCase() : event.key);
    if (!mods.length || !key) return;
    event.preventDefault();
    event.stopPropagation();
    onChange([...mods, key].join("+"));
  };
  return <div className="shortcut-field">
    <Input value={value} readOnly status={error ? "error" : undefined} aria-invalid={Boolean(error)} onFocus={() => setFocused(true)} onBlur={() => setFocused(false)} onKeyDown={keyName} placeholder={focused ? "按下组合键…" : "点击后按下快捷键"} suffix={focused ? <IconLoading /> : undefined} />
    {error && <div className="field-error">{error}</div>}
  </div>;
}

function ThumbGrid({ draft }: { draft?: Snapshot["draft"] }) {
  if (!draft?.thumbnails?.length) return <div className="empty-preview">截图后会在这里显示缩略图</div>;
  return <div className="thumb-grid">{draft.thumbnails.map((src, i) => <div className="thumb" key={`${i}-${src.slice(-20)}`}><img src={src} alt={`第 ${i + 1} 张`} /><span>{i + 1}</span></div>)}</div>;
}

export function TaskPanel({ snapshot, pending, onCommand }: { snapshot: Snapshot; pending?: PendingCommand; onCommand: (kind: PendingCommand["kind"], name: string, args?: Record<string, unknown>) => Promise<void> }) {
  const [selected, setSelected] = useState(snapshot.draft?.preset_id ?? snapshot.presets[0]?.id ?? "general");
  useEffect(() => { if (snapshot.draft?.preset_id) setSelected(snapshot.draft.preset_id); }, [snapshot.draft?.preset_id]);
  const presets = snapshot.presets.length ? snapshot.presets : [{ id: "general", name: "通用", built_in: true, hotkey: "", task_template: "", version: 1 }];
  const busy = Boolean(pending);
  return <Card className="panel-card task-card" title={<span><IconPlayArrow /> 题目工作台</span>} extra={<Tag color={snapshot.status === "idle" ? "gray" : "arcoblue"}>{snapshot.status || "idle"}</Tag>}>
    <Space direction="vertical" size={16}>
      <div className="task-toolbar"><Select value={selected} onChange={setSelected} className="preset-select" placeholder="选择题型">{presets.map(p => <Select.Option key={p.id} value={p.id}>{p.name}</Select.Option>)}</Select><Text type="secondary">快捷键截图可连续加入同一草稿，最多 6 张</Text></div>
      <ThumbGrid draft={snapshot.draft} />
      <div className="task-actions">
        <ActionButton type="primary" loading={pending?.kind === "capture"} disabled={busy} onClick={() => onCommand("capture", "capture_for_preset", { presetId: selected })}><IconUpload /> 截图加入</ActionButton>
        <ActionButton type="primary" loading={pending?.kind === "submit"} disabled={busy || !snapshot.draft?.image_count} onClick={() => onCommand("submit", "submit_draft")}><IconPlayArrow /> 提交解题</ActionButton>
        <ActionButton loading={pending?.kind === "clear"} disabled={busy || (!snapshot.draft && !snapshot.interactive_thread_id)} onClick={() => onCommand("clear", "clear_draft")}><IconDelete /> 清空</ActionButton>
        <ActionButton loading={pending?.kind === "cancel"} disabled={busy} onClick={() => onCommand("cancel", "cancel_current_job")}><IconStop /> 取消</ActionButton>
      </div>
      {snapshot.message && <Alert type={snapshot.status === "error" || snapshot.status === "failed" ? "error" : "info"} content={snapshot.message} showIcon />}
      {snapshot.answer?.text && <div className="answer-preview"><Text type="secondary">最近答案</Text><Paragraph ellipsis={{ rows: 4 }}>{snapshot.answer.text}</Paragraph></div>}
    </Space>
  </Card>;
}

export function ProviderCard({ config, setConfig, threadId }: { config: Config; setConfig: React.Dispatch<React.SetStateAction<Config>>; threadId?: string }) {
  const { snapshot } = useAppState();
  const [models, setModels] = useState<CodexModelOption[]>([]);
  const [loading, setLoading] = useState(false);
  const [threads, setThreads] = useState<CodexThreadOption[]>([]);
  const [threadsLoading, setThreadsLoading] = useState(false);
  const [threadsError, setThreadsError] = useState<string | undefined>(undefined);
  const loadTimer = useRef<number | undefined>(undefined);
  const loadModels = async () => {
    if (config.provider !== "codex-cli") { setModels([]); return; }
    setLoading(true);
    try {
      const result = await invoke<Array<CodexModelOption | string>>("list_codex_models");
      const normalized = Array.isArray(result) ? result.map(item => {
        if (typeof item === "string") return { id: item } as CodexModelOption;
        const id = item.id || item.model || item.displayName || "";
        return id ? { ...item, id } : null;
      }).filter((item): item is CodexModelOption => Boolean(item?.id)) : [];
      setModels(normalized);
    } catch { setModels([]); }
    finally { setLoading(false); }
  };
  useEffect(() => {
    window.clearTimeout(loadTimer.current);
    loadTimer.current = window.setTimeout(() => void loadModels(), 280);
    return () => window.clearTimeout(loadTimer.current);
  }, [config.provider, config.codex_path]);
  useEffect(() => {
    if (config.provider !== "codex-cli" || config.codex_execution_mode !== "interactive") { setThreads([]); setThreadsError(undefined); return; }
    setThreadsLoading(true);
    setThreadsError(undefined);
    invoke<CodexThreadOption[]>("list_codex_threads", { includeArchived: false })
      .then(value => setThreads(Array.isArray(value) ? value : []))
      .catch(error => { setThreads([]); setThreadsError(String(error)); })
      .finally(() => setThreadsLoading(false));
  }, [config.provider, config.codex_execution_mode, config.codex_path]);

  const defaultModel = models.find(model => model.isDefault);
  const selectedModel = models.find(model => model.id === config.codex_model) || (!config.codex_model ? defaultModel : undefined);
  const fallbackReasoning = [
    { reasoningEffort: "none", description: "最低延迟" },
    { reasoningEffort: "minimal", description: "轻量推理" },
    { reasoningEffort: "low", description: "较快" },
    { reasoningEffort: "medium", description: "均衡" },
    { reasoningEffort: "high", description: "深度" },
    { reasoningEffort: "xhigh", description: "更深" },
    { reasoningEffort: "max", description: "最大深度" },
    { reasoningEffort: "ultra", description: "最大深度 + 自动委派" },
  ];
  const reasoningOptions = selectedModel?.supportedReasoningEfforts?.length ? selectedModel.supportedReasoningEfforts : fallbackReasoning;
  const currentReasoning = config.codex_reasoning_effort || selectedModel?.defaultReasoningEffort || "low";
  const reasoningKnown = reasoningOptions.some(option => option.reasoningEffort === currentReasoning);
  const serviceTier = config.codex_service_tier || "default";
  const fastSupported = !selectedModel || selectedModel.serviceTiers?.some(tier => tier.id === "priority" || tier.id === "fast") || selectedModel.additionalSpeedTiers?.includes("fast");
  const modelLabel = selectedModel?.displayName || selectedModel?.id || "CLI 默认模型";
  return <Card className="panel-card" title={<span><IconSettings /> Provider 与模型</span>}>
    <div className="form-grid">
      <div className="form-field"><span className="field-label-text">答题 Provider</span><Select value={config.provider} onChange={v => setConfig(c => ({ ...c, provider: v }))}><Select.Option value="codex-cli">Codex CLI</Select.Option><Select.Option value="claude-code-cli">Claude Code CLI</Select.Option></Select></div>
      {config.provider === "codex-cli" ? <div className="form-field"><span className="field-label-text">执行模式</span><Select value={config.codex_execution_mode} onChange={v => setConfig(c => ({ ...c, codex_execution_mode: v as "exec" | "interactive" }))}><Select.Option value="exec">单题 exec（默认）</Select.Option><Select.Option value="interactive">连续上下文 interactive</Select.Option></Select></div> : null}
      {config.provider === "codex-cli" && config.codex_execution_mode === "interactive" ? <div className="form-field"><span className="field-label-text">已有会话</span><Select showSearch allowClear loading={threadsLoading} value={threadId || snapshot.interactive_thread_id} onChange={v => { if (v === "__new__") { void invoke("select_codex_thread", { threadId: "new" }); } else if (v) void invoke("select_codex_thread", { threadId: v }); }} placeholder={threadsLoading ? "正在读取会话…" : threadsError ? "会话读取失败" : threads.length ? "选择已有 Codex 会话" : "暂无可用会话"}><Select.Option value="__new__">新建会话（提交时自动创建）</Select.Option>{threads.map(thread => <Select.Option key={thread.id} value={thread.id}>{thread.name || thread.id}{formatThreadDate(thread.updatedAt) ? ` · ${formatThreadDate(thread.updatedAt)}` : ""}</Select.Option>)}</Select>{threadsError ? <Alert type="error" showIcon content={<span>读取 Codex 会话失败：<code>{threadsError}</code></span>} /> : !threadsLoading && !threads.length ? <div className="provider-hint">当前没有可列出的未归档会话。</div> : null}</div> : null}
      <label>CLI 路径<Input value={config.codex_path} onChange={v => setConfig(c => ({ ...c, codex_path: v }))} placeholder="codex 或 claude" /></label>
      <div className="form-field"><span className="field-label-text">模型</span><Select showSearch allowClear value={config.codex_model || undefined} loading={loading} onChange={v => setConfig(c => ({ ...c, codex_model: v ?? "" }))} placeholder={loading ? "正在读取模型…" : "选择模型"}>{models.map(model => <Select.Option key={model.id} value={model.id}>{model.displayName ? `${model.displayName} · ${model.id}` : model.id}</Select.Option>)}</Select></div>
      <div className="form-field"><span className="field-label-text">推理深度</span><Select value={config.codex_reasoning_effort || "__default__"} onChange={v => setConfig(c => ({ ...c, codex_reasoning_effort: v === "__default__" ? "" : v }))}><Select.Option value="__default__">跟随模型默认（{selectedModel?.defaultReasoningEffort || "CLI"}）</Select.Option>{reasoningOptions.map(option => <Select.Option key={option.reasoningEffort} value={option.reasoningEffort}>{option.reasoningEffort}{option.description ? ` · ${option.description}` : ""}</Select.Option>)}{!reasoningKnown && config.codex_reasoning_effort ? <Select.Option value={config.codex_reasoning_effort}>{config.codex_reasoning_effort} · 当前配置</Select.Option> : null}</Select></div>
      <div className="form-field"><span className="field-label-text">速度模式</span><Select value={serviceTier} onChange={v => setConfig(c => ({ ...c, codex_service_tier: v }))}><Select.Option value="default">标准 · 1×</Select.Option><Select.Option value="priority" disabled={Boolean(selectedModel && !fastSupported)}>Fast · 约 1.5×{selectedModel && !fastSupported ? "（当前模型不可用）" : ""}</Select.Option></Select></div>
      <label>超时（秒）<InputNumber min={10} max={3600} value={config.codex_timeout_seconds} onChange={v => setConfig(c => ({ ...c, codex_timeout_seconds: Number(v || 120) }))} /></label>
    </div>
    <div className="provider-hint">已读取 {models.length || "当前"} 个 CLI 模型；当前模型：{modelLabel}。{config.codex_execution_mode === "interactive" ? "交互模式会保留 Codex thread，上下文仅在清空时归档。" : "单题模式每次独立执行。"} Fast 使用 <code>service_tier=priority</code>，仅在模型/账号提供该层时可用。</div>
    <div className="current-value"><Tag color="green">当前生效 Provider：{config.provider === "codex-cli" ? "Codex CLI" : "Claude Code CLI"}</Tag><Tag color="arcoblue">当前模型：{config.codex_model || modelLabel}</Tag><Tag color="arcoblue">推理：{config.codex_reasoning_effort || `模型默认（${currentReasoning}）`}</Tag><Tag color={serviceTier === "priority" ? "orange" : "gray"}>速度：{serviceTier === "priority" ? "Fast · 约 1.5×" : "标准 · 1×"}</Tag></div>
  </Card>;
}

export function DisplayCard({ config, setConfig }: { config: Config; setConfig: React.Dispatch<React.SetStateAction<Config>> }) {
  const update = async (field: "overlay_opacity" | "overlay_font_size", value: number) => {
    setConfig(c => ({ ...c, [field]: value }));
    try { await invoke(field === "overlay_opacity" ? "set_overlay_opacity" : "set_overlay_font_size", { value }); } catch { /* saved on submit */ }
  };
  return <Card className="panel-card" title={<span><IconSettings /> 浮窗显示</span>}>
    <div className="form-grid">
      <div className="form-field"><span className="field-label-text">主题</span><Select value={config.overlay_theme} onChange={v => setConfig(c => ({ ...c, overlay_theme: v }))}><Select.Option value="day">日间</Select.Option><Select.Option value="night">夜间</Select.Option><Select.Option value="follow">实时跟随</Select.Option></Select></div>
      <label>窗口宽度<InputNumber min={280} max={1600} value={config.overlay_width} onChange={v => setConfig(c => ({ ...c, overlay_width: Number(v || 560) }))} /></label>
      <label>窗口高度<InputNumber min={160} max={1200} value={config.overlay_height} onChange={v => setConfig(c => ({ ...c, overlay_height: Number(v || 420) }))} /></label>
      <label>透明度 <span className="value-badge">{Math.round(config.overlay_opacity * 100)}%</span><Slider min={0.2} max={1} step={0.01} value={config.overlay_opacity} onChange={v => void update("overlay_opacity", Number(v))} /></label>
      <label>字体大小 <span className="value-badge">{config.overlay_font_size}px</span><Slider min={12} max={48} value={config.overlay_font_size} onChange={v => void update("overlay_font_size", Number(v))} /></label>
    </div>
  </Card>;
}

export function LanControlCard({ config, setConfig }: { config: Config; setConfig: React.Dispatch<React.SetStateAction<Config>> }) {
  return <Card className="panel-card" title="局域网手机控制">
    <div className="form-grid"><label className="switch-line">启用局域网控制<Switch checked={config.lan_control_enabled} onChange={v => setConfig(c => ({ ...c, lan_control_enabled: v }))} /></label><label>端口<InputNumber min={1024} max={65535} value={config.lan_control_port} onChange={v => setConfig(c => ({ ...c, lan_control_port: Number(v || 18765) }))} /></label></div>
    <label className="switch-line">手机自动保存完整截图<Switch checked={config.mobile_auto_save_images} onChange={v => setConfig(c => ({ ...c, mobile_auto_save_images: v }))} /></label>
    <Text type="secondary">关闭局域网控制后会释放监听端口；端口和开关未变化时不会重复重启服务。</Text><Text type="secondary">手机业务数据使用临时 ECDH 会话与 AES-256-GCM 加密传输。</Text>
  </Card>;
}

export function PromptCard({ config, setConfig }: { config: Config; setConfig: React.Dispatch<React.SetStateAction<Config>> }) {
  return <Card className="panel-card" title={<span className="card-title-with-icon"><IconEdit />全局 Prompt 附加要求</span>}><Input.TextArea value={config.prompt_addendum} onChange={v => setConfig(c => ({ ...c, prompt_addendum: v }))} autoSize={{ minRows: 4, maxRows: 8 }} placeholder="例如：用简体中文回答，结论优先，必要时核对单位。" /></Card>;
}

const hotkeyLabels: Record<string, string> = { general: "通用截图", math: "数学截图", code: "代码截图", submit: "提交", clear: "清空", toggle: "显隐浮窗", previous: "上一页", next: "下一页", opacity_down: "降低透明度", opacity_up: "提高透明度", font_down: "减小字号", font_up: "增大字号", move_up: "上移浮窗", move_down: "下移浮窗", move_left: "左移浮窗", move_right: "右移浮窗", port_check: "检测本地端口" };

export function HotkeysCard({ config, setConfig, onValidityChange }: { config: Config; setConfig: React.Dispatch<React.SetStateAction<Config>>; onValidityChange?: (valid: boolean) => void }) {
  const [errors, setErrors] = useState<Record<string, string>>({});
  const entries = Object.keys(hotkeyLabels);
  const validate = () => {
    const seen = new Map<string, string>(); const next: Record<string, string> = {};
    entries.forEach(k => { const v = config.hotkeys[k]; if (!v) return; const prior = seen.get(v.toLowerCase()); if (prior) { next[k] = "与“" + hotkeyLabels[prior] + "”冲突"; next[prior] = "与“" + hotkeyLabels[k] + "”冲突"; } else seen.set(v.toLowerCase(), k); });
    setErrors(next); onValidityChange?.(Object.keys(next).length === 0); return Object.keys(next).length === 0;
  };
  useEffect(() => { void validate(); }, [config.hotkeys]);
  const change = (key: string, value: string) => { setConfig(c => ({ ...c, hotkeys: { ...c.hotkeys, [key]: value } })); };
  return <Card className="panel-card" title="全局快捷键"><div className="hotkey-grid">{entries.map(k => <label key={k}>{hotkeyLabels[k]}<ShortcutInput value={config.hotkeys[k] ?? ""} onChange={v => change(k, v)} error={errors[k]} /></label>)}</div><div className="hotkey-hint">点击输入框后直接按下组合键，建议使用 Ctrl/Alt/Shift 作为修饰键。</div><ActionButton type="secondary" onClick={() => void validate()}>检查冲突</ActionButton></Card>;
}
export function PresetList({ snapshot, onRefresh }: { snapshot: Snapshot; onRefresh: () => Promise<void> }) {
  const [editing, setEditing] = useState<Preset | undefined>(undefined);
  const [draft, setDraft] = useState<Preset | undefined>(undefined);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);
  const open = (preset: Preset) => { setEditing(preset); setDraft({ ...preset }); setError(undefined); };
  const close = () => { if (!saving) { setEditing(undefined); setDraft(undefined); setError(undefined); } };
  const save = async () => {
    if (!draft || saving) return;
    if (!draft.name.trim() || !draft.task_template.trim()) { setError("名称和题型解题 Prompt 不能为空"); return; }
    setSaving(true);
    setError(undefined);
    try {
      // The same command intentionally handles built-in and custom presets.
      // Built-in IDs are kept so their edited template survives a restart.
      await invoke("update_custom_preset", { presetId: draft.id, name: draft.name.trim(), hotkey: draft.hotkey.trim(), taskTemplate: draft.task_template.trim() });
      setEditing(undefined);
      setDraft(undefined);
      await onRefresh();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setSaving(false);
    }
  };
  const copy = async (preset: Preset) => { try { await invoke("copy_preset", { presetId: preset.id }); await onRefresh(); } catch (cause) { setError(String(cause)); } };
  return <Card className="panel-card preset-card" title={<span className="card-title-with-icon"><IconEdit />题型 Prompt 预设</span>}>
    <div className="preset-card-hint">内置预设也可以直接编辑；修改会保存到本机并在下次启动继续使用。</div>
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      {snapshot.presets.map(p => <div className="preset-row" key={p.id} role="button" tabIndex={0} onClick={() => open(p)} onKeyDown={event => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); open(p); } }}>
        <div className="preset-copy">
          <div className="preset-heading"><Text bold>{p.name}</Text><Tag color={p.built_in ? "arcoblue" : "gray"}>{p.built_in ? "内置 · 可编辑" : "自定义"}</Tag></div>
          <div className="preset-template" title={p.task_template}>{p.task_template}</div>
        </div>
        <div className="preset-actions" onClick={event => event.stopPropagation()}>
          <ActionButton type="secondary" icon={<IconEdit />} onClick={() => open(p)}>编辑 Prompt</ActionButton>
          {p.built_in && <ActionButton type="text" icon={<IconCopy />} onClick={() => void copy(p)}>复制</ActionButton>}
        </div>
      </div>)}
    </Space>
    <Modal title={draft?.built_in ? `编辑内置预设：${draft.name}` : "编辑题型预设"} visible={Boolean(editing && draft)} confirmLoading={saving} onCancel={close} onOk={() => void save()} autoFocus={false}>
      <Space direction="vertical" size={14} style={{ width: "100%" }}>
        <label className="dialog-field">预设名称<Input value={draft?.name ?? ""} onChange={value => setDraft(current => current && ({ ...current, name: value }))} placeholder="例如：数学" /></label>
        <label className="dialog-field">截图快捷键<Input value={draft?.hotkey ?? ""} onChange={value => setDraft(current => current && ({ ...current, hotkey: value }))} placeholder="例如：Ctrl+Alt+2" /></label>
        <label className="dialog-field">让模型如何根据图片解题<Input.TextArea value={draft?.task_template ?? ""} onChange={value => setDraft(current => current && ({ ...current, task_template: value }))} autoSize={{ minRows: 8, maxRows: 16 }} placeholder="完整题型解题 Prompt：请说明模型如何根据图片识别、分析和作答。" /></label>
        <Text type="secondary">这里编辑的是该题型的具体解题指令；全局 Prompt 附加要求会在请求时自动追加。</Text>
        {error && <Alert type="error" content={error} showIcon />}
      </Space>
    </Modal>
  </Card>;
}export function ConfigPage() {
  const { snapshot, pending, toast, command, updateConfig, refresh, notify } = useAppState();
  const [config, setConfig] = useState<Config>(snapshot.config);
  const [dirty, setDirty] = useState(false);
  const [hotkeysValid, setHotkeysValid] = useState(true);
  useEffect(() => { if (!dirty) setConfig(snapshot.config); }, [snapshot.config, dirty]);
  const setConfigTracked: React.Dispatch<React.SetStateAction<Config>> = value => { setDirty(true); setConfig(value); };
  const save = async () => { if (!hotkeysValid) { notify("error", "请先修正标红的快捷键冲突"); return; } await updateConfig(config); setDirty(false); };
  return <div className="desktop-shell"><header className="app-header"><div><Title heading={2}>宝宝巴士</Title><Text type="secondary">截图做题助手 · 配置中心</Text></div><ActionButton type="primary" icon={<IconSave />} onClick={() => void save()}>保存配置</ActionButton></header><div className="system-strip"><div className={`protection-status ${snapshot.protected_overlay ? "is-protected" : "is-unverified"}`}><span className="protection-dot" /><div><strong>浮窗捕获保护</strong><small>{snapshot.protected_overlay ? "已启用 · 答案可安全展示" : "待确认 · 暂不展示答案"}</small></div></div><Text type="secondary">保护状态独立于任务提示，配置保存后自动生效</Text></div><main className="config-main"><TaskPanel snapshot={snapshot} pending={pending} onCommand={command} /><Row gutter={[16, 16]}><Col xs={24} md={12}><ProviderCard config={config} setConfig={setConfigTracked} /><DisplayCard config={config} setConfig={setConfigTracked} /><LanControlCard config={config} setConfig={setConfigTracked} /></Col><Col xs={24} md={12}><PromptCard config={config} setConfig={setConfigTracked} /><HotkeysCard config={config} setConfig={setConfigTracked} onValidityChange={setHotkeysValid} /><PresetList snapshot={snapshot} onRefresh={refresh} /></Col></Row></main><ToastView toast={toast} /></div>;
}

export function OverlayApp() {
  const { snapshot } = useAppState();
  const [background, setBackground] = useState<[number, number, number]>([22, 30, 48]);
  const answerRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<{ delta: number }>("overlay-scroll", event => {
      const element = answerRef.current;
      if (!element || disposed) return;
      const direction = event.payload?.delta >= 0 ? 1 : -1;
      const distance = Math.max(96, Math.floor(element.clientHeight * 0.78));
      element.scrollBy({ top: direction * distance, behavior: "smooth" });
    }).then(fn => {
      if (disposed) fn(); else unlisten = fn;
    }).catch(() => undefined);
    return () => { disposed = true; unlisten?.(); };
  }, []);
  useEffect(() => { answerRef.current?.scrollTo({ top: 0 }); }, [snapshot.answer?.text]);  useEffect(() => {
    if (snapshot.config.overlay_theme !== "follow") return;
    let active = true;
    const sample = async () => {
      try {
        const value = await invoke<string>("sample_overlay_background");
        const rgb = value.split(/\s+/).map(Number);
        if (active && rgb.length === 3 && rgb.every(Number.isFinite)) setBackground([rgb[0], rgb[1], rgb[2]]);
      } catch { /* sampling is best effort while the window is moving */ }
    };
    void sample();
    const timer = window.setInterval(() => void sample(), 1800);
    return () => { active = false; window.clearInterval(timer); };
  }, [snapshot.config.overlay_theme]);
  const luminance = (0.2126 * background[0] + 0.7152 * background[1] + 0.0722 * background[2]) / 255;
  const darkPanel = snapshot.config.overlay_theme === "night" ? true : snapshot.config.overlay_theme === "day" ? false : luminance < 0.54;
  const alpha = Math.max(0.12, Math.min(0.95, snapshot.config.overlay_opacity));
  const panel = darkPanel ? "rgba(10, 18, 30, " + alpha + ")" : "rgba(248, 251, 255, " + alpha + ")";
  const textAlpha = Math.max(0.74, Math.min(1, alpha + 0.12));
  const foreground = darkPanel ? "rgba(248, 251, 255, " + textAlpha + ")" : "rgba(16, 24, 39, " + textAlpha + ")";
  return <div className="overlay-root"><div className="overlay-content" style={{ backgroundColor: panel, color: foreground, fontSize: snapshot.config.overlay_font_size }}><div className="overlay-meta" style={{ color: darkPanel ? "rgba(255,255,255,.74)" : "rgba(16,24,39,.72)" }}><span>宝宝巴士 · {snapshot.draft?.image_count ?? 0} 张图片</span><span>{snapshot.status}</span></div>{snapshot.draft?.thumbnails?.length ? <div className="overlay-thumbs">{snapshot.draft.thumbnails.map((src, i) => <img key={i} src={src} alt={"题图 " + (i + 1)} />)}</div> : null}{snapshot.answer?.text ? <div ref={answerRef} className="overlay-answer">{snapshot.answer.text}</div> : <div className="overlay-wait" style={{ color: darkPanel ? "rgba(255,255,255,.72)" : "rgba(16,24,39,.68)" }}>等待答案…</div>}</div></div>;
}
