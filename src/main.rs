//! Teamzi: setup wizard and live dashboard drawn with plain ANSI codes, plus the main loop.
//! Single thread: wait up to 1 s for a key, check the PC's idle time, nudge Teams if due, redraw.
mod keeper;
#[cfg_attr(windows, path = "sys_windows.rs")]
#[cfg_attr(not(windows), path = "sys_unix.rs")]
mod sys;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::time::{SystemTime, UNIX_EPOCH};

use keeper::{Config, Method, Mode, Schedule, Teams, WORKDAYS, check_time, hm, parse_hm};

/// A key press: a character (Enter '\r', Backspace '\x08', Esc '\x1b', Ctrl+C '\x03') or an arrow.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Key {
    Char(char),
    Up,
    Down,
}

// ---- drawing: each frame is one rounded box, built in memory and written at once (no flicker) ----

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const KEYCAP: &str = "\x1b[7m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const ON_GREEN: &str = "\x1b[42;30m";
const ON_YELLOW: &str = "\x1b[43;30m";
const ON_RED: &str = "\x1b[41;97m";
const ON_GREY: &str = "\x1b[100;97m";
const WIDTH: usize = 62; // inside the box; the whole frame is 68 columns

// The name in the Calvin S figlet font. The only place it is drawn, so renaming is one edit.
const LOGO: [&str; 3] = ["┌┬┐┌─┐┌─┐┌┬┐┌─┐┬", " │ ├┤ ├─┤│││┌─┘│", " ┴ └─┘┴ ┴┴ ┴└─┘┴"];

// Big digits for the time picker, same font. 10 is ':', 11 an empty slot.
const DIGITS: [[&str; 3]; 12] = [
    ["┌─┐", "│ │", "└─┘"],
    [" ┐ ", " │ ", " ┴ "],
    ["┌─┐", "┌─┘", "└─┘"],
    ["┌─┐", " ─┤", "└─┘"],
    ["┬ ┬", "└─┤", "  ┴"],
    ["┌─┐", "└─┐", "└─┘"],
    ["┌─┐", "├─┐", "└─┘"],
    ["┌─┐", "  │", "  ┴"],
    ["┌─┐", "├─┤", "└─┘"],
    ["┌─┐", "└─┤", "└─┘"],
    ["   ", " : ", "   "],
    ["   ", "   ", "───"],
];

fn paint(style: &str, s: &str) -> String {
    format!("{style}{s}{RESET}")
}

// The frame and logo shade from dark blue (left) to dark purple (right).
const FROM: (u8, u8, u8) = (30, 58, 160);
const TO: (u8, u8, u8) = (100, 30, 160);

/// 24-bit colour: Windows 10+ consoles have it; elsewhere the terminal says so in COLORTERM
/// (macOS Terminal.app before 26 doesn't, and gets the nearest of the 256 standard colours).
fn truecolor() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| cfg!(windows) || std::env::var("COLORTERM").is_ok_and(|v| v == "truecolor" || v == "24bit"))
}

/// Foreground colour for column i of n along the gradient.
fn hue(i: usize, n: usize) -> String {
    let d = n.saturating_sub(1).max(1);
    let i = i.min(d);
    let mix = |a: u8, b: u8| ((usize::from(a) * (d - i) + usize::from(b) * i) / d) as u8;
    let (r, g, b) = (mix(FROM.0, TO.0), mix(FROM.1, TO.1), mix(FROM.2, TO.2));
    if truecolor() {
        format!("\x1b[38;2;{r};{g};{b}m")
    } else {
        // nearest level of the xterm colour cube (0, 95, 135, 175, 215, 255)
        let q = |v: u8| [48, 115, 155, 195, 235].iter().filter(|&&c| v >= c).count();
        format!("\x1b[38;5;{}m", 16 + 36 * q(r) + 6 * q(g) + q(b))
    }
}

/// Paints s along the gradient: its first column is column `start` of n.
fn gradient(s: &str, start: usize, n: usize) -> String {
    let mut o = String::new();
    for (k, c) in s.chars().enumerate() {
        let _ = write!(o, "{}{c}", hue(start + k, n));
    }
    o + RESET
}

/// One line of the logo, bold, shaded across its own width.
fn logo(line: &str) -> String {
    format!("{BOLD}{}", gradient(line, 0, width(LOGO[0])))
}

/// A key cap, like  P
fn keycap(k: char) -> String {
    paint(KEYCAP, &format!(" {k} "))
}

