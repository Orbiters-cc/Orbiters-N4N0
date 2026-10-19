import { AnimatePresence, MotionConfig, motion } from "motion/react";
import { useEffect, useState } from "react";
import { GlowBackground } from "./components/GlowBackground";
import { Sidebar, type Page } from "./components/Sidebar";
import { Splash } from "./components/Splash";
import { TitleBar } from "./components/TitleBar";
import { Toasts } from "./components/Toasts";
import { spring } from "./components/ui";
import { useApp } from "./lib/store";
import { CapturePage } from "./pages/CapturePage";
import { SessionsPage } from "./pages/SessionsPage";
import { SettingsPage } from "./pages/SettingsPage";

export default function App() {
  const { snapshot } = useApp();
  const [page, setPage] = useState<Page>("capture");
  const [splash, setSplash] = useState(true);

  useEffect(() => {
    const t = window.setTimeout(() => setSplash(false), 1900);
    return () => window.clearTimeout(t);
  }, []);

  const state = snapshot?.capture.state ?? "idle";
  const mood = state === "importing" ? "importing" : state === "recording" || state === "paused" || state === "starting" ? "recording" : "idle";
  const reduce = snapshot?.settings.reduce_motion ?? false;
  const subtitle = state === "recording" ? "Recording" : state === "importing" ? "Importing" : snapshot?.vrchat.world.world_name ?? undefined;

  return (
    <MotionConfig reducedMotion={reduce ? "always" : "user"}>
      <div className="relative flex h-full flex-col overflow-hidden">
        <GlowBackground mood={mood} calm={reduce} />
        <TitleBar subtitle={subtitle} />
        <div className="relative z-10 flex min-h-0 flex-1">
          {!splash && <Sidebar page={page} onPage={setPage} snapshot={snapshot} />}
          <main className="relative min-w-0 flex-1">
            <AnimatePresence mode="wait">
              {!splash && snapshot && (
                <motion.div
                  key={page}
                  className="absolute inset-0"
                  initial={{ opacity: 0, x: 24, filter: "blur(6px)" }}
                  animate={{ opacity: 1, x: 0, filter: "blur(0px)" }}
                  exit={{ opacity: 0, x: -16, filter: "blur(6px)", transition: { duration: 0.16 } }}
                  transition={spring.soft}
                >
                  {page === "capture" && <CapturePage snapshot={snapshot} />}
                  {page === "sessions" && <SessionsPage snapshot={snapshot} />}
                  {page === "settings" && <SettingsPage snapshot={snapshot} />}
                </motion.div>
              )}
            </AnimatePresence>
          </main>
        </div>
        <Toasts />
        <AnimatePresence>{splash && <Splash />}</AnimatePresence>
      </div>
    </MotionConfig>
  );
}
