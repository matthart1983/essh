//! Drives the real binary in a real terminal and asserts it stays responsive.
//!
//! Unit tests render a frame from a fixture; they cannot see a hang, because
//! a hang is not a wrong frame — it is the absence of the next one. Every
//! serious bug in this app so far has been of that shape: a blocking connect
//! on the event loop, a password prompt fighting the input reader for stdin,
//! a session that drew nothing. None of them would fail a render test.
//!
//! So this harness spawns `essh` on a PTY, types at it, and requires the
//! screen to show something specific **within a deadline**. A hang fails the
//! test instead of hanging the suite, and the failure prints the last screen
//! so the cause is visible rather than inferred.
//!
//! Requires the three demo containers on 127.0.0.1:2201-3. Without them the
//! tests skip rather than fail, because a missing fixture is not a defect in
//! the app — but the skip is printed so nobody mistakes it for a pass.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

const COLS: u16 = 120;
const ROWS: u16 = 34;

/// A running `essh` on a pty, with everything it has printed so far.
struct Tui {
    writer: Box<dyn Write + Send>,
    /// A real terminal screen, not a transcript.
    ///
    /// Two earlier designs were wrong. Searching the whole byte transcript
    /// meant any word that ever appeared matched forever, so a later step
    /// could pass on a screen that scrolled by minutes ago. Searching only
    /// *recent* bytes is worse: ratatui redraws changed cells only, so text
    /// plainly visible on screen may not appear in recent output at all.
    ///
    /// Feeding the stream through a terminal emulator and asserting on the
    /// resulting screen is the only model that matches what a user sees.
    screen: Arc<Mutex<vt100::Parser>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _pty: Box<dyn portable_pty::MasterPty + Send>,
}

