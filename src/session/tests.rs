// ssh session tests against a russh server running in the test itself, with a little tcp proxy in between so we can pull the network cable

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{HashAlg, PrivateKey};
use russh::server::{self, Auth, Msg, Response, Session as ServerSession};
use russh::{Channel, ChannelId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use zeroize::Zeroizing;

use super::ssh::{self, Target};
use super::{End, Event, Session};

const WAIT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy)]
enum Mode {
    /// prints a line and exits 0
    Exit0,
    /// prints a line and closes the channel without an exit status, like a killed session
    Kill,
    /// prints a line and stays up
    Stay,
}

#[derive(Clone)]
struct Srv {
    mode: Mode,
    /// only keyboard interactive, like a pam setup with passwords turned off
    kbd_only: bool,
}

impl Srv {
    fn run(&self, channel: ChannelId, session: &mut ServerSession) -> Result<(), russh::Error> {
        session.channel_success(channel)?;
        session.data(channel, b"hello from the server\r\n".to_vec())?;
        match self.mode {
            Mode::Exit0 => {
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            Mode::Kill => session.close(channel)?,
            Mode::Stay => {}
        }
        Ok(())
    }
}

impl server::Handler for Srv {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if !self.kbd_only && user == "taco" && password == "salsa" { Auth::Accept } else { Auth::reject() })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        match response {
            None => Ok(Auth::Partial {
                name: "".into(),
                instructions: "".into(),
                prompts: vec![("Password: ".into(), false)].into(),
            }),
            Some(mut answers) => {
                let ok = user == "taco" && answers.next().as_deref() == Some(b"salsa".as_slice());
                Ok(if ok { Auth::Accept } else { Auth::reject() })
            }
        }
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(&mut self, channel: ChannelId, _data: &[u8], session: &mut ServerSession) -> Result<(), Self::Error> {
        self.run(channel, session)
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut ServerSession) -> Result<(), Self::Error> {
        self.run(channel, session)
    }
}

fn key(seed: u8) -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
}

fn fingerprint(key: &PrivateKey) -> String {
    key.public_key().fingerprint(HashAlg::Sha256).to_string()
}

/// ssh server on a random port
async fn server(srv: Srv, key: PrivateKey) -> u16 {
    let config = Arc::new(server::Config {
        keys: vec![key],
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (config, srv) = (config.clone(), srv.clone());
            tokio::spawn(async move {
                if let Ok(running) = server::run_stream(config, stream, srv).await {
                    let _ = running.await;
                }
            });
        }
    });
    port
}

/// forwards one port to another until cut() is called. then it stops listening and stops moving bytes but keeps the sockets open, so to the client it looks like the network went away, not like a clean close
struct Cable {
    port: u16,
    cut: Arc<AtomicBool>,
    listener: tokio::task::JoinHandle<()>,
}

impl Cable {
    async fn new(to: u16) -> Cable {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let cut = Arc::new(AtomicBool::new(false));
        let cut2 = cut.clone();
        let listener = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let upstream = TcpStream::connect(("127.0.0.1", to)).await.unwrap();
                let (cr, cw) = client.into_split();
                let (ur, uw) = upstream.into_split();
                tokio::spawn(pipe(cr, uw, cut2.clone()));
                tokio::spawn(pipe(ur, cw, cut2.clone()));
            }
        });
        Cable { port, cut, listener }
    }

    fn cut(&self) {
        self.cut.store(true, Ordering::SeqCst);
        self.listener.abort();
    }
}

async fn pipe(mut from: tokio::net::tcp::OwnedReadHalf, mut to: tokio::net::tcp::OwnedWriteHalf, cut: Arc<AtomicBool>) {
    let mut buf = [0u8; 4096];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if cut.load(Ordering::SeqCst) {
            // hold on to both sockets forever, nothing goes through anymore
            std::future::pending::<()>().await;
        }
        if to.write_all(&buf[..n]).await.is_err() {
            return;
        }
    }
}

fn tmp_known_hosts(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tacoshell-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("known_hosts")
}

