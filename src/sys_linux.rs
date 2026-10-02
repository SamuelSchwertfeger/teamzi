//! Linux. Teams here is the browser or teams-for-linux (an unofficial desktop app). teams-for-linux has
//! a supported switch for exactly this job: with `idleDetection.forceState` on, the word `active` in its
//! state file makes it report you as active. That is the whole method: no fake key presses (Wayland
//! blocks them without root setup, and browser Teams ignores keys aimed at other windows).
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Instant;
use std::{env, fs, process};

use super::{home, keep_awake, output, run};

/// The state file is a no-key method, so screens-off mode works.
pub const TEAMS_OWN_WAY: bool = true;
/// No key presses on Linux (see the top of this file).
pub const KEYS: bool = false;
pub const SCREENS_OFF_NOTE: &str = "Screens may sleep; the state file keeps you green.";
pub const TEAMS_HINT: &str = "Needs teams-for-linux with forceState on (README: Linux).";
pub const AUTOSTART_NOTE: &str = "Adds ~/.config/autostart/teamzi.desktop; No removes it.";
pub const NUDGE_HOW: &str = "I tell teams-for-linux you're active (its idle state file).";

/// teams-for-linux writes no presence log, so the Teams line on the dashboard stays empty.
pub fn teams_log_dir() -> Option<PathBuf> {
    None
}

/// Seconds since the last input, None when this desktop can't say. Asked over D-Bus with `busctl`
/// (part of systemd), at most every 5 s. GNOME answers on X11 and Wayland; other desktops only on X11.
/// KDE on Wayland returns a constant 0, so it isn't asked there.
pub fn idle_seconds() -> Result<Option<i64>, String> {
    static LAST: Mutex<Option<(Instant, Option<i64>)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, idle)) = *last
        && at.elapsed().as_secs() < 5
    {
        return Ok(idle.map(|i| i + at.elapsed().as_secs() as i64));
    }
    let ms = |out: Option<String>| out?.split_whitespace().nth(1)?.parse::<u64>().ok(); // "u 1234" / "t 1234"
    let gnome = || {
        let path = "/org/gnome/Mutter/IdleMonitor/Core";
        ms(output(
            "busctl",
            &["--user", "call", "org.gnome.Mutter.IdleMonitor", path, "org.gnome.Mutter.IdleMonitor", "GetIdletime"],
        ))
    };
    let x11 = || {
        let wayland = env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland");
        let ss = "org.freedesktop.ScreenSaver";
        if wayland {
            None
        } else {
            ms(output("busctl", &["--user", "call", ss, "/ScreenSaver", ss, "GetSessionIdleTime"]))
        }
    };
    let idle = gnome().or_else(x11).map(|ms| (ms / 1000) as i64);
    *last = Some((Instant::now(), idle));
    Ok(idle)
}

pub fn press_key() -> Result<(), String> {
    Err("no key presses on Linux".into())
}

/// teams-for-linux config files: normal install first, then Flatpak and Snap (sandboxed).
fn tfl_configs() -> Vec<PathBuf> {
    let Some(h) = home() else { return Vec::new() };
    let xdg = env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()).map_or(h.join(".config"), PathBuf::from);
    vec![
        xdg.join("teams-for-linux/config.json"),
        h.join(".var/app/com.github.IsmaelMartinez.teams_for_linux/config/teams-for-linux/config.json"),
        h.join("snap/teams-for-linux/current/.config/teams-for-linux/config.json"),
    ]
}

/// The string after `"key":` in a JSON text (no escapes; good enough for a path).
fn json_str<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let rest = text[text.find(&format!("\"{key}\""))? + key.len() + 2..].trim_start().strip_prefix(':')?;
    let rest = rest.trim_start().strip_prefix('"')?;
    rest.split_once('"').map(|(v, _)| v)
}

