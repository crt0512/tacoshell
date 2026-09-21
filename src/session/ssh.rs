// logs into a server with russh (not rushb lol) and runs a "shell" (or a command) in a pty there

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::{self, HashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

use super::{channel, Control, End, Event, EventTx, HostKeyQuestion, KeyChange, Session, Waker};

pub struct Target {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Zeroizing<String>,
    /// None = the users login shell
    pub command: Option<String>,
    pub term: String,
    pub cols: u16,
    pub rows: u16,
    /// where trusted server keys go, None = ask every time
    pub known_hosts: Option<PathBuf>,
    /// baked in fingerprints, when set nothing else is accepted and nobody gets asked
    pub pinned: Vec<String>,
    /// trust servers we've never seen without asking
    pub accept_new: bool,
    pub connect_timeout: Duration,
    pub keepalive: Duration,
    pub keepalive_max: usize,
}

enum Cmd {
    Data(Vec<u8>),
    Resize(u16, u16),
}

struct Remote {
    cmds: mpsc::UnboundedSender<Cmd>,
    task: tokio::task::AbortHandle,
}

impl Control for Remote {
    fn write(&self, bytes: &[u8]) {
        let _ = self.cmds.send(Cmd::Data(bytes.to_vec()));
    }

    fn resize(&self, cols: u16, rows: u16) {
        let _ = self.cmds.send(Cmd::Resize(cols, rows));
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        // takes the connection down with it
        self.task.abort();
    }
}

pub fn connect(rt: &tokio::runtime::Handle, target: Target, wake: Waker) -> Session {
    let (tx, rx) = channel(wake);
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let task = rt.spawn(run(target, tx, cmd_rx));
    Session {
        events: rx,
        control: Box::new(Remote { cmds: cmd_tx, task: task.abort_handle() }),
    }
}

#[derive(Debug)]
enum SshError {
    Russh(russh::Error),
    /// the server key check said no, already knows what to tell the user
    HostKey(End),
}

impl From<russh::Error> for SshError {
    fn from(e: russh::Error) -> Self {
        SshError::Russh(e)
    }
}

async fn run(t: Target, tx: EventTx, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
    let end = match session(&t, &tx, &mut cmds).await {
        Ok(end) => end,
        Err(SshError::HostKey(end)) => end,
        Err(SshError::Russh(e)) => gone(&t, e.to_string()).await,
    };
    tx.send(Event::Ended(end));
}

/// the session died without saying why, find out if its us or them
async fn gone(t: &Target, error: String) -> End {
    let probe = TcpStream::connect((t.host.as_str(), t.port));
    match tokio::time::timeout(t.connect_timeout.min(Duration::from_secs(3)), probe).await {
        Ok(Ok(_)) => End::Lost,
        _ => End::Unreachable(error),
    }
}

async fn session(
    t: &Target,
    tx: &EventTx,
    cmds: &mut mpsc::UnboundedReceiver<Cmd>,
) -> Result<End, SshError> {
    let stream = match tokio::time::timeout(
        t.connect_timeout,
        TcpStream::connect((t.host.as_str(), t.port)),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Ok(End::Unreachable(e.to_string())),
        Err(_) => return Ok(End::Unreachable("timed out".into())),
    };
    let _ = stream.set_nodelay(true);

    let config = Arc::new(client::Config {
        keepalive_interval: Some(t.keepalive),
        keepalive_max: t.keepalive_max,
        nodelay: true,
        ..Default::default()
    });
    let asking = Arc::new(AtomicBool::new(false));
    let handler = Client {
        host: t.host.clone(),
        port: t.port,
        known_hosts: t.known_hosts.clone(),
        pinned: t.pinned.clone(),
        accept_new: t.accept_new,
        tx: tx.clone(),
        asking: asking.clone(),
    };

    // a server that takes the tcp connection and then says nothing would hang us
    // forever, so the handshake gets a timeout too. except while a human is
    // reading a fingerprint, they get all the time they want
    let mut handle = tokio::select! {
        h = client::connect_stream(config, stream, handler) => h?,
        () = watchdog(t.connect_timeout, asking) => return Ok(End::Unreachable("server didnt answer".into())),
    };

    if !authenticate(&mut handle, t).await? {
        return Ok(End::AuthFailed);
    }

    let channel = handle.channel_open_session().await?;
    channel
        .request_pty(false, &t.term, t.cols.into(), t.rows.into(), 0, 0, &[])
        .await?;
    match &t.command {
        Some(cmd) => channel.exec(true, cmd.as_bytes()).await?,
        None => channel.request_shell(true).await?,
    }
    tx.send(Event::Connected);

    let (mut read, write) = channel.split();
    let mut status = None;
    let mut signaled = false;
    loop {
        tokio::select! {
            msg = read.wait() => match msg {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    tx.send(Event::Output(data.to_vec()));
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => status = Some(exit_status),
                Some(ChannelMsg::ExitSignal { .. }) => signaled = true,
                Some(ChannelMsg::Failure) => {
                    return Ok(End::Failed("the server refused to start the session".into()));
                }
                Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
            cmd = cmds.recv() => match cmd {
                Some(Cmd::Data(bytes)) => write.data_bytes(bytes).await?,
                Some(Cmd::Resize(cols, rows)) => write.window_change(cols.into(), rows.into(), 0, 0).await?,
                None => {
                    let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
                    return Ok(End::Exited);
                }
            },
        }
    }

    if status == Some(0) && !signaled {
        let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
        return Ok(End::Exited);
    }
    let why = match status {
        Some(s) => format!("exited with status {s}"),
        None => "session closed".into(),
    };
    Ok(gone(t, why).await)
}

async fn watchdog(timeout: Duration, asking: Arc<AtomicBool>) {
    loop {
        tokio::time::sleep(timeout).await;
        if !asking.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// password first, and if the server only does keyboard-interactive (plenty of pam setups) answer its prompts with the same password
async fn authenticate(handle: &mut client::Handle<Client>, t: &Target) -> Result<bool, russh::Error> {
    if handle
        .authenticate_password(&t.username, t.password.as_str())
        .await?
        .success()
    {
        return Ok(true);
    }

    let mut reply = handle
        .authenticate_keyboard_interactive_start(&t.username, None::<String>)
        .await?;
    // a few rounds is plenty, a server asking for more wants something we dont have
    for _ in 0..5 {
        match reply {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts
                    .iter()
                    .map(|p| if p.echo { t.username.clone() } else { t.password.to_string() })
                    .collect();
                reply = handle.authenticate_keyboard_interactive_respond(answers).await?;
            }
        }
    }
    Ok(false)
}

struct Client {
    host: String,
    port: u16,
    known_hosts: Option<PathBuf>,
    pinned: Vec<String>,
    accept_new: bool,
    tx: EventTx,
    asking: Arc<AtomicBool>,
}

impl Client {
    async fn ask(&self, fingerprint: String) -> bool {
        let (reply, answer) = oneshot::channel();
        self.asking.store(true, Ordering::Relaxed);
        self.tx.send(Event::HostKey(HostKeyQuestion { fingerprint, reply }));
        let trust = answer.await.unwrap_or(false);
        self.asking.store(false, Ordering::Relaxed);
        trust
    }
}

impl client::Handler for Client {
    type Error = SshError;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, SshError> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Err(SshError::HostKey(End::Failed("host certificates arent supported".into())));
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();

        if !self.pinned.is_empty() {
            return if self.pinned.contains(&fingerprint) {
                Ok(true)
            } else {
                Err(SshError::HostKey(End::HostKeyMismatch { new: fingerprint }))
            };
        }

        let Some(file) = &self.known_hosts else {
            return if self.ask(fingerprint).await {
                Ok(true)
            } else {
                Err(SshError::HostKey(End::HostKeyDeclined))
            };
        };

        match keys::check_known_hosts_path(&self.host, self.port, key, file) {
            Ok(true) => Ok(true),
            Ok(false) => {
                if !self.accept_new && !self.ask(fingerprint).await {
                    return Err(SshError::HostKey(End::HostKeyDeclined));
                }
                // cant remember it? then well just ask again next time, no reason to fail
                let _ = keys::known_hosts::learn_known_hosts_path(&self.host, self.port, key, file);
                Ok(true)
            }
            Err(keys::Error::KeyChanged { line }) => {
                let old = keys::known_hosts::known_host_keys_path(&self.host, self.port, file)
                    .ok()
                    .and_then(|known| known.into_iter().find(|(l, _)| *l == line))
                    .map(|(_, k)| k.fingerprint(HashAlg::Sha256).to_string())
                    .unwrap_or_else(|| "?".into());
                Err(SshError::HostKey(End::HostKeyChanged(KeyChange {
                    file: file.clone(),
                    line,
                    old,
                    new: fingerprint,
                    new_key: key.to_openssh().unwrap_or_default(),
                })))
            }
            Err(e) => Err(SshError::HostKey(End::Failed(format!(
                "cant read {}: {e}",
                file.display()
            )))),
        }
    }
}

/// the user looked at a changed key and said yes: out with every old key of that type for the server, in with the new one
pub fn replace_host_key(host: &str, port: u16, file: &std::path::Path, new_key: &str) -> Result<(), String> {
    let key = keys::PublicKey::from_openssh(new_key).map_err(|e| e.to_string())?;
    let stale: Vec<usize> = keys::known_hosts::known_host_keys_path(host, port, file)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|(_, old)| old.algorithm() == key.algorithm())
        .map(|(line, _)| line)
        .collect();
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let kept: String = text
        .lines()
        .enumerate()
        .filter(|(i, _)| !stale.contains(&(i + 1)))
        .map(|(_, l)| format!("{l}\n"))
        .collect();
    std::fs::write(file, kept).map_err(|e| e.to_string())?;
    keys::known_hosts::learn_known_hosts_path(host, port, &key, file).map_err(|e| e.to_string())
}
