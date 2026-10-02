//! Windows OS hooks: idle time, F15 key press, ms-teams.exe, chat send, autostart, stay-awake, raw console keys.
//! The few Win32 calls are declared by hand below instead of pulling in the `windows` crate.
use std::ffi::{OsStr, OsString, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::{env, io, thread, time::Duration};

use crate::Key;
use crate::keeper::Tm;

type Handle = isize;
type SystemTime = [u16; 8]; // year, month, weekday, day, hour, minute, second, ms

#[repr(C)]
struct KeybdInput {
    vk: u16,
    scan: u16,
    flags: u32,
    time: u32,
    extra: usize,
}

#[repr(C)]
struct Input {
    kind: u32,
    ki: KeybdInput,
    union_tail: [u32; 2], // the union is as big as MOUSEINPUT
}
const _: () = assert!(size_of::<Input>() == 40, "INPUT is laid out for 64-bit Windows");

#[repr(C)]
#[derive(Default)]
struct KeyRecord {
    kind: u16, // INPUT_RECORD.EventType; the rest is its KEY_EVENT_RECORD
    key_down: i32,
    repeat: u16,
    vk: u16,
    scan: u16,
    ch: u16,
    ctrl: u32,
}

#[repr(C)]
#[derive(Default)]
struct StartupInfo {
    cb: u32,
    reserved: [usize; 3],
    geometry: [u32; 7],
    flags: u32,
    show: u16,
    reserved2: u16,
    tail: [usize; 4],
}

#[repr(C)]
#[derive(Default)]
struct ProcessInfo {
    process: Handle,
    thread: Handle,
    ids: [u32; 2],
}

const _: () =
    assert!(size_of::<KeyRecord>() == 20 && size_of::<StartupInfo>() == 104 && size_of::<ProcessInfo>() == 24);

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetTickCount() -> u32;
    fn GetTickCount64() -> u64;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn CreateProcessW(
        app: *const u16,
        cmd: *mut u16,
        pa: *const u8,
        ta: *const u8,
        inherit: i32,
        flags: u32,
        env: *const u8,
        dir: *const u16,
        si: *const StartupInfo,
        pi: *mut ProcessInfo,
    ) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn SetThreadExecutionState(flags: u32) -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn GetConsoleMode(h: Handle, mode: *mut u32) -> i32;
    fn GetConsoleProcessList(ids: *mut u32, len: u32) -> u32;
    fn SetConsoleMode(h: Handle, mode: u32) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn ReadConsoleInputW(h: Handle, rec: *mut KeyRecord, len: u32, read: *mut u32) -> i32;
    fn FileTimeToSystemTime(ft: *const u64, st: *mut SystemTime) -> i32;
    fn SystemTimeToTzSpecificLocalTime(tz: *const u8, utc: *const SystemTime, local: *mut SystemTime) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn QueryFullProcessImageNameW(h: Handle, flags: u32, name: *mut u16, len: *mut u32) -> i32;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetLastInputInfo(info: *mut [u32; 2]) -> i32;
    fn SendInput(n: u32, inputs: *const Input, size: i32) -> u32;
    fn GetForegroundWindow() -> Handle;
    fn GetWindowThreadProcessId(hwnd: Handle, pid: *mut u32) -> u32;
}

type ShellExecuteW = unsafe extern "system" fn(Handle, *const u16, *const u16, *const u16, *const u16, i32) -> isize;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegCreateKeyExW(
        key: Handle,
        sub: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        sam: u32,
        sa: *const u8,
        out: *mut Handle,
        disposition: *mut u32,
    ) -> i32;
    fn RegSetValueExW(key: Handle, name: *const u16, reserved: u32, kind: u32, data: *const u8, len: u32) -> i32;
    fn RegDeleteValueW(key: Handle, name: *const u16) -> i32;
    fn RegCloseKey(key: Handle) -> i32;
}

/// NUL-terminated UTF-16 for the W functions.
fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain([0]).collect()
}

