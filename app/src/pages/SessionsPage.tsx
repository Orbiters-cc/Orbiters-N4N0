import { AnimatePresence, motion } from "motion/react";
import { Box, Clock, Cpu, Download, FolderOpen, Layers, Search, Sparkles, Trash2, Wand2, X } from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { api } from "../lib/backend";
import { bytes, duration, integer, relativeTime } from "../lib/format";
import { useApp } from "../lib/store";
import type { RawCaptureSummary, SessionSummary, Snapshot } from "../lib/types";
import { Logo } from "../components/Logo";
import { useLive } from "../lib/live";
import { TrailMap } from "../components/TrailMap";
import { Button, Card, cx, IconButton, spring } from "../components/ui";

function useThumbnail(dir: string, has: boolean) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    if (!has) return;
    let alive = true;
    void api.thumbnail(dir).then((b64) => {
      if (!alive || !b64) return;
      setSrc(b64.startsWith("iVBOR") ? `data:image/png;base64,${b64}` : `data:image/bmp;base64,${b64}`);
    });
    return () => {
      alive = false;
    };
  }, [dir, has]);
  return src;
}

const title = (s: SessionSummary) => s.manifest.world?.name ?? s.manifest.capture.source_name ?? "Session";

export function SessionsPage({ snapshot }: { snapshot: Snapshot }) {
  const { sessionsVersion, run } = useApp();
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [importOpen, setImportOpen] = useState(false);

  const reload = () => void api.sessions().then(setSessions);
  useEffect(reload, [sessionsVersion]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (sessions ?? []).filter((s) => !q || title(s).toLowerCase().includes(q) || s.dir.toLowerCase().includes(q));
  }, [sessions, query]);
  const total = (sessions ?? []).reduce((a, s) => a + s.size_bytes, 0);
  const current = sessions?.find((s) => s.dir === selected) ?? null;

  return (
    <div className="relative flex h-full flex-col overflow-hidden">
      <div className="flex items-end justify-between gap-4 px-8 pb-5 pt-2">
        <motion.div initial={{ opacity: 0, y: 14 }} animate={{ opacity: 1, y: 0 }} transition={spring.soft}>
          <div className="label-caps mb-1.5">Library</div>
          <h1 className="text-[30px] font-semibold tracking-tight">Sessions</h1>
          <div className="mt-1 text-[13px] text-fg-3">
            {sessions ? `${sessions.length} session${sessions.length === 1 ? "" : "s"} · ${bytes(total)}` : "Loading…"}
          </div>
        </motion.div>
        <div className="flex items-center gap-2">
          <div className="relative">
            <Search size={15} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-fg-4" />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search worlds"
              className="h-10 w-56 rounded-full bg-black/30 pl-9 pr-4 text-[13px] outline-none hairline transition-shadow placeholder:text-fg-4 focus:ring-2 focus:ring-indigo/60"
            />
          </div>
          <div className="relative">
            <Button variant="secondary" icon={<Download size={15} />} onClick={() => setImportOpen((o) => !o)}>
              Import raw capture
            </Button>
            <AnimatePresence>{importOpen && <ImportPopover onClose={() => setImportOpen(false)} busy={snapshot.capture.state !== "idle"} />}</AnimatePresence>
          </div>
          <IconButton label="Open the sessions folder" onClick={() => run(() => api.open(snapshot.settings.sessions_dir))}>
            <FolderOpen size={17} />
          </IconButton>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto px-8 pb-8">
        {sessions && filtered.length === 0 ? (
          <EmptyState searching={!!query} />
        ) : (
          <motion.div layout className="grid grid-cols-[repeat(auto-fill,minmax(270px,1fr))] gap-4">
            {filtered.map((s, i) => (
              <SessionCard key={s.dir} session={s} index={i} onOpen={() => setSelected(s.dir)} hidden={selected === s.dir} />
            ))}
            {!sessions && Array.from({ length: 6 }, (_, i) => <div key={i} className="shimmer surface relative h-[244px] overflow-hidden rounded-[var(--radius-card)]" />)}
          </motion.div>
        )}
      </div>

      <AnimatePresence>
        {current && (
          <SessionDetail
            session={current}
            onClose={() => setSelected(null)}
            onRemoved={() => {
              setSelected(null);
              reload();
            }}
          />
        )}
      </AnimatePresence>
    </div>
  );
}

