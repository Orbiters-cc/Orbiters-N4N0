import { motion } from "motion/react";
import { Aperture, FolderOpen, Settings2 } from "lucide-react";
import type { ReactNode } from "react";
import type { Snapshot } from "../lib/types";
import { Logo } from "./Logo";
import { cx, spring, StatusDot } from "./ui";

export type Page = "capture" | "sessions" | "settings";

const items: { id: Page; label: string; icon: ReactNode }[] = [
  { id: "capture", label: "Capture", icon: <Aperture size={18} strokeWidth={1.8} /> },
  { id: "sessions", label: "Sessions", icon: <FolderOpen size={18} strokeWidth={1.8} /> },
  { id: "settings", label: "Settings", icon: <Settings2 size={18} strokeWidth={1.8} /> },
];

export function Sidebar({ page, onPage, snapshot }: { page: Page; onPage: (p: Page) => void; snapshot: Snapshot | null }) {
  const capturing = snapshot ? ["starting", "recording", "paused", "importing"].includes(snapshot.capture.state) : false;
  const recording = snapshot?.capture.state === "recording";
  return (
    <aside className="relative z-20 flex w-[232px] shrink-0 flex-col px-4 pb-4">
      <div className="flex items-center gap-3 px-2 pb-7 pt-2">
        <Logo size={36} pulse={recording} />
        <div className="leading-tight">
          <div className="text-[17px] font-semibold tracking-tight">Orbiters</div>
          <div className="text-[11.5px] text-fg-3">desktop</div>
        </div>
      </div>

      <nav className="flex flex-col gap-1">
        {items.map((item, i) => {
          const active = page === item.id;
          return (
            <motion.button
              key={item.id}
              type="button"
              onPointerDown={() => onPage(item.id)}
              initial={{ opacity: 0, x: -12 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ ...spring.soft, delay: 0.15 + i * 0.06 }}
              whileTap={{ scale: 0.95 }}
              className={cx("relative flex h-11 items-center gap-3 rounded-2xl px-3.5 text-[14px] font-medium transition-colors", active ? "text-white" : "text-fg-3 hover:text-fg-2")}
            >
              {active && (
                <motion.span
                  layoutId="nav-pill"
                  transition={spring.bouncy}
                  className="absolute inset-0 -z-10 rounded-2xl bg-white/[0.08] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.06)]"
                >
                  <span className="absolute left-0 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-full bg-gradient-to-b from-cyan to-violet" />
                </motion.span>
              )}
              <span className={cx("transition-colors", active && "text-cyan")}>{item.icon}</span>
              {item.label}
              {item.id === "capture" && capturing && (
                <span className="ml-auto">
                  <StatusDot tone={snapshot?.capture.state === "importing" ? "info" : "rec"} pulse />
                </span>
              )}
              {item.id === "sessions" && snapshot && snapshot.storage.sessions > 0 && (
                <span className="ml-auto rounded-full bg-white/[0.07] px-2 py-0.5 text-[11px] tabular-nums text-fg-3">{snapshot.storage.sessions}</span>
              )}
            </motion.button>
          );
        })}
      </nav>

      <div className="mt-auto space-y-2.5">
        <SideStatus label="VRChat" tone={snapshot?.vrchat.running ? "ok" : "off"} text={snapshot?.vrchat.running ? snapshot.vrchat.world.world_name ?? "Running" : "Not running"} />
        <SideStatus
          label="Action Menu"
          tone={snapshot?.osc.listening ? (snapshot.osc.advertising ? "ok" : "warn") : "off"}
          text={snapshot?.osc.listening ? `OSC ready${snapshot.osc.port ? ` · ${snapshot.osc.port}` : ""}` : snapshot?.osc.enabled ? "OSC unavailable" : "OSC off"}
        />
        <div className="px-2 pt-2 text-[11px] text-fg-4">v{snapshot?.version ?? "0.1.0"}</div>
      </div>
    </aside>
  );
}

function SideStatus({ label, text, tone }: { label: string; text: string; tone: "ok" | "warn" | "off" }) {
  return (
    <div className="flex items-center gap-2.5 rounded-xl px-2.5 py-1.5">
      <StatusDot tone={tone} pulse={tone === "ok"} />
      <div className="min-w-0 leading-tight">
        <div className="label-caps">{label}</div>
        <div className="truncate text-[12px] text-fg-2">{text}</div>
      </div>
    </div>
  );
}
