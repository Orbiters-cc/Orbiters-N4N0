import { AnimatePresence, motion } from "motion/react";
import { Box, Cpu, FlaskConical, FolderOpen, Gauge, HardDrive, Layers, MonitorPlay, Pause, Play, Radio, ScanLine, ShieldCheck, Sparkles } from "lucide-react";
import type { ReactNode } from "react";
import { api } from "../lib/backend";
import { bytes, duration, folderName, percent } from "../lib/format";
import { useApp } from "../lib/store";
import type { CaptureMode, Snapshot, StorageQuality } from "../lib/types";
import { RecordButton } from "../components/RecordButton";
import { TrailMap } from "../components/TrailMap";
import { AnimatedNumber, Button, Card, cx, Segmented, spring, StatusDot } from "../components/ui";

const stagger = (i: number) => ({ initial: { opacity: 0, y: 18 }, animate: { opacity: 1, y: 0 }, transition: { ...spring.soft, delay: 0.05 + i * 0.05 } });

export function CapturePage({ snapshot }: { snapshot: Snapshot }) {
  const { run } = useApp();
  const c = snapshot.capture;
  const s = c.stats;
  const capturing = ["starting", "recording", "paused", "importing", "stopping"].includes(c.state);
  const recording = c.state === "recording" || c.state === "paused";
  const source = snapshot.settings.spout_sender
    ? snapshot.sources.find((x) => x.name === snapshot.settings.spout_sender)
    : snapshot.sources.find((x) => x.name.toLowerCase().includes("vrchat")) ?? snapshot.sources[0];
  const health = s && s.frames > 0 ? s.strip_ok / s.frames : 1;
  const world = snapshot.vrchat.world;
  const canStart = snapshot.settings.consent_acknowledged;

  const save = (patch: Partial<Snapshot["settings"]>) => run(() => api.saveSettings({ ...snapshot.settings, ...patch }));
  const toggle = () => run(() => (capturing ? api.stop() : api.start()));

  const headline = c.state === "importing"
    ? "Importing a raw capture"
    : recording
      ? c.state === "paused"
        ? "Paused"
        : "Recording"
      : world.in_world && world.world_name
        ? world.world_name
        : "Ready to capture";

  return (
    <div className="flex h-full flex-col gap-5 overflow-y-auto px-8 pb-8 pt-2">
      <motion.header {...stagger(0)} className="flex items-end justify-between gap-6">
        <div className="min-w-0">
          <div className="label-caps mb-1.5">{recording ? "Live capture" : world.in_world ? "VRChat world" : "Capture"}</div>
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.h1
              key={headline}
              initial={{ opacity: 0, y: 12, filter: "blur(4px)" }}
              animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
              exit={{ opacity: 0, y: -10, filter: "blur(4px)" }}
              transition={spring.soft}
              className="truncate text-[30px] font-semibold tracking-tight"
            >
              {headline}
            </motion.h1>
          </AnimatePresence>
          <div className="mt-1 truncate text-[13px] text-fg-3">
            {world.world_id ? <span className="font-mono text-[12px]">{world.world_id}</span> : snapshot.vrchat.running ? "VRChat is running; join a world to tag captures with it." : "Start VRChat and enable Spout streaming on the Stream camera."}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {!capturing && (
            <Button variant="ghost" size="sm" icon={<FlaskConical size={15} />} onClick={() => run(api.startTest)} disabled={!canStart} title="Record a generated test pattern (no VRChat needed)">
              Test pattern
            </Button>
          )}
        </div>
      </motion.header>

      <motion.div {...stagger(1)} className="grid grid-cols-4 gap-3">
        <Chip icon={<MonitorPlay size={16} />} label="VRChat" tone={snapshot.vrchat.running ? "ok" : "off"} value={snapshot.vrchat.running ? (world.in_world ? "In a world" : "Running") : "Not running"} />
        <Chip
          icon={<Radio size={16} />}
          label="Spout source"
          tone={source ? "ok" : "warn"}
          value={source ? `${source.name} · ${source.width}×${source.height}` : "No sender yet"}
        />
        <Chip
          icon={<Sparkles size={16} />}
          label="Action Menu"
          tone={snapshot.osc.listening ? "ok" : snapshot.osc.enabled ? "warn" : "off"}
          value={snapshot.osc.last_event ?? (snapshot.osc.listening ? `“${snapshot.settings.osc.record_parameter}” toggle` : snapshot.osc.error ?? "OSC off")}
        />
        <Chip
          icon={<HardDrive size={16} />}
          label="Storage"
          tone={snapshot.storage.free_bytes != null && snapshot.storage.free_bytes < 20e9 ? "warn" : "ok"}
          value={`${bytes(snapshot.storage.used_bytes)} used${snapshot.storage.free_bytes != null ? ` · ${bytes(snapshot.storage.free_bytes)} free` : ""}`}
        />
      </motion.div>

      <div className="grid grid-cols-[minmax(420px,1.05fr)_1fr] items-stretch gap-5">
        <motion.div {...stagger(2)}>
          <Card className="flex h-full flex-col items-center gap-5 overflow-hidden px-6 pb-6 pt-6">
            <div className="flex w-full items-center justify-between">
              <span className="label-caps">Recorder</span>
              <AnimatePresence>
                {recording && (
                  <motion.span initial={{ opacity: 0, scale: 0.8 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.8 }} transition={spring.bouncy} className="flex items-center gap-2 rounded-full bg-rec/15 px-3 py-1 text-[12px] font-semibold text-rec">
                    <StatusDot tone="rec" pulse={c.state === "recording"} />
                    {c.state === "paused" ? "PAUSED" : "REC"}
                    <span className="tabular-nums text-fg">{duration(s?.elapsed_s)}</span>
                  </motion.span>
                )}
              </AnimatePresence>
            </div>

            <div className="flex flex-col items-center gap-2">
              <RecordButton state={c.state} disabled={!canStart || c.state === "importing"} onToggle={toggle} health={health} />
              <AnimatePresence mode="wait" initial={false}>
                <motion.div key={c.state} initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={spring.snappy} className="text-center">
                  <div className="text-[15px] font-semibold">
                    {{ idle: "Start recording", starting: "Starting…", recording: "Stop recording", paused: "Stop recording", stopping: "Saving the session…", importing: "Importing…" }[c.state]}
                  </div>
                  <div className="mt-1 text-[12px] text-fg-3">
                    {c.state === "idle" ? (snapshot.osc.listening ? "or use the Record toggle in your Action Menu" : "Spout frames are decoded and saved as a session") : c.source ?? ""}
                  </div>
                </motion.div>
              </AnimatePresence>
            </div>

            <div className="flex w-full flex-col items-center gap-3">
              <Segmented<CaptureMode>
                value={snapshot.settings.capture_mode}
                onChange={(v) => save({ capture_mode: v })}
                options={[
                  { value: "record_only", label: "Record", hint: "Record now, reconstruct afterwards with the whole GPU" },
                  { value: "live_refine", label: "Live + refine", hint: "Map live while playing, refine afterwards", disabled: true, badge: "soon" },
                  { value: "live_only", label: "Live only", hint: "The live map is the result", disabled: true, badge: "soon" },
                ]}
              />
              <Segmented<StorageQuality>
                size="sm"
                value={snapshot.settings.storage_quality}
                onChange={(v) => save({ storage_quality: v })}
                options={[
                  { value: "lossless", label: "Lossless", hint: "Every frame saved losslessly (very large)" },
                  { value: "high", label: "High", hint: "Lossless keyframes, poses and depth" },
                  { value: "compact", label: "Compact", hint: "Fewer keyframes, smaller sessions" },
                ]}
              />
              {recording && (
                <div className="flex gap-2">
                  <Button size="sm" variant="secondary" icon={c.state === "paused" ? <Play size={14} /> : <Pause size={14} />} onClick={() => run(() => api.pause(c.state !== "paused"))}>
                    {c.state === "paused" ? "Resume" : "Pause"}
                  </Button>
                  {c.session_dir && (
                    <Button size="sm" variant="ghost" icon={<FolderOpen size={14} />} onClick={() => run(() => api.open(c.session_dir!))}>
                      {folderName(c.session_dir)}
                    </Button>
                  )}
                </div>
              )}
            </div>

            <AnimatePresence>{!canStart && <ConsentOverlay onAccept={() => save({ consent_acknowledged: true })} />}</AnimatePresence>
          </Card>
        </motion.div>

        <div className="grid grid-cols-2 grid-rows-3 gap-3">
          <Stat i={3} icon={<Layers size={16} />} label="Frames" value={s?.frames ?? 0} sub={s ? `${s.width}×${s.height}` : "–"} />
          <Stat i={4} icon={<ScanLine size={16} />} label="Strip decoded" value={health * 100} format={(v) => (s ? `${v.toFixed(v > 99.95 ? 0 : 1)}%` : "–")} sub={s ? `${s.strip_failed} unreadable` : "pose strip in every frame"} accent={health > 0.97 ? "ok" : health > 0.8 ? "warn" : "danger"} />
          <Stat i={5} icon={<Box size={16} />} label="Keyframes" value={s?.keyframes ?? 0} sub={s && s.dropped > 0 ? `${s.dropped} dropped` : "lossless images"} />
          <Stat i={6} icon={<Cpu size={16} />} label="Depth frames" value={s?.depth_frames ?? 0} sub={s ? percent(s.depth_frames, s.frames) + " of frames" : "exact depth samples"} />
          <Stat i={7} icon={<Gauge size={16} />} label="Frame rate" value={s?.fps ?? 0} format={(v) => (s ? `${v.toFixed(1)} fps` : "–")} sub={s ? `decode ${s.decode_ms.toFixed(1)} ms` : "per frame"} />
          <Stat i={8} icon={<HardDrive size={16} />} label="Written" value={s?.bytes ?? 0} format={(v) => (s ? bytes(v) : "–")} sub={s ? `${duration(s.elapsed_s)} recorded` : "this session"} />
        </div>
      </div>

      <motion.div {...stagger(9)}>
        <Card className="p-4">
          <div className="mb-3 flex items-center justify-between px-1">
            <div>
              <div className="label-caps">Camera path</div>
              <div className="mt-0.5 text-[12.5px] text-fg-3">
                {s?.position ? `x ${s.position[0].toFixed(1)}  y ${s.position[1].toFixed(1)}  z ${s.position[2].toFixed(1)} m` : "Exact positions from the capture strip, top-down"}
              </div>
            </div>
            {c.error && <div className="max-w-[60%] truncate text-[12px] text-danger">{c.error}</div>}
          </div>
          <TrailMap points={c.trail} heading={s?.heading_deg ?? null} live={c.state === "recording"} className="h-[230px]" />
        </Card>
      </motion.div>
    </div>
  );
}

