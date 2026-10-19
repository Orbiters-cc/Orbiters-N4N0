import { motion } from "motion/react";
import { Folder, Gauge, HardDrive, Info, LogOut, Radio, ScrollText, Sparkles } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { api, isTauri, pickFolder } from "../lib/backend";
import { bytes } from "../lib/format";
import { useApp } from "../lib/store";
import type { Settings, Snapshot } from "../lib/types";
import { Logo } from "../components/Logo";
import { Button, Card, Field, Slider, spring, TextInput, Toggle } from "../components/ui";

export function SettingsPage({ snapshot }: { snapshot: Snapshot }) {
  const { run } = useApp();
  const [draft, setDraft] = useState<Settings>(snapshot.settings);
  useEffect(() => setDraft(snapshot.settings), [snapshot.settings]);

  // Settings apply as you change them: optimistic locally, saved in the background.
  const apply = (patch: Partial<Settings>) => {
    const next = { ...draft, ...patch };
    setDraft(next);
    void run(() => api.saveSettings(next));
  };
  const [log, setLog] = useState<string[] | null>(null);

  const usedGb = snapshot.storage.used_bytes / 1e9;
  const usage = draft.storage_limit_gb > 0 ? Math.min(1, usedGb / draft.storage_limit_gb) : 0;

  return (
    <div className="h-full overflow-y-auto px-8 pb-10 pt-2">
      <motion.div initial={{ opacity: 0, y: 14 }} animate={{ opacity: 1, y: 0 }} transition={spring.soft} className="mb-6">
        <div className="label-caps mb-1.5">Preferences</div>
        <h1 className="text-[30px] font-semibold tracking-tight">Settings</h1>
      </motion.div>

      <div className="grid max-w-[980px] grid-cols-1 gap-5">
        <Section i={0} icon={<Gauge size={17} />} title="Capture" subtitle="How frames become a session">
          <Field label="Keyframe distance" description="A new lossless keyframe after the camera moves this far.">
            <Slider value={draft.keyframes.min_translation} min={0.05} max={2} step={0.05} onChange={(v) => apply({ keyframes: { ...draft.keyframes, min_translation: v } })} format={(v) => `${v.toFixed(2)} m`} />
          </Field>
          <Field label="Keyframe turn" description="…or after it turns this much.">
            <Slider value={draft.keyframes.min_rotation_deg} min={2} max={60} step={1} onChange={(v) => apply({ keyframes: { ...draft.keyframes, min_rotation_deg: v } })} format={(v) => `${v.toFixed(0)}°`} />
          </Field>
          <Field label="Keyframe interval" description="…or after this long, whatever happens.">
            <Slider value={draft.keyframes.max_interval_s} min={0.25} max={10} step={0.25} onChange={(v) => apply({ keyframes: { ...draft.keyframes, max_interval_s: v } })} format={(v) => `${v.toFixed(2)} s`} />
          </Field>
          <Field label="New session on world change" description="Each VRChat world gets its own session while you record.">
            <Toggle checked={draft.split_on_world_change} onChange={(v) => apply({ split_on_world_change: v })} />
          </Field>
          <Field label="Spout sender" description="Leave empty to pick VRChat's Stream camera automatically.">
            <div className="flex items-center gap-2">
              <TextInput value={draft.spout_sender} onChange={(v) => setDraft({ ...draft, spout_sender: v })} placeholder={snapshot.sources[0]?.name ?? "VRChat"} width="w-52" />
              <Button size="sm" variant="secondary" onClick={() => apply({ spout_sender: draft.spout_sender })}>
                Save
              </Button>
            </div>
          </Field>
          {snapshot.sources.length > 0 && (
            <div className="flex flex-wrap gap-2 pb-2">
              {snapshot.sources.map((s) => (
                <motion.button
                  key={s.name}
                  type="button"
                  whileTap={{ scale: 0.95 }}
                  onClick={() => apply({ spout_sender: s.name })}
                  className={`rounded-full px-3 py-1.5 text-[12px] transition-colors ${draft.spout_sender === s.name ? "bg-indigo/30 text-fg" : "bg-white/[0.06] text-fg-2 hover:bg-white/[0.1]"}`}
                >
                  {s.name} · {s.width}×{s.height}
                </motion.button>
              ))}
            </div>
          )}
        </Section>

        <Section i={1} icon={<HardDrive size={17} />} title="Storage" subtitle={`${bytes(snapshot.storage.used_bytes)} in ${snapshot.storage.sessions} sessions${snapshot.storage.free_bytes != null ? ` · ${bytes(snapshot.storage.free_bytes)} free on the drive` : ""}`}>
          <div className="py-3">
            <div className="mb-2 flex justify-between text-[12px] text-fg-3">
              <span>Library usage</span>
              <span className="tabular-nums">
                {usedGb.toFixed(1)} / {draft.storage_limit_gb > 0 ? `${draft.storage_limit_gb.toFixed(0)} GB` : "unlimited"}
              </span>
            </div>
            <div className="h-2.5 overflow-hidden rounded-full bg-white/[0.07]">
              <motion.div
                className={`h-full rounded-full ${usage > 0.9 ? "bg-danger" : usage > 0.7 ? "bg-warn" : "bg-gradient-to-r from-cyan via-indigo to-violet"}`}
                initial={{ width: 0 }}
                animate={{ width: `${usage * 100}%` }}
                transition={spring.soft}
              />
            </div>
          </div>
          <Field label="Storage limit" description="Recording stops when the library reaches it (0 = no limit).">
            <Slider value={draft.storage_limit_gb} min={0} max={2000} step={10} onChange={(v) => apply({ storage_limit_gb: v })} format={(v) => (v === 0 ? "No limit" : `${v} GB`)} />
          </Field>
          <FolderField label="Sessions folder" value={draft.sessions_dir} onPick={(p) => apply({ sessions_dir: p })} />
          <FolderField label="Raw captures folder" description="Where Mixer's capture simulator writes." value={draft.raw_captures_dir} onPick={(p) => apply({ raw_captures_dir: p })} />
        </Section>

        <Section i={2} icon={<Radio size={17} />} title="VRChat and OSC" subtitle={snapshot.osc.listening ? `Listening on port ${snapshot.osc.port}${snapshot.osc.advertising ? ", advertised through OSCQuery" : ""}` : snapshot.osc.error ?? "Off"}>
          <Field label="Action Menu control" description="Start and stop recording from your avatar's Action Menu (OSCQuery, works next to VRCFT and other OSC apps).">
            <Toggle checked={draft.osc.enabled} onChange={(v) => apply({ osc: { ...draft.osc, enabled: v } })} />
          </Field>
          <Field label="Record parameter" description="Bool avatar parameter that toggles recording.">
            <div className="flex items-center gap-2">
              <TextInput value={draft.osc.record_parameter} onChange={(v) => setDraft({ ...draft, osc: { ...draft.osc, record_parameter: v } })} mono width="w-48" />
              <Button size="sm" variant="secondary" onClick={() => apply({ osc: draft.osc })}>
                Save
              </Button>
            </div>
          </Field>
          <Field label="VRChat log" description={snapshot.vrchat.world.log_file ?? (snapshot.vrchat.log_found ? "Found, waiting for VRChat" : "Not found on this machine")}>
            <span className={`text-[12px] ${snapshot.vrchat.log_found ? "text-ok" : "text-fg-4"}`}>{snapshot.vrchat.log_found ? "Read locally" : "–"}</span>
          </Field>
        </Section>

        <Section i={3} icon={<Sparkles size={17} />} title="Interface">
          <Field label="Reduce motion" description="Calmer transitions and a still background.">
            <Toggle checked={draft.reduce_motion} onChange={(v) => apply({ reduce_motion: v })} />
          </Field>
          <Field label="Capture notice" description="The consent reminder shown before the first capture.">
            <Button size="sm" variant="ghost" onClick={() => apply({ consent_acknowledged: false })}>
              Show again
            </Button>
          </Field>
        </Section>

        <Section i={4} icon={<Info size={17} />} title="About">
          <div className="flex items-center gap-4 py-3">
            <Logo size={48} intro={false} />
            <div className="min-w-0 flex-1">
              <div className="text-[15px] font-semibold">Orbiters desktop {snapshot.version}</div>
              <div className="text-[12.5px] text-fg-3">Copyright © 2026 Enzo DUTRA / blackorbit. Free to use; see the licence for what is allowed.</div>
              {snapshot.worker.error && <div className="mt-1 text-[12px] text-danger">{snapshot.worker.error}</div>}
            </div>
            <Button size="sm" variant="secondary" icon={<ScrollText size={14} />} onClick={() => void api.workerLog().then(setLog)}>
              Worker log
            </Button>
            {isTauri && (
              <Button size="sm" variant="danger" icon={<LogOut size={14} />} onClick={() => void api.quit()}>
                Quit
              </Button>
            )}
          </div>
          {log && (
            <motion.pre initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} className="mb-3 max-h-60 overflow-auto rounded-xl bg-black/40 p-3 font-mono text-[11px] leading-relaxed text-fg-2 hairline">
              {log.length ? log.join("\n") : "No output yet."}
            </motion.pre>
          )}
        </Section>
      </div>
    </div>
  );
}