impl Tui {
    fn start(home: &PathBuf) -> Self {
        let pty = NativePtySystem::default()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        let mut cmd = CommandBuilder::new(bin());
        cmd.env("HOME", home);
        cmd.env("TERM", "xterm-256color");
        // Keep the child's own colour decisions out of the matching.
        cmd.env("NO_COLOR", "0");
        cmd.cwd(home);

        let child = pty.slave.spawn_command(cmd).expect("spawn essh");
        let mut reader = pty.master.try_clone_reader().expect("reader");
        let writer = pty.master.take_writer().expect("writer");

        // Drain continuously. If the app stops producing output the buffer
        // simply stops growing, which is exactly what a deadline detects.
        let screen = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 0)));
        let sink = screen.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().process(&buf[..n]);
            }
        });

        Tui {
            writer,
            screen,
            child,
            _pty: pty.master,
        }
    }

    /// What is on screen right now.
    fn text(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write to pty");
        self.writer.flush().expect("flush");
    }

    fn type_text(&mut self, s: &str) {
        // One byte at a time, as a human types: sending a whole line at once
        // can outrun a handler that reads one key per loop iteration.
        for b in s.as_bytes() {
            self.send(&[*b]);
            std::thread::sleep(Duration::from_millis(12));
        }
    }

    /// Wait for `needle`, failing with the screen if it never arrives.
    ///
    /// This is the whole point of the harness: `within` is the responsiveness
    /// budget, and blowing it is a hang.
    fn expect(&self, needle: &str, within: Duration) {
        let started = Instant::now();
        while started.elapsed() < within {
            if self.text().contains(needle) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!(
            "waited {:?} for {needle:?}; it is not on screen.\n\
             ── last screen ──────────────────────────────\n{}\n\
             ─────────────────────────────────────────────",
            within,
            tail(&self.text(), 40)
        );
    }

    /// Assert the app still answers input.
    ///
    /// Not "did the screen change": an idle launcher is entitled to sit
    /// perfectly still, so a repaint is not evidence of life and its absence
    /// is not evidence of a hang. The only honest test is to press something
    /// and see the app react — which is also exactly what a user does when
    /// they suspect it has frozen.
    fn expect_responsive(&mut self, within: Duration) {
        // A printable character is the one probe that shows up on every
        // screen that accepts input at all — a list with a single row does
        // not move when you press Down, which says nothing about whether the
        // app is alive.
        self.expect_responds_to(b"x", b"\x7f", within)
    }

    /// Press `probe`, require the screen to change, then press `undo`.
    fn expect_responds_to(&mut self, probe: &[u8], undo: &[u8], within: Duration) {
        let before = self.text();
        self.send(probe);
        let started = Instant::now();
        let mut changed = self.text() != before;
        while !changed && started.elapsed() < within {
            std::thread::sleep(Duration::from_millis(50));
            changed = self.text() != before;
        }
        if !changed {
            panic!(
                "no reaction to a keypress within {:?} — the UI is not \
                 processing input.\n\
                 ── last screen ──────────────────────────────\n{}\n\
                 ─────────────────────────────────────────────",
                within,
                tail(&self.text(), 40)
            );
        }
        self.send(undo);
        std::thread::sleep(Duration::from_millis(200));
    }

    fn quit(&mut self) {
        self.send(b"q");
        std::thread::sleep(Duration::from_millis(300));
        let _ = self.child.kill();
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn tail(s: &str, lines: usize) -> String {
    let all: Vec<&str> = s.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

fn bin() -> PathBuf {
    let mut p = std::env::current_exe().expect("test exe");
    p.pop(); // deps/
    p.pop(); // release/
    p.push("essh");
    p
}

/// Are the demo containers up?
fn demo_hosts_available() -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let addr = match "127.0.0.1:2201".to_socket_addrs() {
        Ok(mut it) => match it.next() {
            Some(a) => a,
            None => return false,
        },
        Err(_) => return false,
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_ok()
}

/// The key the demo containers trust. Taken from the real home, because the
/// containers' `authorized_keys` were provisioned with it.
fn real_key() -> PathBuf {
    PathBuf::from(std::env::var("REAL_HOME").unwrap_or_else(|_| "/Users/matt".into()))
        .join(".ssh/id_ed25519")
}

/// A key that exists but is not authorised anywhere, so key auth fails and
/// the password fallback is exercised.
fn bogus_key() -> PathBuf {
    let dir = std::env::temp_dir().join("essh-harness-keys");
    std::fs::create_dir_all(&dir).expect("mkdir keys");
    let path = dir.join("bogus_ed25519");
    if !path.exists() {
        std::process::Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-N", "", "-q", "-f"])
            .arg(&path)
            .status()
            .expect("ssh-keygen");
    }
    path
}

/// A HOME with an ssh_config pointing at the demo containers.
fn fixture_home() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("essh-harness-{}", std::process::id()));
    let ssh = dir.join(".ssh");
    std::fs::create_dir_all(&ssh).expect("mkdir");
    std::fs::write(
        ssh.join("config"),
        // The demo containers accept the developer's default key. Naming it
        // explicitly keeps the harness off the agent and off any prompt —
        // a password prompt in a test is an infinite wait, not a failure.
        format!(
            "Host web-01\n  HostName 127.0.0.1\n  Port 2201\n  User root\n  IdentityFile {key}\n  IdentitiesOnly yes\n\n\
             Host web-02\n  HostName 127.0.0.1\n  Port 2202\n  User root\n  IdentityFile {key}\n  IdentitiesOnly yes\n\n\
             Host unreachable-blackhole\n  HostName 192.0.2.1\n  Port 22\n  User nobody\n  IdentityFile {key}\n  IdentitiesOnly yes\n\n\
             Host needs-pw\n  HostName 127.0.0.1\n  Port 2201\n  User nosuchuser\n  IdentityFile {bogus}\n  IdentitiesOnly yes\n",
            key = real_key().display(),
            bogus = bogus_key().display()
        ),
    )
    .expect("write ssh config");
    dir
}

fn skip(reason: &str) -> bool {
    eprintln!("SKIP: {reason}");
    true
}

// ── the tests ───────────────────────────────────────────────────────────────

/// The launcher must be on screen almost immediately.
#[test]
fn it_starts_and_shows_the_launcher() {
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.quit();
}

/// Typing filters, and the app keeps redrawing while it does.
#[test]
fn typing_in_the_launcher_filters_without_stalling() {
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.expect("web-01", Duration::from_secs(5));
    tui.expect_responsive(Duration::from_secs(3));
    tui.quit();
}

/// Connecting must reach a session, and the session must name its host.
///
/// This is the "screen goes blank" case: before the fix a connected session
/// with a quiet shell drew nothing at all.
#[test]
fn connecting_reaches_a_session_that_names_its_host() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");

    // The tab bar names the host as soon as the session exists.
    tui.expect("web-01", Duration::from_secs(25));
    tui.expect("ESSH", Duration::from_secs(5));
    // And the one-press keys are advertised, so nothing must be memorised.
    tui.expect("F2 monitor", Duration::from_secs(5));
    tui.quit();
}