function Chip({ icon, label, value, tone }: { icon: ReactNode; label: string; value: string; tone: "ok" | "warn" | "off" }) {
  return (
    <motion.div layout transition={spring.soft} className="surface flex min-w-0 items-center gap-3 rounded-2xl px-3.5 py-3">
      <div className={cx("grid size-9 shrink-0 place-items-center rounded-xl", tone === "ok" ? "bg-ok/10 text-ok" : tone === "warn" ? "bg-warn/10 text-warn" : "bg-white/5 text-fg-4")}>{icon}</div>
      <div className="min-w-0">
        <div className="label-caps">{label}</div>
        <AnimatePresence mode="wait" initial={false}>
          <motion.div key={value} initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={{ duration: 0.18 }} className="truncate text-[13px] font-medium text-fg">
            {value}
          </motion.div>
        </AnimatePresence>
      </div>
    </motion.div>
  );
}

function Stat({ i, icon, label, value, format, sub, accent }: { i: number; icon: ReactNode; label: string; value: number; format?: (v: number) => string; sub: string; accent?: "ok" | "warn" | "danger" }) {
  return (
    <motion.div {...stagger(i)} whileHover={{ y: -3 }} className="surface group relative flex flex-col justify-between overflow-hidden rounded-[var(--radius-tile)] p-4">
      <div className="pointer-events-none absolute -right-10 -top-10 size-28 rounded-full bg-indigo/0 blur-2xl transition-colors duration-500 group-hover:bg-indigo/20" />
      <div className="flex items-center justify-between text-fg-3">
        <span className="label-caps">{label}</span>
        <span className={cx(accent === "ok" ? "text-ok" : accent === "warn" ? "text-warn" : accent === "danger" ? "text-danger" : "text-fg-4")}>{icon}</span>
      </div>
      <AnimatedNumber value={value} format={format} className="text-[28px] font-semibold tracking-tight" />
      <div className="truncate text-[12px] text-fg-3">{sub}</div>
    </motion.div>
  );
}

