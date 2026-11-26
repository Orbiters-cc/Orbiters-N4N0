//! What stands between the service and a client when a test, or `--link-smoke`, wants the link to go wrong: every line
//! passes through it both ways, and it can damage a line the service sends, stop reading what the service sends (so the
//! socket's buffers fill and the service's link has to wait), or drop the connection. The service and the client are
//! the real ones, each with its own socket: nothing in them knows this is there. Never part of how the app runs.

use orbiters_core::locks::*;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// Which of the service's lines to damage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// The parts of the state that changed.
    Changes,
    /// The whole state: the first line, or the answer to a client that asked for everything again.
    Whole,
}

#[derive(Default)]
struct Shared {
    /// The lines to damage, in the order they will come.
    damage: Mutex<VecDeque<Line>>,
    /// Nothing the service sends is read while this is set.
    held: Mutex<bool>,
    let_go: Condvar,
    /// Lines of the service passed on to a client (damaged ones too).
    passed: AtomicU64,
    /// Lines damaged so far.
    damaged: AtomicU64,
    sockets: Mutex<Vec<TcpStream>>,
    over: AtomicBool,
    /// The service's next lines that start with one of these are lost on the way, once each.
    lose: Mutex<VecDeque<Vec<u8>>>,
    /// Lines lost so far.
    lost: AtomicU64,
}

pub struct Between {
    port: u16,
    shared: Arc<Shared>,
}

impl Between {
    /// Listens on a port of its own and, for each client that connects there, connects to the service on
    /// `service_port`.
    pub fn start(service_port: u16) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared::default());
        let all = shared.clone();
        std::thread::Builder::new().name("link-between".into()).spawn(move || {
            for client in listener.incoming().flatten() {
                if all.over.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(service) = TcpStream::connect(("127.0.0.1", service_port)) else { continue };
                let _ = (client.set_nodelay(true), service.set_nodelay(true));
                let (Ok(client_in), Ok(service_in)) = (client.try_clone(), service.try_clone()) else { continue };
                if let (Ok(a), Ok(b)) = (client.try_clone(), service.try_clone()) {
                    all.sockets.locked().extend([a, b]);
                }
                // What the client says goes to the service as it is.
                let mut to_service = service;
                let _ = std::thread::Builder::new().name("link-between-up".into()).spawn(move || {
                    let _ = std::io::copy(&mut BufReader::new(client_in), &mut to_service);
                    let _ = to_service.shutdown(Shutdown::Both);
                });
                // What the service says goes to the client a line at a time.
                let shared = all.clone();
                let mut to_client = client;
                let _ = std::thread::Builder::new().name("link-between-down".into()).spawn(move || {
                    let mut reader = BufReader::new(service_in);
                    let mut line = Vec::new();
                    loop {
                        {
                            let mut held = shared.held.locked();
                            while *held {
                                held = shared.let_go.wait_locked(held);
                            }
                        }
                        line.clear();
                        if !matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
                            break;
                        }
                        let kind = if line.starts_with(br#"{"type":"changes""#) {
                            Some(Line::Changes)
                        } else if line.starts_with(br#"{"type":"snapshot""#) {
                            Some(Line::Whole)
                        } else {
                            None
                        };
                        let damage = {
                            let mut damage = shared.damage.locked();
                            let due = kind.is_some() && damage.front().copied() == kind;
                            if due {
                                damage.pop_front();
                            }
                            due
                        };
                        // A line lost on the way (a reply the client never gets).
                        let lost = {
                            let mut lose = shared.lose.locked();
                            let hit = lose.front().is_some_and(|p| line.starts_with(p));
                            if hit {
                                lose.pop_front();
                            }
                            hit
                        };
                        if lost {
                            shared.lost.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        if damage {
                            // Cut short on its way: its first half, and the line's end, so the lines after it are whole.
                            line.truncate((line.len() / 2).max(1));
                            line.push(b'\n');
                            shared.damaged.fetch_add(1, Ordering::Relaxed);
                        }
                        if to_client.write_all(&line).and_then(|_| to_client.flush()).is_err() {
                            break;
                        }
                        shared.passed.fetch_add(1, Ordering::Relaxed);
                    }
                    let _ = to_client.shutdown(Shutdown::Both);
                    let _ = reader.get_ref().shutdown(Shutdown::Both);
                });
            }
        })?;
        Ok(Self { port, shared })
    }

    /// The port a client connects to instead of the service's.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The service's next lines of these kinds arrive cut short, in this order (each waits for a line of its kind at
    /// the front: `[Changes, Whole]` damages the next change, then the whole state asked for because of it).
    pub fn damage(&self, lines: &[Line]) {
        self.shared.damage.locked().extend(lines.iter().copied());
    }

    /// Stops reading what the service sends (`true`), as a client that stopped reading does, or reads again.
    pub fn hold(&self, on: bool) {
        *self.shared.held.locked() = on;
        self.shared.let_go.notify_all();
    }

    /// Lines of the service passed on to a client so far, and how many of them were damaged.
    pub fn passed(&self) -> (u64, u64) {
        (self.shared.passed.load(Ordering::Relaxed), self.shared.damaged.load(Ordering::Relaxed))
    }

    /// The service's next line that starts with `prefix` is lost on its way to the client.
    pub fn lose(&self, prefix: &str) {
        self.shared.lose.locked().push_back(prefix.as_bytes().to_vec());
    }

    /// Lines lost so far.
    pub fn lost(&self) -> u64 {
        self.shared.lost.load(Ordering::Relaxed)
    }

    /// Drops every connection, both ways.
    pub fn hang_up(&self) {
        for socket in self.shared.sockets.locked().drain(..) {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Between {
    fn drop(&mut self) {
        self.shared.over.store(true, Ordering::Relaxed);
        self.hold(false);
        self.hang_up();
        // Its listening thread wakes on one last connection, and ends.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}