/// The UI must stay alive while a connection to a black hole is in flight.
///
/// `192.0.2.1` is TEST-NET-1: routed nowhere, answers nothing. Before the
/// connect timeout existed this wedged the event loop for the OS TCP timeout
/// — ~75s on macOS — with no redraw and no way out.
#[test]
fn an_unreachable_host_does_not_wedge_the_ui() {
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("unreachable");
    tui.send(b"\r");

    // The attempt is announced rather than swallowed.
    tui.expect("Connecting", Duration::from_secs(6));

    // Wait for the evidence of recovery rather than for a fixed duration.
    // How long an unroutable address takes to fail depends on the network
    // stack — a CI runner may reject instantly where a laptop waits out the
    // full timeout — so sleeping past "however long it should take" is a
    // guess that eventually goes wrong on someone else's machine.
    tui.expect("could not connect", Duration::from_secs(45));
    tui.expect_responsive(Duration::from_secs(10));
    tui.quit();
}

/// Opening the monitor must not stall, and it must keep the host on screen.
#[test]
fn the_monitor_opens_and_keeps_the_host_visible() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(25));

    tui.send(b"\x01"); // Ctrl-A
    std::thread::sleep(Duration::from_millis(400));
    tui.send(b"m");

    tui.expect("processes", Duration::from_secs(20));
    tui.expect("terminal", Duration::from_secs(5)); // the way out is shown
                                                    // The monitor ignores typing; scrolling is its visible response.
    tui.expect_responds_to(b"\x1b[B", b"\x1b[A", Duration::from_secs(5));
    tui.quit();
}

/// The file browser lists a remote directory over SFTP. That is network work
/// on the event loop, so it is a prime hang candidate.
#[test]
fn the_file_browser_opens_and_stays_responsive() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(25));

    tui.send(b"\x01");
    std::thread::sleep(Duration::from_millis(400));
    tui.send(b"f");

    tui.expect("local", Duration::from_secs(15));
    tui.expect("remote", Duration::from_secs(15));
    tui.expect_responds_to(b"\x1b[B", b"\x1b[A", Duration::from_secs(5));
    tui.quit();
}

/// Every prefix command must produce its screen inside a budget.
///
/// Runs them in one session so a stall in an earlier screen shows up as a
/// failure in the next, which is how a real user would meet it.
#[test]
fn every_prefix_command_responds_within_budget() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(25));

    for (key, expect) in [
        (b'p', "port forwards"),
        (b'm', "processes"),
        (b'd', "Hosts"),
    ] {
        tui.send(b"\x01");
        std::thread::sleep(Duration::from_millis(400));
        tui.send(&[key]);
        tui.expect(expect, Duration::from_secs(15));
        // Back to the session for the next one, except after detach.
        if key != b'd' {
            tui.send(b"\x1b");
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    tui.quit();
}

/// The prefix menu must appear the moment the prefix is pressed.
///
/// The keys are useless if the only way to learn them is the manual.
#[test]
fn pressing_the_prefix_shows_the_menu() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(25));

    tui.send(b"\x01");
    tui.expect("monitor", Duration::from_secs(4));
    tui.expect("detach", Duration::from_secs(4));
    tui.quit();
}

