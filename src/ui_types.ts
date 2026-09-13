export type ToastLevel = "info" | "success" | "error";

export type ToastState = { level: ToastLevel; text: string };

export type Preset = {
  id: string;
  name: string;
  built_in: boolean;
  hotkey: string;
  task_template: string;
  version: number;
};

export type CodexReasoningOption = {
  reasoningEffort: string;
  description?: string;
};

export type CodexServiceTierOption = {
  id: string;
  name?: string;
  description?: string;
};

export type CodexModelOption = {
  id: string;
  model?: string;
  displayName?: string;
  description?: string;
  isDefault?: boolean;
  hidden?: boolean;
  supportedReasoningEfforts?: CodexReasoningOption[];
  defaultReasoningEffort?: string;
  additionalSpeedTiers?: string[];
  serviceTiers?: CodexServiceTierOption[];
};
export type CodexThreadOption = { id: string; name?: string; model?: string; updatedAt?: string; archived?: boolean };

export type Config = {
  provider: string;
  codex_path: string;
  codex_execution_mode: "exec" | "interactive";
  openai_base_url: string;
  openai_api_key: string;
  openai_model: string;
  overlay_opacity: number;
  overlay_theme: string;
  overlay_font_size: number;
  overlay_width: number;
  overlay_height: number;
  codex_timeout_seconds: number;
  codex_model: string;
  codex_reasoning_effort: string;
  codex_service_tier: string;
  prompt_addendum: string;
  lan_control_enabled: boolean;
  lan_control_port: number;
  mobile_auto_save_images: boolean;
  hotkeys: Record<string, string>;
};

export type Draft = {
  id: string;
  preset_id: string;
  image_count: number;
  thumbnails: string[];
};

export type Snapshot = {
  draft?: Draft;
  answer?: { text: string };
  answer_page: number;
  interactive_thread_id?: string;

  presets: Preset[];
  status: string;
  message?: string;
  stream_output?: string;
  protected_overlay: boolean;
  config: Config;
};
