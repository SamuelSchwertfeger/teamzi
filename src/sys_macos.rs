//! macOS. Untested: written to Apple's documented APIs, no Mac was available to run it.
//! Idle time and key presses go through CoreGraphics; key presses need the Accessibility permission
//! for the terminal app that runs Teamzi. `caffeinate` (built in) keeps the Mac awake.
use std::ffi::c_void;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use std::{env, fs, process, thread};

use super::{home, keep_awake, output, run};

/// No Teams command here, and any key press wakes the screens: no screens-off mode.
pub const TEAMS_OWN_WAY: bool = false;
pub const KEYS: bool = true;
pub const SCREENS_OFF_NOTE: &str = "";
pub const TEAMS_HINT: &str = "! New Teams not found; open it";
pub const AUTOSTART_NOTE: &str = "Adds a LaunchAgent (opens me in Terminal); No removes it.";
pub const NUDGE_HOW: &str = "I tap F18, a key nothing uses, so Teams sees you as here.";

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: i32, kind: u32) -> f64;
    fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> *mut c_void;
    fn CGEventPost(tap: u32, event: *mut c_void);
    fn AXIsProcessTrusted() -> u8;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(p: *const c_void);
}

pub fn teams_log_dir() -> Option<PathBuf> {
    Some(home()?.join("Library/Group Containers/UBF8T346G9.com.microsoft.teams/Library/Application Support/Logs"))
}

/// Seconds since the last keyboard or mouse input (HID system state, any event type).
pub fn idle_seconds() -> Result<Option<i64>, String> {
    let s = unsafe { CGEventSourceSecondsSinceLastEventType(1, u32::MAX) };
    Ok(Some(if s.is_finite() && s > 0.0 { s as i64 } else { 0 }))
}

/// One key down and up. Without the Accessibility permission macOS drops it silently, so check first.
fn tap(key: u16) -> Result<(), String> {
    if unsafe { AXIsProcessTrusted() } == 0 {
        return Err("allow your terminal in Settings > Privacy & Security > Accessibility".into());
    }
    for down in [true, false] {
        let e = unsafe { CGEventCreateKeyboardEvent(std::ptr::null(), key, down) };
        if e.is_null() {
            return Err("CGEventCreateKeyboardEvent failed".into());
        }
        unsafe {
            CGEventPost(0, e); // kCGHIDEventTap
            CFRelease(e);
        }
    }
    Ok(())
}

/// F18 (kVK_F18): on no laptop keyboard and bound to nothing by default. F15 would dim the screen.
pub fn press_key() -> Result<(), String> {
    tap(0x4F)
}

pub fn run_teams_cli() -> Result<(), String> {
    Err("no Teams command on macOS; use method=auto".into())
}

/// Whether the frontmost app is Teams. `lsappinfo` needs no extra permission (System Events would).
fn teams_in_front() -> bool {
    let Some(front) = output("lsappinfo", &["front"]) else { return false };
    output("lsappinfo", &["info", "-only", "name", front.trim()]).is_some_and(|n| n.contains("Teams"))
}

/// Opens the `msteams:` chat link (message pre-typed), waits for Teams in front, presses Enter.
pub fn send_chat(link: &str) -> Result<&'static str, String> {
    run("open", &[link]).map_err(|_| "Teams link did not open; nothing sent".to_string())?;
    for _ in 0..80 {
        thread::sleep(Duration::from_millis(250));
        if teams_in_front() {
            thread::sleep(Duration::from_secs(4)); // the chat opens and the draft fills in
            if !teams_in_front() {
                return Err("something else came to the front; nothing sent (the draft is waiting in Teams)".into());
            }
            return tap(0x24).map(|()| "sent"); // Return
        }
    }
    Err("Teams did not come to the front within 20 s (Mac locked?); nothing sent".into())
}

/// A LaunchAgent that opens Teamzi in Terminal at login.
pub fn set_autostart(on: bool) -> Result<(), String> {
    let dir = home().ok_or("HOME not set")?.join("Library/LaunchAgents");
    let file = dir.join("io.github.samuelschwertfeger.teamzi.plist");
    if !on {
        return match fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.display().to_string().replace('&', "&amp;").replace('<', "&lt;");
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\"><dict>\n\
<key>Label</key><string>io.github.samuelschwertfeger.teamzi</string>\n\
<key>ProgramArguments</key><array><string>/usr/bin/open</string><string>-a</string><string>Terminal</string><string>{exe}</string></array>\n\
<key>RunAtLoad</key><true/>\n\
</dict></plist>\n"
    );
    fs::create_dir_all(&dir).and_then(|()| fs::write(&file, plist)).map_err(|e| e.to_string())
}

/// While on: `caffeinate` keeps the Mac (and, with `screens`, the display) awake; it also quits when
/// Teamzi does (-w).
pub fn set_exec_state(on: bool, screens: bool) {
    keep_awake(on.then(|| {
        let mut c = Command::new("caffeinate");
        c.args([if screens { "-di" } else { "-i" }, "-w", &process::id().to_string()]);
        c
    }));
}