/// When key auth fails, the password prompt must actually appear and accept
/// typing — it must not hang.
///
/// This is the bug that made `mattbot` unusable: key auth failed, ESSH fell
/// back to a password prompt, left the alternate screen, and then blocked
/// forever because the event-reader thread was still draining stdin. Every
/// keystroke went to the reader, including Ctrl-C, and the app could not be
/// recovered without killing it from another terminal.
#[test]
fn a_host_needing_a_password_prompts_and_stays_usable() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("needs-pw");
    tui.send(b"\r");

    // Wait for the prompt itself, not for any screen containing the word:
    // an earlier version matched the host's own name in the launcher and
    // began typing the password into the search box.
    tui.expect("Key authentication failed", Duration::from_secs(30));
    tui.expect("'s password:", Duration::from_secs(10));

    // And it must be typeable: a wrong password should be rejected and land
    // us back somewhere usable, not leave a shell-less tab with no way out.
    tui.type_text("definitely-not-the-password");
    tui.send(b"\r");

    // Back at the launcher, and told what to check — not just "password
    // incorrect", which sends you hunting for the wrong thing.
    tui.expect("username", Duration::from_secs(30));
    tui.expect("nosuchuser@127.0.0.1", Duration::from_secs(6));

    // Recovery means actually being able to connect afterwards.
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(30));
    tui.quit();
}

/// The help screen must teach a binding the reader can actually press.
///
/// `Option+m` is not that binding on macOS unless the terminal is configured
/// to send Alt as Meta, which it is not by default — so it types `µ` and the
/// app looks broken. The prefix works everywhere, so the prefix is what the
/// help teaches.
#[test]
fn help_teaches_a_binding_that_works_everywhere() {
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.send(b"\x1b"); // Esc to the dashboard, where '?' opens help
    std::thread::sleep(Duration::from_millis(600));
    tui.send(b"?");

    tui.expect("One press, no prefix", Duration::from_secs(8));
    tui.expect("F2", Duration::from_secs(4));
    tui.expect("let go, then the key", Duration::from_secs(4));
    tui.quit();
}

/// The function keys must work with a single press, in a session.
///
/// This is what "I cannot memorise the keys" actually needs: no prefix, no
/// modifier, no terminal configuration. Sent as the escape sequences a real
/// terminal emits, so the test exercises the same bytes a keyboard produces.
#[test]
fn function_keys_open_their_screens_with_one_press() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(25));

    // xterm sequences: F2 = ESC O Q, F3 = ESC O R, F4 = ESC O S.
    tui.send(b"\x1bOQ");
    tui.expect("processes", Duration::from_secs(15));

    tui.send(b"\x1b"); // back to the session
    std::thread::sleep(Duration::from_millis(600));
    tui.send(b"\x1bOR");
    tui.expect("remote", Duration::from_secs(15));

    tui.expect_responsive(Duration::from_secs(5));
    tui.quit();
}

/// F1 opens help from inside a session, where plain `?` belongs to the shell.
#[test]
fn f1_opens_help_even_inside_a_session() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(25));

    tui.send(b"\x1bOP"); // F1
    tui.expect("One press, no prefix", Duration::from_secs(10));
    tui.quit();
}