fn failed(what: &str) -> String {
    format!("{what} failed (code {})", io::Error::last_os_error().raw_os_error().unwrap_or(0))
}

fn quoted(p: impl AsRef<OsStr>) -> OsString {
    let mut s = OsString::from("\"");
    s.push(p);
    s.push("\"");
    s
}

/// Teams has its own "be available" command here (ms-teams.exe), so screens-off mode works.
pub const TEAMS_OWN_WAY: bool = true;
/// Key presses work here (SendInput).
pub const KEYS: bool = true;
pub const SCREENS_OFF_NOTE: &str = "Screens may sleep; only Teams' own command keeps you green.";
pub const TEAMS_HINT: &str = "! New Teams not found; open it";
pub const AUTOSTART_NOTE: &str = "Adds one registry value; choosing No removes it.";
pub const NUDGE_HOW: &str = "I tap F15, a key no keyboard has, so Teams sees you as here.";

/// %APPDATA% / %LOCALAPPDATA%: the env vars, not SHGetKnownFolderPath, which loads shell32 (+10 MB RAM).
fn env_dir(name: &str) -> Option<PathBuf> {
    env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn config_dir() -> Option<PathBuf> {
    env_dir("APPDATA")
}

pub fn teams_log_dir() -> Option<PathBuf> {
    Some(env_dir("LOCALAPPDATA")?.join(r"Packages\MSTeams_8wekyb3d8bbwe\LocalCache\Microsoft\MSTeams\Logs"))
}

/// Seconds since the last keyboard or mouse input.
pub fn idle_seconds() -> Result<Option<i64>, String> {
    let mut info = [8, 0]; // LASTINPUTINFO { cbSize, dwTime }
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return Err(failed("GetLastInputInfo"));
    }
    // wrapping: survives the 49-day rollover; a "future" input time (injected by some app) counts as now
    let ms = unsafe { GetTickCount() }.wrapping_sub(info[1]);
    Ok(Some(if ms > 0x8000_0000 { 0 } else { i64::from(ms / 1000) }))
}

/// One key down and up.
fn tap(vk: u16) -> Result<(), String> {
    let key = |flags| Input { kind: 1, ki: KeybdInput { vk, scan: 0, flags, time: 0, extra: 0 }, union_tail: [0; 2] };
    let keys = [key(0), key(2)]; // down, up
    match unsafe { SendInput(2, keys.as_ptr(), size_of::<Input>() as i32) } {
        2 => Ok(()),
        _ => Err(failed("SendInput")),
    }
}

/// F15: a key no keyboard has.
pub fn press_key() -> Result<(), String> {
    tap(0x7E)
}

/// Whether the window in front belongs to ms-teams.exe.
fn teams_in_front() -> bool {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut pid) };
    let h = if pid == 0 { 0 } else { unsafe { OpenProcess(0x1000, 0, pid) } }; // PROCESS_QUERY_LIMITED_INFORMATION
    if h == 0 {
        return false;
    }
    let (mut buf, mut len) = ([0u16; 512], 512);
    let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) } != 0;
    unsafe { CloseHandle(h) };
    ok && String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase().ends_with(r"\ms-teams.exe")
}

/// Opens an `msteams:` chat link (the message pre-typed), waits for Teams to be in front, presses Enter.
/// Never presses Enter anywhere else. shell32 is loaded only here, so the keeper itself stays small.
pub fn send_chat(link: &str) -> Result<&'static str, String> {
    let shell = unsafe { LoadLibraryW(wide("shell32.dll").as_ptr()) };
    let f = if shell == 0 { null() } else { unsafe { GetProcAddress(shell, c"ShellExecuteW".as_ptr().cast()) } };
    if f.is_null() {
        return Err(failed("loading shell32"));
    }
    let shell_execute: ShellExecuteW = unsafe { std::mem::transmute(f) };
    let r = unsafe { shell_execute(0, wide("open").as_ptr(), wide(link).as_ptr(), null(), null(), 1) };
    if r <= 32 {
        return Err(format!("Teams link did not open (code {r}); nothing sent"));
    }
    for _ in 0..80 {
        thread::sleep(Duration::from_millis(250));
        if teams_in_front() {
            thread::sleep(Duration::from_secs(4)); // the chat opens and the draft fills in
            if !teams_in_front() {
                return Err("something else came to the front; nothing sent (the draft is waiting in Teams)".into());
            }
            return tap(0x0D).map(|()| "sent"); // Enter
        }
    }
    Err("Teams did not come to the front within 20 s (PC locked?); nothing sent".into())
}

