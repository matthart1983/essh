use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

const META_PREFIX_TIMEOUT: Duration = Duration::from_millis(30);

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Tick,
    Resize(u16, u16),
}

/// Whether the reader thread should keep its hands off stdin.
///
/// The reader owns stdin for the life of the program. Anything else that needs
/// the terminal — a password prompt, `$EDITOR` — is therefore in a race it
/// always loses: the reader consumes the keystrokes and the prompt blocks
/// forever, which presents as the app hanging on a blank screen.
///
/// A static rather than a field because the code that needs to suspend input
/// sits deep inside the connect path, and threading a handle down to it would
/// mean changing every signature in between for a flag two call sites use.
static INPUT_PAUSED: AtomicBool = AtomicBool::new(false);

/// Set when something else has had the terminal and the screen must be
/// repainted in full.
///
/// ratatui draws by diffing against its own copy of what is on screen. Leave
/// the alternate screen for a password prompt and come back, and that copy is
/// a lie: the real terminal was cleared, but ratatui still believes the old
/// frame is there, so it writes only the cells it thinks changed. The result
/// is a screen with characters missing — `mattbo @192.168.0.54` instead of
/// `mattbot@192.168.0.54`, which sends the reader hunting for a host that
/// does not exist.
static NEEDS_FULL_REDRAW: AtomicBool = AtomicBool::new(false);

/// Whether the next frame must be drawn from scratch. Clears the flag.
pub fn take_needs_full_redraw() -> bool {
    NEEDS_FULL_REDRAW.swap(false, Ordering::SeqCst)
}

/// Stop reading stdin and return a guard that resumes on drop.
///
/// Give the reader a moment to fall out of its current `poll` before taking
/// the terminal, or the first keystroke still goes to the wrong place.
pub fn pause_input() -> InputPause {
    INPUT_PAUSED.store(true, Ordering::SeqCst);
    // Let the reader fall out of its current poll before taking the terminal.
    std::thread::sleep(Duration::from_millis(150));

    // Then throw away anything already queued.
    //
    // Pausing stops *future* reads; it does nothing about bytes the reader
    // has already buffered, or about keystrokes that arrive in the gap
    // between the prompt being printed and the reader being told to stop.
    // Those leak into whatever screen comes back afterwards — and, worse,
    // are missing from the front of the password, so the server rejects a
    // password the user typed correctly.
    while event::poll(Duration::from_millis(0)).unwrap_or(false) {
        if event::read().is_err() {
            break;
        }
    }
    InputPause(())
}

/// Resumes input when dropped, including on an early return or a panic —
/// leaving input paused would wedge the whole UI.
pub struct InputPause(());

impl Drop for InputPause {
    fn drop(&mut self) {
        INPUT_PAUSED.store(false, Ordering::SeqCst);
        // Whoever borrowed the terminal has repainted it; ratatui's idea of
        // what is on screen is now stale.
        NEEDS_FULL_REDRAW.store(true, Ordering::SeqCst);
    }
}

pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<AppEvent>,
}

impl EventHandler {
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        // Use a dedicated OS thread instead of tokio::spawn, since
        // crossterm::event::poll() is a blocking call that would tie up
        // a tokio worker thread permanently.
        std::thread::spawn(move || loop {
            // Someone else needs the terminal. Do not touch stdin, and do not
            // emit ticks that would redraw over their prompt.
            if INPUT_PAUSED.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            if event::poll(tick_rate).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(key)) => {
                        let next_event = if key.code == KeyCode::Esc && key.modifiers.is_empty() {
                            if event::poll(META_PREFIX_TIMEOUT).unwrap_or(false) {
                                event::read().ok()
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        for app_event in expand_key_event(key, next_event) {
                            if tx.send(app_event).is_err() {
                                return;
                            }
                        }
                    }
                    Ok(Event::Resize(w, h)) if tx.send(AppEvent::Resize(w, h)).is_err() => {
                        return;
                    }
                    _ => {}
                }
            } else if tx.send(AppEvent::Tick).is_err() {
                return;
            }
        });

        Self { rx }
    }

    pub async fn next(&mut self) -> Result<AppEvent> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("Event channel closed"))
    }
}

fn expand_key_event(key: KeyEvent, next_event: Option<Event>) -> Vec<AppEvent> {
    if key.code != KeyCode::Esc || !key.modifiers.is_empty() {
        return vec![AppEvent::Key(key)];
    }

    match next_event {
        Some(Event::Key(mut next_key)) if next_key.code != KeyCode::Esc => {
            next_key.modifiers |= KeyModifiers::ALT;
            vec![AppEvent::Key(next_key)]
        }
        Some(Event::Resize(w, h)) => vec![AppEvent::Key(key), AppEvent::Resize(w, h)],
        _ => vec![AppEvent::Key(key)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_key_event_converts_esc_prefixed_key_to_alt() {
        let events = expand_key_event(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            Some(Event::Key(KeyEvent::new(
                KeyCode::Char('1'),
                KeyModifiers::NONE,
            ))),
        );

        assert_eq!(events.len(), 1);
        match events[0] {
            AppEvent::Key(key) => {
                assert_eq!(key.code, KeyCode::Char('1'));
                assert!(key.modifiers.contains(KeyModifiers::ALT));
            }
            _ => panic!("expected key event"),
        }
    }

    #[test]
    fn test_expand_key_event_keeps_bare_escape() {
        let events = expand_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), None);

        assert_eq!(events.len(), 1);
        match events[0] {
            AppEvent::Key(key) => assert_eq!(key.code, KeyCode::Esc),
            _ => panic!("expected key event"),
        }
    }

    #[test]
    fn test_expand_key_event_preserves_resize_after_escape() {
        let events = expand_key_event(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            Some(Event::Resize(120, 40)),
        );

        assert_eq!(events.len(), 2);
        match events[0] {
            AppEvent::Key(key) => assert_eq!(key.code, KeyCode::Esc),
            _ => panic!("expected escape key"),
        }
        match events[1] {
            AppEvent::Resize(w, h) => {
                assert_eq!((w, h), (120, 40));
            }
            _ => panic!("expected resize event"),
        }
    }
}
