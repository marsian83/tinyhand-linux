//! Control socket: `tinyhand off|clear|real …` talk to the running pointer through
//! `$XDG_RUNTIME_DIR/tinyhand-<display>.sock`. One line per connection.
//!
//! Single instance: the pointer holds an flock on `tinyhand-<display>.lock` for its whole
//! life, so two starts racing (a double-pressed toggle key) can't both win and orphan
//! a full-screen input sink. Clients only connect: if that works, a pointer is running.

use std::{
    fs::File,
    io::{ErrorKind, Read, Write},
    os::{
        fd::{AsRawFd, BorrowedFd, RawFd},
        unix::net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rustix::{
    event::{PollFd, PollFlags, Timespec},
    fs::{flock, FlockOperation},
};

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    /// Turn the pointer off (the process exits).
    Off,
    /// Clear the drawing.
    Clear,
    /// Re-read the config file (the settings window sends this after saving).
    Reload,
    /// Click-for-real mode: Some(true) on, Some(false) off, None toggles.
    Real(Option<bool>),
    /// Switch to a hand id, a PNG path, or "laser".
    Hand(String),
    /// Tap sound on/off/toggle.
    Sound(Option<bool>),
    /// Pick the tap sound (thock, click, pop, wood, boop, squeak).
    TapSound(String),
}

impl Cmd {
    pub fn parse(line: &str) -> Option<Cmd> {
        let mut it = line.split_whitespace();
        let onoff = |v: Option<&str>| match v {
            Some("on") => Some(Some(true)),
            Some("off") => Some(Some(false)),
            None | Some("toggle") => Some(None),
            _ => None,
        };
        Some(match it.next()? {
            "off" => Cmd::Off,
            "clear" => Cmd::Clear,
            "reload" => Cmd::Reload,
            "real" => Cmd::Real(onoff(it.next())?),
            // The rest of the line, so image paths may contain spaces.
            "hand" => {
                let rest = line.trim_start().strip_prefix("hand")?.trim();
                if rest.is_empty() {
                    return None;
                }
                Cmd::Hand(rest.to_string())
            }
            "sound" => {
                let v = it.next();
                match onoff(v) {
                    Some(o) => Cmd::Sound(o),
                    None => Cmd::TapSound(v?.to_string()),
                }
            }
            _ => return None,
        })
    }
}

/// `$XDG_RUNTIME_DIR`, or (in sessions without one, like `su`) a private directory in
/// /tmp named after the user, so no one else can take over the socket.
fn runtime_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let uid = rustix::process::getuid().as_raw();
    let dir = std::env::temp_dir().join(format!("tinyhand-{uid}"));
    let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() && m.uid() == uid && m.permissions().mode() & 0o077 == 0 => dir,
        // Someone else made it: don't use it (binding will fail with a clear error).
        _ => PathBuf::from("/nonexistent/tinyhand"),
    }
}

pub fn socket_path() -> PathBuf {
    let dir = runtime_dir();
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let display = var("WAYLAND_DISPLAY").or_else(|| var("DISPLAY")).unwrap_or_default();
    let tag: String = display.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    dir.join(format!("tinyhand-{tag}.sock"))
}

/// Send one command to a running pointer. Err if none is running.
pub fn send(line: &str) -> std::io::Result<()> {
    send_to(&socket_path(), line)
}

fn send_to(path: &Path, line: &str) -> std::io::Result<()> {
    let mut s = UnixStream::connect(path)?;
    s.set_write_timeout(Some(Duration::from_millis(500)))?;
    s.write_all(line.as_bytes())?;
    s.write_all(b"\n")
}

pub fn running() -> bool {
    UnixStream::connect(socket_path()).is_ok()
}

pub struct Server {
    listener: UnixListener,
    path: PathBuf,
    /// Held (flocked) until the pointer exits; the kernel drops the lock if it crashes.
    _lock: File,
}

/// How long `drain` waits for a client that connected but hasn't written yet.
const READ_BUDGET: Duration = Duration::from_millis(20);

impl Server {
    /// Take the single-instance lock and bind the socket. Fails with AddrInUse if
    /// another pointer holds the lock.
    pub fn bind() -> std::io::Result<Server> {
        Self::bind_at(socket_path())
    }

    fn bind_at(path: PathBuf) -> std::io::Result<Server> {
        let lock = File::options().create(true).truncate(false).write(true).open(path.with_extension("lock"))?;
        match flock(&lock, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(e) if e == rustix::io::Errno::WOULDBLOCK => {
                return Err(std::io::Error::new(ErrorKind::AddrInUse, "already running"));
            }
            Err(e) => return Err(e.into()),
        }
        // We hold the lock, so any socket file left here belongs to a dead pointer.
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        Ok(Server { listener, path, _lock: lock })
    }