/// F5 puts the monitor beside the shell without leaving the shell.
///
/// The "mini monitor" is a different thing from the full-screen monitor and
/// from a session split, and it had no advertised key at all — the binding
/// existed but nothing on screen mentioned it, so it may as well not have.
#[test]
fn f5_opens_the_mini_monitor_beside_the_shell() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F5 mini", Duration::from_secs(25));

    tui.send(b"\x1b[15~"); // F5
                           // The essentials pane appears while the shell is still there.
    tui.expect("cpu", Duration::from_secs(20));
    tui.expect_responsive(Duration::from_secs(5));
    tui.quit();
}

/// Every advertised command must visibly do something, or say why not.
///
/// The systematic version of "does it work". Picking a few commands to test
/// by hand is how the pane split shipped broken: it set a status explaining
/// that it needed a second session, and nothing rendered statuses in a
/// session, so the key silently did nothing. This walks the whole advertised
/// set and fails on any key that produces neither a new screen nor a message.
#[test]
fn no_advertised_command_does_nothing() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }

    // (label, bytes, a marker proving it landed)
    let commands: &[(&str, &[u8], &str)] = &[
        ("F1 help", b"\x1bOP", "One press, no prefix"),
        ("F2 monitor", b"\x1bOQ", "processes"),
        ("F3 files", b"\x1bOR", "remote"),
        ("F4 forwards", b"\x1bOS", "port forwards"),
        ("F5 mini", b"\x1b[15~", "memory"),
        ("F6 detach", b"\x1b[17~", "Hosts"),
        // Refuses with one session open — and must say so.
        ("prefix s (pane split)", b"\x01s", "no other session"),
    ];

    for (label, press, marker) in commands {
        let home = fixture_home();
        let mut tui = Tui::start(&home);
        tui.expect("connect", Duration::from_secs(10));
        tui.type_text("web-01");
        tui.send(b"\r");
        tui.expect("F2 monitor", Duration::from_secs(25));
        std::thread::sleep(Duration::from_millis(700));

        tui.send(press);
        // A generous budget: this is about "did anything happen at all",
        // not about latency.
        let started = Instant::now();
        let mut seen = false;
        while !seen && started.elapsed() < Duration::from_secs(15) {
            seen = tui.text().contains(marker);
            if !seen {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        assert!(
            seen,
            "{label} produced no sign of {marker:?}.\n\
             ── screen ───────────────────────────────────\n{}\n\
             ─────────────────────────────────────────────",
            tail(&tui.text(), 40)
        );
        tui.quit();
    }
}

/// With two sessions open, the split must actually split.
///
/// The refusal path is only correct if the accepting path works; a command
/// that always declines is indistinguishable from one that is broken.
#[test]
fn the_pane_split_works_once_a_second_session_exists() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(25));

    // Detach, open a second host, come back.
    tui.send(b"\x1b[17~"); // F6 detach
    tui.expect("Hosts", Duration::from_secs(15));
    // Back to the launcher, which is where the ssh_config hosts live.
    tui.send(b"n");
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-02");
    std::thread::sleep(Duration::from_millis(400));
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(25));

    // Now the split has somewhere to go.
    std::thread::sleep(Duration::from_millis(700));
    tui.send(b"\x01s");

    // Both hosts named at once is the proof that two panes are on screen.
    let started = Instant::now();
    let mut both = false;
    while !both && started.elapsed() < Duration::from_secs(15) {
        let t = tui.text();
        both = t.contains("web-01") && t.contains("web-02");
        if !both {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    assert!(
        both,
        "split did not put both sessions on screen.\n\
         ── screen ───────────────────────────────────\n{}\n\
         ─────────────────────────────────────────────",
        tail(&tui.text(), 40)
    );
    tui.quit();
}

/// Switching between sessions must work by number, by arrow, and by Tab.
///
/// All three are advertised in the help; advertised is not the same as
/// working, which is the lesson of everything else in this file.
#[test]
fn sessions_can_be_switched_by_number_arrow_and_tab() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(25));

    // Second session. From inside a shell this needs the prefix — a bare `n`
    // is the shell's.
    tui.send(b"\x01n");
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-02");
    std::thread::sleep(Duration::from_millis(400));
    tui.send(b"\r");
    tui.expect("web-02", Duration::from_secs(30));
    tui.expect("F2 monitor", Duration::from_secs(25));
    std::thread::sleep(Duration::from_millis(600));

    // The tab bar brackets the active session, so `[1]` proves we are on it.
    // With two sessions open, the strip must say how to move between them —
    // otherwise the only way to learn it is to ask.
    tui.expect("prev/next", Duration::from_secs(6));

    tui.send(b"\x011"); // prefix, then 1 — still supported
    let started = Instant::now();
    let mut on_one = false;
    while !on_one && started.elapsed() < Duration::from_secs(8) {
        on_one = tui.text().contains("[1] web-01");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(on_one, "prefix+1 did not switch:\n{}", tail(&tui.text(), 6));

    tui.send(b"\x1b[19~"); // F8: next session, no prefix
    let started = Instant::now();
    let mut moved = false;
    while !moved && started.elapsed() < Duration::from_secs(8) {
        moved = tui.text().contains("[2] web-02");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        moved,
        "prefix+Right did not switch:\n{}",
        tail(&tui.text(), 6)
    );

    tui.send(b"\x01\t"); // prefix, then Tab — back to the last session
    let started = Instant::now();
    let mut back = false;
    while !back && started.elapsed() < Duration::from_secs(8) {
        back = tui.text().contains("[1] web-01");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        back,
        "prefix+Tab did not switch back:\n{}",
        tail(&tui.text(), 6)
    );
    tui.quit();
}