/// ms-teams.exe --set-presence-to-available, hidden and not waited for.
pub fn run_teams_cli() -> Result<(), String> {
    let exe = env_dir("LOCALAPPDATA").ok_or("LOCALAPPDATA not set")?.join(r"Microsoft\WindowsApps\ms-teams.exe");
    let path = wide(&exe);
    // GetFileAttributesW, not Path::is_file: the WindowsApps alias is a reparse point
    let attr = unsafe { GetFileAttributesW(path.as_ptr()) };
    if attr == u32::MAX || attr & 0x10 != 0 {
        return Err("ms-teams.exe not found".into());
    }
    let mut cmd = quoted(&exe);
    cmd.push(" --set-presence-to-available");
    let mut cmd = wide(cmd);
    let si = StartupInfo { cb: size_of::<StartupInfo>() as u32, flags: 1, show: 0, ..Default::default() }; // SW_HIDE
    let mut pi = ProcessInfo::default();
    let detached = 0x8 | 0x200; // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    if unsafe {
        CreateProcessW(path.as_ptr(), cmd.as_mut_ptr(), null(), null(), 0, detached, null(), null(), &si, &mut pi)
    } == 0
    {
        return Err(failed("CreateProcess"));
    }
    unsafe {
        CloseHandle(pi.thread);
        CloseHandle(pi.process);
    }
    Ok(())
}

/// The HKCU Run value that opens the app at sign-in; off removes it.
pub fn set_autostart(on: bool) -> Result<(), String> {
    const HKEY_CURRENT_USER: Handle = 0x8000_0001_u32 as i32 as Handle; // sign-extended, as in the SDK
    let run = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let name = wide("teamzi");
    let mut key = 0;
    let mut r =
        unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, run.as_ptr(), 0, null(), 0, 2, null(), &mut key, null_mut()) };
    if r == 0 {
        r = match (on, env::current_exe()) {
            (true, Ok(exe)) => {
                let v = wide(quoted(exe));
                unsafe { RegSetValueExW(key, name.as_ptr(), 0, 1, v.as_ptr().cast(), (v.len() * 2) as u32) } // REG_SZ
            }
            (true, Err(_)) => 161, // ERROR_BAD_PATHNAME
            (false, _) => match unsafe { RegDeleteValueW(key, name.as_ptr()) } {
                2 => 0, // already gone
                r => r,
            },
        };
        unsafe { RegCloseKey(key) };
    }
    if r == 0 { Ok(()) } else { Err(format!("registry error {r}")) }
}

/// Keeps the PC (and, with `screens`, the screens) awake. Per-thread state: main loop's thread only.
pub fn set_exec_state(on: bool, screens: bool) {
    let display = if screens { 0x2 } else { 0 };
    let flags = 0x8000_0000 | if on { 0x1 | display } else { 0 }; // ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
    unsafe { SetThreadExecutionState(flags) };
}

pub fn local_time(t: i64) -> Tm {
    let ft = ((t + 11_644_473_600) * 10_000_000) as u64; // FILETIME: 100 ns ticks since 1601
    let (mut utc, mut local) = ([0; 8], [0; 8]);
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc);
        if SystemTimeToTzSpecificLocalTime(null(), &utc, &mut local) == 0 {
            local = utc; // no time zone info: UTC beats a made-up Sunday midnight
        }
    }
    Tm { wday: local[2] as u8, hour: local[4] as u8, min: local[5] as u8 }
}

