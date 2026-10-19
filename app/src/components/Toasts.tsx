import { AnimatePresence, motion } from "motion/react";
import { AlertTriangle, CheckCircle2, Info, XCircle } from "lucide-react";
import { useApp } from "../lib/store";
import { spring } from "./ui";

const icons = {
  info: <Info size={17} className="text-cyan" />,
  success: <CheckCircle2 size={17} className="text-ok" />,
  warning: <AlertTriangle size={17} className="text-warn" />,
  error: <XCircle size={17} className="text-danger" />,
};

export function Toasts() {
  const { toasts, dismissToast } = useApp();
  return (
    <div className="pointer-events-none fixed bottom-5 right-5 z-50 flex w-[380px] flex-col items-end gap-2.5">
      <AnimatePresence initial={false}>
        {toasts.map((t) => (
          <motion.div
            key={t.id}
            layout
            initial={{ opacity: 0, x: 60, scale: 0.92 }}
            animate={{ opacity: 1, x: 0, scale: 1 }}
            exit={{ opacity: 0, x: 40, scale: 0.9, transition: { duration: 0.18 } }}
            transition={spring.bouncy}
            onClick={() => dismissToast(t.id)}
            className="surface-strong pointer-events-auto relative flex w-full cursor-pointer items-start gap-3 overflow-hidden rounded-2xl px-4 py-3.5 shadow-[0_20px_50px_-20px_rgba(0,0,0,0.8)]"
          >
            <span className="mt-px">{icons[t.kind]}</span>
            <span className="text-[13px] leading-relaxed text-fg">{t.message}</span>
            <motion.span
              className="absolute bottom-0 left-0 h-[2px] bg-gradient-to-r from-cyan to-violet"
              initial={{ width: "100%" }}
              animate={{ width: "0%" }}
              transition={{ duration: t.kind === "error" ? 7 : 4.2, ease: "linear" }}
            />
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}
