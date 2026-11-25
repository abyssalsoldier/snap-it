//! Caret and selection for typing into a text or balloon annotation, shared by the
//! editor and the overlay's quick edit so both behave like a Windows edit box:
//! click / drag / Shift to select, double-click a word, arrows, Ctrl+A/C/X/V, and
//! step-by-step Ctrl+Z / Ctrl+Y inside the box ([`TextUndo`]).

use snapit_annotate::{CaretMove, Item};

/// Byte indices into the item's text. `anchor == caret` means nothing is selected.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct TextCaret {
    pub caret: usize,
    pub anchor: usize,
}

/// What a typed character did.
#[derive(PartialEq, Debug)]
pub enum Typed {
    Edited,
    /// Ctrl+Enter: the caller finishes editing.
    Commit,
    Ignored,
}

/// How an edit groups with the one before it for undo.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    /// Typing a character: a run of these is one step, a space starts a new one.
    Type,
    /// Backspace / Delete one character: a run of these is one step.
    Delete,
    /// Paste, cut, Enter, or replacing a selection: always its own step.
    Other,
}

/// Undo history for the text box being edited. Lives only while it is open; once it
/// is empty, Ctrl+Z falls back to the window's undo (which removes the whole edit).
#[derive(Default, Debug)]
pub struct TextUndo {
    undo: Vec<(String, TextCaret)>,
    redo: Vec<(String, TextCaret)>,
    /// The last edit's kind and the caret it left, while typing can still join it.
    last: Option<(Kind, TextCaret)>,
}

const MAX_STEPS: usize = 500;

