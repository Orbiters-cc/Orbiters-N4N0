import { AnimatePresence, motion } from "motion/react";
import { useLive } from "../lib/live";
import type { CaptureStateName } from "../lib/types";
import { spring } from "./ui";

/**
 * The capture button. Two orbits (the Orbiters ring motif) circle it, faster while recording; the record
 * dot morphs into a stop square with a spring; ripples breathe out while frames arrive.
 */
export function RecordButton({ state, disabled, onToggle, health }: { state: CaptureStateName; disabled?: boolean; onToggle: () => void; health: number }) {
  const recording = state === "recording" || state === "paused";
  const busy = state === "starting" || state === "stopping" || state === "importing";
  const paused = state === "paused";
  const live = useLive();
  const size = 252;
  // Loops only run while the window is live; otherwise the rings rest where they are.
  const spin = (duration: number, direction: 1 | -1) =>
    live ? { animate: { rotate: 360 * direction }, transition: { duration, repeat: Infinity, ease: "linear" as const } } : { animate: { rotate: 0 }, transition: { duration: 0 } };

  return (
    <div className="relative grid place-items-center" style={{ width: size, height: size }}>
      {/* ripples */}
      <AnimatePresence>
        {recording && !paused && live &&
          [0, 1, 2].map((i) => (
            <motion.span
              key={i}
              className="absolute size-[136px] rounded-full border border-rec/50"
              initial={{ scale: 1, opacity: 0 }}
              animate={{ scale: [1, 1.75], opacity: [0.55, 0] }}
              exit={{ opacity: 0 }}
              transition={{ duration: 2.4, repeat: Infinity, delay: i * 0.8, ease: "easeOut" }}
            />
          ))}
      </AnimatePresence>

      {/* orbits */}
      <svg viewBox="0 0 252 252" className="pointer-events-none absolute inset-0">
        <defs>
          <linearGradient id="orbit-a" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stopColor="#48e6ff" />
            <stop offset="0.5" stopColor="#7466ff" />
            <stop offset="1" stopColor="#bd65ff" />
          </linearGradient>
          <linearGradient id="orbit-rec" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stopColor="#ff4d6d" />
            <stop offset="1" stopColor="#bd65ff" />
          </linearGradient>
        </defs>
        <motion.g style={{ originX: "126px", originY: "126px" }} {...spin(recording ? (paused ? 60 : 9) : 38, 1)}>
          <circle cx="126" cy="126" r="118" fill="none" stroke="rgba(255,255,255,0.07)" strokeWidth="1" />
          <circle cx="126" cy="126" r="118" fill="none" stroke={recording ? "url(#orbit-rec)" : "url(#orbit-a)"} strokeWidth="1.6" strokeDasharray="60 680" strokeLinecap="round" />
          <motion.circle cx="126" cy="8" r="4.5" animate={{ fill: recording ? "#ff4d6d" : "#48e6ff" }} style={{ filter: "drop-shadow(0 0 6px currentColor)" }} />
        </motion.g>
        <motion.g style={{ originX: "126px", originY: "126px" }} {...spin(recording ? (paused ? 80 : 14) : 52, -1)}>
          <circle cx="126" cy="126" r="100" fill="none" stroke="rgba(255,255,255,0.05)" strokeWidth="1" strokeDasharray="2 7" />
          <motion.circle cx="226" cy="126" r="3.2" animate={{ fill: recording ? "#ffb4c2" : "#bd65ff" }} />
        </motion.g>
        {/* decode health arc */}
        <motion.circle
          cx="126"
          cy="126"
          r="84"
          fill="none"
          stroke={health > 0.97 ? "#3cf29a" : health > 0.8 ? "#f5b74f" : "#ff6b7a"}
          strokeWidth="2.2"
          strokeLinecap="round"
          transform="rotate(-90 126 126)"
          initial={{ pathLength: 0, opacity: 0 }}
          animate={{ pathLength: recording ? Math.max(0.02, health) : 0, opacity: recording ? 0.9 : 0 }}
          transition={spring.soft}
        />
      </svg>

      {/* button */}
      <motion.button
        type="button"
        aria-label={recording ? "Stop recording" : "Start recording"}
        disabled={disabled || busy}
        onPointerDown={() => !disabled && !busy && onToggle()}
        whileHover={disabled || busy ? undefined : { scale: 1.035 }}
        whileTap={disabled || busy ? undefined : { scale: 0.9 }}
        transition={spring.bouncy}
        className="relative grid size-[140px] place-items-center rounded-full outline-none disabled:cursor-not-allowed"
      >
        {/* rotating gradient rim */}
        <motion.span
          className="absolute -inset-[2px] rounded-full"
          style={{
            background: recording
              ? "conic-gradient(from 0deg, #ff4d6d, #bd65ff, #ff8a5c, #ff4d6d)"
              : "conic-gradient(from 0deg, #48e6ff, #7466ff, #bd65ff, #4f89ff, #48e6ff)",
          }}
          animate={{ rotate: live ? 360 : 0, opacity: disabled ? 0.25 : 1 }}
          transition={{ rotate: live ? { duration: recording ? 3 : 8, repeat: Infinity, ease: "linear" } : { duration: 0 }, opacity: { duration: 0.3 } }}
        />
        <motion.span
          className="absolute inset-[2px] rounded-full"
          animate={{
            background: recording
              ? "radial-gradient(circle at 50% 35%, #3a1220 0%, #170910 70%)"
              : "radial-gradient(circle at 50% 35%, #22223a 0%, #0e0e17 72%)",
            boxShadow: recording ? "0 0 60px -6px rgba(255,77,109,0.6), inset 0 1px 0 rgba(255,255,255,0.08)" : "0 0 50px -10px rgba(116,102,255,0.55), inset 0 1px 0 rgba(255,255,255,0.08)",
          }}
          transition={{ duration: 0.5 }}
        />
        {/* glyph: dot ↔ square */}
        <AnimatePresence mode="wait">
          {busy ? (
            <motion.span
              key="busy"
              className="relative size-12 rounded-full border-[3px] border-white/15 border-t-white"
              initial={{ opacity: 0, scale: 0.6 }}
              animate={{ opacity: 1, scale: 1, rotate: 360 }}
              exit={{ opacity: 0, scale: 0.6 }}
              transition={{ rotate: { duration: 0.9, repeat: Infinity, ease: "linear" }, default: spring.snappy }}
            />
          ) : (
            <motion.span
              key="glyph"
              className="relative"
              initial={{ opacity: 0, scale: 0.4 }}
              animate={{
                opacity: 1,
                scale: 1,
                width: recording ? 42 : 50,
                height: recording ? 42 : 50,
                borderRadius: recording ? 12 : 999,
                backgroundColor: recording ? (paused ? "#f5b74f" : "#ffffff") : "#ff4d6d",
                boxShadow: recording ? "0 0 0 rgba(0,0,0,0)" : "0 0 30px rgba(255,77,109,0.7)",
              }}
              exit={{ opacity: 0, scale: 0.4 }}
              transition={spring.morph}
            />
          )}
        </AnimatePresence>
      </motion.button>
    </div>
  );
}
