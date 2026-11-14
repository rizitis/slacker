//! Runs flatpak on a pseudo-terminal, for the commands that ask questions.
//!
//! flatpak answers "n" by itself to every [Y/n] question when stdin or stdout
//! is not a terminal (flatpak_yes_no_prompt, app/flatpak-tty-utils.c). To show
//! its questions to the user, the commands that change something run on a
//! pty. FLATPAK_FANCY_OUTPUT=0 keeps the output plain even so.
//!
//! The child calls setsid() before exec: it gets its own session and process
//! group, and no controlling terminal. Without a controlling terminal
//! flatpak's fallback polkit agent cannot ask for a password on /dev/tty (it
//! opens ctermid()), so a password is never asked in a terminal the user
//! cannot see, and the GUI never handles one. The process group lets Stop end
//! flatpak and anything it started, and nothing else.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::runner::ENV;

pub struct PtyChild {
    pub master: OwnedFd,
    pub child: Child,
}

impl PtyChild {
    pub fn pid(&self) -> i32 {
        self.child.id() as i32
    }
}

fn check(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

fn set_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor this process owns.
    unsafe {
        let flags = check(libc::fcntl(fd, libc::F_GETFD))?;
        check(libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC))?;
    }
    Ok(())
}

pub fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor this process owns.
    unsafe {
        let flags = check(libc::fcntl(fd, libc::F_GETFL))?;
        check(libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK))?;
    }
    Ok(())
}

pub fn spawn(flatpak: &Path, argv: &[String]) -> io::Result<PtyChild> {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    // SAFETY: openpty fills two descriptors; the null pointers mean no name,
    // default terminal settings and window size.
    check(unsafe {
        libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null())
    })?;
    // SAFETY: both were just opened by openpty and are owned by nobody else.
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    set_cloexec(master.as_raw_fd())?;
    set_cloexec(slave.as_raw_fd())?;

    let mut cmd = Command::new(flatpak);
    cmd.args(argv)
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave.try_clone()?));
    for (k, v) in ENV {
        cmd.env(k, v);
    }
    // SAFETY: setsid is async-signal-safe, as required between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    drop(slave);
    set_nonblocking(master.as_raw_fd())?;
    Ok(PtyChild { master, child })
}

/// Writes all of `data` to the pty.
pub fn write_all(fd: RawFd, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        // SAFETY: writing from a valid slice to a descriptor we own.
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted || e.kind() == io::ErrorKind::WouldBlock {
                continue;
            }
            return Err(e);
        }
        data = &data[n as usize..];
    }
    Ok(())
}

/// What one non-blocking read of the pty gave.
pub enum Read {
    Data(Vec<u8>),
    /// Nothing right now.
    Again,
    /// The other side is closed: flatpak and everything it started are gone.
    End,
}

pub fn read(fd: RawFd) -> Read {
    let mut buf = [0u8; 8192];
    // SAFETY: reading into a local buffer from a descriptor we own.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if n > 0 {
        return Read::Data(buf[..n as usize].to_vec());
    }
    if n == 0 {
        return Read::End;
    }
    match io::Error::last_os_error().kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Read::Again,
        // Linux reports EIO on the master once the last slave is closed.
        _ => Read::End,
    }
}

/// Sends `sig` to flatpak's whole process group (its pid, after setsid).
pub fn signal_group(pid: i32, sig: libc::c_int) {
    if pid > 1 {
        // SAFETY: a negative pid addresses the process group flatpak leads.
        unsafe {
            libc::kill(-pid, sig);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(c: &mut Child, secs: u64) -> Option<std::process::ExitStatus> {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if let Ok(Some(s)) = c.try_wait() {
                return Some(s);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    fn drain(fd: RawFd, secs: u64) -> String {
        let mut out = Vec::new();
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            match read(fd) {
                Read::Data(d) => out.extend(d),
                Read::Again => std::thread::sleep(Duration::from_millis(20)),
                Read::End => break,
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn the_child_sees_a_terminal_but_has_no_controlling_one() {
        // Checked with programs that open no terminal themselves. A shell is
        // no good here: bash, as a session leader with a terminal on stdin,
        // takes that terminal as its controlling one (dash does not), so the
        // answer would depend on what /bin/sh is.
        let mut c = spawn(Path::new("test"), &["-t".to_string(), "0".to_string()]).expect("spawn test");
        drain(c.master.as_raw_fd(), 2);
        assert!(wait(&mut c.child, 5).expect("exit").success(), "stdin is not a terminal");

        let mut c = spawn(Path::new("cat"), &["/proc/self/stat".to_string()]).expect("spawn cat");
        let out = drain(c.master.as_raw_fd(), 5);
        wait(&mut c.child, 5).expect("exit");
        // Field 7 of /proc/PID/stat is tty_nr: 0 means no controlling terminal.
        let after_comm = out.rsplit(") ").next().unwrap_or("");
        let tty_nr = after_comm.split_whitespace().nth(4).unwrap_or("?");
        assert_eq!(tty_nr, "0", "{out}");
    }

    #[test]
    fn stop_ends_the_whole_process_group() {
        let sh = Path::new("/bin/sh");
        // A child of the child: both must go.
        let a = vec!["-c".to_string(), "sleep 30 & sleep 30; echo late".to_string()];
        let mut c = spawn(sh, &a).expect("spawn");
        std::thread::sleep(Duration::from_millis(300));
        signal_group(c.pid(), libc::SIGTERM);
        let st = wait(&mut c.child, 5).expect("exited after SIGTERM");
        assert!(!st.success());
        let out = drain(c.master.as_raw_fd(), 2);
        assert!(!out.contains("late"), "{out}");
    }

    #[test]
    fn answers_reach_the_child() {
        let sh = Path::new("/bin/sh");
        let a = vec!["-c".to_string(), "printf 'Go? [Y/n]: '; read x; echo got=$x".to_string()];
        let mut c = spawn(sh, &a).expect("spawn");
        let fd = c.master.as_raw_fd();
        let first = drain(fd, 1);
        assert!(first.ends_with("[Y/n]: "), "{first}");
        write_all(fd, b"y\n").unwrap();
        let rest = drain(fd, 3);
        wait(&mut c.child, 5).expect("exit");
        assert!(rest.contains("got=y"), "{rest}");
    }
}
