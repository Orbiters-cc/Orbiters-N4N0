// Mirrors of the Rust types sent by the shell (app/src-tauri/src/state.rs and orbiters-core).

export type CaptureMode = "live_refine" | "live_only" | "record_only";
export type StorageQuality = "lossless" | "high" | "compact";
export type CaptureStateName = "idle" | "starting" | "recording" | "paused" | "stopping" | "importing";

export interface KeyframeSettings {
  min_translation: number;
  min_rotation_deg: number;
  max_interval_s: number;
}

export interface OscSettings {
  enabled: boolean;
  record_parameter: string;
  vrchat_port: number;
}

export interface Settings {
  sessions_dir: string;
  raw_captures_dir: string;
  capture_mode: CaptureMode;
  storage_quality: StorageQuality;
  spout_sender: string;
  keyframes: KeyframeSettings;
  storage_limit_gb: number;
  osc: OscSettings;
  split_on_world_change: boolean;
  consent_acknowledged: boolean;
  reduce_motion: boolean;
}

export interface CaptureStats {
  frames: number;
  strip_ok: number;
  strip_failed: number;
  keyframes: number;
  depth_frames: number;
  gated: number;
  dropped: number;
  fps: number;
  decode_ms: number;
  bytes: number;
  elapsed_s: number;
  position: [number, number, number] | null;
  heading_deg: number | null;
  width: number;
  height: number;
  flipped: boolean;
  last_error: string | null;
  paused: boolean;
}

export interface WorldState {
  world_id: string | null;
  world_name: string | null;
  instance: string | null;
  in_world: boolean;
  log_file: string | null;
}

export interface SpoutSource {
  name: string;
  width: number;
  height: number;
  format: number;
  description: string;
}

export interface Snapshot {
  version: string;
  platform: string;
  vrchat: { running: boolean; world: WorldState; log_found: boolean };
  sources: SpoutSource[];
  osc: { enabled: boolean; listening: boolean; advertising: boolean; port: number | null; last_event: string | null; error: string | null };
  capture: {
    state: CaptureStateName;
    session_dir: string | null;
    source: string | null;
    stats: CaptureStats | null;
    error: string | null;
    trail: [number, number, number][];
    from_osc: boolean;
  };
  settings: Settings;
  storage: { sessions_dir: string; used_bytes: number; free_bytes: number | null; limit_gb: number; sessions: number };
  worker: { ready: boolean; error: string | null };
}

export interface WorldInfo {
  id: string | null;
  name: string | null;
  author_name: string | null;
  instance: string | null;
}

export interface SessionManifest {
  format: string;
  version: number;
  id: string;
  created_utc: string;
  app: { name: string; version: string };
  contracts_version: string;
  world: WorldInfo | null;
  capture: { mode: CaptureMode; quality: StorageQuality; source: string; source_name: string; width: number; height: number; video: string | null };
  stats: {
    frames: number;
    strip_ok: number;
    keyframes: number;
    depth_frames: number;
    gated: number;
    duration_s: number;
    first_game_time: number | null;
    last_game_time: number | null;
    bytes: number;
  };
  consent: { own_world: boolean; note: string };
  notes: string;
}

export interface SessionSummary {
  dir: string;
  manifest: SessionManifest;
  has_thumbnail: boolean;
  size_bytes: number;
  modified: number;
}

export interface RawCaptureSummary {
  dir: string;
  name: string;
  scene: string | null;
  frames: number;
  width: number;
  height: number;
  depth_every: number;
  has_ground_truth: boolean;
  created_utc: string;
}

export interface Toast {
  id: number;
  kind: "info" | "success" | "warning" | "error";
  message: string;
}

export interface SessionSaved {
  dir: string;
  reason: string;
  frames: number;
  keyframes: number;
  depth_frames: number;
}