function Section({ i, icon, title, subtitle, children }: { i: number; icon: ReactNode; title: string; subtitle?: string; children: ReactNode }) {
  return (
    <Card initial={{ opacity: 0, y: 18 }} animate={{ opacity: 1, y: 0 }} transition={{ ...spring.soft, delay: 0.06 + i * 0.06 }} className="px-6 pb-3 pt-5">
      <div className="mb-1 flex items-center gap-3">
        <div className="grid size-9 place-items-center rounded-xl bg-gradient-to-br from-indigo/25 to-violet/15 text-violet">{icon}</div>
        <div>
          <div className="text-[15.5px] font-semibold">{title}</div>
          {subtitle && <div className="text-[12px] text-fg-3">{subtitle}</div>}
        </div>
      </div>
      <div className="divide-y divide-white/[0.05]">{children}</div>
    </Card>
  );
}

function FolderField({ label, description, value, onPick }: { label: string; description?: string; value: string; onPick: (p: string) => void }) {
  return (
    <Field label={label} description={description ?? value}>
      <div className="flex items-center gap-2">
        {description && <span className="max-w-[260px] truncate text-[12px] text-fg-3">{value}</span>}
        <Button
          size="sm"
          variant="secondary"
          icon={<Folder size={14} />}
          onClick={async () => {
            const picked = await pickFolder(value);
            if (picked) onPick(picked);
          }}
        >
          Change
        </Button>
      </div>
    </Field>
  );
}
