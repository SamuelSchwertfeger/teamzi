//! OS-independent logic: settings file, schedule, nudge timing, Teams log parsing. Unit tests at the bottom.
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::sys;

// ---- settings, stored as key=value lines in config.ini (no TOML/JSON library) ----

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Always,
    Hours,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Method {
    Auto,
    TeamsCli,
    Keypress,
}

pub const WORKDAYS: u8 = 0b011_1110; // bit n = weekday n, 0 = Sunday: Monday to Friday

/// Times are minutes since midnight.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Schedule {
    pub mode: Mode,
    pub start: u16,
    pub end: u16,
    pub days: u8,
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule { mode: Mode::Always, start: 9 * 60, end: 17 * 60, days: WORKDAYS }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Config {
    pub accepted: bool,
    pub autostart: bool,
    pub schedule: Schedule,
    pub method: Method,
    pub idle_after_sec: i64,
    pub interval_sec: i64,
    /// false = the screens may sleep: no key presses, only the OS's own method (Teams command / state file)
    pub screens_on: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            accepted: false,
            autostart: false,
            schedule: Schedule::default(),
            method: Method::Auto,
            idle_after_sec: 60,
            interval_sec: 120,
            screens_on: true,
        }
    }
}

pub fn validate(mut c: Config) -> Config {
    c.idle_after_sec = c.idle_after_sec.clamp(30, 86400);
    c.interval_sec = c.interval_sec.clamp(30, 86400);
    if !sys::TEAMS_OWN_WAY {
        // no Teams command here: key presses are the only method, so they can't be switched off
        c.screens_on = true;
        if c.method == Method::TeamsCli {
            c.method = Method::Auto;
        }
    }
    let s = &mut c.schedule;
    if s.mode == Mode::Hours {
        let d = Schedule::default();
        if s.days == 0 {
            s.days = d.days;
        }
        if s.start == s.end {
            (s.start, s.end) = (d.start, d.end);
        }
    }
    c
}

/// None = corrupt. Unknown keys are ignored; unknown modes and methods fall back to the defaults.
pub fn parse_config(text: &str) -> Option<Config> {
    let (mut c, d) = (Config::default(), Schedule::default());
    let flag = |v| match v {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (k, v) = line.split_once('=')?;
        let v = v.trim();
        match k.trim() {
            "accepted" => c.accepted = flag(v)?,
            "autostart" => c.autostart = flag(v)?,
            "mode" => c.schedule.mode = if v == "hours" { Mode::Hours } else { Mode::Always },
            "start" => c.schedule.start = parse_hm(v).unwrap_or(d.start),
            "end" => c.schedule.end = parse_hm(v).unwrap_or(d.end),
            "method" => {
                c.method = match v {
                    "teams-cli" => Method::TeamsCli,
                    "keypress" => Method::Keypress,
                    _ => Method::Auto,
                }
            }
            "idle_after_sec" => c.idle_after_sec = v.parse().ok()?,
            "interval_sec" => c.interval_sec = v.parse().ok()?,
            "screens" => c.screens_on = v != "off",
            "days" => {
                c.schedule.days = 0;
                for day in v.split(',').map(str::trim).filter(|d| !d.is_empty()) {
                    c.schedule.days |= 1 << day.parse::<u8>().ok().filter(|&n| n <= 6)?;
                }
            }
            _ => {}
        }
    }
    Some(validate(c))
}

pub fn serialize_config(c: &Config) -> String {
    let s = &c.schedule;
    let days: Vec<String> = (0..7).filter(|d| s.days >> d & 1 == 1).map(|d| d.to_string()).collect();
    let mode = if s.mode == Mode::Hours { "hours" } else { "always" };
    let method = match c.method {
        Method::Auto => "auto",
        Method::TeamsCli => "teams-cli",
        Method::Keypress => "keypress",
    };
    format!(
        "accepted={}\nmode={mode}\nstart={}\nend={}\ndays={}\nautostart={}\nmethod={method}\nidle_after_sec={}\ninterval_sec={}\nscreens={}\n",
        c.accepted,
        hm(s.start),
        hm(s.end),
        days.join(","),
        c.autostart,
        c.idle_after_sec,
        c.interval_sec,
        if c.screens_on { "on" } else { "off" }
    )
}