function ConsentOverlay({ onAccept }: { onAccept: () => void }) {
  return (
    <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="absolute inset-0 z-10 grid place-items-center bg-ink-950/75 p-6 backdrop-blur-md">
      <motion.div initial={{ scale: 0.92, y: 16 }} animate={{ scale: 1, y: 0 }} exit={{ scale: 0.95, y: 10 }} transition={spring.bouncy} className="surface-strong max-w-[420px] rounded-3xl p-6">
        <div className="mb-3 flex items-center gap-2.5">
          <div className="grid size-9 place-items-center rounded-xl bg-violet/15 text-violet">
            <ShieldCheck size={18} />
          </div>
          <div className="text-[16px] font-semibold">Before you capture</div>
        </div>
        <ul className="mb-5 space-y-2 text-[13px] leading-relaxed text-fg-2">
          <li>Capture your own worlds, or worlds whose creators agreed, and keep the results private.</li>
          <li>Other players are left out: hide them in the Stream camera (Remote Players off).</li>
          <li>Orbiters desktop never asks for your VRChat login and only reads VRChat's local log, OSC and Spout.</li>
        </ul>
        <Button variant="primary" className="w-full" onClick={onAccept}>
          I understand
        </Button>
      </motion.div>
    </motion.div>
  );
}