/// teams-for-linux's state file (its `stateFile` setting, else its default) and whether forceState is on.
fn state_file() -> Result<PathBuf, String> {
    let user = env::var("USER").unwrap_or_default();
    let found = tfl_configs().iter().enumerate().find_map(|(i, p)| Some((fs::read_to_string(p).ok()?, i > 0)));
    let Some((text, sandboxed)) = found else {
        return Err("teams-for-linux config.json not found (README: Linux)".into());
    };
    let forced = text.find("\"forceState\"").is_some_and(|i| {
        text[i + 12..].trim_start().strip_prefix(':').is_some_and(|v| v.trim_start().starts_with("true"))
    });
    if !forced {
        return Err("turn on idleDetection.forceState in teams-for-linux (README: Linux)".into());
    }
    let path = match json_str(&text, "stateFile") {
        Some(p) if p.contains('\\') => return Err("stateFile: use a path without backslashes".into()),
        Some(p) => p.replace("$USER", &user),
        // Flatpak and Snap have their own private /tmp: the default path there is out of reach
        None if sandboxed => return Err("Flatpak/Snap: set idleDetection.stateFile (README: Linux)".into()),
        None => format!("/tmp/teams-for-linux-idle-state-{user}"),
    };
    Ok(match (path.strip_prefix("~/"), home()) {
        (Some(rest), Some(h)) => h.join(rest),
        _ => PathBuf::from(path),
    })
}

/// Writes `active` to the state file: teams-for-linux then reports you as active, whatever the input.
pub fn run_teams_cli() -> Result<(), String> {
    let path = state_file()?;
    fs::write(&path, "active\n").map_err(|e| format!("{}: {e}", path.display()))
}

/// Opens the chat with the message typed in, in teams-for-linux when it handles `msteams:` links, else in
/// the browser. Enter is left to you: on Wayland nothing can check which window is in front.
pub fn send_chat(link: &str) -> Result<&'static str, String> {
    let handler = output("xdg-mime", &["query", "default", "x-scheme-handler/msteams"]).unwrap_or_default();
    let web;
    let link = if handler.trim().is_empty() {
        web = link.replacen("msteams:", "https://teams.microsoft.com", 1);
        &web
    } else {
        link
    };
    run("xdg-open", &[link]).map_err(|_| "xdg-open failed; nothing sent".to_string())?;
    Ok("draft opened (press Enter in Teams to send)")
}

/// A desktop entry in ~/.config/autostart (KDE, GNOME and most desktops read it), in a terminal window.
pub fn set_autostart(on: bool) -> Result<(), String> {
    let dir = super::config_dir().ok_or("HOME not set")?.join("autostart");
    let file = dir.join("teamzi.desktop");
    if !on {
        return match fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Teamzi\nExec=\"{}\"\nTerminal=true\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    fs::create_dir_all(&dir).and_then(|()| fs::write(&file, entry)).map_err(|e| e.to_string())
}

/// Removes the state file if it says `active` (ours), so teams-for-linux goes back to its own idle detection.
fn clear_state(path: &Path) {
    if fs::read_to_string(path).is_ok_and(|t| t.trim() == "active") {
        let _ = fs::remove_file(path);
    }
}

/// While on: a small `sh` helper runs `systemd-inhibit` to block sleep (and, with `screens`, the idle
/// screen-off) for as long as this process lives, and itself waits for this process to end, then clears
/// the state file, even if this process was killed or its terminal closed, and even if the inhibit was
/// refused (polkit can refuse it outside a local desktop session, e.g. over SSH). Off (and at startup):
/// lets go and clears the state file at once.
pub fn set_exec_state(on: bool, screens: bool) {
    let state = state_file().ok();
    if !on && let Some(p) = &state {
        clear_state(p);
    }
    keep_awake(on.then(|| {
        let what = if screens { "--what=sleep:idle" } else { "--what=sleep" };
        let pid = process::id().to_string();
        let file = state.map(|p| p.display().to_string()).unwrap_or_default();
        let helper = r#"systemd-inhibit --no-ask-password "$2" --who=teamzi --why="Keeping Teams green" tail --pid="$0" -f /dev/null &
tail --pid="$0" -f /dev/null; [ -n "$1" ] && [ "$(cat "$1")" = active ] && rm -f "$1""#;
        let mut c = Command::new("sh");
        c.args(["-c", helper, &pid, &file, what]);
        c
    }));
}