function SessionCard({ session, index, onOpen, hidden }: { session: SessionSummary; index: number; onOpen: () => void; hidden: boolean }) {
  const src = useThumbnail(session.dir, session.has_thumbnail);
  const st = session.manifest.stats;
  return (
    <motion.button
      type="button"
      layoutId={`card-${session.dir}`}
      initial={{ opacity: 0, y: 20, scale: 0.97 }}
      animate={{ opacity: hidden ? 0 : 1, y: 0, scale: 1 }}
      transition={{ ...spring.soft, delay: Math.min(index, 12) * 0.035 }}
      whileHover={{ y: -4 }}
      whileTap={{ scale: 0.97 }}
      onClick={onOpen}
      className="surface group relative overflow-hidden rounded-[var(--radius-card)] text-left"
    >
      <motion.div layoutId={`thumb-${session.dir}`} className="relative aspect-video overflow-hidden bg-black/40">
        {src ? (
          <img src={src} alt="" className="size-full object-cover transition-transform duration-700 ease-[var(--ease-out-expo)] group-hover:scale-[1.06]" />
        ) : (
          <div className="grid size-full place-items-center bg-gradient-to-br from-indigo/20 to-violet/10">
            <Logo size={40} intro={false} />
          </div>
        )}
        <div className="absolute inset-0 bg-gradient-to-t from-ink-950/90 via-ink-950/10 to-transparent" />
        <span className="absolute left-3 top-3 rounded-full bg-black/55 px-2.5 py-1 text-[10.5px] font-semibold tracking-wide text-fg-2 backdrop-blur">
          {duration(st.duration_s)}
        </span>
        {session.manifest.consent.own_world && <span className="absolute right-3 top-3 rounded-full bg-ok/20 px-2.5 py-1 text-[10.5px] font-semibold text-ok backdrop-blur">Own world</span>}
      </motion.div>
      <div className="px-4 pb-4 pt-3">
        <div className="truncate text-[15px] font-semibold">{title(session)}</div>
        <div className="mt-0.5 text-[12px] text-fg-3">{relativeTime(session.modified)}</div>
        <div className="mt-3 flex flex-wrap gap-1.5 text-[11.5px] text-fg-2">
          <Pill icon={<Layers size={12} />}>{integer(st.frames)}</Pill>
          <Pill icon={<Box size={12} />}>{integer(st.keyframes)}</Pill>
          <Pill icon={<Cpu size={12} />}>{integer(st.depth_frames)}</Pill>
          <Pill>{bytes(session.size_bytes)}</Pill>
        </div>
      </div>
    </motion.button>
  );
}

function Pill({ icon, children }: { icon?: ReactNode; children: ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1 rounded-full bg-white/[0.06] px-2 py-0.5 tabular-nums">
      {icon && <span className="text-fg-4">{icon}</span>}
      {children}
    </span>
  );
}

