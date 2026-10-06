//! One window per home directory. The first launch locks `instance.lock` and
//! listens on a loopback port; a later launch hands its `--run` commands to
//! that window and exits. So a desktop shortcut such as
//! `--run "GP ALTE.ALTE"` opens the chart in the running terminal, and MISO is
//! never polled by two copies at once.
//!
//! The port, a random token and the owner's process id are written to
//! `instance.port`. Only a client that can read that file (the same user) can
//! send commands, and commands only open panels: an order ticket opened this
//! way sends nothing until its Confirm is clicked in the window (the test
//! `commands_from_outside_the_window_only_open_tickets` holds that).

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mt_ui::remote::Remote;

/// Longest message accepted, and most commands per message.
const MAX_MESSAGE: u64 = 64 * 1024;
const MAX_COMMANDS: usize = 32;
/// How long a second launch keeps trying to reach the first.
const FORWARD_PATIENCE: Duration = Duration::from_secs(5);

pub enum Claim {
    /// This process owns the home. Keep it alive for the process's lifetime.
    Primary(Primary),
    /// Another process owns it and has taken the commands.
    Forwarded,
}

pub struct Primary {
    _lock: File,
    listener: TcpListener,
    token: String,
}

/// Take the home, or hand `commands` to whoever has it.
pub fn claim(dir: &Path, commands: &[String]) -> std::io::Result<Claim> {
    fs::create_dir_all(dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("instance.lock"))?;
    match lock.try_lock() {
        Ok(()) => {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            let token = token();
            let info = format!(
                "{} {token} {}\n",
                listener.local_addr()?.port(),
                std::process::id()
            );
            fs::write(info_path(dir), info)?;
            Ok(Claim::Primary(Primary {
                _lock: lock,
                listener,
                token,
            }))
        }
        Err(TryLockError::WouldBlock) => {
            forward(dir, commands)?;
            Ok(Claim::Forwarded)
        }
        Err(TryLockError::Error(e)) => Err(e),
    }
}

impl Primary {
    /// Accept forwarded commands on a background thread for as long as the
    /// process runs. The returned value holds the lock; keep it alive.
    pub fn serve(self, remote: Remote) -> File {
        let (listener, token) = (self.listener, self.token);
        let spawned = std::thread::Builder::new()
            .name("mt-instance".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    match receive(stream, &token) {
                        Ok(commands) => {
                            tracing::info!("commands from another launch: {commands:?}");
                            if !remote.send(commands) {
                                break;
                            }
                        }
                        Err(e) => tracing::warn!("ignored a connection: {e}"),
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("single-instance listener did not start: {e}");
        }
        self._lock
    }
}

fn info_path(dir: &Path) -> PathBuf {
    dir.join("instance.port")
}

/// Read one message: the token, then one command per line.
fn receive(mut stream: TcpStream, token: &str) -> std::io::Result<Vec<String>> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut lines = BufReader::new((&stream).take(MAX_MESSAGE)).lines();
    let first = lines.next().transpose()?.unwrap_or_default();
    if first != token {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "wrong token",
        ));
    }
    let mut commands = Vec::new();
    for line in lines {
        let line = line?;
        let line = line.trim();
        if !line.is_empty() && commands.len() < MAX_COMMANDS {
            commands.push(line.to_owned());
        }
    }
    stream.write_all(b"ok\n")?;
    Ok(commands)
}

/// Send `commands` to the owner, retrying while it starts up.
fn forward(dir: &Path, commands: &[String]) -> std::io::Result<()> {
    let started = Instant::now();
    loop {
        match try_forward(dir, commands) {
            Ok(()) => return Ok(()),
            Err(e) if started.elapsed() > FORWARD_PATIENCE => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn try_forward(dir: &Path, commands: &[String]) -> std::io::Result<()> {
    let info = fs::read_to_string(info_path(dir))?;
    let mut parts = info.split_whitespace();
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "bad instance file");
    let port: u16 = parts.next().and_then(|p| p.parse().ok()).ok_or_else(bad)?;
    let token = parts.next().ok_or_else(bad)?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(1))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut message = format!("{token}\n");
    for c in commands {
        // One command per line: flatten any line breaks inside one.
        message.push_str(&c.replace(['\r', '\n'], " "));
        message.push('\n');
    }
    stream.write_all(message.as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    if reply.trim() == "ok" {
        Ok(())
    } else {
        Err(std::io::Error::other("the running window did not answer"))
    }
}

/// An unguessable token from the standard library's randomly keyed hasher.
fn token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut parts = [0u64; 2];
    for (i, part) in parts.iter_mut().enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.write_u32(std::process::id());
        h.write_usize(i);
        *part = h.finish();
    }
    format!("{:016x}{:016x}", parts[0], parts[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_claim_forwards_to_the_first() {
        let dir = std::env::temp_dir().join(format!("mt-instance-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let Ok(Claim::Primary(primary)) = claim(&dir, &[]) else {
            panic!("the first claim owns the home");
        };
        let (remote, inbox) = mt_ui::remote::channel_pair();
        let _lock = primary.serve(remote);

        let commands = vec!["GP MINN.HUB 7".to_owned(), "MAP\nMCC".to_owned()];
        assert!(matches!(claim(&dir, &commands), Ok(Claim::Forwarded)));
        let got = inbox.recv_timeout(Duration::from_secs(5));
        assert_eq!(got, vec!["GP MINN.HUB 7", "MAP MCC"]);

        // A client without the token is turned away.
        let info = fs::read_to_string(info_path(&dir)).unwrap();
        let port: u16 = info.split_whitespace().next().unwrap().parse().unwrap();
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        s.write_all(b"guess\nLOG\n").unwrap();
        s.shutdown(std::net::Shutdown::Write).unwrap();
        let mut reply = String::new();
        let _ = BufReader::new(&s).read_line(&mut reply);
        assert_eq!(reply, "");
        assert!(inbox.recv_timeout(Duration::from_millis(300)).is_empty());
    }

    #[test]
    fn tokens_differ() {
        assert_ne!(token(), token());
        assert_eq!(token().len(), 32);
    }
}