/// Screen columns: skips colour codes (every glyph used here is one column wide).
fn width(s: &str) -> usize {
    let (mut n, mut code) = (0, false);
    for c in s.chars() {
        match c {
            '\x1b' => code = true,
            'm' if code => code = false,
            _ if !code => n += 1,
            _ => {}
        }
    }
    n
}

#[derive(Default)]
struct Frame(String);

impl Frame {
    fn rule(&mut self, left: &str, right: &str) {
        let bar = format!("{left}{}{right}", "─".repeat(WIDTH + 2));
        let _ = writeln!(self.0, "  {}\x1b[K", gradient(&bar, 0, WIDTH + 4)); // K: clear the rest of the line
    }

    fn row(&mut self, s: &str) {
        let (left, right) = (gradient("│", 0, WIDTH + 4), gradient("│", WIDTH + 3, WIDTH + 4));
        let pad = WIDTH.saturating_sub(width(s));
        let _ = writeln!(self.0, "  {left} {s}{RESET}{:pad$} {right}\x1b[K", "");
    }

    fn blank(&mut self) {
        self.row("");
    }

    /// Closes the box and draws it from the top-left, clearing whatever was below.
    fn show(&mut self) {
        self.rule("╰", "╯");
        out(&format!("\x1b[H{}\x1b[J", self.0));
        self.0.clear();
    }
}

fn out(s: &str) {
    let mut o = io::stdout().lock();
    let _ = o.write_all(s.as_bytes()).and_then(|()| o.flush());
}

