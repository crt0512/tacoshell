// a pretend ssh server for poking at ssh mode without a real one
//
//   cargo run --example dev_server [port] [--key N]     (default 2222, key 42)
//
// any username, password "taco". it echoes what you type (control bytes as ^X)
// and does things on these keys:
//   q  exit 0 like the app quit on purpose
//   k  drop the session without an exit status, like it got killed -> "Connection Lost"
//   x  exit 1, also counts as lost
//   m  mouse reporting on/off (every move + sgr), clicks and hover get echoed
// ctrl+c the server itself to see "Connection Interrupted", start it again to see it resume.
// restart it with another --key to see the changed key warning

use std::sync::Arc;

use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::PrivateKey;
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};

#[derive(Clone, Default)]
struct Dev {
    mouse: bool,
}

impl server::Handler for Dev {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        println!("login {user:?} {}", if password == "taco" { "ok" } else { "wrong password" });
        Ok(if password == "taco" { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_session(&mut self, _: Channel<Msg>, reply: ChannelOpenHandle, _: &mut Session) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        _: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        println!("pty {term} {cols}x{rows}");
        Ok(())
    }

    async fn window_change_request(&mut self, _: ChannelId, cols: u32, rows: u32, _: u32, _: u32, _: &mut Session) -> Result<(), Self::Error> {
        println!("resize {cols}x{rows}");
        Ok(())
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        println!("shell");
        hello(channel, session)
    }

    async fn exec_request(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        println!("exec {:?}", String::from_utf8_lossy(data));
        hello(channel, session)
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        // arrow keys, mouse reports: show them, dont read their letters as commands
        if data.first() == Some(&0x1b) {
            let shown = String::from_utf8_lossy(&data[1..]);
            return session.data(channel, format!("\x1b[33m^[\x1b[0m{shown} ").into_bytes());
        }
        for &b in data {
            match b {
                b'q' => return quit(channel, session, Some(0)),
                b'x' => return quit(channel, session, Some(1)),
                b'k' => return quit(channel, session, None),
                b'm' => {
                    self.mouse = !self.mouse;
                    let (seq, what) = if self.mouse { ("h", "on") } else { ("l", "off") };
                    session.data(channel, format!("\x1b[?1003{seq}\x1b[?1006{seq}[mouse reporting {what}]\r\n").into_bytes())?;
                }
                b'\r' => session.data(channel, b"\r\n".to_vec())?,
                0x20..=0x7e => session.data(channel, vec![b])?,
                0x7f => session.data(channel, b"\x08 \x08".to_vec())?,
                c => session.data(channel, format!("\x1b[33m^{}\x1b[0m", (c ^ 0x40) as char).into_bytes())?,
            }
        }
        Ok(())
    }
}

fn hello(channel: ChannelId, session: &mut Session) -> Result<(), russh::Error> {
    session.channel_success(channel)?;
    let text = "\x1b[1;32mtacoshell dev server\x1b[0m\r\n\
                type away, q = quit, k = kill session, x = exit 1, m = mouse reporting\r\n\
                stop the server to test the interrupted screen\r\n\r\n";
    session.data(channel, text.as_bytes().to_vec())
}

fn quit(channel: ChannelId, session: &mut Session, status: Option<u32>) -> Result<(), russh::Error> {
    println!("ending session, exit status {status:?}");
    if let Some(s) = status {
        session.exit_status_request(channel, s)?;
        session.eof(channel)?;
    }
    session.close(channel)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port: u16 = args.first().and_then(|p| p.parse().ok()).unwrap_or(2222);
    let seed: u8 = args
        .iter()
        .position(|a| a == "--key")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse().ok())
        .unwrap_or(42);
    // same key every run so the trust prompt only shows up once. another --key = "the server changed"
    let key = PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]));
    println!("host key {}", key.public_key().fingerprint(Default::default()));
    let config = Arc::new(server::Config { keys: vec![key], ..Default::default() });

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("cant listen");
    println!("listening on 127.0.0.1:{port}, password \"taco\"");
    while let Ok((stream, addr)) = listener.accept().await {
        println!("connection from {addr}");
        let config = config.clone();
        tokio::spawn(async move {
            if let Ok(running) = server::run_stream(config, stream, Dev::default()).await {
                let _ = running.await;
            }
        });
    }
}
