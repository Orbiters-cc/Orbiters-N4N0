import { motion, useAnimationControls } from "motion/react";
import { useEffect } from "react";
import { useLive } from "../lib/live";

// The Orbiters mark (paths from the official logo, logo-animation/out/orbiters-static.svg).
const PATHS = [
  "M981.949 343.795L584.95 422.295L858.45 467.295C866.283 486.295 877.25 578.595 858.45 671.795C839.65 764.995 802.116 836.629 777.449 883.296C835.949 839.796 943.397 712.253 968.449 631.795C1003.01 520.795 1004.28 415.962 981.949 343.795Z",
  "M424.448 422.295L27.4482 343.795C5.11482 415.962 6.38698 520.795 40.9487 631.795C66.0004 712.253 173.448 839.795 231.948 883.295C207.282 836.629 169.748 764.995 150.948 671.795C132.148 578.595 143.114 486.295 150.948 467.295L424.448 422.295Z",
  "M150.948 244.404C178.038 163.552 279.673 11.001 501.532 12.5111C723.391 14.0213 831.919 165.077 858.45 244.404C781.649 209.571 674.868 91.4406 501.532 91.4406C328.196 91.4406 228.307 209.571 150.948 244.404Z",
];

function Mark({ fill }: { fill: string }) {
  return (
    <g fill={fill}>
      {PATHS.map((d) => (
        <path key={d} d={d} />
      ))}
    </g>
  );
}

/**
 * The mark draws itself in while its cyan and violet echoes slide apart and back together, like the
 * website's signal reveal; hovering replays the glitch. `pulse` adds a breathing glow (while recording).
 */
export function Logo({ size = 34, pulse = false, intro = true }: { size?: number; pulse?: boolean; intro?: boolean }) {
  const cyan = useAnimationControls();
  const violet = useAnimationControls();
  const live = useLive();
  pulse = pulse && live;

  const glitch = (strength: number, delay = 0) => {
    const t = { duration: 0.6 + strength * 0.3, delay, times: [0, 0.3, 0.65, 1], ease: "easeOut" as const };
    void cyan.start({ x: [0, -14 * strength, 8 * strength, 0], opacity: [0, 0.9, 0.55, 0], transition: t });
    void violet.start({ x: [0, 14 * strength, -8 * strength, 0], opacity: [0, 0.9, 0.55, 0], transition: t });
  };

  useEffect(() => {
    if (intro) glitch(1.2, 0.3);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [intro]);

  const height = (size * 900) / 1010;
  return (
    <motion.div className="relative shrink-0" style={{ width: size, height }} onHoverStart={() => glitch(0.8)}>
      {pulse && (
        <motion.div
          className="absolute -inset-3 rounded-full bg-violet/45 blur-xl"
          animate={{ opacity: [0.2, 0.75, 0.2], scale: [0.9, 1.18, 0.9] }}
          transition={{ duration: 2.4, repeat: Infinity, ease: "easeInOut" }}
        />
      )}
      <motion.svg viewBox="0 0 1010 900" className="absolute inset-0 overflow-visible mix-blend-screen" initial={{ opacity: 0 }} animate={cyan}>
        <Mark fill="#48e6ff" />
      </motion.svg>
      <motion.svg viewBox="0 0 1010 900" className="absolute inset-0 overflow-visible mix-blend-screen" initial={{ opacity: 0 }} animate={violet}>
        <Mark fill="#bd65ff" />
      </motion.svg>
      <svg viewBox="0 0 1010 900" className="absolute inset-0 overflow-visible">
        {PATHS.map((d, i) => (
          <motion.path
            key={d}
            d={d}
            fill="#e9e9f2"
            stroke="#e9e9f2"
            strokeWidth={22}
            strokeLinejoin="round"
            initial={intro ? { pathLength: 0, fillOpacity: 0 } : false}
            animate={{ pathLength: 1, fillOpacity: 1 }}
            transition={{ pathLength: { duration: 1.1, delay: i * 0.08, ease: [0.16, 1, 0.3, 1] }, fillOpacity: { delay: 0.65 + i * 0.08, duration: 0.45 } }}
          />
        ))}
      </svg>
    </motion.div>
  );
}