/// The Sessions tab must have a visible cursor, move it, and attach on Enter.
///
/// It had none of the three: the table configured a highlight but was drawn
/// without state so nothing was ever selected, the arrows drove the *hosts*
/// cursor on a list that was not on screen, and Enter opened a new connection
/// instead of returning to the highlighted session.
#[test]
fn the_sessions_tab_has_a_working_cursor() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(25));

    // A second session, so there is something to move between.
    tui.send(b"\x01n");
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-02");
    std::thread::sleep(Duration::from_millis(400));
    tui.send(b"\r");
    tui.expect("web-02", Duration::from_secs(30));
    tui.expect("F2 monitor", Duration::from_secs(25));

    // Dashboard, Sessions tab.
    tui.send(b"\x1b[17~"); // F6 detach
    tui.expect("Hosts", Duration::from_secs(15));
    tui.send(b"1");
    tui.expect("sessions", Duration::from_secs(8));

    // The cursor must be drawn.
    tui.expect("▌", Duration::from_secs(6));

    // And it must move: with the cursor on row 1, Down puts it on row 2.
    let before = tui.text();
    tui.send(b"\x1b[B");
    let started = Instant::now();
    let mut moved = false;
    while !moved && started.elapsed() < Duration::from_secs(6) {
        moved = tui.text() != before;
        std::thread::sleep(Duration::from_millis(80));
    }
    assert!(
        moved,
        "the Sessions cursor did not move:\n{}",
        tail(&tui.text(), 14)
    );

    // Enter attaches to the highlighted session rather than dialling a host.
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(15));
    tui.quit();
}

/// F10 must open the command menu with one press, from inside a session.
///
/// It was only on `Ctrl+P` — which is readline's previous-history, so we were
/// stealing a shell key — and it appeared on no hint strip at all, so there
/// was no way to discover it.
#[test]
fn f10_opens_the_command_menu() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(25));
    // The strip advertises it.
    tui.expect("F10 menu", Duration::from_secs(10));

    tui.send(b"\x1b[21~"); // F10
    tui.expect("Dashboard: Fleet", Duration::from_secs(15));
    tui.send(b"\x1b"); // close
    std::thread::sleep(Duration::from_millis(600));

    // The prefix route must work too — the strip advertises it, and a
    // terminal that swallows function keys still needs a way in.
    tui.send(b"\x01k");
    tui.expect("Dashboard: Fleet", Duration::from_secs(10));

    // And it must stay open. A menu that dismisses itself while you are
    // reading it is worse than one that never opened.
    for _ in 0..8 {
        std::thread::sleep(Duration::from_millis(1000));
        assert!(
            tui.text().contains("Dashboard: Fleet"),
            "the command menu closed on its own:\n{}",
            tail(&tui.text(), 12)
        );
    }
    tui.quit();
}

