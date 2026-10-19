import { motion } from "motion/react";

type Mood = "idle" | "recording" | "importing";

const palettes: Record<Mood, [string, string, string]> = {
  idle: ["rgba(162,89,247,0.55)", "rgba(79,137,255,0.42)", "rgba(72,230,255,0.22)"],
  recording: ["rgba(255,77,109,0.42)", "rgba(189,101,255,0.45)", "rgba(255,138,92,0.2)"],
  importing: ["rgba(72,230,255,0.4)", "rgba(116,102,255,0.45)", "rgba(60,242,154,0.18)"],
};

/** The website's swirling glow, slowed down for a desktop app, tinted by what the app is doing. */
export function GlowBackground({ mood, calm }: { mood: Mood; calm?: boolean }) {
  const [a, b, c] = palettes[mood];
  const loop = (duration: number) => (calm ? { duration: 0 } : { duration, repeat: Infinity, repeatType: "mirror" as const, ease: "easeInOut" as const });
  return (
    <div className="grain pointer-events-none absolute inset-0 overflow-hidden">
      <motion.div
        className="absolute -left-[12%] -top-[22%] h-[70%] w-[55%] rounded-full blur-[110px]"
        animate={{ backgroundColor: a, x: [0, 80, -30], y: [0, 40, 90], scale: [1, 1.12, 0.95] }}
        transition={{ backgroundColor: { duration: 1.6 }, default: loop(22) }}
      />
      <motion.div
        className="absolute -right-[10%] top-[8%] h-[60%] w-[48%] rounded-full blur-[120px]"
        animate={{ backgroundColor: b, x: [0, -70, 20], y: [0, 60, -20], scale: [1, 0.92, 1.1] }}
        transition={{ backgroundColor: { duration: 1.6 }, default: loop(26) }}
      />
      <motion.div
        className="absolute bottom-[-25%] left-[30%] h-[55%] w-[45%] rounded-full blur-[130px]"
        animate={{ backgroundColor: c, x: [0, 60, -60], y: [0, -40, 10], scale: [1, 1.15, 1] }}
        transition={{ backgroundColor: { duration: 1.6 }, default: loop(30) }}
      />
      <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_center,transparent_35%,rgba(7,7,11,0.85)_100%)]" />
    </div>
  );
}
