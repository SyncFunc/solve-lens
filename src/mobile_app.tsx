import React, { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { Alert, Button, Card, ConfigProvider, Select, Space, Switch, Tag, Typography } from "@arco-design/web-react";
import { IconCheckCircle, IconClose, IconLoading, IconPlayArrow, IconUpload } from "@arco-design/web-react/icon";
import { gcm } from "@noble/ciphers/aes.js";
import { p256 } from "@noble/curves/nist.js";
import { sha256 } from "@noble/hashes/sha256.js";
import "@arco-design/web-react/dist/css/arco.css";
import "./mobile_ui.css";
import type { Snapshot } from "./ui_types";

const { Title, Text, Paragraph } = Typography;
type MobileState = {
  status: string;
  message?: string;
  answer?: Snapshot["answer"];
  draft?: Snapshot["draft"];
  config: Partial<Snapshot["config"]>;
  interactive_thread_id?: Snapshot["interactive_thread_id"];
  has_openai_history?: Snapshot["has_openai_history"];
  openai_history_turn_count?: Snapshot["openai_history_turn_count"];
  presets?: Snapshot["presets"];
  quick_request_id?: string | null;
  quick_request_status?: string | null;
};
type Pending = { id: string; kind: "capture" | "submit" | "clear" | "cancel"; before: number };
type QuickAttempt = { id: string; stage: "sending" | "accepted" | "succeeded" | "failed" };
type MobileSettingsPatch = Partial<Pick<Snapshot["config"], "auto_submit_after_capture" | "conversation_mode">>;
type CryptoKeyMaterial = CryptoKey | Uint8Array;
type CryptoSession = { id: string; key: CryptoKeyMaterial; native: boolean };
type CryptoEnvelope = { encrypted?: boolean; iv?: string; data?: string };

function bytesToBase64(input: ArrayBuffer | Uint8Array) {
  const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, Math.min(offset + 0x8000, bytes.length)));
  }
  return btoa(binary);
}

function base64ToBytes(value: string) {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

function randomIv() {
  if (!globalThis.crypto?.getRandomValues) throw new Error("当前浏览器不支持安全随机数");
  return globalThis.crypto.getRandomValues(new Uint8Array(12));
}

async function requestHandshake(clientPublic: ArrayBuffer | Uint8Array) {
  const handshake = await fetch("/api/handshake", {
    method: "POST",
    cache: "no-store",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ clientPublic: bytesToBase64(clientPublic) }),
  });
  if (!handshake.ok) throw new Error("加密握手失败（HTTP " + handshake.status + "）");
  const result = await handshake.json() as { sessionId?: string; serverPublic?: string; error?: string };
  if (!result.sessionId || !result.serverPublic) throw new Error(result.error || "加密握手响应无效");
  return { id: result.sessionId, serverPublic: base64ToBytes(result.serverPublic) };
}

async function createCryptoSession(): Promise<CryptoSession> {
  const subtle = globalThis.crypto?.subtle;
  if (subtle) {
    const pair = await subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"]);
    const clientPublic = await subtle.exportKey("raw", pair.publicKey);
    const handshake = await requestHandshake(clientPublic);
    const serverPublic = await subtle.importKey("raw", handshake.serverPublic, { name: "ECDH", namedCurve: "P-256" }, false, []);
    const shared = await subtle.deriveBits({ name: "ECDH", public: serverPublic }, pair.privateKey, 256);
    const digest = await subtle.digest("SHA-256", shared);
    const key = await subtle.importKey("raw", digest, { name: "AES-GCM" }, false, ["encrypt", "decrypt"]);
    return { id: handshake.id, key, native: true };
  }

  // SubtleCrypto is often unavailable on a phone's plain HTTP LAN origin.
  // Noble keeps the same P-256 + AES-256-GCM protocol available there.
  const clientPrivate = p256.utils.randomSecretKey();
  const clientPublic = p256.getPublicKey(clientPrivate, false);
  const handshake = await requestHandshake(clientPublic);
  const sharedPoint = p256.getSharedSecret(clientPrivate, handshake.serverPublic, false);
  const digest = sha256(sharedPoint.slice(1, 33));
  return { id: handshake.id, key: digest, native: false };
}