impl TextUndo {
    /// Call before changing the text: saves it unless this edit continues the last step.
    fn begin(&mut self, it: &Item, c: TextCaret, kind: Kind, ch: Option<char>) {
        let joins = kind != Kind::Other
            && c.caret == c.anchor
            && self.last == Some((kind, c))
            && !ch.is_some_and(char::is_whitespace);
        if !joins {
            self.undo.push((it.text().unwrap_or_default().to_string(), c));
            if self.undo.len() > MAX_STEPS {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last = Some((kind, c));
    }

    /// Call after the change, with the caret it left.
    fn end(&mut self, c: TextCaret) {
        if let Some((k, _)) = self.last {
            self.last = Some((k, c));
        }
    }

    fn swap(from: &mut Vec<(String, TextCaret)>, to: &mut Vec<(String, TextCaret)>, it: &mut Item, c: &mut TextCaret) -> bool {
        let Some((text, caret)) = from.pop() else { return false };
        let Some(t) = it.text_mut() else { return false };
        to.push((std::mem::replace(t, text), *c));
        *c = caret;
        true
    }

    /// Steps back one edit. False when there is nothing left to undo in this box.
    pub fn undo(&mut self, it: &mut Item, c: &mut TextCaret) -> bool {
        self.last = None;
        Self::swap(&mut self.undo, &mut self.redo, it, c)
    }

    /// Re-applies an undone edit. False when there is nothing to redo in this box.
    pub fn redo(&mut self, it: &mut Item, c: &mut TextCaret) -> bool {
        self.last = None;
        Self::swap(&mut self.redo, &mut self.undo, it, c)
    }
}

impl TextCaret {
    pub fn at(i: usize) -> Self {
        TextCaret { caret: i, anchor: i }
    }

    pub fn range(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    /// Moves the caret to `i`; `extend` (Shift, or dragging) keeps the anchor.
    pub fn place(&mut self, i: usize, extend: bool) {
        self.caret = i;
        if !extend {
            self.anchor = i;
        }
    }

    fn delete_selection(&mut self, it: &mut Item) -> bool {
        if self.caret == self.anchor {
            return false;
        }
        *self = TextCaret::at(it.delete_range(self.caret, self.anchor));
        true
    }

    /// Types or pastes `s`, replacing the selection.
    fn insert(&mut self, it: &mut Item, s: &str) {
        self.delete_selection(it);
        let mut c = self.caret;
        it.insert_at_caret(&mut c, s);
        *self = TextCaret::at(c);
    }

    /// Backspace (`forward == false`) or Delete: the selection, else one character.
    /// False when there was nothing to delete.
    fn delete(&mut self, it: &mut Item, forward: bool) -> bool {
        if self.delete_selection(it) {
            return true;
        }
        let mut c = self.caret;
        let did = it.delete_at_caret(&mut c, forward);
        *self = TextCaret::at(c);
        did
    }

    fn deletes_something(&self, it: &Item, forward: bool) -> bool {
        let t = it.text().unwrap_or_default();
        self.caret != self.anchor || if forward { self.caret < t.len() } else { self.caret > 0 }
    }

    pub fn selected<'a>(&self, it: &'a Item) -> Option<&'a str> {
        let (lo, hi) = self.range();
        it.text().and_then(|t| t.get(lo..hi)).filter(|s| !s.is_empty())
    }

    /// A WM_CHAR while editing.
    pub fn char_input(&mut self, it: &mut Item, u: &mut TextUndo, c: u32) -> Typed {
        match c {
            0x08 => {
                if !self.deletes_something(it, false) {
                    return Typed::Ignored;
                }
                let kind = if self.caret == self.anchor { Kind::Delete } else { Kind::Other };
                u.begin(it, *self, kind, None);
                self.delete(it, false);
            }
            // Enter starts a new line; Ctrl+Enter (sent as LF) finishes, like Esc.
            0x0D => {
                u.begin(it, *self, Kind::Other, None);
                self.insert(it, "\n");
            }
            0x0A => return Typed::Commit,
            // Ctrl+Backspace (sent as DEL): delete the word before the caret, as in Windows.
            0x7F => {
                if !self.deletes_something(it, false) {
                    return Typed::Ignored;
                }
                u.begin(it, *self, Kind::Other, None);
                if self.caret == self.anchor {
                    self.anchor = word_start_before(it.text().unwrap_or_default(), self.caret);
                }
                self.delete(it, false);
            }
            // Other control characters (C1 range) never belong in the text.
            0x80..=0x9F => return Typed::Ignored,
            c if c >= 0x20 => {
                let Some(ch) = char::from_u32(c) else { return Typed::Ignored };
                let kind = if self.caret == self.anchor { Kind::Type } else { Kind::Other };
                u.begin(it, *self, kind, Some(ch));
                self.insert(it, ch.encode_utf8(&mut [0; 4]));
            }
            _ => return Typed::Ignored,
        }
        u.end(*self);
        Typed::Edited
    }

    /// A WM_KEYDOWN while editing (Esc and Ctrl+Z / Ctrl+Y are the caller's).
    /// Returns true when the text changed.
    pub fn key_down(&mut self, it: &mut Item, u: &mut TextUndo, vk: u16, ctrl: bool, shift: bool) -> bool {
        let m = match vk {
            0x25 => Some(CaretMove::Left),
            0x27 => Some(CaretMove::Right),
            0x26 => Some(CaretMove::Up),
            0x28 => Some(CaretMove::Down),
            0x24 => Some(CaretMove::Home),
            0x23 => Some(CaretMove::End),
            _ => None,
        };
        if let Some(m) = m {
            let (lo, hi) = self.range();
            let to = match m {
                // Left / Right with a selection just collapse it to that side.
                CaretMove::Left if !shift && lo != hi => lo,
                CaretMove::Right if !shift && lo != hi => hi,
                _ => it.move_caret(self.caret, m),
            };
            self.place(to, shift);
            return false;
        }
        let changed = match vk {
            0x2E => {
                if !self.deletes_something(it, true) {
                    return false;
                }
                let kind = if self.caret == self.anchor { Kind::Delete } else { Kind::Other };
                u.begin(it, *self, kind, None);
                self.delete(it, true)
            }
            0x41 if ctrl => {
                let len = it.text().map_or(0, str::len);
                *self = TextCaret { caret: len, anchor: 0 };
                false
            }
            0x43 | 0x58 if ctrl => {
                let Some(s) = self.selected(it) else { return false };
                let _ = crate::clipboard::set_text(&s.replace('\n', "\r\n"));
                if vk != 0x58 {
                    return false;
                }
                u.begin(it, *self, Kind::Other, None);
                self.delete_selection(it)
            }
            0x56 if ctrl => {
                let Some(t) = crate::clipboard::get_text() else { return false };
                u.begin(it, *self, Kind::Other, None);
                self.insert(it, &t.replace("\r\n", "\n"));
                true
            }
            _ => return false,
        };
        u.end(*self);
        changed
    }
}

/// Where Ctrl+Backspace at byte index `i` deletes back to: over any spaces, then over one
/// word (or run of punctuation). Right after a line break it removes just the break.
fn word_start_before(t: &str, i: usize) -> usize {
    let i = i.min(t.len());
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut start = i;
    let mut chars = t[..i].char_indices().rev().peekable();
    while let Some(&(b, c)) = chars.peek() {
        if c == '\n' || !c.is_whitespace() {
            break;
        }
        start = b;
        chars.next();
    }
    match chars.peek() {
        Some(&(b, '\n')) if start == i => b,
        Some(&(_, c0)) if c0 != '\n' => {
            let k = word(c0);
            for (b, c) in chars {
                if c.is_whitespace() || word(c) != k {
                    break;
                }
                start = b;
            }
            start
        }
        _ => start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapit_annotate::{Style, Tool};
    use snapit_gfx::{PointF, Rgba};

    fn item(s: &str) -> Item {
        let mut it = Item::begin(Tool::Text, PointF::new(0.0, 0.0), Style::default(), &[], Rgba::WHITE).unwrap();
        it.text_mut().unwrap().push_str(s);
        it
    }

    fn type_str(c: &mut TextCaret, it: &mut Item, u: &mut TextUndo, s: &str) {
        for ch in s.chars() {
            c.char_input(it, u, ch as u32);
        }
    }

    #[test]
    fn typing_replaces_the_selection_and_keys_extend_it() {
        let mut it = item("Hello world");
        let mut u = TextUndo::default();
        let mut c = TextCaret::at(0);
        c.place(5, true); // drag over "Hello"
        assert_eq!(c.selected(&it), Some("Hello"));
        assert_eq!(c.char_input(&mut it, &mut u, 'J' as u32), Typed::Edited);
        assert_eq!((it.text().unwrap(), c), ("J world", TextCaret::at(1)));
        c.key_down(&mut it, &mut u, 0x27, false, true); // Shift+Right
        c.key_down(&mut it, &mut u, 0x27, false, true);
        assert_eq!(c.selected(&it), Some(" w"));
        c.key_down(&mut it, &mut u, 0x25, false, false); // Left collapses to the start
        assert_eq!(c, TextCaret::at(1));
        c.key_down(&mut it, &mut u, 0x41, true, false); // Ctrl+A
        assert_eq!(c.selected(&it), Some("J world"));
        assert_eq!(c.char_input(&mut it, &mut u, 0x08), Typed::Edited);
        assert_eq!((it.text().unwrap(), c), ("", TextCaret::at(0)));
        assert_eq!(c.char_input(&mut it, &mut u, 0x0D), Typed::Edited);
        assert_eq!(it.text().unwrap(), "\n");
        assert_eq!(c.char_input(&mut it, &mut u, 0x0A), Typed::Commit);
    }

    #[test]
    fn undo_steps_back_through_words_deletes_and_line_breaks() {
        let mut it = item("");
        let mut u = TextUndo::default();
        let mut c = TextCaret::at(0);
        type_str(&mut c, &mut it, &mut u, "Ship it");
        c.char_input(&mut it, &mut u, 0x0D); // Enter
        type_str(&mut c, &mut it, &mut u, "now");
        c.char_input(&mut it, &mut u, 0x08);
        c.char_input(&mut it, &mut u, 0x08); // one Backspace run
        assert_eq!(it.text().unwrap(), "Ship it\nn");

        let steps = ["Ship it\nnow", "Ship it\n", "Ship it", "Ship", ""];
        for want in steps {
            assert!(u.undo(&mut it, &mut c));
            assert_eq!(it.text().unwrap(), want);
            assert_eq!(c, TextCaret::at(want.len()), "caret after undo to {want:?}");
        }
        assert!(!u.undo(&mut it, &mut c), "nothing left: the window's undo takes over");

        for want in steps.iter().rev().skip(1).chain(std::iter::once(&"Ship it\nn")) {
            assert!(u.redo(&mut it, &mut c));
            assert_eq!(it.text().unwrap(), *want);
        }
        assert!(!u.redo(&mut it, &mut c));

        // A new edit after undoing drops the redo steps.
        u.undo(&mut it, &mut c);
        type_str(&mut c, &mut it, &mut u, "!");
        assert!(!u.redo(&mut it, &mut c));
    }

    #[test]
    fn ctrl_backspace_deletes_the_word_before_the_caret() {
        let mut it = item("Ship it, now  \nnext");
        let mut u = TextUndo::default();
        let len = it.text().unwrap().len();
        let mut c = TextCaret::at(len);
        let steps = ["Ship it, now  \n", "Ship it, now  ", "Ship it, ", "Ship it", "Ship ", ""];
        for want in steps {
            assert_eq!(c.char_input(&mut it, &mut u, 0x7F), Typed::Edited);
            assert_eq!((it.text().unwrap(), c), (want, TextCaret::at(want.len())));
        }
        assert_eq!(c.char_input(&mut it, &mut u, 0x7F), Typed::Ignored);
        assert!(u.undo(&mut it, &mut c));
        assert_eq!(it.text().unwrap(), "Ship ");
        // Never inserts the DEL character, nor C1 control characters.
        assert_eq!(c.char_input(&mut it, &mut u, 0x85), Typed::Ignored);
        assert_eq!(it.text().unwrap(), "Ship ");
        // With a selection it deletes just the selection.
        let mut it = item("Hello world");
        let mut c = TextCaret { caret: 2, anchor: 4 };
        c.char_input(&mut it, &mut u, 0x7F);
        assert_eq!(it.text().unwrap(), "Heo world");
    }

    #[test]
    fn moving_the_caret_starts_a_new_undo_step() {
        let mut it = item("");
        let mut u = TextUndo::default();
        let mut c = TextCaret::at(0);
        type_str(&mut c, &mut it, &mut u, "ab");
        c.key_down(&mut it, &mut u, 0x24, false, false); // Home
        type_str(&mut c, &mut it, &mut u, "xy");
        assert_eq!(it.text().unwrap(), "xyab");
        assert!(u.undo(&mut it, &mut c));
        assert_eq!((it.text().unwrap(), c), ("ab", TextCaret::at(0)));
        // Backspace at the very start changes nothing and records no step.
        assert_eq!(c.char_input(&mut it, &mut u, 0x08), Typed::Ignored);
        assert!(u.undo(&mut it, &mut c));
        assert_eq!(it.text().unwrap(), "");
    }
}
