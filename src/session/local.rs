// runs a program on this machine in a pty

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

use super::{channel, dead, Control, End, Event, Session, Waker};

pub struct Spawn<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub term: &'a str,
    pub cols: u16,
    pub rows: u16,
}

/// look left and right of our own binary first, if we see it we launchin, otherwise PATH.
/// None if its nowhere, so the ui can say how to install it instead of a spawn error
pub fn find_program(program: &str) -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join(program);
        if candidate.is_file() && candidate != exe {
            return Some(candidate);
        }
    }
    // a path of its own, relative or absolute, isnt looked up anywhere
    let given = Path::new(program);
    if given.components().count() > 1 {
        return runnable(given).then(|| given.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(program);
        if runnable(&candidate) {
            return Some(candidate);
        }
        // windows wants the extension spelled out
        #[cfg(windows)]
        for ext in ["exe", "com", "bat", "cmd"] {
            let candidate = candidate.with_extension(ext);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    })
}

fn runnable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

struct Local {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

impl Control for Local {
    fn write(&self, bytes: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes);
        }
    }

    fn resize(&self, cols: u16, rows: u16) {
        if let Ok(m) = self.master.lock() {
            let _ = m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        if let Ok(mut k) = self.killer.lock() {
            let _ = k.kill();
        }
    }
}

pub fn spawn(s: Spawn, wake: Waker) -> Session {
    let Some(program) = find_program(s.program) else {
        return dead(End::NotFound, wake);
    };
    let fail = |e: &dyn std::fmt::Display| End::Failed(e.to_string());

    let pair = match native_pty_system().openpty(PtySize {
        rows: s.rows,
        cols: s.cols,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => return dead(fail(&e), wake),
    };

    let mut cmd = CommandBuilder::new(&program);
    cmd.args(s.args);
    cmd.env("TERM", s.term);

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => return dead(fail(&e), wake),
    };
    // the slaves end in the parent so tty gets proper eof (oof)
    drop(pair.slave);

    let (reader, writer) = match (pair.master.try_clone_reader(), pair.master.take_writer()) {
        (Ok(r), Ok(w)) => (r, w),
        (Err(e), _) | (_, Err(e)) => {
            let _ = child.kill();
            return dead(fail(&e), wake);
        }
    };
    let killer = child.clone_killer();
    let (tx, rx) = channel(wake);

    // reading reading yes yes je suis cooked
    let out = tx.clone();
    thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if !out.send(Event::Output(buf[..n].to_vec())) {
                        break;
                    }
                }
            }
        }
    });

    // child reaper thread, the one that gets to say its over
    thread::spawn(move || {
        let _ = child.wait();
        tx.send(Event::Ended(End::Exited));
    });

    Session {
        events: rx,
        control: Box::new(Local {
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
        }),
    }
}
