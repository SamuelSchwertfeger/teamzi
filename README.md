# Teamzi

Keeps Microsoft Teams presence green while you are away from the keyboard. ASCII TUI with a first-run wizard.

Tested on Windows. Linux (with teams-for-linux) and macOS builds are included but **unverified**: they build and pass CI, but nobody has confirmed them on a real machine yet. If you try one, please [comment on issue #2](https://github.com/SamuelSchwertfeger/teamzi/issues/2) whether it worked or not.

## Disclaimer

At your own risk. This simulates activity. Employers can detect it. You accept responsibility.

While keeping green with the screens on (the default), your screen will NOT auto-lock. Anyone at the machine can use it.

## What it installs

Nothing. It is a single program (Rust, about 165 KB on Windows, under 2 MB of RAM, no libraries). It talks only to the local machine and the local Teams app: no network, no Graph API, nothing connects to your employer's Microsoft tenant.

| | Windows | Linux | macOS |
|---|---|---|---|
| Settings | `%APPDATA%\teamzi\config.ini` | `~/.config/teamzi/config.ini` | `~/Library/Application Support/teamzi/config.ini` |
| Autostart (only if you pick it in setup; picking "no" removes it) | registry value `HKCU\...\Run\teamzi` | `~/.config/autostart/teamzi.desktop` | `~/Library/LaunchAgents/io.github.samuelschwertfeger.teamzi.plist` |
| How it keeps you green | F15 key press + `ms-teams.exe --set-presence-to-available` | writes `active` to teams-for-linux's idle state file | F18 key press |
| Keeps the machine awake with | `SetThreadExecutionState` | `systemd-inhibit` | `caffeinate` |
| Other programs it runs (all come with the OS) | `ms-teams.exe` | `busctl`, `systemd-inhibit`, `tail`, `xdg-open`, `xdg-mime`, `stty` | `caffeinate`, `open`, `lsappinfo`, `stty` |

## Install

Download the file for your system from the [latest release](https://github.com/SamuelSchwertfeger/teamzi/releases/latest), or build it (see Build). To check the download, put `SHA256SUMS` from the release next to it and run `sha256sum -c --ignore-missing SHA256SUMS` (macOS: `shasum -a 256 -c --ignore-missing SHA256SUMS`; Windows PowerShell: compare `Get-FileHash teamzi-x86_64-windows.exe` with the line in the file).

- **Windows:** `teamzi-x86_64-windows.exe`. Double-click it. The exe isn't signed, so SmartScreen may warn about an unknown publisher: More info > Run anyway.
- **Linux:** `teamzi-x86_64-linux`, a static binary for any x86_64 distro. It needs teams-for-linux: follow [Linux setup](#setup-once).
- **macOS:** `teamzi-aarch64-macos` (Apple silicon) or `teamzi-x86_64-macos` (Intel). It isn't signed, so clear the download quarantine, then install it (on an Intel Mac use the `x86_64` name):

  ```
  xattr -d com.apple.quarantine teamzi-aarch64-macos
  sudo mkdir -p /usr/local/bin
  sudo install -m755 teamzi-aarch64-macos /usr/local/bin/teamzi
  ```

## Use

Run `teamzi` in a terminal (on Windows, double-click it). The first run is a short setup: ground rules, when to keep you green (always or work hours), and whether to start at login.

The frame and logo shade from dark blue to dark purple. That uses 24-bit colour on Windows and in terminals that set `COLORTERM=truecolor`; other terminals get the nearest of their 256 colours.

Dashboard keys: `P` pause/resume, `N` nudge now, `S` screens on/off, `W` rerun setup, `Q` quit. Keeping runs only while the app is open; quitting stops it and undoes everything it set.

### Screens-off mode (`S`)

By default it keeps the screens on, because a key press wakes them anyway. Press `S` to let the screens sleep: it then stops pressing keys and uses only Teams' own way of saying "I'm here" (Windows: the `ms-teams.exe` command; Linux: the state file). The PC itself still stays awake. Not available on macOS, where a key press is the only method.

Caveat: many work PCs lock when the screen turns off, and Teams shows Away while the PC is locked. Test once: press `S`, leave for 15 minutes, check your status on your phone.

## Windows

Download `teamzi-x86_64-windows.exe` (see Install) or build it, and run it. Windows 10/11, new Teams. Tested: kept green 6+ hours unattended.

## Linux (unverified)

**Most common setup:** a work laptop or desktop running KDE Plasma or GNOME, with Teams in the **teams-for-linux** desktop app. That is the setup this is built for.

### Why teams-for-linux and not the browser

Teams in a browser tab (or as a PWA) turns Away a few minutes after you stop using *that tab*, whatever else you do on the machine. Nothing outside the browser can change that without typing into the tab. teams-for-linux, an unofficial open-source Teams app, has a supported switch for this exact job: when its `idleDetection.forceState` option is on, the word `active` in a small text file makes it report you as active. teamzi writes that file while it is keeping you green and deletes it when it stops (pause, off hours, quit, or even if it is killed or its terminal is closed), so teams-for-linux goes back to its normal idle detection. It only ever deletes the file when it says `active`.

Because of that, the Linux version never fakes key presses. That also means no root setup: Wayland blocks injected input unless you open `/dev/uinput` with a udev rule, and this avoids it.

### Setup (once)

1. Install teams-for-linux. It is not in the Arch repos (AUR only), so on Arch/CachyOS without the AUR use the Flatpak:

   ```
   pacman -Si teams-for-linux     # if your distro's repos have it, use: sudo pacman -S teams-for-linux
   sudo pacman -S flatpak
   flatpak install flathub com.github.IsmaelMartinez.teams_for_linux
   ```

   (Debian/Ubuntu/Fedora: packages at https://teamsforlinux.de.)

2. Turn on its state-file switch. Create or edit its `config.json` and add the `idleDetection` part:

   - normal install: `~/.config/teams-for-linux/config.json`
   - Flatpak: `~/.var/app/com.github.IsmaelMartinez.teams_for_linux/config/teams-for-linux/config.json`

   ```json
   {
     "idleDetection": { "forceState": true }
   }
   ```

   Flatpak apps get their own private `/tmp`, so for the Flatpak also point the state file at a folder both sides can see (replace `you` with your user name):

   ```json
   {
     "idleDetection": {
       "forceState": true,
       "stateFile": "/home/you/.var/app/com.github.IsmaelMartinez.teams_for_linux/idle-state"
     }
   }
   ```

   Restart teams-for-linux. teamzi reads the same config file, so it finds the state file on its own.

3. Install teamzi. Download `teamzi-x86_64-linux` from the [latest release](https://github.com/SamuelSchwertfeger/teamzi/releases/latest), then:

   ```
   install -Dm755 teamzi-x86_64-linux ~/.local/bin/teamzi
   ```

   Or build it (no download beyond the Rust toolchain; no crates):

   ```
   sudo pacman -S rust
   cargo build --release
   install -Dm755 target/release/teamzi ~/.local/bin/teamzi
   ```

4. Run `teamzi` in a terminal and go through setup. If it says "command not found", `~/.local/bin` isn't on your PATH: add it in your shell's config (fish: `fish_add_path ~/.local/bin`), or run `~/.local/bin/teamzi`.

### Check it works

- After setup, with the dashboard showing KEEPING YOU GREEN, the state file says `active`:
  `cat /tmp/teams-for-linux-idle-state-$USER` (or your Flatpak path).
- The dashboard's "Nudges" line ends in `Teams ok`. If it shows an error instead, it tells you what is missing (config file not found, or forceState not on).
- Leave the machine for 15 minutes and check your status on your phone.

If Teams still goes Away after a few minutes: teams-for-linux issue #2077 reports Teams itself started marking people away after about 3 minutes of not using its window, and one user fixed it with a newer option in Teams' own settings under **Notifications and activity**. Check there.

### What you see on Linux

- **Idle time:** GNOME (X11 or Wayland) and X11 sessions of other desktops report how long you've been idle, so the dashboard shows "You're active." vs "You've stepped away.". KDE on Wayland can't report it (its `GetSessionIdleTime` always returns 0), so there the dashboard shows only KEEPING YOU GREEN during your hours. Keeping green works the same either way.
- **Teams line:** teams-for-linux writes no presence log, so the dashboard can't show your Teams status. Check your phone.
- **Screens:** with screens on, `systemd-inhibit` blocks sleep and screen blanking; with `S` (screens off) it blocks only sleep. Not every desktop honours that idle block (GNOME may not); if your screen still locks, turn off automatic lock in its settings. The block is only granted to a program started in your desktop session: run over SSH, polkit refuses it (keeping green and the cleanup still work).
- **Autostart:** a desktop entry that opens teamzi in your terminal at login. KDE, GNOME, Xfce and most desktops read it; bare window managers (i3, sway) don't, so there add `exec <terminal> -e teamzi` to their config instead.

### Sending on Linux

`teamzi send <email> <message>` opens the chat with the message typed in (in teams-for-linux when it handles `msteams:` links, else in the browser) and stops there: **you press Enter**. On Wayland no program can check which window is in front, so it never presses Enter for you.

## macOS (unverified)

Written against Apple's documented APIs and built in CI, but not yet run on a Mac. If you try it, please [comment on issue #2](https://github.com/SamuelSchwertfeger/teamzi/issues/2).

- Install: see Install above, or build it: install Rust (`brew install rust` or rustup), then `cargo build --release`.
- Run `teamzi` in Terminal. Key presses need the **Accessibility** permission for the terminal app: System Settings > Privacy & Security > Accessibility > add Terminal (or iTerm). Without it the dashboard says so.
- It taps F18 (on no laptop keyboard, bound to nothing; F15 would dim the screen) and runs `caffeinate` while keeping you green. Screens-off mode is not available.
- Teams status line: read from new Teams' log in `~/Library/Group Containers/UBF8T346G9.com.microsoft.teams/Library/Application Support/Logs` if it is there.
- Send presses Return only once `lsappinfo` reports Teams as the app in front, like on Windows.

## Send a Teams message

```
teamzi send esam@company.com running 5 late
```

Windows and macOS: opens the chat in the Teams app with the text typed in, waits until Teams is the window in front, and presses Enter. It never presses Enter in any other window: if Teams doesn't come to the front within 20 seconds, or something else takes the front before sending, it stops and says "not sent". The machine must be unlocked. Linux: see above, you press Enter. Like everything else here it goes through the local Teams app only, no Graph API.

`claude-code/teams-send/SKILL.md` is an optional Claude Code skill that lets you text "send elliot: running 5 late" from your phone. It looks the name up in `contacts.txt` in the settings folder (`name = email` per line), asks which one if several match, shows who and what, and sends only after you say yes. To use it, copy the folder into `~/.claude/skills/` and set the exe path at its top.

## Build

Needs the Rust toolchain (rustup, stable channel). There are no crates, so Cargo downloads nothing.

```
cargo build --release      # target/release/teamzi(.exe)
cargo test --release
```

The Windows release exe is built smaller (about 165 KB instead of 330 KB) by rebuilding the standard library for size. That uses nightly-only flags on the stable compiler and needs `rustup component add rust-src`:

```
RUSTC_BOOTSTRAP=1 RUSTFLAGS="-Zunstable-options -Cpanic=immediate-abort -Zlocation-detail=none -Zfmt-debug=none" \
  cargo build --release --target x86_64-pc-windows-gnu -Z build-std=std,panic_abort
```

Output: `target/x86_64-pc-windows-gnu/release/teamzi.exe`. It imports only Windows system DLLs (kernel32, user32, advapi32, ntdll, and the UCRT built into Windows 10/11). The same flags work for Linux and macOS targets.

## Code layout

No UI library and no dependencies beyond the Rust standard library and the OS. The few OS calls are declared by hand.

- `src/keeper.rs`: OS-independent logic: config file, schedule, nudge timing, Teams log parsing, and their unit tests. Start here.
- `src/main.rs`: the wizard and dashboard (plain ANSI terminal codes) and the single-threaded main loop.
- `src/sys_windows.rs`: Windows idle time, F15 key press, ms-teams.exe, autostart, stay-awake, console keys, send.
- `src/sys_unix.rs`: macOS/Linux shared parts: config folder, clock, terminal, keep-awake helper process.
- `src/sys_linux.rs`: idle time over D-Bus, the teams-for-linux state file, systemd-inhibit, autostart entry, send.
- `src/sys_macos.rs`: CoreGraphics idle time and F18 key press, caffeinate, LaunchAgent, send.

## License

MIT. See [LICENSE](LICENSE).
