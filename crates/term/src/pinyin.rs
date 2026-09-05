// pinyin — the M40 IME engine: single-syllable pinyin → hanzi candidates.
//
// The data is ../data/pinyin.tsv (scripts/gen-pinyin-table.sh derives it
// on the host from mozillazg/pinyin-data readings + the Jun Da frequency
// list, restricted to GB2312 = the font's coverage): one line per
// syllable, candidates pre-ranked, top 12. 413 syllables ≈ 14 KB, embedded
// via include_bytes! — no runtime file, the 拼 key works in every boot.
//
// Scope is deliberately V1: char-level, one syllable at a time (type "ni",
// tap 你, buffer clears, type the next syllable). No word segmentation, no
// fuzzy match, no tone input. The one keyboard convention that IS kept:
// ü is typed "v" (nv 女, lv 绿) — the table keys are already in that form.
//
// Wiring (main.rs): the 拼 key toggles Kb::pinyin; while it's on and a
// terminal runs, key events come here FIRST — letters build the buffer,
// backspace edits it, space/enter commit the top candidate (or the raw
// letters when nothing matches — the IME never traps you), a candidate
// tap commits that hanzi. Commits leave through the same inject() every
// other input uses; anything this engine doesn't care about returns Pass
// and flows through unchanged.

use crate::input::{InputEvent, KeyEvent};
use std::collections::HashMap;
use std::sync::OnceLock;

const TABLE: &[u8] = include_bytes!("../data/pinyin.tsv");

/// Candidates for a toneless syllable, most frequent first. Empty slice
/// for an unknown syllable (or the empty buffer).
pub fn lookup(syl: &str) -> &'static [char] {
    static MAP: OnceLock<HashMap<&'static str, Vec<char>>> = OnceLock::new();
    let m = MAP.get_or_init(|| {
        let text = std::str::from_utf8(TABLE).expect("pinyin.tsv is utf-8");
        text.lines()
            .filter_map(|l| {
                let (syl, chars) = l.split_once('\t')?;
                Some((
                    // leak: the table is a forever-static ~14 KB of keys
                    Box::leak(syl.to_string().into_boxed_str()) as &'static str,
                    chars.chars().collect::<Vec<char>>(),
                ))
            })
            .collect()
    });
    m.get(syl).map(|v| &v[..]).unwrap_or(&[])
}

/// How the strip pages: 6 candidates visible at a time (12 in the table).
pub const PAGE: usize = 6;
/// Longest syllable ("zhuang") — letters beyond this are dropped.
const MAX_BUF: usize = 6;

/// What feed() decided to do with one input event.
pub enum Outcome {
    /// Buffer/page changed — repaint the strip, write nothing.
    Consumed,
    /// Commit this string to the pty.
    Commit(String),
    /// Not this engine's business — the caller injects the event verbatim.
    Pass,
}

#[derive(Default)]
pub struct Ime {
    /// typed letters (a-z); empty = idle
    pub buf: String,
    page: usize,
}

impl Ime {
    pub fn new() -> Ime {
        Ime::default()
    }