/// True when the console window is this app's alone (opened by double-click or at sign-in, not from a shell).
pub fn own_window() -> bool {
    let mut ids = [0u32; 2];
    unsafe { GetConsoleProcessList(ids.as_mut_ptr(), 2) == 1 }
}

/// Opened on its own inside Windows Terminal, which can't be resized from inside: reopen in a classic
/// console window, which can. True: reopened, so exit.
pub fn reopen_in_console() -> bool {
    if env::var_os("WT_SESSION").is_none() || !own_window() {
        return false;
    }
    let (Some(root), Ok(exe)) = (env_dir("SystemRoot"), env::current_exe()) else { return false };
    let conhost = wide(root.join(r"System32\conhost.exe"));
    let mut cmd = quoted(root.join(r"System32\conhost.exe"));
    cmd.push(" ");
    cmd.push(quoted(exe));
    let mut cmd = wide(cmd);
    unsafe { env::remove_var("WT_SESSION") }; // single-threaded here; the copy must not reopen again
    let si = StartupInfo { cb: size_of::<StartupInfo>() as u32, ..Default::default() };
    let mut pi = ProcessInfo::default();
    // DETACHED_PROCESS: conhost opens its own window
    if unsafe {
        CreateProcessW(conhost.as_ptr(), cmd.as_mut_ptr(), null(), null(), 0, 0x8, null(), null(), &si, &mut pi)
    } == 0
    {
        return false;
    }
    unsafe {
        CloseHandle(pi.thread);
        CloseHandle(pi.process);
    }
    true
}

/// The console in raw-key mode for as long as this lives.
pub struct Term {
    input: Handle,
    output: Handle,
    saved: [u32; 2],
}

impl Term {
    /// No line editing or echo, Ctrl+C arrives as a key, and QuickEdit off: a stray click would freeze output.
    pub fn open() -> Term {
        let (input, output) = unsafe { (GetStdHandle(-10i32 as u32), GetStdHandle(-11i32 as u32)) };
        let mut saved = [0; 2];
        unsafe {
            GetConsoleMode(input, &mut saved[0]);
            GetConsoleMode(output, &mut saved[1]);
            SetConsoleMode(input, 0x80); // ENABLE_EXTENDED_FLAGS alone
            SetConsoleMode(output, saved[1] | 0x1 | 0x4); // processed output, VT escape codes
        }
        Term { input, output, saved }
    }

    /// Waits up to `ms` for a key press.
    pub fn read_key(&self, ms: u32) -> Option<Key> {
        let end = unsafe { GetTickCount64() } + u64::from(ms);
        loop {
            let left = end.saturating_sub(unsafe { GetTickCount64() }) as u32;
            if left == 0 {
                return None;
            }
            let (mut rec, mut n) = (KeyRecord::default(), 0);
            match unsafe { WaitForSingleObject(self.input, left) } {
                0x102 => return None, // WAIT_TIMEOUT
                0 if unsafe { ReadConsoleInputW(self.input, &mut rec, 1, &mut n) } != 0 => {}
                _ => {
                    thread::sleep(Duration::from_millis(left.into())); // no console: wait anyway, don't spin
                    return None;
                }
            }
            if n != 1 || rec.kind != 1 || rec.key_down == 0 {
                continue; // not a key press
            }
            match (rec.ch, rec.vk) {
                (0, 0x26) => return Some(Key::Up),
                (0, 0x28) => return Some(Key::Down),
                (0, _) => {}
                (c, _) => {
                    if let Some(c) = char::from_u32(c.into()) {
                        return Some(Key::Char(c));
                    }
                }
            }
        }
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        unsafe {
            SetConsoleMode(self.input, self.saved[0]);
            SetConsoleMode(self.output, self.saved[1]);
        }
    }
}
