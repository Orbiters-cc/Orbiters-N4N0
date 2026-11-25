//! The icon in the tray: not yet (L2: StatusNotifierItem over the session's D-Bus, and "closing the window quits" where no
//! desktop shows it). Until then the service has no message loop: it waits for Quit, and closing the window is Quit
//! (`orbiters_core::here::Here::tray`).
use crate::service::Service;
use std::sync::{mpsc, Arc};

/// Runs on the main thread until Quit.
pub fn run(_service: &Arc<Service>, quit: &mpsc::Receiver<()>) {
    let _ = quit.recv();
}