function SessionDetail({ session, onClose, onRemoved }: { session: SessionSummary; onClose: () => void; onRemoved: () => void }) {
  const { run, pushToast } = useApp();
  const src = useThumbnail(session.dir, session.has_thumbnail);
  const [trail, setTrail] = useState<[number, number, number][]>([]);
  const [confirm, setConfirm] = useState(false);
  const st = session.manifest.stats;
  useEffect(() => {
    void api.trail(session.dir).then(setTrail).catch(() => setTrail([]));
  }, [session.dir]);

  return (
    <motion.div className="absolute inset-0 z-20 flex items-center justify-center p-8" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
      <motion.div className="absolute inset-0 bg-ink-950/70 backdrop-blur-sm" onClick={onClose} />
      <motion.div layoutId={`card-${session.dir}`} transition={spring.morph} className="surface-strong relative flex max-h-full w-full max-w-[980px] flex-col overflow-hidden rounded-[28px]">
        <motion.div layoutId={`thumb-${session.dir}`} transition={spring.morph} className="relative h-[260px] shrink-0 overflow-hidden bg-black/40">
          {src && <img src={src} alt="" className="size-full object-cover" />}
          <div className="absolute inset-0 bg-gradient-to-t from-ink-750 via-ink-750/20 to-transparent" />
          <IconButton label="Close" onClick={onClose} className="absolute right-4 top-4 bg-black/40 backdrop-blur">
            <X size={18} />
          </IconButton>
          <div className="absolute bottom-5 left-6 right-6">
            <motion.div initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} transition={{ ...spring.soft, delay: 0.12 }} className="text-[26px] font-semibold tracking-tight">
              {title(session)}
            </motion.div>
            <div className="mt-1 truncate font-mono text-[11.5px] text-fg-3">{session.manifest.world?.id ?? session.dir}</div>
          </div>
        </motion.div>

        <motion.div initial={{ opacity: 0, y: 14 }} animate={{ opacity: 1, y: 0 }} transition={{ ...spring.soft, delay: 0.16 }} className="grid grid-cols-[1fr_300px] gap-5 overflow-y-auto p-6">
          <div className="space-y-4">
            <div className="grid grid-cols-4 gap-3">
              <MiniStat icon={<Clock size={14} />} label="Duration" value={duration(st.duration_s)} />
              <MiniStat icon={<Layers size={14} />} label="Frames" value={integer(st.frames)} sub={`${((st.strip_ok / Math.max(1, st.frames)) * 100).toFixed(1)}% decoded`} />
              <MiniStat icon={<Box size={14} />} label="Keyframes" value={integer(st.keyframes)} />
              <MiniStat icon={<Cpu size={14} />} label="Depth frames" value={integer(st.depth_frames)} />
            </div>
            <TrailMap points={trail} className="h-[230px]" />
          </div>
          <div className="flex flex-col gap-3">
            <Card className="space-y-2.5 p-4 text-[12.5px]">
              <Row k="Captured" v={new Date(session.manifest.created_utc).toLocaleString()} />
              <Row k="Source" v={session.manifest.capture.source_name || session.manifest.capture.source} />
              <Row k="Resolution" v={session.manifest.capture.width ? `${session.manifest.capture.width}×${session.manifest.capture.height}` : "–"} />
              <Row k="Quality" v={session.manifest.capture.quality} />
              <Row k="Size" v={bytes(session.size_bytes)} />
              <Row k="Format" v={`session v${session.manifest.version} · contracts ${session.manifest.contracts_version}`} />
            </Card>
            <Button variant="primary" icon={<Wand2 size={15} />} disabled title="The reconstruction worker is the next milestone">
              Reconstruct
            </Button>
            <Button variant="secondary" icon={<Sparkles size={15} />} disabled title="Mixer import arrives with the Mixer link">
              Send to Mixer
            </Button>
            <div className="flex gap-2">
              <Button variant="secondary" className="flex-1" icon={<FolderOpen size={15} />} onClick={() => run(() => api.open(session.dir))}>
                Open folder
              </Button>
              <AnimatePresence mode="wait" initial={false}>
                {confirm ? (
                  <motion.div key="confirm" initial={{ opacity: 0, scale: 0.9 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.9 }} transition={spring.snappy}>
                    <Button
                      variant="danger"
                      icon={<Trash2 size={15} />}
                      onClick={async () => {
                        if (await run(() => api.trash(session.dir))) {
                          pushToast("success", "Session moved to the recycle bin.");
                          onRemoved();
                        }
                      }}
                    >
                      Confirm
                    </Button>
                  </motion.div>
                ) : (
                  <motion.div key="ask" initial={{ opacity: 0, scale: 0.9 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.9 }} transition={spring.snappy}>
                    <IconButton label="Move to the recycle bin" onClick={() => setConfirm(true)} className="size-10 hover:bg-danger/15 hover:text-danger">
                      <Trash2 size={17} />
                    </IconButton>
                  </motion.div>
                )}
              </AnimatePresence>
            </div>
          </div>
        </motion.div>
      </motion.div>
    </motion.div>
  );
}

