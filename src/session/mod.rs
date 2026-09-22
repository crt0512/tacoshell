#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub mod local;
pub mod ssh;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;

/// pokes the ui so it drains events, called after every event
pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub enum Event {
    /// bytes for the terminal
    Output(Vec<u8>),
    /// ssh: logged in and the shell / command is running
    Connected,
    /// ssh: never seen this server before, ui has to ask
    HostKey(HostKeyQuestion),
    /// its over, always the last event
    Ended(End),
}

#[derive(Debug)]
pub enum End {
    /// the program quit on its own. ssh only says this for exit status 0
    Exited,
    /// ssh: session is gone (killed, crashed, dropped) but the server still answers
    Lost,
    /// ssh: cant reach the server (anymore)
    Unreachable(String),
    AuthFailed,
    HostKeyDeclined,
    /// the key in known_hosts (line, 1 based) isnt what the server showed us
    HostKeyChanged(KeyChange),
    /// not one of the keys pinned in the config
    HostKeyMismatch { new: String },
    /// local: the program isnt next to us or in PATH
    #[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
    NotFound,
    /// anything else, server said no, pty trouble, ...
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct KeyChange {
    pub file: PathBuf,
    pub line: usize,
    /// fingerprints, SHA256:...
    pub old: String,
    pub new: String,
    /// the new key in openssh format, for when the user accepts it
    pub new_key: String,
}

pub struct HostKeyQuestion {
    pub fingerprint: String,
    reply: tokio::sync::oneshot::Sender<bool>,
}

impl HostKeyQuestion {
    pub fn answer(self, trust: bool) {
        let _ = self.reply.send(trust);
    }
}

trait Control: Send {
    fn write(&self, bytes: &[u8]);
    fn resize(&self, cols: u16, rows: u16);
}

pub struct Session {
    events: mpsc::Receiver<Event>,
    control: Box<dyn Control>,
}

impl Session {
    pub fn events(&self) -> mpsc::TryIter<'_, Event> {
        self.events.try_iter()
    }

    pub fn write(&self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.control.write(bytes);
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        self.control.resize(cols, rows);
    }

    #[cfg(test)]
    fn next_event(&self, timeout: std::time::Duration) -> Option<Event> {
        self.events.recv_timeout(timeout).ok()
    }
}

/// sending half, wakes the ui after every event
#[derive(Clone)]
struct EventTx {
    tx: mpsc::Sender<Event>,
    wake: Waker,
}

impl EventTx {
    /// false once nobody listens anymore (session was dropped)
    fn send(&self, event: Event) -> bool {
        let ok = self.tx.send(event).is_ok();
        (self.wake)();
        ok
    }
}

fn channel(wake: Waker) -> (EventTx, mpsc::Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    (EventTx { tx, wake }, rx)
}

/// a session that failed before it started, so the ui only has one code path
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn dead(end: End, wake: Waker) -> Session {
    struct Nothing;
    impl Control for Nothing {
        fn write(&self, _: &[u8]) {}
        fn resize(&self, _: u16, _: u16) {}
    }
    let (tx, rx) = channel(wake);
    tx.send(Event::Ended(end));
    Session { events: rx, control: Box::new(Nothing) }
}