/// 45 -> "45s", 420 -> "7m", 432 -> "7m 12s", 7380 -> "2h 3m"
fn span(s: i64) -> String {
    match (s / 3600, s / 60 % 60, s % 60) {
        (0, 0, sec) => format!("{sec}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, sec) => format!("{m}m {sec}s"),
        (h, m, _) => format!("{h}h {m}m"),
    }
}

fn clock(t: i64) -> String {
    let tm = sys::local_time(t);
    hm(u16::from(tm.hour) * 60 + u16::from(tm.min))
}

/// The mascot closes its eyes for one second in six.
fn blink(face: &str, now: i64) -> String {
    if now % 6 == 0 { face.replace(['•', '^'], "-") } else { face.to_string() }
}

fn due(now: i64, last: i64, sec: i64) -> bool {
    now < last || now - last >= sec // clock went back: due
}

// ---- app state ----

#[derive(Clone, Copy, PartialEq, Default, Debug)]
enum Screen {
    Disclaimer,
    Schedule,
    StartTime,
    EndTime,
    Autostart,
    #[default]
    Dashboard,
}

/// One graph cell; a later variant wins within the minute.
#[derive(Clone, Copy, PartialEq, PartialOrd, Default)]
enum Minute {
    #[default]
    Empty,
    Off,
    Kept,
    You,
}

/// The last hour, one cell per minute, indexed by minute % 60.
struct Graph([(i64, Minute); 60]);

impl Default for Graph {
    fn default() -> Self {
        Graph([(0, Minute::Empty); 60])
    }
}

impl Graph {
    fn mark(&mut self, minute: i64, m: Minute) {
        let cell = &mut self.0[minute.rem_euclid(60) as usize];
        if cell.0 != minute {
            *cell = (minute, Minute::Empty);
        }
        if m > cell.1 {
            cell.1 = m;
        }
    }

    fn get(&self, minute: i64) -> Minute {
        let (at, m) = self.0[minute.rem_euclid(60) as usize];
        if at == minute { m } else { Minute::Empty }
    }
}

#[derive(Default)]
struct App {
    cfg: Config,
    draft: Config, // the wizard edits this; it becomes `cfg` when finished
    screen: Screen,
    sel: usize, // highlighted menu choice
    first_run: bool,
    paused: bool,
    keeping: bool,
    teams_found: bool,
    input: String,  // digits of the time being typed
    notice: String, // message for the user
    idle_err: Option<String>,
    idle_known: bool, // false: this desktop can't report idle time (KDE on Wayland), so assume away
    away_since: i64,
    last_nudge: i64,
    last_cli: i64,
    last_tick: i64,
    teams_read: i64,
    nudges: u32,
    nudge_result: String,
    teams: Option<Teams>,
    graph: Graph,
}

// ---- behaviour ----

impl App {
    fn away_for(&self, now: i64) -> i64 {
        now - self.away_since // real input only
    }

    fn user_here(&self, now: i64) -> bool {
        self.away_for(now) < self.cfg.idle_after_sec
    }

    /// Every second. Our own F15 also resets the OS idle clock, so only input after a nudge counts as you.
    fn sample(&mut self, now: i64) {
        // clock stepped back: a "future" nudge would freeze away tracking and delay the next nudge
        self.last_nudge = self.last_nudge.min(now);
        self.last_cli = self.last_cli.min(now);
        match sys::idle_seconds() {
            Ok(None) => {
                (self.idle_err, self.idle_known) = (None, false);
                self.away_since = 0;
            }
            Ok(Some(idle)) => {
                (self.idle_err, self.idle_known) = (None, true);
                let last_input_was_nudge = self.last_nudge != 0 && idle >= now - self.last_nudge - 2;
                if !last_input_was_nudge {
                    self.away_since = now - idle;
                }
            }
            Err(e) => self.idle_err = Some(e),
        }
    }

    fn nudge(&mut self, now: i64) {
        let result = |r: Result<(), String>| r.err().unwrap_or_else(|| "ok".into());
        let mut done = Vec::new();
        // a key press wakes the screens, so with screens off only Teams' own way is used
        let key = sys::KEYS && self.cfg.screens_on && self.cfg.method != Method::TeamsCli;
        if key {
            done.push(format!("key {}", result(sys::press_key())));
        }
        let own_way = match self.cfg.method {
            _ if !key => true,
            Method::Auto => sys::TEAMS_OWN_WAY && due(now, self.last_cli, 240),
            Method::TeamsCli => true,
            Method::Keypress => false,
        };
        if own_way {
            done.push(format!("Teams {}", result(sys::run_teams_cli())));
            self.last_cli = now;
        }
        self.nudge_result = done.join(", ");
        if let Some(e) = done.iter().find(|d| !d.ends_with(" ok")) {
            self.notice = format!("Nudge: {e}"); // the dashboard row only has room for the start
        }
        self.last_nudge = now;
        self.nudges += 1;
    }

    /// Every 5 s, or right after a setting changes.
    fn tick(&mut self, now: i64) {
        let on = self.cfg.accepted && !self.paused && keeper::allowed(self.cfg.schedule, sys::local_time(now));
        if on != self.keeping {
            self.keeping = on;
            sys::set_exec_state(on, self.cfg.screens_on);
        }
        if due(now, self.teams_read, 30) {
            self.teams = keeper::read_teams(); // Teams writes it every 5 min anyway
            self.teams_read = now;
        }
        let nudge_due = keeper::next_nudge_in(&self.cfg, self.away_for(now), now, self.last_nudge) == 0;
        if on && self.idle_err.is_none() && nudge_due {
            self.nudge(now);
        }
        let m = match (on, self.user_here(now)) {
            (false, _) => Minute::Off,
            (true, true) => Minute::You,
            (true, false) => Minute::Kept,
        };
        self.graph.mark(now / 60, m);
        self.last_tick = now;
    }
}

// ---- pieces shared by the screens ----

fn header(f: &mut Frame, face: &str) {
    f.rule("╭", "╮");
    f.row(&logo(LOGO[0]));
    let pad = WIDTH - width(LOGO[1]) - width(face);
    f.row(&format!("{}{:pad$}{}", logo(LOGO[1]), "", paint(BOLD, face)));
    f.row(&logo(LOGO[2]));
    f.rule("├", "┤");
}

fn footer(f: &mut Frame, keys: &str) {
    f.rule("├", "┤");
    f.row(keys);
}

/// "▰▰▱  STEP 2 OF 3 · title"
fn steps(f: &mut Frame, n: usize, title: &str) {
    let bar: String = (1..=3).map(|i| if i <= n { paint(GREEN, "▰") } else { paint(DIM, "▱") }).collect();
    f.row(&format!("{bar}{}{}", paint(DIM, &format!("  STEP {n} OF 3 · ")), paint(BOLD, title)));
    f.blank();
}

/// The two choices on a menu screen: (key, label).
fn options(a: &App) -> [(char, &'static str); 2] {
    match a.screen {
        Screen::Disclaimer => {
            [('y', "I accept, let's go"), ('n', if a.first_run { "No thanks, exit" } else { "Go back" })]
        }
        Screen::Schedule => [('1', "Always, while I'm open"), ('2', "Work hours only, Monday to Friday")],
        _ => [('y', "Yes, open me when I sign in"), ('n', "No, I'll open it myself")],
    }
}

fn menu(f: &mut Frame, a: &App) {
    for (i, (k, label)) in options(a).into_iter().enumerate() {
        let on = i == a.sel;
        let arrow = if on { paint(GREEN, "▸ ") } else { "  ".into() };
        f.row(&format!("{arrow}{} {}", keycap(k.to_ascii_uppercase()), paint(if on { BOLD } else { DIM }, label)));
    }
}

/// Typed digits "0930" -> "09:30" (call with four digits).
fn as_time(d: &str) -> String {
    format!("{}:{}", &d[..2], &d[2..])
}

/// HH:MM in big digits: the digits typed so far, or the current time dimmed until typing starts.
fn big_time(f: &mut Frame, typed: &str, current: u16) {
    let shown = if typed.is_empty() { hm(current).replace(':', "") } else { typed.to_string() };
    for (r, colon) in DIGITS[10].iter().enumerate() {
        let mut s = " ".repeat(5);
        for i in 0..4 {
            let g = shown.as_bytes().get(i).map_or(11, |d| usize::from(d - b'0'));
            s += &paint(if typed.is_empty() || g == 11 { DIM } else { BOLD }, DIGITS[g][r]);
            s += if i == 1 { colon } else { " " };
        }
        f.row(&s);
    }
}

/// The day as 48 half-hour cells with the work hours lit.
fn day_bar(f: &mut Frame, from: Option<u16>, to: Option<u16>) {
    let span_of = from.zip(to);
    let lit = |m| span_of.is_some_and(|(st, en)| if st < en { m >= st && m < en } else { m >= st || m < en });
    let bar: String = (0..1440).step_by(30).map(|m| if lit(m) { paint(GREEN, "█") } else { paint(DIM, "·") }).collect();
    f.row(&bar);
    f.row(&paint(DIM, "00          06          12          18        24"));
    if let Some((st, en)) = span_of.filter(|(st, en)| st != en) {
        let day = span(i64::from((en + 1440 - st) % 1440) * 60);
        f.row(&paint(DIM, &format!("{day} a day, Monday to Friday")));
    }
}

/// Plain text cut to `n` columns, with … when cut.
fn fit(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.into() } else { s.chars().take(n - 1).chain(['…']).collect() }
}

/// Up to two rows, split at a space.
fn notice(f: &mut Frame, a: &App) {
    if a.notice.is_empty() {
        return;
    }
    let text = format!("! {}", a.notice);
    let cut = text.char_indices().nth(WIDTH).map_or(text.len(), |(i, _)| text[..i].rfind(' ').unwrap_or(i));
    f.row(&paint(YELLOW, &text[..cut]));
    if cut < text.len() {
        f.row(&paint(YELLOW, &fit(&format!("  {}", text[cut..].trim_start()), WIDTH)));
    }
}

// ---- screens ----

fn draw_wizard(f: &mut Frame, a: &App, now: i64) {
    let s = a.draft.schedule;
    let start = a.screen == Screen::StartTime;
    let face = match a.screen {
        Screen::Disclaimer => "(•_•)",
        Screen::Autostart => "(^‿^)",
        _ => "(•‿•)",
    };
    header(f, &blink(face, now));
    match a.screen {
        Screen::Disclaimer => {
            steps(f, 1, "Ground rules");
            f.row(&paint(YELLOW, "▲ Use at your own risk."));
            f.row(sys::NUDGE_HOW);
            f.row("Employers can detect this.");
            f.row("While I'm on, your screen won't lock by itself (unless S).");
        }
        Screen::Schedule => {
            steps(f, 2, "When should I keep you green?");
            f.row(&if a.teams_found {
                paint(GREEN, "✓ New Teams found on this PC")
            } else if sys::teams_log_dir().is_none() {
                paint(DIM, sys::TEAMS_HINT)
            } else {
                paint(YELLOW, "! New Teams not found; open it")
            });
        }
        Screen::StartTime | Screen::EndTime => {
            steps(f, 2, if start { "Your work day starts at" } else { "and ends at" });
            big_time(f, &a.input, if start { s.start } else { s.end });
            f.blank();
            let typed = (a.input.len() == 4).then(|| parse_hm(&as_time(&a.input)));
            let (from, to) = if start {
                (typed.unwrap_or(Some(s.start)), Some(s.end))
            } else {
                (Some(s.start), typed.unwrap_or(Some(s.end)))
            };
            day_bar(f, from, to);
            f.blank();
            notice(f, a);
            return footer(f, &paint(DIM, "type 4 digits   ↑↓ ±15 min   enter ok   esc back"));
        }
        _ => {
            steps(f, 3, "Open me when you sign in?");
            f.row("Then you never have to remember me.");
            f.row(&paint(DIM, sys::AUTOSTART_NOTE));
        }
    }
    f.blank();
    menu(f, a);
    if !a.notice.is_empty() {
        f.blank();
        notice(f, a);
    }
    footer(f, &paint(DIM, "↑↓ choose   enter ok   esc back   ctrl+c quit"));
}

/// Things worth knowing that the screen doesn't already say. Each shows for 8 s, in turn.
const TIPS: [&str; 6] = [
    "Tip: /rc in Claude Code lets you send Teams messages by phone.",
    "Tip: leave me running in the background; I notice when you go.",
    "Tip: N nudges Teams right now if it shows you Away.",
    "Tip: W sets work hours; outside them I sleep.",
    "Tip: let me open at sign-in (W) and forget about me.",
    "Tip: P pauses me, for a meeting room or a lunch break.",
];
const SCREENS_TIP: &str = "Tip: S lets the screens sleep while I keep you green.";

fn tip(now: i64) -> String {
    let n = TIPS.len() + usize::from(sys::TEAMS_OWN_WAY);
    let i = (now / 8).rem_euclid(n as i64) as usize;
    paint(DIM, TIPS.get(i).copied().unwrap_or(SCREENS_TIP))
}

fn draw_dashboard(f: &mut Frame, a: &App, now: i64) {
    let here = a.user_here(now);
    let face = match &a.idle_err {
        Some(_) => "(o_O)",
        None if a.paused => "(-_-)",
        None if !a.keeping => ["(-.-) Zz", "(-.-) zZ"][(now % 2) as usize],
        None if now - a.last_nudge < 2 => "\\(^o^)/",
        None if here => "(^‿^)",
        None => "(•‿•)",
    };
    header(f, &blink(face, now));

    // the one thing to read: a coloured badge and a few words; below it what to do, or a rotating tip
    let s = &a.cfg.schedule;
    let (badge, words, line) = match &a.idle_err {
        Some(e) => (paint(ON_RED, " ✗ SOMETHING'S WRONG "), "", e.clone()),
        None if a.paused => {
            (paint(ON_YELLOW, " ❚❚ PAUSED "), "", "Teams shows you Away after 5 min without input.".into())
        }
        None if !a.keeping => (
            paint(ON_GREY, " ○ OFF HOURS "),
            "",
            format!("Outside {}-{}. I'll start again on my own.", hm(s.start), hm(s.end)),
        ),
        None if here => (paint(ON_GREEN, " ● GREEN "), "You're active.", tip(now)),
        // every way of nudging failed (e.g. teams-for-linux not set up): don't claim green
        None if !a.nudge_result.is_empty() && !a.nudge_result.split(", ").any(|d| d.ends_with(" ok")) => {
            (paint(ON_RED, " ✗ NOT KEEPING YOU GREEN "), "", "Teams can't be told you're here. Why, below.".into())
        }
        None if !a.idle_known => (paint(ON_GREEN, " ● KEEPING YOU GREEN "), "", tip(now)),
        None => (paint(ON_GREEN, " ● KEEPING YOU GREEN "), "You've stepped away.", tip(now)),
    };
    f.row(&format!("{badge}  {}", paint(BOLD, words)));
    f.row(&line);
    f.blank();

    let teams = match &a.teams {
        None if sys::teams_log_dir().is_none() => paint(DIM, "status not readable on this OS"),
        None => paint(DIM, "can't read its log yet (is new Teams open?)"),
        Some(t) => {
            let color = match t.status.as_str() {
                "Available" => GREEN,
                "Away" | "BeRightBack" => YELLOW,
                _ => RED,
            };
            let seen = t.at.map_or(String::new(), |at| paint(DIM, &format!(" · seen {} ago", span((now - at).max(0)))));
            format!("{}{}{} unread{seen}", paint(color, &format!("● {}", t.status)), paint(DIM, " · "), t.unread)
        }
    };
    let mut next = if a.keeping && !here && a.idle_err.is_none() {
        format!("in {}", span(keeper::next_nudge_in(&a.cfg, a.away_for(now), now, a.last_nudge)))
    } else {
        paint(DIM, "-")
    };
    next += &paint(DIM, if a.cfg.screens_on { " · screens stay on" } else { " · screens may sleep" });
    let mut done = a.nudges.to_string();
    if a.last_nudge != 0 {
        let room = WIDTH - 12 - done.len() - 16; // "Nudges      ", " · last HH:MM · "
        done += &paint(DIM, &format!(" · last {} · {}", clock(a.last_nudge), fit(&a.nudge_result, room)));
    }
    f.row(&format!("{}{teams}", paint(DIM, "Teams       ")));
    f.row(&format!("{}{next}", paint(DIM, "Next nudge  ")));
    f.row(&format!("{}{done}", paint(DIM, "Nudges      ")));
    f.blank();

    // last hour, one cell per minute
    let mut count = [0; 4];
    let bar: String = (now / 60 - 59..=now / 60)
        .map(|m| {
            let v = a.graph.get(m);
            count[v as usize] += 1;
            match v {
                Minute::You => paint(GREEN, "█"),
                Minute::Kept => paint(CYAN, "▄"),
                Minute::Off => paint(DIM, "·"),
                Minute::Empty => " ".into(),
            }
        })
        .collect();
    let mins = |v: Minute| format!(" {}m   ", count[v as usize]);
    f.row(&format!(
        "{}{} you{}{} kept{}{} off{}",
        paint(BOLD, "LAST HOUR   "),
        paint(GREEN, "█"),
        mins(Minute::You),
        paint(CYAN, "▄"),
        mins(Minute::Kept),
        paint(DIM, "·"),
        mins(Minute::Off)
    ));
    f.row(&bar);
    f.row(&paint(DIM, &format!("60m ago{:50}now", "")));
    if !a.notice.is_empty() {
        f.blank();
        notice(f, a);
    }
    let pause = if a.paused { "resume " } else { "pause  " };
    let keys = format!(
        "{} {pause}{} nudge  {} screens  {} setup  {} quit",
        keycap('P'),
        keycap('N'),
        keycap('S'),
        keycap('W'),
        keycap('Q')
    );
    footer(f, &keys);
}

// ---- keys ----

impl App {
    fn go(&mut self, s: Screen) {
        self.screen = s;
        self.input.clear();
        self.notice.clear();
        self.sel = match s {
            Screen::Schedule => usize::from(self.draft.schedule.mode == Mode::Hours),
            Screen::Autostart => usize::from(!self.draft.autostart),
            _ => 0,
        };
    }

    /// Keeps the notice, so a corrupt-settings message survives into the wizard.
    fn start_wizard(&mut self) {
        self.draft = self.cfg;
        self.screen = Screen::Disclaimer;
        self.sel = 0;
        self.input.clear();
        self.teams_found = sys::teams_log_dir().is_some_and(|d| d.exists());
    }

    fn finish_wizard(&mut self, autostart: bool) {
        self.draft.accepted = true;
        self.draft.autostart = autostart;
        self.go(Screen::Dashboard);
        if let Err(e) = sys::set_autostart(autostart) {
            self.notice = format!("Autostart: {e}. ");
        }
        if let Err(e) = keeper::save_config(&self.draft) {
            let _ = write!(self.notice, "Couldn't save settings: {e}");
        }
        self.cfg = self.draft;
        self.first_run = false;
        self.last_tick = 0;
    }

    /// The picked option's index. Up/Down move the highlight, Enter picks it, or press the option's own key.
    fn pick(&mut self, k: Key) -> Option<usize> {
        match k {
            Key::Up | Key::Down | Key::Char('\t') => {
                self.sel = 1 - self.sel;
                None
            }
            Key::Char('\r') => Some(self.sel),
            Key::Char(c) => options(self).iter().position(|o| o.0 == c),
        }
    }

    /// Four digits fill HH:MM, arrows step 15 minutes, Enter keeps the shown time.
    fn type_time(&mut self, k: Key) {
        let start = self.screen == Screen::StartTime;
        let saved = if start { self.draft.schedule.start } else { self.draft.schedule.end };
        if k != Key::Char('\r') {
            self.notice.clear();
        }
        match k {
            Key::Char('\x1b') => self.go(if start { Screen::Schedule } else { Screen::StartTime }),
            Key::Up | Key::Down => {
                let base = if self.input.len() == 4 { parse_hm(&as_time(&self.input)) } else { Some(saved) };
                let step = if k == Key::Up { 15 } else { 1440 - 15 };
                self.input = hm((base.unwrap_or(540) + step) % 1440).replace(':', "");
            }
            Key::Char('\x08') => {
                self.input.pop();
            }
            Key::Char(c @ '0'..='9') if self.input.len() < 4 => self.input.push(c),
            Key::Char('\r') => {
                let differ = (!start).then_some(self.draft.schedule.start);
                let t = match self.input.len() {
                    0 => check_time(&hm(saved), differ),
                    4 => check_time(&as_time(&self.input), differ),
                    _ => Err("Type all four digits, like 0930."),
                };
                match (t, start) {
                    (Err(e), _) => self.notice = e.into(),
                    (Ok(t), true) => {
                        self.draft.schedule.start = t;
                        self.go(Screen::EndTime);
                    }
                    (Ok(t), false) => {
                        self.draft.schedule.end = t;
                        self.go(Screen::Autostart);
                    }
                }
            }
            _ => {}
        }
    }

    /// Returns false to quit.
    fn on_key(&mut self, k: Key, now: i64) -> bool {
        let k = match k {
            Key::Char('\x03') => return false,                 // Ctrl+C
            Key::Char(c) => Key::Char(c.to_ascii_lowercase()), // Caps Lock
            k => k,
        };
        let esc = k == Key::Char('\x1b');
        let p = match self.screen {
            Screen::Dashboard | Screen::StartTime | Screen::EndTime => None,
            _ => self.pick(k),
        };
        match self.screen {
            Screen::Disclaimer => match p {
                Some(0) => self.go(Screen::Schedule),
                Some(_) if self.first_run => return false,
                Some(_) => self.go(Screen::Dashboard),
                None if esc && !self.first_run => self.go(Screen::Dashboard),
                None => {}
            },
            Screen::Schedule => match p {
                Some(0) => {
                    self.draft.schedule.mode = Mode::Always;
                    self.go(Screen::Autostart);
                }
                Some(_) => {
                    self.draft.schedule = Schedule { mode: Mode::Hours, days: WORKDAYS, ..self.draft.schedule };
                    self.go(Screen::StartTime);
                }
                None if esc => self.go(Screen::Disclaimer),
                None => {}
            },
            Screen::StartTime | Screen::EndTime => self.type_time(k),
            Screen::Autostart => match p {
                Some(i) => self.finish_wizard(i == 0),
                None if esc => {
                    let hours = self.draft.schedule.mode == Mode::Hours;
                    self.go(if hours { Screen::EndTime } else { Screen::Schedule });
                }
                None => {}
            },
            Screen::Dashboard => match k {
                Key::Char('q') => return false,
                Key::Char('p') => {
                    self.paused = !self.paused;
                    self.last_tick = 0;
                }
                Key::Char('n') if self.keeping => {
                    self.away_since = now; // the key press itself was you
                    self.nudge(now);
                }
                Key::Char('s') if !self.cfg.screens_on || sys::TEAMS_OWN_WAY => {
                    self.cfg.screens_on = !self.cfg.screens_on;
                    sys::set_exec_state(self.keeping, self.cfg.screens_on);
                    self.notice = match keeper::save_config(&self.cfg) {
                        Err(e) => format!("Couldn't save settings: {e}"),
                        Ok(()) if self.cfg.screens_on => "Screens stay on while I keep you green.".into(),
                        Ok(()) => sys::SCREENS_OFF_NOTE.into(),
                    };
                }
                Key::Char('s') => self.notice = "Screens-off isn't possible on this OS (see README).".into(),
                Key::Char('w') => {
                    self.notice.clear();
                    self.start_wizard();
                }
                _ => {}
            },
        }
        true
    }
}

/// `teamzi send <email> <message...>`: one Teams chat message through the local app, then exit.
fn send(args: &[String]) -> i32 {
    let [email, words @ ..] = args else {
        eprintln!("usage: teamzi send <email> <message>");
        return 2;
    };
    let text = words.join(" ");
    let result = keeper::chat_link(email, &text).map_err(String::from).and_then(|link| sys::send_chat(&link));
    match result {
        Ok(what) => {
            println!("{what} -> {email}: {text}");
            0
        }
        Err(e) => {
            eprintln!("not sent: {e}");
            1
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    if let Some((cmd, rest)) = args.split_first() {
        std::process::exit(match cmd.as_str() {
            "send" => send(rest),
            _ => {
                eprintln!("usage: teamzi [send <email> <message>]");
                2
            }
        });
    }
    if sys::reopen_in_console() {
        return;
    }
    sys::set_exec_state(false, true); // clears anything a crashed earlier run left behind
    let (cfg, notice) = keeper::load_config().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let mut a = App { cfg, notice: notice.unwrap_or_default().into(), ..App::default() };
    if !a.cfg.accepted {
        a.start_wizard();
        a.first_run = true;
    }

    let term = sys::Term::open();
    if sys::own_window() {
        out("\x1b[8;21;70t"); // window just fits the box (+1 row for the cursor, so nothing scrolls)
    }
    out("\x1b]0;Teamzi\x07\x1b[?1049h\x1b[?25l"); // window title, own screen, hide cursor
    let mut f = Frame::default();
    loop {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        a.sample(now);
        if due(now, a.last_tick, 5) {
            a.tick(now);
        }
        if a.screen == Screen::Dashboard {
            draw_dashboard(&mut f, &a, now)
        } else {
            draw_wizard(&mut f, &a, now)
        }
        f.show();
        if let Some(k) = term.read_key(1000)
            && !a.on_key(k, now)
        {
            break;
        }
    }
    sys::set_exec_state(false, true);
    out("\x1b[?25h\x1b[?1049l"); // restore the cursor and the normal screen
    drop(term);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(a: &App, now: i64) -> String {
        let mut f = Frame::default();
        if a.screen == Screen::Dashboard {
            draw_dashboard(&mut f, a, now)
        } else {
            draw_wizard(&mut f, a, now)
        }
        f.rule("╰", "╯");
        f.0
    }

    #[test]
    fn gradient_shades_without_taking_space() {
        assert_eq!(width(&gradient("╭──╮", 0, 4)), 4);
        assert_ne!(hue(0, WIDTH + 4), hue(WIDTH + 3, WIDTH + 4));
        assert_eq!(hue(9, 4), hue(3, 4)); // past the end stays on the last colour
        let _ = hue(0, 0); // empty width: no division by zero
    }

    /// Every screen in every state fits the 68-column box. SHOW=1 cargo test -- --nocapture prints them.
    #[test]
    fn every_screen_fits_the_box() {
        let now = 1_790_885_581;
        let cfg = Config { accepted: true, ..Config::default() };
        let mut a = App { cfg, draft: cfg, first_run: true, ..App::default() };
        let mut frames = Vec::new();
        for s in [Screen::Disclaimer, Screen::Schedule, Screen::StartTime, Screen::EndTime, Screen::Autostart] {
            a.go(s);
            frames.push(render(&a, now));
        }
        a.go(Screen::StartTime);
        a.input = "09".into();
        a.notice = "Type all four digits, like 0930.".into();
        frames.push(render(&a, now));
        a.input = "2230".into();
        frames.push(render(&a, now));

        a.go(Screen::Dashboard);
        a.paused = true;
        frames.push(render(&a, now)); // paused, Teams unknown
        a.paused = false;
        frames.push(render(&a, now)); // off hours
        a.keeping = true;
        a.away_since = now - 4000;
        a.last_nudge = now - 30;
        a.nudges = 9999;
        a.nudge_result = "key ok, Teams ok".into();
        a.teams = Some(Teams { status: "BeRightBack".into(), unread: 12, at: Some(now - 300) });
        a.notice = "Couldn't save settings: Access is denied. (os error 5)".into();
        for m in 0..60 {
            a.graph.mark(now / 60 - m, [Minute::You, Minute::Kept, Minute::Off][(m % 3) as usize]);
        }
        frames.push(render(&a, now)); // keeping
        a.nudge_result = "Teams turn on idleDetection.forceState in teams-for-linux (README: Linux)".into();
        let failing = render(&a, now);
        assert!(failing.contains("NOT KEEPING YOU GREEN"));
        frames.push(failing);
        a.idle_err = Some("GetLastInputInfo failed (code 5)".into());
        frames.push(render(&a, now));

        for fr in &frames {
            for line in fr.lines() {
                assert_eq!(width(line), 68, "{line}");
            }
        }
        if std::env::var_os("SHOW").is_some() {
            println!("{}", frames.join("\n"));
        }
    }

    #[test]
    fn tips_fit_and_rotate() {
        for t in TIPS.iter().chain([&SCREENS_TIP]) {
            assert!(width(t) <= WIDTH, "{t}");
        }
        assert_ne!(tip(0), tip(8));
    }

    #[test]
    fn wizard_flow() {
        let mut a = App { first_run: true, ..App::default() };
        a.start_wizard();
        for k in ['\r', '2', '0', '8', '3', '0', '\r', '\x1b'] {
            assert!(a.on_key(Key::Char(k), 0));
        }
        assert_eq!((a.screen, a.draft.schedule.start, a.draft.schedule.mode), (Screen::StartTime, 510, Mode::Hours));
        a.on_key(Key::Char('\r'), 0); // keep 08:30
        a.on_key(Key::Char('\r'), 0); // keep 17:00
        assert_eq!(a.screen, Screen::Autostart);
        a.on_key(Key::Char('\x1b'), 0);
        a.on_key(Key::Up, 0); // 17:15
        a.on_key(Key::Char('\r'), 0);
        assert_eq!((a.screen, a.draft.schedule.end), (Screen::Autostart, 17 * 60 + 15));
        a.go(Screen::EndTime);
        for k in ['0', '8', '3', '0', '\r'] {
            a.on_key(Key::Char(k), 0);
        }
        assert_eq!(a.notice, "End time must differ from start time.");
        let mut b = App { first_run: true, ..App::default() };
        b.start_wizard();
        assert!(!b.on_key(Key::Char('N'), 0)); // Caps Lock still picks "No thanks, exit"
    }
}