fn config_dir() -> Option<PathBuf> {
    Some(sys::config_dir()?.join("teamzi"))
}

/// Err = unreadable, left alone. Ok carries a notice when a corrupt file was set aside.
pub fn load_config() -> Result<(Config, Option<&'static str>), String> {
    let p = config_dir().ok_or("Cannot locate the config folder.")?.join("config.ini");
    if !p.exists() {
        return Ok((Config::default(), None));
    }
    let text = read_tail(&p, 1 << 20)
        .ok_or_else(|| format!("Settings file exists but can't be read; left untouched: {}", p.display()))?;
    if let Some(c) = parse_config(&text) {
        return Ok((c, None));
    }
    let bad = p.with_extension("ini.bad");
    let _ = fs::remove_file(&bad);
    let notice = match fs::rename(&p, &bad) {
        Ok(()) => "Settings file was corrupt; moved to config.ini.bad.",
        Err(_) => "Settings file was corrupt; starting fresh.",
    };
    Ok((Config::default(), Some(notice)))
}

/// Write-then-rename: a crash never leaves a half-written file.
pub fn save_config(c: &Config) -> Result<(), String> {
    let dir = config_dir().ok_or("cannot locate the config folder")?;
    let (p, tmp) = (dir.join("config.ini"), dir.join("config.ini.tmp"));
    let saved = fs::create_dir_all(&dir)
        .and_then(|()| fs::write(&tmp, serialize_config(c)))
        .and_then(|()| fs::rename(&tmp, &p));
    saved.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })
}

// ---- schedule and timing ----

/// Minutes since midnight -> "HH:MM"
pub fn hm(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Strict "H:MM" or "HH:MM" on a 24-hour clock. No sign, spaces or trailing junk.
pub fn parse_hm(s: &str) -> Option<u16> {
    let (h, m) = s.split_once(':')?;
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if !(1..=2).contains(&h.len()) || m.len() != 2 || !digits(h) || !digits(m) {
        return None;
    }
    let (h, m) = (h.parse::<u16>().ok()?, m.parse::<u16>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// The time in minutes, or why it is not usable.
pub fn check_time(s: &str, differ_from: Option<u16>) -> Result<u16, &'static str> {
    let t = parse_hm(s).ok_or("Use HH:MM on a 24-hour clock, like 09:00 or 17:30.")?;
    if Some(t) == differ_from {
        return Err("End time must differ from start time.");
    }
    Ok(t)
}

/// Local wall-clock time, as much as the schedule needs. `wday` 0 = Sunday.
#[derive(Clone, Copy, Debug)]
pub struct Tm {
    pub wday: u8,
    pub hour: u8,
    pub min: u8,
}

/// Overnight spans (start > end) count the after-midnight part as the previous day's.
pub fn allowed(s: Schedule, t: Tm) -> bool {
    if s.mode == Mode::Always {
        return true;
    }
    let workday = |wd: u8| s.days >> wd & 1 == 1;
    let m = u16::from(t.hour) * 60 + u16::from(t.min);
    if s.start < s.end {
        return m >= s.start && m < s.end && workday(t.wday);
    }
    if m >= s.start {
        return workday(t.wday);
    }
    m < s.end && workday((t.wday + 6) % 7)
}

/// Seconds until the next nudge, 0 = due now: no input for idle_after_sec and interval_sec since the last nudge.
pub fn next_nudge_in(c: &Config, idle: i64, now: i64, last_nudge: i64) -> i64 {
    let since = if last_nudge == 0 { c.interval_sec } else { (now - last_nudge).max(0) }; // clock went back: 0
    0.max(c.idle_after_sec - idle).max(c.interval_sec - since)
}

// ---- Teams' own log, which records the presence every 5 minutes ----

#[derive(Clone, PartialEq, Debug)]
pub struct Teams {
    pub status: String,
    pub unread: u32,
    pub at: Option<i64>, // time of the log line
}

/// The last line like "... { availability: Available, unread notification count: 1 }" wins.
pub fn parse_log(data: &str) -> Option<Teams> {
    const KEY: &str = "availability: ";
    const MID: &str = ", unread notification count: ";
    for (pos, _) in data.rmatch_indices(KEY) {
        let rest = &data[pos + KEY.len()..];
        let word = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
        let Some(count) = rest[word..].strip_prefix(MID) else { continue };
        let digits = count.find(|c: char| !c.is_ascii_digit()).unwrap_or(count.len());
        if word == 0 || digits == 0 {
            continue;
        }
        let line = &data[data[..pos].rfind('\n').map_or(0, |n| n + 1)..pos];
        return Some(Teams {
            status: rest[..word].to_string(),
            unread: count[..digits].parse().unwrap_or(0),
            at: line.split(' ').next().and_then(parse_time),
        });
    }
    None
}

/// Reads the last 256 KB of the newest MSTeams_*.log. Unknown for up to 5 min after Teams starts a new log.
pub fn read_teams() -> Option<Teams> {
    let is_log = |n: &str| n.starts_with("MSTeams_2") && n.ends_with(".log");
    let (_, newest) = fs::read_dir(sys::teams_log_dir()?)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(is_log))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max()?;
    parse_log(&read_tail(&newest, 256 * 1024)?)
}

/// The last `max` bytes of a file, invalid UTF-8 replaced.
fn read_tail(p: &Path, max: u64) -> Option<String> {
    let mut f = fs::File::open(p).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(max))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Fixed RFC 3339 layout as Teams writes it: 2026-10-01T16:13:01.957195-04:00 -> Unix seconds.
fn parse_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |at: usize, len: usize| {
        let d = b.get(at..at + len)?;
        d.iter().all(u8::is_ascii_digit).then(|| d.iter().fold(0, |n, c| n * 10 + i64::from(c - b'0')))
    };
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (num(0, 4)?, num(5, 2)?, num(8, 2)?, num(11, 2)?, num(14, 2)?, num(17, 2)?);
    let mut i = 19;
    if b[i] == b'.' {
        i += 1 + b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count(); // fraction
    }
    let offset = match b.get(i) {
        Some(&sign @ (b'+' | b'-')) if b.len() == i + 6 && b[i + 3] == b':' => {
            let o = num(i + 1, 2)? * 3600 + num(i + 4, 2)? * 60;
            if sign == b'-' { -o } else { o }
        }
        Some(b'Z') if b.len() == i + 1 => 0,
        _ => return None,
    };
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = match mo {
        2 => 28 + i64::from(leap),
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=12).contains(&mo) || !(1..=month_days).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec - offset)
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let (era, yoe) = (y.div_euclid(400), y.rem_euclid(400));
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

