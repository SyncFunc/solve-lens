import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Config, Snapshot, ToastLevel } from "../ui_types";

export type PendingCommand = {
  id: string;
  kind: "capture" | "submit" | "clear" | "cancel";
  beforeImageCount?: number;
};

export type UiState = {
  snapshot: Snapshot;
  pending?: PendingCommand;
  toast?: { level: ToastLevel; text: string };
};

const fallbackConfig: Config = {
  provider: "codex-cli",
  codex_path: "codex",
  codex_execution_mode: "exec",
  overlay_opacity: 0.86,
  overlay_theme: "follow",
  overlay_font_size: 20,
  overlay_width: 560,
  overlay_height: 420,
  codex_timeout_seconds: 120,
  codex_model: "",
  codex_reasoning_effort: "low",
  codex_service_tier: "default",
  prompt_addendum: "",
  lan_control_enabled: false,
  lan_control_port: 18765,
  mobile_auto_save_images: false,
  hotkeys: { general: "Ctrl+Alt+1", math: "Ctrl+Alt+2", code: "Ctrl+Alt+3", submit: "Ctrl+Alt+Enter", clear: "Ctrl+Alt+Backspace", toggle: "Ctrl+Alt+H", next: "Ctrl+Alt+N", previous: "Ctrl+Alt+P", opacity_up: "Ctrl+Alt+F11", opacity_down: "Ctrl+Alt+F12", font_up: "Ctrl+Alt+F9", font_down: "Ctrl+Alt+F10", move_left: "Ctrl+Alt+Left", move_right: "Ctrl+Alt+Right", move_up: "Ctrl+Alt+Up", move_down: "Ctrl+Alt+Down", port_check: "Ctrl+Alt+F8" },
};

const fallbackSnapshot: Snapshot = {
  presets: [],
  answer_page: 0,

  status: "idle",
  protected_overlay: false,
  config: fallbackConfig,
};

type ContextValue = UiState & {
  refresh: () => Promise<void>;
  command: (kind: PendingCommand["kind"], name: string, args?: Record<string, unknown>) => Promise<void>;
  updateConfig: (config: Config) => Promise<void>;
  notify: (level: ToastLevel, text: string) => void;
  clearPending: () => void;
};

const AppContext = createContext<ContextValue | null>(null);

function commandId() {
  try { return crypto.randomUUID(); } catch { return `${Date.now()}-${Math.random()}`; }
}

export function AppProvider({ children }: { children: React.ReactNode }) {
  const [snapshot, setSnapshot] = useState<Snapshot>(fallbackSnapshot);
  const [pending, setPending] = useState<PendingCommand | undefined>(undefined);
  const [toast, setToast] = useState<UiState["toast"] | undefined>(undefined);
  const toastTimer = useRef<number | undefined>(undefined);

  const notify = useCallback((level: ToastLevel, text: string) => {
    setToast({ level, text });
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(undefined), 3400);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const next = await invoke<Snapshot>("get_snapshot");
      if (next) setSnapshot(next);
    } catch (error) {
      notify("error", String(error));
    }
  }, [notify]);

  useEffect(() => {
    void refresh();
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<Snapshot>("state-changed", (event) => {
      if (!disposed && event.payload) setSnapshot(event.payload);
    }).then((fn) => {
      if (disposed) fn(); else unlisten = fn;
    }).catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
      if (toastTimer.current) window.clearTimeout(toastTimer.current);
    };
  }, [refresh]);

  const command = useCallback(async (kind: PendingCommand["kind"], name: string, args: Record<string, unknown> = {}) => {
    const beforeImageCount = snapshot.draft?.image_count ?? 0;
    const id = commandId();
    setPending({ id, kind, beforeImageCount });
    notify("info", kind === "capture" ? "正在发送截图请求" : "正在发送操作请求");
    try {
      await invoke(name, args);
      notify("info", "请求已发送，等待状态确认");
    } catch (error) {
      setPending(undefined);
      notify("error", String(error));
    }
  }, [notify, snapshot.draft?.image_count]);

  const updateConfig = useCallback(async (config: Config) => {
    // Send only fields changed in this editor session. The Rust side merges
    // this patch with its latest state, so a remote/state refresh cannot
    // overwrite unrelated settings while the form is dirty.
    const baseline = snapshot.config;
    const patch: Record<string, unknown> = {};
    const scalarFields: (keyof Config)[] = [
      "provider", "codex_path", "codex_execution_mode", "overlay_opacity", "overlay_theme", "overlay_font_size",
      "overlay_width", "overlay_height", "codex_timeout_seconds", "codex_model", "codex_reasoning_effort", "codex_service_tier",
      "prompt_addendum", "lan_control_enabled", "lan_control_port", "mobile_auto_save_images",
    ];
    scalarFields.forEach(field => { if (config[field] !== baseline[field]) patch[field] = config[field]; });
    const hotkeys: Record<string, string> = {};
    const hotkeyKeys = new Set([...Object.keys(baseline.hotkeys), ...Object.keys(config.hotkeys)]);
    hotkeyKeys.forEach(key => { if (config.hotkeys[key] !== baseline.hotkeys[key]) hotkeys[key] = config.hotkeys[key] ?? ""; });
    if (Object.keys(hotkeys).length) patch.hotkeys = hotkeys;
    if (!Object.keys(patch).length) { notify("info", "没有需要保存的修改"); return; }
    try {
      await invoke("update_config", { config: patch });
      notify("success", "配置已保存");
      await refresh();
    } catch (error) {
      notify("error", String(error));
      throw error;
    }
  }, [notify, refresh, snapshot.config]);

  useEffect(() => {
    if (!pending) return;
    const count = snapshot.draft?.image_count ?? 0;
    if (pending.kind === "capture" && count > (pending.beforeImageCount ?? 0)) {
      notify("success", "截图已成功加入草稿");
      setPending(undefined);
    } else if (pending.kind === "submit" && ["displaying", "done", "completed"].includes(snapshot.status)) {
      notify("success", "题目已完成");
      setPending(undefined);
    } else if (pending.kind === "clear" && !snapshot.draft) {
      notify("success", "草稿已清空");
      setPending(undefined);
    } else if (pending.kind === "cancel" && ["idle", "cancelled", "canceled"].includes(snapshot.status)) {
      notify("success", "任务已取消");
      setPending(undefined);
    } else if (["error", "failed"].includes(snapshot.status) && snapshot.message) {
      notify("error", snapshot.message);
      setPending(undefined);
    }
  }, [pending, snapshot, notify]);

  const value = useMemo(() => ({ snapshot, pending, toast, refresh, command, updateConfig, notify, clearPending: () => setPending(undefined) }), [snapshot, pending, toast, refresh, command, updateConfig, notify]);
  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}

export function useAppState() {
  const value = useContext(AppContext);
  if (!value) throw new Error("useAppState must be used inside AppProvider");
  return value;
}