fn target(port: u16, password: &str, known_hosts: Option<PathBuf>, pinned: Vec<String>) -> Target {
    Target {
        host: "127.0.0.1".into(),
        port,
        username: "taco".into(),
        password: Zeroizing::new(password.into()),
        command: Some("run".into()),
        term: "xterm-256color".into(),
        cols: 80,
        rows: 24,
        known_hosts,
        pinned,
        accept_new: false,
        connect_timeout: Duration::from_secs(3),
        keepalive: Duration::from_secs(1),
        keepalive_max: 2,
    }
}

fn connect(rt: &tokio::runtime::Runtime, t: Target) -> Session {
    ssh::connect(rt.handle(), t, Arc::new(|| {}))
}

/// everything up to and including the end, trusting whatever host key shows up
fn run_to_end(s: &Session) -> (bool, String, End) {
    let (mut connected, mut output) = (false, Vec::new());
    loop {
        match s.next_event(WAIT).expect("session went quiet") {
            Event::Connected => connected = true,
            Event::Output(b) => output.extend(b),
            Event::HostKey(q) => q.answer(true),
            Event::Ended(end) => return (connected, String::from_utf8_lossy(&output).into(), end),
        }
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
}

fn pinned_to(k: &PrivateKey) -> Vec<String> {
    vec![fingerprint(k)]
}

#[test]
fn clean_exit_is_exited() {
    let rt = rt();
    let k = key(1);
    let port = rt.block_on(server(Srv { mode: Mode::Exit0, kbd_only: false }, k.clone()));
    let (connected, output, end) = run_to_end(&connect(&rt, target(port, "salsa", None, pinned_to(&k))));
    assert!(connected);
    assert!(output.contains("hello from the server"), "{output:?}");
    assert!(matches!(end, End::Exited), "{end:?}");
}

#[test]
fn killed_session_is_lost() {
    let rt = rt();
    let k = key(1);
    let port = rt.block_on(server(Srv { mode: Mode::Kill, kbd_only: false }, k.clone()));
    let (connected, _, end) = run_to_end(&connect(&rt, target(port, "salsa", None, pinned_to(&k))));
    assert!(connected);
    assert!(matches!(end, End::Lost), "{end:?}");
}

#[test]
fn pulled_cable_is_unreachable() {
    let rt = rt();
    let k = key(1);
    let port = rt.block_on(server(Srv { mode: Mode::Stay, kbd_only: false }, k.clone()));
    let cable = rt.block_on(Cable::new(port));
    let s = connect(&rt, target(cable.port, "salsa", None, pinned_to(&k)));
    loop {
        match s.next_event(WAIT).expect("never connected") {
            Event::Connected => break,
            Event::Ended(end) => panic!("ended early: {end:?}"),
            _ => {}
        }
    }
    cable.cut();
    // keepalive every 1s, gives up after 2 missed, then the port probe fails
    let (_, _, end) = run_to_end(&s);
    assert!(matches!(end, End::Unreachable(_)), "{end:?}");
}

#[test]
fn nobody_listening_is_unreachable() {
    let rt = rt();
    let port = rt.block_on(async { TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port() });
    let (connected, _, end) = run_to_end(&connect(&rt, target(port, "salsa", None, vec![])));
    assert!(!connected);
    assert!(matches!(end, End::Unreachable(_)), "{end:?}");
}

#[test]
fn wrong_password_is_auth_failed() {
    let rt = rt();
    let k = key(1);
    let port = rt.block_on(server(Srv { mode: Mode::Exit0, kbd_only: false }, k.clone()));
    let (connected, _, end) = run_to_end(&connect(&rt, target(port, "nachos", None, pinned_to(&k))));
    assert!(!connected);
    assert!(matches!(end, End::AuthFailed), "{end:?}");
}

#[test]
fn keyboard_interactive_only_server_works() {
    let rt = rt();
    let k = key(1);
    let port = rt.block_on(server(Srv { mode: Mode::Exit0, kbd_only: true }, k.clone()));
    let (connected, _, end) = run_to_end(&connect(&rt, target(port, "salsa", None, pinned_to(&k))));
    assert!(connected);
    assert!(matches!(end, End::Exited), "{end:?}");
}

#[test]
fn unknown_host_asks_once_then_remembers() {
    let rt = rt();
    let k = key(2);
    let port = rt.block_on(server(Srv { mode: Mode::Exit0, kbd_only: false }, k.clone()));
    let kh = tmp_known_hosts("remember");

    let s = connect(&rt, target(port, "salsa", Some(kh.clone()), vec![]));
    match s.next_event(WAIT) {
        Some(Event::HostKey(q)) => {
            assert_eq!(q.fingerprint, fingerprint(&k));
            q.answer(true);
        }
        _ => panic!("expected a host key question first"),
    }
    let (connected, _, end) = run_to_end(&s);
    assert!(connected && matches!(end, End::Exited), "{end:?}");
    assert!(std::fs::read_to_string(&kh).unwrap().contains(&format!("[127.0.0.1]:{port}")));

    // second time around nobody gets asked
    let s = connect(&rt, target(port, "salsa", Some(kh.clone()), vec![]));
    loop {
        match s.next_event(WAIT).expect("session went quiet") {
            Event::HostKey(_) => panic!("asked again for a known host"),
            Event::Ended(end) => {
                assert!(matches!(end, End::Exited), "{end:?}");
                break;
            }
            _ => {}
        }
    }
}

#[test]
fn declined_host_key() {
    let rt = rt();
    let port = rt.block_on(server(Srv { mode: Mode::Exit0, kbd_only: false }, key(3)));
    let kh = tmp_known_hosts("decline");
    let s = connect(&rt, target(port, "salsa", Some(kh.clone()), vec![]));
    match s.next_event(WAIT) {
        Some(Event::HostKey(q)) => q.answer(false),
        _ => panic!("expected a host key question"),
    }
    let (connected, _, end) = run_to_end(&s);
    assert!(!connected);
    assert!(matches!(end, End::HostKeyDeclined), "{end:?}");
    assert!(!kh.exists());
}

#[test]
fn changed_host_key_is_refused() {
    let rt = rt();
    let port = rt.block_on(server(Srv { mode: Mode::Exit0, kbd_only: false }, key(4)));
    let kh = tmp_known_hosts("changed");
    // remember some other key for this server first
    russh::keys::known_hosts::learn_known_hosts_path("127.0.0.1", port, key(5).public_key(), &kh).unwrap();

    let (connected, _, end) = run_to_end(&connect(&rt, target(port, "salsa", Some(kh.clone()), vec![])));
    assert!(!connected);
    // russh starts a fresh file with a blank line, so the old key sits on line 2
    let End::HostKeyChanged(change) = end else { panic!("{end:?}") };
    assert_eq!(change.line, 2);
    assert_eq!(change.old, fingerprint(&key(5)));
    assert_eq!(change.new, fingerprint(&key(4)));

    // accepting it swaps the keys, and then it connects without a word
    ssh::replace_host_key("127.0.0.1", port, &kh, &change.new_key).unwrap();
    let known = russh::keys::known_hosts::known_host_keys_path("127.0.0.1", port, &kh).unwrap();
    assert_eq!(known.len(), 1, "old key still there: {known:?}");
    let (connected, _, end) = run_to_end(&connect(&rt, target(port, "salsa", Some(kh), vec![])));
    assert!(connected && matches!(end, End::Exited), "{end:?}");
}

#[test]
fn accept_new_skips_the_question() {
    let rt = rt();
    let k = key(8);
    let port = rt.block_on(server(Srv { mode: Mode::Exit0, kbd_only: false }, k.clone()));
    let kh = tmp_known_hosts("accept-new");
    let mut t = target(port, "salsa", Some(kh.clone()), vec![]);
    t.accept_new = true;
    let s = connect(&rt, t);
    loop {
        match s.next_event(WAIT).expect("session went quiet") {
            Event::HostKey(_) => panic!("asked even though accept_new is on"),
            Event::Ended(end) => {
                assert!(matches!(end, End::Exited), "{end:?}");
                break;
            }
            _ => {}
        }
    }
    // and it got remembered like a trusted one
    let known = russh::keys::known_hosts::known_host_keys_path("127.0.0.1", port, &kh).unwrap();
    assert_eq!(known.len(), 1);
}

#[test]
fn pinned_host_key_mismatch_is_refused() {
    let rt = rt();
    let port = rt.block_on(server(Srv { mode: Mode::Exit0, kbd_only: false }, key(6)));
    let pinned = vec![fingerprint(&key(7))];
    let (connected, _, end) = run_to_end(&connect(&rt, target(port, "salsa", None, pinned)));
    assert!(!connected);
    assert!(matches!(end, End::HostKeyMismatch { .. }), "{end:?}");
}
