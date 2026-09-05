// Input path, split in two (SYSTEM.md §12.6, borrowed from orbital):
//
//   KeyEvent      scancode-level — Esc / Tab / arrows / Ctrl+letter. What
//                 key was pressed, not what text it makes.
//   TextInputEvent  composed text — the letter/symbol page and, from M18,
//                 voice ASR / a future IME. Anything that *types*.
//
// The on-screen keyboard, a physical keyboard, and voice all funnel into
// the same InputEvent enum; byte encoding happens only here, at the
// terminal layer, because ESC [ A vs ESC O A is a terminal-mode concern
// (DECCKM), not a keyboard concern. M18's PTT path calls inject() in
// main.rs with TextInputEvent — no second input path gets invented.
//
// Physical-key shape (inputd): evdev_key() maps a real keyboard's codes
// to the same KeyEvents. Not wired — redfin's only physical keys
// (power/volume on qpnp_pon) are device control, not text input.

/// Terminal-layer byte encoding. `app_cursor` is the child's DECCKM state
/// (?1): SS3 arrows when set, CSI arrows otherwise.
pub fn encode(ev: &InputEvent, app_cursor: bool) -> Vec<u8> {
    match ev {
        InputEvent::Text(s) => s.as_bytes().to_vec(),
        InputEvent::Key(k) => match k {
            KeyEvent::Esc => vec![0x1b],
            KeyEvent::Tab => vec![b'\t'],
            KeyEvent::Enter => vec![b'\r'],
            KeyEvent::Backspace => vec![0x7f],
            KeyEvent::Arrow(d) => {
                let c = match d {
                    Dir::Up => b'A',
                    Dir::Down => b'B',
                    Dir::Right => b'C',
                    Dir::Left => b'D',
                };
                if app_cursor {
                    vec![0x1b, b'O', c]
                } else {
                    vec![0x1b, b'[', c]
                }
            }
            KeyEvent::Ctrl(c) => vec![c.to_ascii_lowercase() as u8 & 0x1f],
        },
    }
}

/// Hold-to-repeat applies to these (Termux repetitive keys: DEL + arrows).
pub fn repeatable(ev: &InputEvent) -> bool {
    matches!(
        ev,
        InputEvent::Key(KeyEvent::Backspace) | InputEvent::Key(KeyEvent::Arrow(_))
    )
}

/// Evdev codes → KeyEvent for a physical keyboard. Arrows only for now —
/// the vocabulary matches the on-screen key table; letters would need
/// modifier state, which a real inputd would track on top of this map.
/// Unused until something plugs a keyboard in.
#[allow(dead_code)]
pub fn evdev_key(code: u16) -> Option<KeyEvent> {
    use Dir::*;
    use KeyEvent::*;
    Some(match code {
        1 => Esc,
        14 => Backspace,
        15 => Tab,
        28 => Enter,
        103 => Arrow(Up),
        105 => Arrow(Left),
        106 => Arrow(Right),
        108 => Arrow(Down),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    Esc,
    Tab,
    Enter,
    Backspace,
    Arrow(Dir),
    /// Ctrl + letter (the ctl one-shot + letter keycap composes this).
    Ctrl(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Key(KeyEvent),
    /// Composed text. Empty = a modifier toggle: the tap was consumed, the
    /// keyboard state changed, nothing gets written.
    Text(String),
}
