import { motion } from "motion/react";
import { Minus, Square, X } from "lucide-react";
import { windowAction } from "../lib/backend";
import { spring } from "./ui";

/** Frameless window chrome: the whole bar drags the window, the buttons keep the platform's order. */
export function TitleBar({ subtitle }: { subtitle?: string }) {
  return (
    <div className="drag relative z-30 flex h-10 shrink-0 items-center justify-between pl-4 pr-1" onDoubleClick={() => void windowAction("maximize")}>
      <div className="flex items-center gap-2 text-[12px] text-fg-3">
        <span className="font-semibold tracking-wide text-fg-2">Orbiters desktop</span>
        {subtitle && (
          <motion.span key={subtitle} initial={{ opacity: 0, y: 4 }} animate={{ opacity: 1, y: 0 }} transition={spring.soft} className="truncate">
            · {subtitle}
          </motion.span>
        )}
      </div>
      <div className="no-drag flex items-center">
        <WindowButton label="Minimize" onClick={() => void windowAction("minimize")}>
          <Minus size={15} strokeWidth={1.8} />
        </WindowButton>
        <WindowButton label="Maximize" onClick={() => void windowAction("maximize")}>
          <Square size={12} strokeWidth={1.8} />
        </WindowButton>
        <WindowButton label="Hide to tray" danger onClick={() => void windowAction("close")}>
          <X size={16} strokeWidth={1.8} />
        </WindowButton>
      </div>
    </div>
  );
}

function WindowButton({ label, danger, onClick, children }: { label: string; danger?: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <motion.button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      whileTap={{ scale: 0.86 }}
      transition={spring.snappy}
      className={`grid h-8 w-11 place-items-center rounded-lg text-fg-3 transition-colors ${danger ? "hover:bg-rec/85 hover:text-white" : "hover:bg-white/[0.08] hover:text-fg"}`}
    >
      {children}
    </motion.button>
  );
}