function MiniStat({ icon, label, value, sub }: { icon: ReactNode; label: string; value: string; sub?: string }) {
  return (
    <div className="rounded-2xl bg-black/25 p-3 hairline">
      <div className="flex items-center gap-1.5 text-fg-4">
        {icon}
        <span className="label-caps">{label}</span>
      </div>
      <div className="mt-1.5 text-[18px] font-semibold tabular-nums">{value}</div>
      {sub && <div className="text-[11px] text-fg-3">{sub}</div>}
    </div>
  );
}

function Row({ k, v }: { k: string; v: string }) {
  return (
    <div className="flex justify-between gap-3">
      <span className="text-fg-3">{k}</span>
      <span className="truncate text-right text-fg">{v}</span>
    </div>
  );
}

function ImportPopover({ onClose, busy }: { onClose: () => void; busy: boolean }) {
  const { run, pushToast } = useApp();
  const [raws, setRaws] = useState<RawCaptureSummary[] | null>(null);
  useEffect(() => {
    void api.rawCaptures().then(setRaws);
  }, []);
  return (
    <>
      <div className="fixed inset-0 z-30" onClick={onClose} />
      <motion.div
        initial={{ opacity: 0, y: -8, scale: 0.96 }}
        animate={{ opacity: 1, y: 0, scale: 1 }}
        exit={{ opacity: 0, y: -6, scale: 0.97 }}
        transition={spring.bouncy}
        style={{ originX: 1, originY: 0 }}
        className="surface-strong absolute right-0 top-12 z-40 w-[420px] rounded-2xl p-2 shadow-[0_30px_60px_-20px_rgba(0,0,0,0.85)]"
      >
        <div className="px-3 pb-2 pt-2">
          <div className="text-[13.5px] font-semibold">Raw captures</div>
          <div className="text-[12px] text-fg-3">Frames written by Mixer's capture simulator, replayed through the same decoder as Spout.</div>
        </div>
        <div className="max-h-[320px] overflow-y-auto">
          {raws === null && <div className="shimmer relative m-2 h-14 overflow-hidden rounded-xl bg-white/5" />}
          {raws?.length === 0 && <div className="px-3 py-6 text-center text-[12.5px] text-fg-4">No raw captures in the raw captures folder yet.</div>}
          {raws?.map((r) => (
            <motion.button
              key={r.dir}
              type="button"
              whileTap={{ scale: 0.98 }}
              disabled={busy}
              onClick={async () => {
                if (await run(() => api.importRaw(r.dir))) {
                  pushToast("info", `Importing ${r.name}…`);
                  onClose();
                }
              }}
              className={cx("flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left transition-colors hover:bg-white/[0.06]", busy && "opacity-50")}
            >
              <div className="grid size-9 place-items-center rounded-lg bg-cyan/10 text-cyan">
                <Download size={16} />
              </div>
              <div className="min-w-0 flex-1">
                <div className="truncate text-[13px] font-medium">{r.name}</div>
                <div className="truncate text-[11.5px] text-fg-3">
                  {r.frames} frames · {r.width}×{r.height}
                  {r.depth_every ? ` · depth every ${r.depth_every}` : ""}
                  {r.has_ground_truth ? " · ground truth" : ""}
                </div>
              </div>
            </motion.button>
          ))}
        </div>
      </motion.div>
    </>
  );
}

function EmptyState({ searching }: { searching: boolean }) {
  const live = useLive();
  return (
    <motion.div initial={{ opacity: 0, y: 20 }} animate={{ opacity: 1, y: 0 }} transition={spring.soft} className="flex h-[60%] flex-col items-center justify-center gap-4 text-center">
      <motion.div animate={live ? { y: [0, -8, 0] } : { y: 0 }} transition={live ? { duration: 4, repeat: Infinity, ease: "easeInOut" } : { duration: 0 }}>
        <Logo size={70} intro={false} />
      </motion.div>
      <div className="text-[17px] font-semibold">{searching ? "No session matches" : "No sessions yet"}</div>
      <div className="max-w-sm text-[13px] text-fg-3">
        {searching ? "Try another world name." : "Record in VRChat from the Capture page, or import a raw capture from Mixer's simulator."}
      </div>
    </motion.div>
  );
}
