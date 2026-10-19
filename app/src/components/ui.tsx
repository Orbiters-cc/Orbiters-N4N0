import { motion, useMotionValueEvent, useSpring, type HTMLMotionProps } from "motion/react";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useLive } from "../lib/live";

// Spring presets shared by every animation: quick and slightly bouncy, like the tools' ease-out-back presses.
export const spring = {
  snappy: { type: "spring", stiffness: 560, damping: 32, mass: 0.7 } as const,
  bouncy: { type: "spring", stiffness: 420, damping: 17, mass: 0.8 } as const,
  soft: { type: "spring", stiffness: 170, damping: 24 } as const,
  morph: { type: "spring", stiffness: 300, damping: 26, mass: 0.9 } as const,
};

export function cx(...parts: (string | false | null | undefined)[]) {
  return parts.filter(Boolean).join(" ");
}

type Variant = "primary" | "secondary" | "ghost" | "danger" | "glass";

const variants: Record<Variant, string> = {
  primary: "bg-gradient-to-r from-blue via-indigo to-violet text-white shadow-[0_8px_28px_-10px_rgba(116,102,255,0.75)]",
  secondary: "bg-white/[0.07] text-fg hover:bg-white/[0.11]",
  ghost: "bg-transparent text-fg-2 hover:bg-white/[0.06] hover:text-fg",
  danger: "bg-danger/15 text-danger hover:bg-danger/22",
  glass: "surface text-fg hover:bg-white/[0.08]",
};

export function Button({
  variant = "secondary",
  size = "md",
  icon,
  children,
  className,
  disabled,
  ...rest
}: { variant?: Variant; size?: "sm" | "md" | "lg"; icon?: ReactNode } & HTMLMotionProps<"button">) {
  const sizes = { sm: "h-8 px-3.5 text-[12.5px] gap-1.5", md: "h-10 px-4.5 text-[13.5px] gap-2", lg: "h-12 px-6 text-[15px] gap-2.5" };
  return (
    <motion.button
      type="button"
      whileHover={disabled ? undefined : { y: -1 }}
      whileTap={disabled ? undefined : { scale: 0.94 }}
      transition={spring.snappy}
      disabled={disabled}
      className={cx(
        "relative inline-flex items-center justify-center rounded-full font-medium outline-none transition-colors duration-150 select-none",
        "focus-visible:ring-2 focus-visible:ring-indigo/70 disabled:pointer-events-none disabled:opacity-40",
        sizes[size],
        variants[variant],
        className,
      )}
      {...rest}
    >
      {icon}
      {children as ReactNode}
    </motion.button>
  );
}

export function IconButton({ label, children, className, ...rest }: { label: string; children: ReactNode } & HTMLMotionProps<"button">) {
  return (
    <motion.button
      type="button"
      aria-label={label}
      title={label}
      whileTap={{ scale: 0.88 }}
      transition={spring.snappy}
      className={cx("grid size-9 place-items-center rounded-full text-fg-2 transition-colors hover:bg-white/[0.08] hover:text-fg", className)}
      {...rest}
    >
      {children}
    </motion.button>
  );
}

export interface SegmentOption<T extends string> {
  value: T;
  label: string;
  hint?: string;
  disabled?: boolean;
  badge?: string;
}

/** Segmented control whose highlight slides between options with a spring (shared layout). */
export function Segmented<T extends string>({ value, options, onChange, size = "md" }: { value: T; options: SegmentOption<T>[]; onChange: (v: T) => void; size?: "sm" | "md" }) {
  const id = useId();
  return (
    <div className={cx("relative inline-flex rounded-full bg-black/30 p-1 hairline", size === "sm" ? "text-[12px]" : "text-[13px]")}>
      {options.map((o) => {
        const active = o.value === value;
        return (
          <motion.button
            key={o.value}
            type="button"
            title={o.hint}
            disabled={o.disabled}
            whileTap={o.disabled ? undefined : { scale: 0.94 }}
            transition={spring.snappy}
            onPointerDown={() => !o.disabled && onChange(o.value)}
            className={cx(
              "relative z-0 flex items-center gap-1.5 rounded-full font-medium transition-colors duration-200",
              size === "sm" ? "h-7 px-3" : "h-8 px-3.5",
              active ? "text-white" : "text-fg-3 hover:text-fg-2",
              o.disabled && "cursor-not-allowed opacity-45",
            )}
          >
            {active && (
              <motion.span
                layoutId={`seg-${id}`}
                transition={spring.bouncy}
                className="absolute inset-0 -z-10 rounded-full bg-gradient-to-r from-indigo/80 to-violet/80 shadow-[0_4px_18px_-6px_rgba(189,101,255,0.8)]"
              />
            )}
            {o.label}
            {o.badge && <span className="rounded-full bg-white/10 px-1.5 py-px text-[9.5px] font-semibold uppercase tracking-wide text-fg-2">{o.badge}</span>}
          </motion.button>
        );
      })}
    </div>
  );
}