    pub fn candidates(&self) -> &'static [char] {
        lookup(&self.buf)
    }

    /// Candidate at strip slot `i` (0-based within the visible page).
    pub fn page_candidate(&self, i: usize) -> Option<char> {
        self.candidates().get(self.page * PAGE + i).copied()
    }

    pub fn next_page(&mut self) {
        let len = self.candidates().len();
        if len > 0 {
            let pages = (len + PAGE - 1) / PAGE;
            self.page = (self.page + 1) % pages;
        } else {
            self.page = 0;
        }
    }

    /// Candidate at strip slot `i` commits NOW: return it and empty the
    /// buffer so the next syllable starts clean (a tap on the strip is a
    /// commit just like space/enter — take_commit's sibling).
    pub fn take_candidate(&mut self, i: usize) -> Option<char> {
        let ch = self.page_candidate(i)?;
        self.clear();
        Some(ch)
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.page = 0;
    }

    /// One input event under IME rules. Call only while the IME is on and
    /// a terminal is running; everything else behaves as a plain keyboard.
    pub fn feed(&mut self, ev: &InputEvent) -> Outcome {
        match ev {
            InputEvent::Text(s) => {
                if s.len() == 1 {
                    let ch = s.chars().next().unwrap();
                    if ch.is_ascii_lowercase() && self.buf.len() < MAX_BUF {
                        self.buf.push(ch);
                        self.page = 0;
                        return Outcome::Consumed;
                    }
                }
                match s.as_str() {
                    // commit top candidate — or the raw letters when the
                    // buffer matches nothing, so the mode never traps you
                    " " | "\n" if !self.buf.is_empty() => Outcome::Commit(self.take_commit()),
                    _ => Outcome::Pass,
                }
            }
            InputEvent::Key(KeyEvent::Backspace) => {
                if self.buf.pop().is_some() {
                    self.page = 0;
                    Outcome::Consumed
                } else {
                    Outcome::Pass
                }
            }
            // Esc clears the buffer instead of reaching the child — one
            // press to back out of a half-typed syllable
            InputEvent::Key(KeyEvent::Esc) if !self.buf.is_empty() => {
                self.clear();
                Outcome::Consumed
            }
            InputEvent::Key(KeyEvent::Enter) if !self.buf.is_empty() => {
                Outcome::Commit(self.take_commit())
            }
            _ => Outcome::Pass,
        }
    }

    /// The commit string for space/enter: top candidate, or the raw
    /// letters when nothing matches. Empties the buffer.
    fn take_commit(&mut self) -> String {
        let out = match self.candidates().first() {
            Some(&ch) => ch.to_string(),
            None => std::mem::take(&mut self.buf),
        };
        self.clear();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_well_formed() {
        let text = std::str::from_utf8(TABLE).unwrap();
        let mut n = 0;
        for line in text.lines() {
            let (syl, chars) = line.split_once('\t').expect("tab");
            assert!(
                !syl.is_empty() && syl.chars().all(|c| c.is_ascii_lowercase()),
                "bad syllable {syl:?}"
            );
            assert!(syl.len() <= MAX_BUF, "syllable too long: {syl}");
            let n_chars = chars.chars().count();
            assert!(!chars.is_empty() && n_chars > 0 && n_chars <= 12, "empty/long row {syl}");
            let mut seen: Vec<char> = chars.chars().collect();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), n_chars, "dup char in {syl}");
            n += 1;
        }
        assert!(n > 400, "suspiciously small table: {n} syllables");
    }

    #[test]
    fn common_syllables_rank_sensibly() {
        // Jun Da ordering surfaced through the table
        assert_eq!(lookup("ni").first(), Some(&'你'));
        assert_eq!(lookup("de").first(), Some(&'的'));
        assert_eq!(lookup("shi").first(), Some(&'是'));
        assert_eq!(lookup("nv").first(), Some(&'女')); // ü typed as v
        assert_eq!(lookup("zhuang").first(), Some(&'装'));
    }

    #[test]
    fn unknown_and_empty_lookup() {
        assert!(lookup("").is_empty());
        assert!(lookup("zhongg").is_empty());
        assert!(lookup("QQ").is_empty());
    }

    fn text(s: &str) -> InputEvent {
        InputEvent::Text(s.into())
    }

    #[test]
    fn feed_types_commits_and_edits() {
        let mut ime = Ime::new();
        assert!(matches!(ime.feed(&text("n")), Outcome::Consumed));
        assert!(matches!(ime.feed(&text("i")), Outcome::Consumed));
        assert_eq!(ime.buf, "ni");
        // space with a matching buffer commits the top candidate
        assert!(matches!(ime.feed(&text(" ")), Outcome::Commit(s) if s == "你"));
        assert!(ime.buf.is_empty());
        // backspace on empty buffer passes through to the terminal
        assert!(matches!(
            ime.feed(&InputEvent::Key(KeyEvent::Backspace)),
            Outcome::Pass
        ));
        // no match: space commits the raw letters (the escape hatch)
        ime.feed(&text("q")); // "q" alone is not a syllable
        assert!(matches!(ime.feed(&text(" ")), Outcome::Commit(s) if s == "q"));
        // esc drops a half-typed syllable
        ime.feed(&text("z"));
        ime.feed(&text("h"));
        assert!(matches!(ime.feed(&InputEvent::Key(KeyEvent::Esc)), Outcome::Consumed));
        assert!(ime.buf.is_empty());
        // enter commits like space; with an empty buffer it passes through
        assert!(matches!(ime.feed(&InputEvent::Key(KeyEvent::Enter)), Outcome::Pass));
    }

    #[test]
    fn non_letters_pass_through() {
        let mut ime = Ime::new();
        ime.feed(&text("n"));
        for s in ["A", "-", "/", "!", "。", ""] {
            assert!(matches!(ime.feed(&text(s)), Outcome::Pass), "{s:?}");
        }
        // arrows and ctrl chords are not IME business
        assert!(matches!(
            ime.feed(&InputEvent::Key(KeyEvent::Arrow(crate::input::Dir::Left))),
            Outcome::Pass
        ));
        assert_eq!(ime.buf, "n"); // untouched
    }

    #[test]
    fn candidate_tap_commits_and_empties() {
        // regression (2026-09-03 device session): the strip tap path used
        // page_candidate() and never cleared — after tapping one hanzi the
        // buffer kept "ni" and the next syllable could never form
        let mut ime = Ime::new();
        ime.feed(&text("n"));
        ime.feed(&text("i"));
        assert_eq!(ime.take_candidate(1), Some('呢'));
        assert!(ime.buf.is_empty());
        assert_eq!(ime.take_candidate(0), None); // empty buffer, no strip commit
        // the next syllable starts clean and matches again
        ime.feed(&text("h"));
        ime.feed(&text("a"));
        ime.feed(&text("o"));
        assert_eq!(ime.take_candidate(0), Some('好'));
    }

    #[test]
    fn paging_walks_the_candidate_list() {
        let mut ime = Ime::new();
        for c in "zhuang".chars() {
            ime.feed(&text(&c.to_string()));
        }
        let all = ime.candidates().to_vec();
        assert!(all.len() > PAGE, "zhuang should have 2 pages");
        let first_page: Vec<char> = (0..PAGE).filter_map(|i| ime.page_candidate(i)).collect();
        assert_eq!(&first_page[..], &all[..PAGE]);
        ime.next_page();
        let second: Vec<char> = (0..PAGE).filter_map(|i| ime.page_candidate(i)).collect();
        assert_eq!(&second[..], &all[PAGE..]);
        ime.next_page(); // wraps back to page 0
        assert_eq!(ime.page_candidate(0), Some(all[0]));
        // typing resets the page (backspace also edits the syllable —
        // "zhuang" becomes "zhuan", so compare against that row)
        ime.next_page();
        ime.feed(&InputEvent::Key(KeyEvent::Backspace));
        assert_eq!(ime.buf, "zhuan");
        assert_eq!(ime.page_candidate(0), Some(lookup("zhuan")[0]));
    }
}