    pub fn fd(&self) -> RawFd {
        self.listener.as_raw_fd()
    }

    /// Accept every pending connection and read its command. Runs on the render thread,
    /// so all clients together get at most READ_BUDGET: one that connects and stays
    /// silent can't freeze the pointer.
    pub fn drain(&self) -> Vec<Cmd> {
        let mut out = Vec::new();
        let deadline = Instant::now() + READ_BUDGET;
        while let Ok((s, _)) = self.listener.accept() {
            let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
            let _ = s.set_nonblocking(false);
            let _ = s.set_read_timeout(Some(left));
            let mut buf = String::new();
            let _ = s.take(256).read_to_string(&mut buf);
            for line in buf.lines() {
                match Cmd::parse(line) {
                    Some(c) => out.push(c),
                    None => eprintln!("tinyhand: unknown command {line:?}"),
                }
            }
        }
        out
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Block until one of `fds` is readable or `timeout` passes (None = forever).
/// Returns which fds are readable. EINTR counts as a timeout; any other error is
/// returned (a closed fd would otherwise wake the caller's loop forever). The timeout
/// has nanosecond precision, so a deadline 0.9 ms away sleeps instead of spinning.
pub fn wait<const N: usize>(fds: [RawFd; N], timeout: Option<Duration>) -> std::io::Result<[bool; N]> {
    // SAFETY: callers pass fds they own and keep open for the duration of the call.
    let borrowed = fds.map(|fd| unsafe { BorrowedFd::borrow_raw(fd) });
    let mut p = borrowed.each_ref().map(|fd| PollFd::new(fd, PollFlags::IN));
    let ts =
        timeout.map(|d| Timespec { tv_sec: d.as_secs().min(i64::MAX as u64) as _, tv_nsec: d.subsec_nanos() as _ });
    match rustix::event::poll(&mut p, ts.as_ref()) {
        Ok(n) if n > 0 => Ok(p.each_ref().map(|x| !x.revents().is_empty())),
        Ok(_) | Err(rustix::io::Errno::INTR) => Ok([false; N]),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_server_at_a_time() {
        let dir = std::env::temp_dir().join(format!("tinyhand-ipc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tinyhand-test.sock");
        let first = Server::bind_at(path.clone()).unwrap();
        let second = Server::bind_at(path.clone()).map(|_| ()).unwrap_err();
        assert_eq!(second.kind(), ErrorKind::AddrInUse);
        send_to(&path, "hand coral").unwrap();
        assert_eq!(first.drain(), vec![Cmd::Hand("coral".into())]);
        drop(first);
        assert!(!path.exists(), "socket removed on exit");
        drop(Server::bind_at(path).unwrap()); // the lock is free again
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn wait_sleeps_for_sub_millisecond_timeouts() {
        let (a, _b) = UnixStream::pair().unwrap();
        let t = Instant::now();
        assert_eq!(wait([a.as_raw_fd()], Some(Duration::from_micros(900))).unwrap(), [false]);
        assert!(t.elapsed() >= Duration::from_micros(800), "returned after {:?}", t.elapsed());
    }

    #[test]
    fn wait_reports_readable_fds() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let (c, _d) = UnixStream::pair().unwrap();
        b.write_all(b"x").unwrap();
        assert_eq!(wait([c.as_raw_fd(), a.as_raw_fd()], Some(Duration::from_secs(1))).unwrap(), [false, true]);
    }

    #[test]
    fn parses_commands() {
        assert_eq!(Cmd::parse("off"), Some(Cmd::Off));
        assert_eq!(Cmd::parse("real on"), Some(Cmd::Real(Some(true))));
        assert_eq!(Cmd::parse("real"), Some(Cmd::Real(None)));
        assert_eq!(Cmd::parse("sound pop"), Some(Cmd::TapSound("pop".into())));
        assert_eq!(Cmd::parse("sound off"), Some(Cmd::Sound(Some(false))));
        assert_eq!(Cmd::parse("hand coral"), Some(Cmd::Hand("coral".into())));
        assert_eq!(Cmd::parse("hand /home/me/My Hands/knit.png"), Some(Cmd::Hand("/home/me/My Hands/knit.png".into())));
        assert_eq!(Cmd::parse("hand   "), None);
        assert_eq!(Cmd::parse("hand"), None);
        assert_eq!(Cmd::parse("dance"), None);
    }
}
