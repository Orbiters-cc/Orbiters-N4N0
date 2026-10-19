import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { isTauri } from "./backend";

// "Live" means the window is visible and focused. Looping animations (glow, orbits, pulses) only run while
// live: an unfocused window (you are in VRChat) costs no GPU time. Measured: the WebView kept the GPU's 3D
// engine at 14-17 % with looping animations even when hidden; without them it idles.

const LiveContext = createContext(true);

export function LiveProvider({ children, reduceMotion }: { children: ReactNode; reduceMotion: boolean }) {
  const [focused, setFocused] = useState(() => (typeof document === "undefined" ? true : document.hasFocus()));
  const [visible, setVisible] = useState(() => (typeof document === "undefined" ? true : document.visibilityState === "visible"));

  useEffect(() => {
    const onVisibility = () => setVisible(document.visibilityState === "visible");
    const onFocus = () => setFocused(true);
    const onBlur = () => setFocused(false);
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    let unlisten: (() => void) | undefined;
    if (isTauri) {
      void import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
        getCurrentWindow()
          .onFocusChanged(({ payload }) => setFocused(payload))
          .then((u) => (unlisten = u)),
      );
    }
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onBlur);
      unlisten?.();
    };
  }, []);

  return <LiveContext.Provider value={focused && visible && !reduceMotion}>{children}</LiveContext.Provider>;
}

/** True when looping animations may run. */
export function useLive(): boolean {
  return useContext(LiveContext);
}
