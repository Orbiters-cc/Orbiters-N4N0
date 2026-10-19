import { motion } from "motion/react";
import { Logo } from "./Logo";

/** Opening moment: the mark assembles with its signal echoes, the name rises, then everything lifts away. */
export function Splash() {
  return (
    <motion.div
      className="fixed inset-0 z-[60] grid place-items-center bg-ink-950"
      initial={{ opacity: 1 }}
      exit={{ opacity: 0, scale: 1.04, filter: "blur(12px)", transition: { duration: 0.55, ease: [0.16, 1, 0.3, 1] } }}
    >
      <div className="flex flex-col items-center gap-6">
        <motion.div initial={{ scale: 0.82, opacity: 0 }} animate={{ scale: 1, opacity: 1 }} transition={{ type: "spring", stiffness: 180, damping: 18 }}>
          <Logo size={128} />
        </motion.div>
        <motion.div
          className="text-center"
          initial={{ opacity: 0, y: 14, filter: "blur(6px)" }}
          animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
          transition={{ delay: 0.75, duration: 0.6, ease: [0.16, 1, 0.3, 1] }}
        >
          <div className="text-[28px] font-semibold tracking-tight">Orbiters</div>
          <div className="text-gradient text-[13px] font-medium tracking-[0.32em] uppercase">desktop</div>
        </motion.div>
      </div>
    </motion.div>
  );
}