// ---- send: a Teams deep link that opens the chat with the text already typed ----

/// `msteams:` link to a chat with `email`, `text` in the compose box. Err says what is wrong.
pub fn chat_link(email: &str, text: &str) -> Result<String, &'static str> {
    let ok_char = |c: char| c.is_ascii_alphanumeric() || "._%+-'".contains(c);
    match email.split_once('@') {
        Some((user, domain))
            if !user.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && user.chars().chain(domain.chars()).all(ok_char) => {}
        _ => return Err("not an email address"),
    }
    if text.trim().is_empty() {
        return Err("empty message");
    }
    let link = format!("msteams:/l/chat/0/0?users={}&message={}", url_encode(email), url_encode(text));
    if link.len() > 2000 { Err("message too long") } else { Ok(link) } // Windows' URL limit is 2048
}

/// Percent-encodes everything but A-Z a-z 0-9 - . _ ~
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_link() {
        assert_eq!(
            chat_link("e.sam@corp.com", "running 5 late, sorry & thanks!").unwrap(),
            "msteams:/l/chat/0/0?users=e.sam%40corp.com&message=running%205%20late%2C%20sorry%20%26%20thanks%21"
        );
        assert_eq!(chat_link("a@b.co", "café").unwrap(), "msteams:/l/chat/0/0?users=a%40b.co&message=caf%C3%A9");
        for bad in
            ["", "esam", "@corp.com", "e@corp", "e@.com", "e@corp.", "e sam@corp.com", "e@corp.com&x=1", "a@b@c.d"]
        {
            assert!(chat_link(bad, "hi").is_err(), "{bad}");
        }
        assert!(chat_link("a@b.co", "  ").is_err());
        assert!(chat_link("a@b.co", &"x".repeat(2000)).is_err());
    }

    fn at(wday: u8, hour: u8, min: u8) -> Tm {
        Tm { wday, hour, min } // 1 = Monday, 6 = Saturday
    }

    #[test]
    fn schedule() {
        let day = Schedule { mode: Mode::Hours, ..Schedule::default() };
        let night = Schedule { start: 22 * 60, end: 6 * 60, ..day };
        assert!(allowed(day, at(1, 10, 0)));
        assert!(!allowed(day, at(1, 8, 59)));
        assert!(!allowed(day, at(1, 17, 0)));
        assert!(!allowed(day, at(6, 10, 0)));
        assert!(allowed(Schedule::default(), at(6, 3, 0)));
        assert!(allowed(night, at(1, 23, 0)));
        assert!(allowed(night, at(2, 5, 0)));
        assert!(!allowed(night, at(2, 12, 0)));
        assert!(allowed(night, at(6, 5, 0))); // started Friday night
        assert!(!allowed(night, at(1, 5, 0))); // started Sunday night
        assert!(!allowed(night, at(6, 23, 0)));
    }

    #[test]
    fn times() {
        assert!(check_time("09:00", Some(540)).is_err());
        assert_eq!(check_time("17:00", Some(540)), Ok(1020));
        assert!(check_time("25:00", None).is_err() && check_time("9", None).is_err());
        assert_eq!(parse_hm("09:30"), Some(570));
        assert!(parse_hm("9:00") == Some(540) && parse_hm("09:00") == Some(540));
        for bad in [" 9:00", "+9:00", "09:00junk", "9:5", "24:00", "09:+5", "123:00"] {
            assert_eq!(parse_hm(bad), None, "{bad}");
        }
        assert_eq!(hm(570), "09:30");
    }

    #[test]
    fn nudge_timing() {
        let c = Config::default(); // idle 60 s, 120 s between nudges
        assert_eq!(next_nudge_in(&c, 0, 1000, 0), 60);
        assert_eq!(next_nudge_in(&c, 60, 1000, 0), 0);
        assert_eq!(next_nudge_in(&c, 70, 1000, 950), 70);
        assert_eq!(next_nudge_in(&c, 70, 1000, 2000), 120); // clock went backwards
    }

    #[test]
    fn teams_log() {
        let line = "2026-10-01T16:13:01.957195-04:00 0x00002d90 <INFO> native_modules::UserDataCrossCloudModule: \
                    BroadcastGlobalState: New Global State Event: UserDataGlobalState total number of users: 1 { \
                    availability: Available, unread notification count: 1 }";
        let t = parse_log(&format!("junk\n{line}\ntrailing\n")).unwrap();
        assert_eq!(t, Teams { status: "Available".into(), unread: 1, at: Some(1_790_885_581) });
        assert_eq!(parse_log(line).unwrap().at, Some(1_790_885_581)); // first line of the file
        assert_eq!(parse_log("nothing here"), None);
        assert_eq!(parse_log("availability: Away, unread notification count: x"), None);
        assert_eq!(parse_time("2026-02-29T00:00:00Z"), None);
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn settings() {
        let d = parse_config("accepted=true\nmode=window\nidle_after_sec=5\ninterval_sec=7\nmethod=bogus\n").unwrap();
        assert!(d.accepted && d.schedule.mode == Mode::Always && d.method == Method::Auto);
        assert_eq!((d.idle_after_sec, d.interval_sec), (30, 30));
        assert_eq!(parse_config("mode=hours\ndays=\n").unwrap().schedule.days, WORKDAYS);
        let junk = parse_config("mode=hours\nstart=junk\nend=09:00\n").unwrap().schedule;
        assert_eq!((junk.start, junk.end), (540, 1020)); // junk start became 09:00 = end, so both reset
        let s = parse_config("mode=hours\r\nstart=22:00\r\nend=6:00\r\ndays=0, 6\r\n").unwrap().schedule;
        assert_eq!((s.start, s.end, s.days), (1320, 360, 0b100_0001));
        assert_eq!(parse_config(&serialize_config(&d)), Some(d));
        for bad in ["accepted=maybe", "garbage line", "idle_after_sec=abc", "days=1,9"] {
            assert_eq!(parse_config(bad), None, "{bad}");
        }
    }
}