export function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label?: string }) {
  return (
    <motion.button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onPointerDown={() => onChange(!checked)}
      whileTap={{ scale: 0.92 }}
      className={cx(
        "relative h-7 w-12 shrink-0 rounded-full p-1 transition-colors duration-300",
        checked ? "bg-gradient-to-r from-indigo to-violet" : "bg-white/[0.1]",
      )}
    >
      <motion.span
        layout
        transition={spring.bouncy}
        className={cx("block size-5 rounded-full bg-white shadow-[0_2px_8px_rgba(0,0,0,0.4)]", checked ? "ml-auto" : "ml-0")}
      />
    </motion.button>
  );
}

export function Card({ children, className, ...rest }: { children: ReactNode } & HTMLMotionProps<"div">) {
  return (
    <motion.div className={cx("surface relative rounded-[var(--radius-card)]", className)} {...rest}>
      {children}
    </motion.div>
  );
}

export function StatusDot({ tone, pulse }: { tone: "ok" | "warn" | "off" | "rec" | "info"; pulse?: boolean }) {
  const live = useLive();
  pulse = pulse && live;
  const colors = { ok: "bg-ok", warn: "bg-warn", off: "bg-fg-4", rec: "bg-rec", info: "bg-cyan" };
  return (
    <span className="relative inline-flex size-2.5">
      {pulse && <span className={cx("absolute inset-0 animate-ping rounded-full opacity-60", colors[tone])} />}
      <span className={cx("relative inline-flex size-2.5 rounded-full", colors[tone])} />
    </span>
  );
}

/** Number that springs to its new value. */
export function AnimatedNumber({ value, format = defaultFormat, className }: { value: number; format?: (v: number) => string; className?: string }) {
  const mv = useSpring(value, { stiffness: 120, damping: 22, mass: 0.6 });
  const formatRef = useRef(format);
  formatRef.current = format;
  const [text, setText] = useState(() => format(value));
  useEffect(() => {
    mv.set(value);
  }, [mv, value]);
  // The formatter can change without the value changing (units appear once data arrives).
  useEffect(() => {
    setText(formatRef.current(mv.get()));
  });
  useMotionValueEvent(mv, "change", (v) => setText(formatRef.current(v)));
  return <span className={cx("tabular-nums", className)}>{text}</span>;
}

function defaultFormat(v: number) {
  return Math.round(v).toLocaleString("en-US");
}

export function Field({ label, description, children }: { label: string; description?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-6 py-3.5">
      <div className="min-w-0">
        <div className="text-[13.5px] font-medium text-fg">{label}</div>
        {description && <div className="mt-0.5 text-[12px] leading-relaxed text-fg-3">{description}</div>}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

export function Slider({ value, min, max, step, onChange, format }: { value: number; min: number; max: number; step: number; onChange: (v: number) => void; format?: (v: number) => string }) {
  const pct = ((value - min) / (max - min)) * 100;
  return (
    <div className="flex items-center gap-3">
      <div className="relative h-7 w-48">
        <div className="absolute top-1/2 h-1.5 w-full -translate-y-1/2 rounded-full bg-white/[0.09]" />
        <motion.div className="absolute top-1/2 h-1.5 -translate-y-1/2 rounded-full bg-gradient-to-r from-cyan via-indigo to-violet" animate={{ width: `${pct}%` }} transition={spring.snappy} />
        <motion.div
          className="pointer-events-none absolute top-1/2 size-4.5 -translate-x-1/2 -translate-y-1/2 rounded-full bg-white shadow-[0_2px_10px_rgba(0,0,0,0.5)]"
          animate={{ left: `${pct}%` }}
          transition={spring.bouncy}
        />
        <input
          type="range"
          min={min}
          max={max}
          step={step}
          value={value}
          onChange={(e) => onChange(Number(e.target.value))}
          className="absolute inset-0 w-full cursor-pointer opacity-0"
        />
      </div>
      <span className="w-20 text-right text-[12.5px] tabular-nums text-fg-2">{format ? format(value) : value}</span>
    </div>
  );
}

export function TextInput({ value, onChange, placeholder, mono, width = "w-64" }: { value: string; onChange: (v: string) => void; placeholder?: string; mono?: boolean; width?: string }) {
  return (
    <input
      value={value}
      placeholder={placeholder}
      onChange={(e) => onChange(e.target.value)}
      className={cx(
        "h-9 rounded-xl bg-black/30 px-3 text-[13px] text-fg outline-none hairline transition-shadow placeholder:text-fg-4 focus:ring-2 focus:ring-indigo/60",
        mono && "font-mono text-[12px]",
        width,
      )}
    />
  );
}