async function encryptPayload(session: CryptoSession, value: unknown) {
  const iv = randomIv();
  const plaintext = new TextEncoder().encode(JSON.stringify(value));
  const ciphertext = session.native
    ? new Uint8Array(await globalThis.crypto.subtle.encrypt({ name: "AES-GCM", iv }, session.key as CryptoKey, plaintext))
    : gcm(session.key as Uint8Array, iv).encrypt(plaintext);
  return JSON.stringify({ encrypted: true, iv: bytesToBase64(iv), data: bytesToBase64(ciphertext) });
}

async function decryptPayload<T>(session: CryptoSession, source: Response | string): Promise<T> {
  const raw = typeof source === "string" ? source : await source.text();
  const envelope = JSON.parse(raw) as CryptoEnvelope;
  if (envelope.encrypted !== true || !envelope.iv || !envelope.data) throw new Error("收到未加密或格式无效的数据");
  const iv = base64ToBytes(envelope.iv);
  const data = base64ToBytes(envelope.data);
  const plaintext = session.native
    ? new Uint8Array(await globalThis.crypto.subtle.decrypt({ name: "AES-GCM", iv }, session.key as CryptoKey, data))
    : gcm(session.key as Uint8Array, iv).decrypt(data);
  return JSON.parse(new TextDecoder().decode(plaintext)) as T;
}

function toastIcon(level: string) { return level === "success" ? <IconCheckCircle /> : level === "error" ? <IconClose /> : <IconLoading />; }

