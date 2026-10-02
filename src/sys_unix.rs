//! macOS/Linux: the terminal, clock, config folder and keep-awake helper shared by both.
//! The rest is per OS: sys_linux.rs, sys_macos.rs.
use std::ffi::c_void;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::{env, ptr};

use crate::Key;
use crate::keeper::Tm;

#[cfg_attr(target_os = "macos", path = "sys_macos.rs")]
#[cfg_attr(not(target_os = "macos"), path = "sys_linux.rs")]
mod os;
pub use os::*;

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[repr(C)]
#[derive(Default)]
struct CTm {
    sec: i32,
    min: i32,
    hour: i32,
    mday: i32,
    mon: i32,
    year: i32,
    wday: i32,
    yday: i32,
    isdst: i32,
    gmtoff: i64,
    zone: usize,
}

#[cfg(target_os = "macos")]
type NFds = std::ffi::c_uint;
#[cfg(not(target_os = "macos"))]
type NFds = std::ffi::c_ulong;

unsafe extern "C" {
    fn poll(fds: *mut PollFd, n: NFds, ms: i32) -> i32;
    fn read(fd: i32, buf: *mut c_void, n: usize) -> isize;
    fn localtime_r(t: *const i64, out: *mut CTm) -> *mut CTm;
    fn kill(pid: i32, sig: i32) -> i32;
}

fn home() -> Option<PathBuf> {
    env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// A command's stdout when it succeeds.
fn output(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs a command, waits, Ok if it exits 0. No pipes: an app it starts may outlive it (xdg-open, open).
fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    if status.is_ok_and(|s| s.success()) { Ok(()) } else { Err(format!("{cmd} failed")) }
}

/// The helper process that keeps the machine awake (systemd-inhibit / caffeinate) while it runs.
static AWAKE: Mutex<Option<Child>> = Mutex::new(None);

/// Stops the current helper, then starts `cmd` (if any) in its own process group.
fn keep_awake(cmd: Option<Command>) {
    let mut slot = AWAKE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut c) = slot.take() {
        // SIGTERM to the group (the helper and its children), then SIGCONT: a stopped helper (one that
        // touched the terminal from the background) only acts on the SIGTERM once continued
        unsafe {
            kill(-(c.id() as i32), 15);
            kill(-(c.id() as i32), 18);
        }
        let _ = c.wait();
    }
    if let Some(mut cmd) = cmd {
        cmd.process_group(0).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        *slot = cmd.spawn().ok();
    }
}

pub fn config_dir() -> Option<PathBuf> {
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        return Some(xdg.into());
    }
    Some(home()?.join(if cfg!(target_os = "macos") { "Library/Application Support" } else { ".config" }))
}

pub fn reopen_in_console() -> bool {
    false
}

pub fn own_window() -> bool {
    false // always started from a terminal here: leave its window alone
}

pub fn local_time(t: i64) -> Tm {
    let mut out = CTm::default();
    unsafe { localtime_r(&t, &mut out) };
    Tm { wday: out.wday as u8, hour: out.hour as u8, min: out.min as u8 }
}

/// `stty` on the controlling terminal; its output on success.
fn stty(args: &[&str]) -> Option<String> {
    let out = Command::new("stty").args(args).stdin(Stdio::inherit()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The terminal in raw-key mode for as long as this lives.
pub struct Term {
    saved: Option<String>,
}

impl Term {
    /// Keys one at a time, no echo, Ctrl+C as a key.
    pub fn open() -> Term {
        let saved = stty(&["-g"]);
        stty(&["-icanon", "-echo", "-isig"]);
        Term { saved }
    }

    /// Waits up to `ms` for a key press. Arrows arrive as ESC [ A / ESC [ B.
    pub fn read_key(&self, ms: u32) -> Option<Key> {
        let mut p = PollFd { fd: 0, events: 1, revents: 0 }; // stdin, POLLIN
        let mut c = [0u8; 3];
        let ready = unsafe { poll(&mut p, 1, ms as i32) };
        if ready > 0 && unsafe { read(0, c.as_mut_ptr().cast(), 1) } == 1 {
            if c[0] == 27
                && unsafe { poll(&mut p, 1, 30) } > 0
                && unsafe { read(0, c[1..].as_mut_ptr().cast(), 2) } == 2
                && c[1] == b'['
            {
                return match c[2] {
                    b'A' => Some(Key::Up),
                    b'B' => Some(Key::Down),
                    _ => None,
                };
            }
            return Some(Key::Char(match c[0] {
                b'\n' => '\r',
                127 => '\x08',
                b => b as char,
            }));
        }
        if ready != 0 {
            unsafe { poll(ptr::null_mut(), 0, ms as i32) }; // error or closed stdin: wait anyway, don't spin
        }
        None
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        if let Some(s) = &self.saved {
            stty(&[s]);
        }
    }
}