/// The monitor must have data the moment it opens.
///
/// Sampling used to run inline in the draw loop and only while the monitor
/// was visible, so opening it always showed "waiting for first sample" and
/// the UI stalled for a round trip every two seconds.
#[test]
fn the_monitor_has_data_the_moment_it_opens() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }
    let home = fixture_home();
    let mut tui = Tui::start(&home);
    tui.expect("connect", Duration::from_secs(10));
    tui.type_text("web-01");
    tui.send(b"\r");
    tui.expect("F2 monitor", Duration::from_secs(25));

    // Give the background sampler a couple of intervals while we sit in the
    // shell — which is the whole point: it samples when nobody is looking.
    std::thread::sleep(Duration::from_secs(6));

    tui.send(b"\x1bOQ"); // F2
    tui.expect("processes", Duration::from_secs(10));
    // Warm data, not the empty state.
    let started = Instant::now();
    let mut warm = false;
    while !warm && started.elapsed() < Duration::from_secs(4) {
        warm = !tui.text().contains("waiting for first sample");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        warm,
        "the monitor opened empty — background sampling is not running:\n{}",
        tail(&tui.text(), 20)
    );
    tui.quit();
}

// ── diagnostics ─────────────────────────────────────────────────────────────

/// Press every advertised binding and print the screen it produces.
///
/// Not an assertion — a way to see, in one run, what each key actually does.
/// `cargo test --release --test tui_harness dump_every_command -- --ignored
/// --nocapture`
#[test]
#[ignore]
fn dump_every_command() {
    if !demo_hosts_available() && skip("demo containers not running on :2201") {
        return;
    }

    // (label, bytes to press, bytes to get back to the session)
    let commands: &[(&str, &[u8], &[u8])] = &[
        ("F1 help", b"\x1bOP", b"\x1b"),
        ("F2 monitor", b"\x1bOQ", b"\x1b"),
        ("F3 files", b"\x1bOR", b"\x1b"),
        ("F4 forwards", b"\x1bOS", b"\x1b"),
        ("F5 mini", b"\x1b[15~", b"\x1b[15~"),
        ("F6 detach", b"\x1b[17~", b""),
        ("prefix s (pane split)", b"\x01s", b""),
        ("prefix M (mini)", b"\x01M", b"\x01M"),
        ("prefix [ (resize)", b"\x01[", b""),
    ];

    for (label, press, back) in commands {
        let home = fixture_home();
        let mut tui = Tui::start(&home);
        tui.expect("connect", Duration::from_secs(10));
        tui.type_text("web-01");
        tui.send(b"\r");
        tui.expect("F2 monitor", Duration::from_secs(25));
        // Give the shell a moment so its banner is not mid-flight.
        std::thread::sleep(Duration::from_millis(800));

        let before = tui.text();
        tui.send(press);
        std::thread::sleep(Duration::from_millis(2500));
        let after = tui.text();

        println!("\n════ {label} ════");
        println!(
            "changed: {}",
            if before == after {
                "NO — nothing happened"
            } else {
                "yes"
            }
        );
        for line in after.lines() {
            let t = line.trim_end();
            if !t.is_empty() {
                println!("│ {t}");
            }
        }
        if !back.is_empty() {
            tui.send(back);
        }
        tui.quit();
    }
}
