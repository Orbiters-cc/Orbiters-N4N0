import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, listen } from "./backend";
import type { SessionSaved, Snapshot, Toast } from "./types";

interface AppContextValue {
  snapshot: Snapshot | null;
  toasts: Toast[];
  pushToast: (kind: Toast["kind"], message: string) => void;
  dismissToast: (id: number) => void;
  /// Bumps whenever a session is saved, so lists can reload.
  sessionsVersion: number;
  lastSaved: SessionSaved | null;
  run: (action: () => Promise<unknown>) => Promise<boolean>;
}

const AppContext = createContext<AppContextValue | null>(null);

export function AppProvider({ children }: { children: ReactNode }) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [sessionsVersion, setSessionsVersion] = useState(0);
  const [lastSaved, setLastSaved] = useState<SessionSaved | null>(null);
  const nextId = useRef(1);

  const pushToast = useCallback((kind: Toast["kind"], message: string) => {
    const id = nextId.current++;
    setToasts((t) => [...t.slice(-3), { id, kind, message }]);
    window.setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), kind === "error" ? 7000 : 4200);
  }, []);

  const dismissToast = useCallback((id: number) => setToasts((t) => t.filter((x) => x.id !== id)), []);

  useEffect(() => {
    let alive = true;
    void api.snapshot().then((s) => alive && setSnapshot(s));
    const offs = [
      listen<Snapshot>("snapshot", (s) => setSnapshot(s)),
      listen<Omit<Toast, "id">>("toast", (t) => pushToast(t.kind, t.message)),
      listen<SessionSaved>("session-saved", (s) => {
        setLastSaved(s);
        setSessionsVersion((v) => v + 1);
        if (s.reason !== "restarted") pushToast("success", `Session saved: ${s.frames.toLocaleString()} frames, ${s.keyframes} keyframes, ${s.depth_frames} depth frames.`);
      }),
    ];
    return () => {
      alive = false;
      offs.forEach((off) => off());
    };
  }, [pushToast]);

  const run = useCallback(
    async (action: () => Promise<unknown>) => {
      try {
        await action();
        return true;
      } catch (e) {
        pushToast("error", typeof e === "string" ? e : e instanceof Error ? e.message : String(e));
        return false;
      }
    },
    [pushToast],
  );

  const value = useMemo(
    () => ({ snapshot, toasts, pushToast, dismissToast, sessionsVersion, lastSaved, run }),
    [snapshot, toasts, pushToast, dismissToast, sessionsVersion, lastSaved, run],
  );
  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}

export function useApp(): AppContextValue {
  const ctx = useContext(AppContext);
  if (!ctx) throw new Error("useApp outside AppProvider");
  return ctx;
}
