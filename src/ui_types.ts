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

export type Config = {
  provider: string;
  codex_path: string;
  overlay_opacity: number;
  overlay_theme: string;
  overlay_font_size: number;
  overlay_width: number;
  overlay_height: number;
  codex_timeout_seconds: number;
  codex_model: string;
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

  presets: Preset[];
  status: string;
  message?: string;
  stream_output?: string;
  protected_overlay: boolean;
  config: Config;
};