function MobileApp() {
  const [state, setState] = useState<MobileState | undefined>(undefined);
  const [connection, setConnection] = useState<"connecting" | "connected" | "reconnecting" | "failed">("connecting");
  const [toast, setToast] = useState<{ level: "info" | "success" | "error"; text: string } | undefined>(undefined);
  const [pending, setPending] = useState<Pending | undefined>(undefined);
  const [quickAttempt, setQuickAttempt] = useState<QuickAttempt | undefined>(undefined);
  const [settingsUpdating, setSettingsUpdating] = useState(false);
  const [preset, setPreset] = useState("general");
  const wsRef = useRef<WebSocket | undefined>(undefined);
  const cryptoRef = useRef<CryptoSession | undefined>(undefined);
  const cryptoPromiseRef = useRef<Promise<CryptoSession> | undefined>(undefined);
  const pollTimer = useRef<number | undefined>(undefined);
  const retryTimer = useRef<number | undefined>(undefined);
  const retry = useRef(0);
  const alive = useRef(true);
  const toastTimer = useRef<number | undefined>(undefined);
  const pendingTimer = useRef<number | undefined>(undefined);
  const quickTimer = useRef<number | undefined>(undefined);
  const pendingPollTimer = useRef<number | undefined>(undefined);
  const quickPollTimer = useRef<number | undefined>(undefined);
  const downloadedCount = useRef(0);
  const notify = useCallback((level: "info" | "success" | "error", text: string) => { setToast({ level, text }); if (toastTimer.current) clearTimeout(toastTimer.current); toastTimer.current = window.setTimeout(() => setToast(undefined), 3200); }, []);
  const applyState = useCallback((next: MobileState) => {
    if (!alive.current || !next) return;
    setState(prev => next.config?.simple_mode_enabled
      ? { status: next.status, config: next.config, quick_request_id: next.quick_request_id, quick_request_status: next.quick_request_status }
      : ({ ...(prev ?? {}), ...next }));
  }, []);
  const ensureCryptoSession = useCallback((): Promise<CryptoSession> => {
    if (cryptoRef.current) return Promise.resolve(cryptoRef.current);
    if (!cryptoPromiseRef.current) {
      const promise = createCryptoSession();
      cryptoPromiseRef.current = promise;
      void promise.then(session => { cryptoRef.current = session; }).finally(() => { cryptoPromiseRef.current = undefined; });
    }
    return cryptoPromiseRef.current;
  }, []);
  const poll = useCallback(async () => {
    try {
      const session = await ensureCryptoSession();
      const response = await fetch(`/api/state?t=${Date.now()}`, { cache: "no-store", headers: { "X-Baobao-Session": session.id } });
      if (!response.ok) { if (response.status === 401) cryptoRef.current = undefined; throw new Error(`HTTP ${response.status}`); }
      applyState(await decryptPayload<MobileState>(session, response));
      if (wsRef.current?.readyState !== WebSocket.OPEN) setConnection("connected");
    } catch {
      if (wsRef.current?.readyState !== WebSocket.OPEN) setConnection("reconnecting");
    } finally {
      if (alive.current && wsRef.current?.readyState !== WebSocket.OPEN) pollTimer.current = window.setTimeout(poll, 1800);
    }
  }, [applyState, ensureCryptoSession]);
  const connect = useCallback(() => {
    if (!alive.current || wsRef.current?.readyState === WebSocket.OPEN || wsRef.current?.readyState === WebSocket.CONNECTING) return;
    void ensureCryptoSession().then(session => {
      if (!alive.current || wsRef.current?.readyState === WebSocket.OPEN || wsRef.current?.readyState === WebSocket.CONNECTING) return;
      const protocol = location.protocol === "https:" ? "wss" : "ws";
      const ws = new WebSocket(`${protocol}://${location.host}/ws?session=${encodeURIComponent(session.id)}`);
      wsRef.current = ws;
      ws.onopen = () => { retry.current = 0; setConnection("connected"); if (pollTimer.current) clearTimeout(pollTimer.current); void poll(); };
      ws.onmessage = event => { void decryptPayload<{ snapshot?: MobileState } & MobileState>(session, String(event.data)).then(packet => applyState(packet.snapshot ?? packet)).catch(() => undefined); };
      ws.onerror = () => { if (alive.current) setConnection("reconnecting"); };
      ws.onclose = () => { if (!alive.current) return; if (pollTimer.current) clearTimeout(pollTimer.current); setConnection("reconnecting"); void poll(); const delay = Math.min(15000, 700 * 2 ** retry.current++); retryTimer.current = window.setTimeout(connect, delay); };
    }).catch(() => {
      if (!alive.current) return;
      setConnection("reconnecting");
      const delay = Math.min(15000, 700 * 2 ** retry.current++);
      retryTimer.current = window.setTimeout(connect, delay);
    });
  }, [applyState, ensureCryptoSession, poll]);  useEffect(() => { document.documentElement.dataset.theme = state?.config.overlay_theme === "day" ? "day" : "night"; }, [state?.config.overlay_theme]);
  useEffect(() => { alive.current = true; void poll(); connect(); return () => { alive.current = false; if (pollTimer.current) clearTimeout(pollTimer.current); if (retryTimer.current) clearTimeout(retryTimer.current); if (toastTimer.current) clearTimeout(toastTimer.current); if (pendingTimer.current) clearTimeout(pendingTimer.current); if (quickTimer.current) clearTimeout(quickTimer.current); if (pendingPollTimer.current) clearInterval(pendingPollTimer.current); if (quickPollTimer.current) clearInterval(quickPollTimer.current); wsRef.current?.close(); }; }, [connect, poll]);
  useEffect(() => {
    const count = state?.draft?.image_count ?? 0;
    if (!state?.config.mobile_auto_save_images || !state?.draft) {
      if (count === 0) downloadedCount.current = 0;
      return;
    }
    if (count <= downloadedCount.current) return;
    state.draft.thumbnails.slice(downloadedCount.current, count).forEach((src, offset) => {
      const anchor = document.createElement("a");
      anchor.href = src;
      anchor.download = "baobao-bashi-" + (downloadedCount.current + offset + 1) + ".png";
      anchor.click();
    });
    downloadedCount.current = count;
  }, [state?.draft?.image_count, state?.config.mobile_auto_save_images]);  useEffect(() => {
    if (!pending || !state) return;
    const count = state.draft?.image_count ?? 0;
    if (pending.kind === "capture" && count > pending.before) { if (pendingTimer.current) clearTimeout(pendingTimer.current); setPending(undefined); notify("success", "截图已成功加入草稿"); }
    else if (pending.kind === "submit" && ["displaying", "done", "completed"].includes(state.status)) { if (pendingTimer.current) clearTimeout(pendingTimer.current); setPending(undefined); notify("success", "答案已生成"); }
    else if (pending.kind === "clear" && !state.draft) { if (pendingTimer.current) clearTimeout(pendingTimer.current); setPending(undefined); notify("success", "草稿已清空"); }
    else if (pending.kind === "cancel" && ["idle", "cancelled", "canceled"].includes(state.status)) { if (pendingTimer.current) clearTimeout(pendingTimer.current); setPending(undefined); notify("success", "任务已取消"); }
    else if (["error", "failed"].includes(state.status) && state.message) { if (pendingTimer.current) clearTimeout(pendingTimer.current); setPending(undefined); notify("error", state.message); }
  }, [pending, state, notify]);
  useEffect(() => {
    if (pendingPollTimer.current) clearInterval(pendingPollTimer.current);
    if (!pending) return;
    // Keep polling while a command is pending even when a WebSocket reports
    // OPEN but has stopped delivering frames. This is the completion safety
    // net for mobile browsers and captive-network proxies.
    void poll();
    pendingPollTimer.current = window.setInterval(() => { void poll(); }, 800);
    return () => { if (pendingPollTimer.current) clearInterval(pendingPollTimer.current); };
  }, [pending?.id, poll]);
  const updateAnswerSettings = async (patch: MobileSettingsPatch) => {
    if (!state || settingsUpdating) return;
    const before = state.config;
    setSettingsUpdating(true);
    setState(current => current ? { ...current, config: { ...current.config, ...patch } } : current);
    try {
      const session = await ensureCryptoSession();
      const payload = await encryptPayload(session, patch);
      const response = await fetch("/api/config", {
        method: "POST",
        cache: "no-store",
        headers: { "Content-Type": "application/json", "X-Baobao-Session": session.id },
        body: payload,
      });
      const result = await decryptPayload<{ accepted?: boolean; error?: string }>(session, response);
      if (!response.ok || result.accepted !== true) {
        throw new Error(result.error || `配置保存失败（HTTP ${response.status}）`);
      }
      notify("success", "答题设置已保存");
      void poll();
    } catch (error) {
      setState(current => current ? { ...current, config: before } : current);
      notify("error", error instanceof Error ? error.message : String(error));
    } finally {
      setSettingsUpdating(false);
    }
  };
  const command = async (kind: Pending["kind"], action: string, preset?: string) => {
    const id = String(Date.now()) + "-" + Math.random().toString(16).slice(2);
    if (pendingTimer.current) clearTimeout(pendingTimer.current);
    setPending({ id, kind, before: state?.draft?.image_count ?? 0 });
    pendingTimer.current = window.setTimeout(() => { setPending(undefined); notify("error", "操作等待超时，请检查桌面端连接"); }, 30000);
    notify("info", kind === "capture" ? "正在发送截图请求" : "正在发送操作请求");
    try {
      const session = await ensureCryptoSession();
      const payload = await encryptPayload(session, { id, ...(preset ? { presetId: preset } : {}) });
      const controller = new AbortController();
      const requestTimer = window.setTimeout(() => controller.abort(), 8000);
      const response = await fetch("/api/" + action, { method: "POST", cache: "no-store", headers: { "Content-Type": "application/json", "X-Baobao-Session": session.id }, body: payload, signal: controller.signal });
      clearTimeout(requestTimer);
      if (!response.ok) { if (response.status === 401) cryptoRef.current = undefined; throw new Error("请求失败（" + response.status + "）"); }
      await decryptPayload<{ accepted: boolean }>(session, response);
      notify("info", "请求已发送，等待完成确认");
      void poll();
    } catch (error) {
      if (pendingTimer.current) clearTimeout(pendingTimer.current);
      setPending(undefined);
      notify("error", error instanceof DOMException && error.name === "AbortError" ? "桌面端请求超时，请检查局域网连接" : String(error));
    }
  };
  const quickServerStatus = quickAttempt && state?.quick_request_id === quickAttempt.id ? state.quick_request_status ?? undefined : undefined;
  useEffect(() => {
    if (!quickAttempt) return;
    if (quickServerStatus === "succeeded" || quickServerStatus === "failed") {
      if (quickTimer.current) clearTimeout(quickTimer.current);
      setQuickAttempt(current => current?.id === quickAttempt.id ? { ...current, stage: quickServerStatus } : current);
      return;
    }
    if (quickServerStatus === "capturing" || quickServerStatus === "solving") {
      setQuickAttempt(current => current?.id === quickAttempt.id && current.stage !== "accepted" ? { ...current, stage: "accepted" } : current);
    } else if (quickAttempt.stage !== "sending" && quickAttempt.stage !== "accepted") {
      return;
    }
    void poll();
    if (quickPollTimer.current) clearInterval(quickPollTimer.current);
    quickPollTimer.current = window.setInterval(() => { void poll(); }, 800);
    return () => { if (quickPollTimer.current) clearInterval(quickPollTimer.current); };
  }, [quickAttempt?.id, quickAttempt?.stage, quickServerStatus, poll]);
  const quickSolve = async () => {
    if (!state || connection !== "connected") { connect(); void poll(); return; }
    if (quickAttempt && (quickAttempt.stage === "sending" || quickAttempt.stage === "accepted")) return;
    const id = globalThis.crypto?.randomUUID
      ? globalThis.crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
    setQuickAttempt({ id, stage: "sending" });
    if (quickTimer.current) clearTimeout(quickTimer.current);
    quickTimer.current = window.setTimeout(() => setQuickAttempt(current => current?.id === id ? { ...current, stage: "failed" } : current), 30000);
    try {
      const session = await ensureCryptoSession();
      const payload = await encryptPayload(session, { id });
      const controller = new AbortController();
      const requestTimer = window.setTimeout(() => controller.abort(), 8000);
      let response: Response;
      try {
        response = await fetch("/api/quick-solve", { method: "POST", cache: "no-store", headers: { "Content-Type": "application/json", "X-Baobao-Session": session.id }, body: payload, signal: controller.signal });
      } finally { clearTimeout(requestTimer); }
      const result = await decryptPayload<{ accepted?: boolean; error?: string }>(session, response);
      if (!response.ok || result.accepted !== true) throw new Error(result.error || `请求失败（HTTP ${response.status}）`);
      setQuickAttempt(current => current?.id === id ? { ...current, stage: "accepted" } : current);
      void poll();
    } catch {
      if (quickTimer.current) clearTimeout(quickTimer.current);
      setQuickAttempt(current => current?.id === id ? { ...current, stage: "failed" } : current);
    }
  };
  const imageCount = state?.draft?.image_count ?? 0;
  const interactiveThread = state?.interactive_thread_id;
  const busy = Boolean(pending) || state?.status === "capturing" || state?.status === "solving";
  const autoSubmit = state?.config.auto_submit_after_capture ?? false;
  const continuous = state?.config.conversation_mode === "continuous";
  const presetName = state?.presets?.find(item => item.id === preset)?.name
    ?? (preset === "math" ? "数学" : preset === "code" ? "代码" : "通用");
  const conversationHint = state?.config.provider === "openai-api"
    ? "OpenAI API 连续模式会在本机维护本次运行的聊天记录。"
    : "Codex 连续模式会复用同一个会话。";

  if (!state || state.config.simple_mode_enabled) {
    const serverBusy = state?.quick_request_status === "capturing" || state?.quick_request_status === "solving";
    const attemptBusy = quickAttempt?.stage === "sending" || quickAttempt?.stage === "accepted";
    const completed = state?.quick_request_status === "succeeded" || quickAttempt?.stage === "succeeded";
    const failed = state?.quick_request_status === "failed" || quickAttempt?.stage === "failed";
    const label = !state
      ? connection === "reconnecting" ? "连接中断，点击重连" : "正在连接电脑…"
      : connection !== "connected"
        ? connection === "connecting" ? "正在连接电脑…" : "连接中断，点击重连"
      : serverBusy || attemptBusy || state.status === "capturing" || state.status === "solving" ? "处理中…"
      : completed ? "完成 · 点击再答一题"
      : failed ? "失败 · 点击重试"
      : "点击开始答题";
    return <main className="quick-mode-shell"><button className={`quick-mode-button${serverBusy || attemptBusy ? " is-processing" : failed ? " is-failed" : completed ? " is-complete" : ""}`} type="button" onClick={() => void quickSolve()} disabled={serverBusy || attemptBusy} aria-label={label}>{label}</button></main>;
  }

  return <div className="mobile-shell">
    <header className="mobile-header">
      <div><Title heading={3}>宝宝巴士</Title><Text type="secondary">手机控制台</Text></div>
      <Tag color={connection === "connected" ? "green" : connection === "reconnecting" ? "orange" : "gray"}>{connection === "connected" ? "已连接 · 加密" : connection === "reconnecting" ? "正在重连" : "连接中"}</Tag>
    </header>
    <main className="mobile-main">
      <Card className="mobile-status" bordered={false}>
        <Space direction="vertical">
          <div className="status-line"><Text type="secondary">任务状态</Text><Tag color={state?.status === "idle" ? "gray" : "arcoblue"}>{state?.status ?? "加载中"}</Tag></div>
          <Text type="secondary">草稿图片：{imageCount} 张</Text>
          {state?.config.provider === "openai-api" && continuous && <Text type="secondary">连续上下文：已累计 {state.openai_history_turn_count} 轮</Text>}
          {state?.message && <Alert type="info" content={state.message} showIcon />}
        </Space>
      </Card>
      <Card className="mobile-card mobile-preset-card" bordered={false}>
        <div className="mobile-preset"><Text type="secondary">截图题型</Text><Select value={preset} onChange={setPreset} size="large"><Select.Option value="general">通用</Select.Option><Select.Option value="math">数学</Select.Option><Select.Option value="code">代码</Select.Option></Select></div>
      </Card>
      <Card className="mobile-card mobile-settings-card" bordered={false}>
        <div className="mobile-setting-line"><div><Text>截图后直接提交</Text><Text type="secondary">截图完成后立即开始解题</Text></div><Switch checked={autoSubmit} disabled={!state || busy || settingsUpdating} onChange={value => void updateAnswerSettings({ auto_submit_after_capture: value })} /></div>
        <div className="mobile-setting-line"><div><Text>连续对话</Text><Text type="secondary">{continuous ? conversationHint : "单轮模式不会保留上次解题上下文。"}</Text></div><Switch checked={continuous} disabled={!state || busy || settingsUpdating} onChange={value => void updateAnswerSettings({ conversation_mode: value ? "continuous" : "single" })} /></div>
      </Card>
      <div className="mobile-actions">
        <Button type="primary" className="mobile-action" size="large" loading={pending?.kind === "capture"} disabled={busy} icon={<IconUpload />} onClick={() => void command("capture", "capture", preset)}>{autoSubmit ? `截图并提交（${presetName}）` : `截图加入草稿（${presetName}）`}</Button>
        <Button type="primary" className="mobile-action" size="large" loading={pending?.kind === "submit"} disabled={busy || !imageCount} icon={<IconPlayArrow />} onClick={() => void command("submit", "submit")}>提交题目</Button>
        <Button type="secondary" className="mobile-action" size="large" loading={pending?.kind === "clear"} disabled={busy || (!imageCount && !interactiveThread && !state?.has_openai_history)} onClick={() => void command("clear", "clear")}>清空草稿</Button>
        <Button type="secondary" className="mobile-action" size="large" loading={pending?.kind === "cancel"} disabled={Boolean(pending) || !busy} onClick={() => void command("cancel", "cancel")}>取消任务</Button>
      </div>
      {state?.draft?.thumbnails?.length ? <Card title="截图预览" className="mobile-card"><div className="mobile-thumbs">{state.draft.thumbnails.map((src, i) => <img key={i} src={src} alt={`第 ${i + 1} 张`} />)}</div></Card> : null}
      {state?.answer?.text ? <Card title="答题结果" className="mobile-card"><Paragraph className="mobile-answer">{state.answer.text}</Paragraph></Card> : <Card className="mobile-card empty-answer"><Text type="secondary">完成提交后，答案会显示在这里</Text></Card>}
    </main>
    {toast && <div className={`mobile-toast toast-${toast.level}`}><span>{toastIcon(toast.level)}</span>{toast.text}</div>}
  </div>;
}

const root = document.getElementById("mobile-app");
if (root) createRoot(root).render(<ConfigProvider><MobileApp /></ConfigProvider>);
