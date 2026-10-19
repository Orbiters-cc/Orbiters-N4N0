// Talks to the Rust shell through Tauri. Outside Tauri (a plain browser on the Vite dev server) a small
// simulator answers instead, so the interface can be designed and reviewed without the native app.

import type { RawCaptureSummary, SessionSaved, SessionSummary, Settings, Snapshot, Toast } from "./types";

type Unlisten = () => void;

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

let tauriCore: typeof import("@tauri-apps/api/core") | null = null;
let tauriEvent: typeof import("@tauri-apps/api/event") | null = null;

async function core() {
  tauriCore ??= await import("@tauri-apps/api/core");
  return tauriCore;
}

async function events() {
  tauriEvent ??= await import("@tauri-apps/api/event");
  return tauriEvent;
}

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri) return (await core()).invoke<T>(cmd, args);
  return mock.call(cmd, args) as Promise<T>;
}

export function listen<T>(event: string, cb: (payload: T) => void): Unlisten {
  if (!isTauri) return mock.listen(event, cb as (p: unknown) => void);
  let off: Unlisten | null = null;
  let cancelled = false;
  void events().then((e) =>
    e.listen<T>(event, (ev) => cb(ev.payload)).then((u) => {
      if (cancelled) u();
      else off = u;
    }),
  );
  return () => {
    cancelled = true;
    off?.();
  };
}

export const api = {
  snapshot: () => call<Snapshot>("get_snapshot"),
  start: () => call<void>("start_capture"),
  stop: () => call<void>("stop_capture"),
  pause: (paused: boolean) => call<void>("pause_capture", { paused }),
  startTest: () => call<void>("start_test_capture"),
  sessions: () => call<SessionSummary[]>("list_sessions"),
  thumbnail: (dir: string) => call<string | null>("session_thumbnail", { dir }),
  trail: (dir: string) => call<[number, number, number][]>("session_trail", { dir }),
  trash: (dir: string) => call<void>("trash_session", { dir }),
  open: (dir: string) => call<void>("open_path", { dir }),
  rawCaptures: () => call<RawCaptureSummary[]>("list_raw_captures"),
  importRaw: (dir: string) => call<void>("import_raw_capture", { dir }),
  saveSettings: (settings: Settings) => call<Snapshot>("save_settings", { settings }),
  workerLog: () => call<string[]>("worker_log"),
  quit: () => call<void>("quit_app"),
};

export async function windowAction(action: "minimize" | "maximize" | "close" | "drag") {
  if (!isTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const w = getCurrentWindow();
  if (action === "minimize") await w.minimize();
  else if (action === "maximize") await w.toggleMaximize();
  else if (action === "close") await w.close();
  else await w.startDragging();
}

export async function pickFolder(defaultPath?: string): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ directory: true, defaultPath });
  return typeof r === "string" ? r : null;
}

// ------------------------------------------------------------------ browser simulator

const mock = (() => {
  const listeners = new Map<string, Set<(p: unknown) => void>>();
  const emit = (event: string, payload: unknown) => listeners.get(event)?.forEach((cb) => cb(payload));

  const settings: Settings = {
    sessions_dir: "C:\\Users\\you\\Documents\\Orbiters\\Sessions",
    raw_captures_dir: "C:\\Users\\you\\Documents\\Orbiters\\Raw captures",
    capture_mode: "record_only",
    storage_quality: "high",
    spout_sender: "",
    keyframes: { min_translation: 0.3, min_rotation_deg: 15, max_interval_s: 2 },
    storage_limit_gb: 200,
    osc: { enabled: true, record_parameter: "OrbitersRecord", vrchat_port: 9000 },
    split_on_world_change: true,
    consent_acknowledged: false,
    reduce_motion: false,
  };

  const snap: Snapshot = {
    version: "0.1.0",
    platform: "windows",
    vrchat: {
      running: true,
      world: { world_id: "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b", world_name: "The Black Cat", instance: "48213~private", in_world: true, log_file: null },
      log_found: true,
    },
    sources: [{ name: "VRChat", width: 1920, height: 1080, format: 87, description: "VRChat.exe" }],
    osc: { enabled: true, listening: true, advertising: true, port: 61042, last_event: null, error: null },
    capture: { state: "idle", session_dir: null, source: null, stats: null, error: null, trail: [], from_osc: false },
    settings,
    storage: { sessions_dir: settings.sessions_dir, used_bytes: 38.4e9, free_bytes: 612e9, limit_gb: 200, sessions: 6 },
    worker: { ready: true, error: null },
  };

  let timer: number | null = null;
  let t = 0;

  const tick = () => {
    t += 0.25;
    const prev = snap.capture.stats;
    const frames = (prev?.frames ?? 0) + 15;
    const a = t * 0.18;
    const r = 6 + Math.sin(t * 0.07) * 2.5;
    const pos: [number, number, number] = [Math.cos(a) * r + Math.sin(t * 0.31) * 1.2, 1.6, Math.sin(a) * r * 0.8];
    snap.capture.trail = [...snap.capture.trail, pos].slice(-1500);
    snap.capture.stats = {
      frames,
      strip_ok: frames - Math.floor(frames / 400),
      strip_failed: Math.floor(frames / 400),
      keyframes: Math.floor(frames / 11),
      depth_frames: Math.floor(frames / 4),
      gated: Math.floor(frames / 900),
      dropped: 0,
      fps: 59 + Math.sin(t) * 1.2,
      decode_ms: 1.8 + Math.sin(t * 0.5) * 0.3,
      bytes: (prev?.bytes ?? 0) + 6.1e6,
      elapsed_s: t,
      position: pos,
      heading_deg: ((a * 180) / Math.PI + 90) % 360,
      width: 1920,
      height: 1080,
      flipped: false,
      last_error: null,
      paused: snap.capture.state === "paused",
    };
    emit("snapshot", structuredClone(snap));
  };

  const sessions: SessionSummary[] = [
    ["The Black Cat", 1824, 162, 455, 7.9e9, 0.05],
    ["Midnight Rooftops", 5310, 488, 1327, 21.4e9, 1.2],
    ["Neon Arcade", 2240, 201, 560, 9.2e9, 3.4],
    ["Japan Shrine", 3901, 344, 975, 15.0e9, 6.0],
    ["Test pattern", 600, 61, 150, 1.1e9, 9.5],
    ["Ultinova showcase", 2780, 240, 695, 11.8e9, 14.0],
  ].map(([name, frames, keyframes, depth, bytes, daysAgo], i) => ({
    dir: `C:\\Users\\you\\Documents\\Orbiters\\Sessions\\${name} ${i}`,
    has_thumbnail: true,
    size_bytes: bytes as number,
    modified: Math.floor(Date.now() / 1000 - (daysAgo as number) * 86400),
    manifest: {
      format: "orbiters.session",
      version: 1,
      id: `mock-${i}`,
      created_utc: new Date(Date.now() - (daysAgo as number) * 86400e3).toISOString(),
      app: { name: "Orbiters desktop", version: "0.1.0" },
      contracts_version: "0.1.0",
      world: { id: `wrld_mock_${i}`, name: name as string, author_name: null, instance: null },
      capture: { mode: "record_only", quality: "high", source: "spout", source_name: "Spout: VRChat", width: 1920, height: 1080, video: null },
      stats: {
        frames: frames as number,
        strip_ok: (frames as number) - 3,
        keyframes: keyframes as number,
        depth_frames: depth as number,
        gated: 2,
        duration_s: (frames as number) / 60,
        first_game_time: 10,
        last_game_time: 10 + (frames as number) / 60,
        bytes: bytes as number,
      },
      consent: { own_world: i % 2 === 0, note: "" },
      notes: "",
    },
  }));

  const hues = [265, 200, 320, 15, 180, 240];
  const thumb = (i: number) => {
    const c = document.createElement("canvas");
    c.width = 480;
    c.height = 270;
    const g = c.getContext("2d")!;
    const h = hues[i % hues.length];
    const grad = g.createLinearGradient(0, 0, 480, 270);
    grad.addColorStop(0, `hsl(${h} 70% 18%)`);
    grad.addColorStop(1, `hsl(${(h + 60) % 360} 80% 8%)`);
    g.fillStyle = grad;
    g.fillRect(0, 0, 480, 270);
    for (let k = 0; k < 14; k++) {
      g.fillStyle = `hsla(${(h + k * 9) % 360} 90% ${40 + (k % 4) * 10}% / ${0.18 + (k % 3) * 0.1})`;
      const x = (k * 97 + i * 31) % 480;
      const w = 30 + ((k * 53) % 90);
      g.fillRect(x, 150 - ((k * 37) % 120), w, 270);
    }
    g.fillStyle = "rgba(255,255,255,0.85)";
    g.beginPath();
    g.arc(360 - i * 20, 70, 10 + i, 0, Math.PI * 2);
    g.fill();
    return c.toDataURL("image/png").replace(/^data:image\/png;base64,/, "");
  };

  return {
    listen(event: string, cb: (p: unknown) => void): Unlisten {
      if (!listeners.has(event)) listeners.set(event, new Set());
      listeners.get(event)!.add(cb);
      return () => listeners.get(event)?.delete(cb);
    },
    async call(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
      await new Promise((r) => setTimeout(r, 60));
      switch (cmd) {
        case "get_snapshot":
          return structuredClone(snap);
        case "start_capture":
        case "start_test_capture":
          if (!snap.settings.consent_acknowledged) throw "Please read and accept the capture notice first (Capture page).";
          snap.capture = { ...snap.capture, state: "starting", trail: [], stats: null, error: null };
          emit("snapshot", structuredClone(snap));
          setTimeout(() => {
            snap.capture.state = "recording";
            snap.capture.session_dir = `${settings.sessions_dir}\\The Black Cat 2026-10-20 01-23-45`;
            snap.capture.source = "Spout: VRChat";
            t = 0;
            timer = window.setInterval(tick, 250);
            emit("snapshot", structuredClone(snap));
          }, 650);
          return null;
        case "stop_capture":
          snap.capture.state = "stopping";
          emit("snapshot", structuredClone(snap));
          setTimeout(() => {
            if (timer) window.clearInterval(timer);
            const s = snap.capture.stats;
            snap.capture.state = "idle";
            emit("snapshot", structuredClone(snap));
            emit("session-saved", {
              dir: snap.capture.session_dir ?? "",
              reason: "stopped",
              frames: s?.frames ?? 0,
              keyframes: s?.keyframes ?? 0,
              depth_frames: s?.depth_frames ?? 0,
            } satisfies SessionSaved);
          }, 700);
          return null;
        case "pause_capture":
          snap.capture.state = args?.paused ? "paused" : "recording";
          emit("snapshot", structuredClone(snap));
          return null;
        case "list_sessions":
          return structuredClone(sessions);
        case "session_thumbnail": {
          const i = sessions.findIndex((s) => s.dir === args?.dir);
          return thumb(i < 0 ? 0 : i);
        }
        case "session_trail": {
          const i = Math.max(0, sessions.findIndex((s) => s.dir === args?.dir));
          return Array.from({ length: 400 }, (_, k) => {
            const a = k * 0.03 + i;
            return [Math.cos(a) * (5 + Math.sin(k * 0.02) * 2) + i, 1.6, Math.sin(a * 1.3) * 4];
          });
        }
        case "trash_session": {
          const i = sessions.findIndex((s) => s.dir === args?.dir);
          if (i >= 0) sessions.splice(i, 1);
          return null;
        }
        case "list_raw_captures":
          return [
            {
              dir: `${settings.raw_captures_dir}\\Canine 2026-10-20 00-40-12`,
              name: "Canine 2026-10-20 00-40-12",
              scene: "Assets/MasculineCanine/Canine.unity",
              frames: 900,
              width: 1920,
              height: 1080,
              depth_every: 4,
              has_ground_truth: true,
              created_utc: new Date().toISOString(),
            },
          ] satisfies RawCaptureSummary[];
        case "import_raw_capture":
          snap.capture.state = "importing";
          emit("snapshot", structuredClone(snap));
          setTimeout(() => {
            snap.capture.state = "idle";
            emit("snapshot", structuredClone(snap));
            emit("toast", { id: 0, kind: "success", message: "Raw capture imported as a session." } satisfies Toast);
          }, 1800);
          return null;
        case "save_settings":
          Object.assign(snap.settings, args?.settings as Settings);
          emit("snapshot", structuredClone(snap));
          return structuredClone(snap);
        case "worker_log":
          return ["spout: connected to \"VRChat\" 1920x1080 (DXGI format 87)"];
        default:
          return null;
      }
    },
  };
})();
