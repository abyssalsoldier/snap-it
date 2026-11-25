//! Single- and multi-line text boxes for Snap It's custom-drawn windows.
//!
//! Supports what people expect from a Windows edit box: a caret drawn where it really is,
//! selection with Shift+arrows / mouse drag / double-click (word) / Ctrl+A, clipboard on the
//! selection (Ctrl+C / X / V), undo / redo (Ctrl+Z / Y, Ctrl+Shift+Z), word jumps and word
//! delete (Ctrl+arrows, Ctrl+Backspace / Delete), Home / End, Up / Down across wrapped lines,
//! characters outside the BMP (emoji arrive as two `WM_CHAR` surrogates), and IME input
//! (`DefWindowProc` turns composition results into `WM_CHAR`; [`place_ime`] keeps the
//! composition and candidate windows at the caret).

use snapit_gfx::effects::rounded_rect;
use snapit_gfx::tiny_skia::{FillRule, Pixmap, PixmapMut, PixmapPaint, Stroke, Transform};
use snapit_gfx::{text, Rect, Rgba};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::Input::Ime::{
    ImmGetContext, ImmReleaseContext, ImmSetCandidateWindow, ImmSetCompositionWindow, CANDIDATEFORM, CFS_CANDIDATEPOS,
    CFS_POINT, COMPOSITIONFORM,
};

use super::theme::Theme;
use super::widgets::ellipsize;

/// What a key, character or click did to the box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// Not for the text box (Tab, Esc, Enter in a single-line box…): the window handles it.
    None,
    /// Handled: the caret or selection moved, or nothing visible changed.
    Moved,
    /// The text changed.
    Edited,
}

impl Change {
    pub fn handled(self) -> bool {
        self != Change::None
    }
    pub fn edited(self) -> bool {
        self == Change::Edited
    }
}

#[derive(Clone, Debug)]
struct Snap {
    value: String,
    cursor: usize,
    anchor: usize,
}

/// Where the last frame put the text, for mouse hit testing, Up / Down and the IME.
#[derive(Clone, Debug, Default)]
struct Geom {
    /// Text area (inside the padding), client coordinates.
    x: f32,
    y: f32,
    h: f32,
    fs: f32,
    /// Multi-line: line height and the wrapped lines as char ranges `[start, end)`.
    lh: f32,
    lines: Vec<(usize, usize)>,
}

const UNDO_LIMIT: usize = 200;

#[derive(Clone, Debug)]
pub struct TextInput {
    /// The text. Read it freely; change it with [`TextInput::set`] so undo and the caret follow.
    pub value: String,
    pub placeholder: &'static str,
    pub numeric: bool,
    pub multiline: bool,
    /// Caret and selection anchor, in chars (`0..=chars().count()`).
    cursor: usize,
    anchor: usize,
    /// Single-line: horizontal scroll in px. Multi-line: vertical scroll in px.
    scroll: f32,
    /// Up / Down keep this x so the caret doesn't drift across short lines.
    goal_x: Option<f32>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    /// The last edit was typing (consecutive typed characters undo as one step).
    typing: bool,
    /// First half of a UTF-16 surrogate pair from `WM_CHAR`.
    pending_high: Option<u16>,
    geom: Option<Geom>,
    drag: bool,
}

fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// Index of the char boundary nearest to `x` in `pos` (from [`text::caret_positions`]).
fn nearest(pos: &[f32], x: f32) -> usize {
    for i in 0..pos.len().saturating_sub(1) {
        if x < (pos[i] + pos[i + 1]) / 2.0 {
            return i;
        }
    }
    pos.len().saturating_sub(1)
}

/// The wrapped line the caret at `c` is drawn on. At a soft wrap the boundary belongs to
/// the next line; at a hard newline (or the end) to this one.
fn line_of(lines: &[(usize, usize)], c: usize) -> usize {
    for (i, &(s, e)) in lines.iter().enumerate() {
        let soft_next = lines.get(i + 1).is_some_and(|n| n.0 == e);
        if c >= s && (c < e || (c == e && !soft_next)) {
            return i;
        }
    }
    lines.len().saturating_sub(1)
}

/// Draws `s` with its top-left at `(x, y)`, clipped to `clip` (x, y, w, h).
fn draw_clipped(pm: &mut PixmapMut, s: &str, x: f32, y: f32, clip: (f32, f32, f32, f32), fs: f32, color: Rgba) {
    let (cx, cy) = (clip.0.floor(), clip.1.floor());
    let (cw, ch) = ((clip.0 + clip.2).ceil() - cx, (clip.1 + clip.3).ceil() - cy);
    if s.is_empty() || cw < 1.0 || ch < 1.0 {
        return;
    }
    let Some(mut tmp) = Pixmap::new(cw as u32, ch as u32) else { return };
    text::draw(&mut tmp.as_mut(), s, x - cx, y - cy, fs, false, color, None);
    pm.draw_pixmap(cx as i32, cy as i32, tmp.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

fn fill(pm: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba) {
    if let Some(p) = rounded_rect(x, y, w, h, r) {
        pm.fill_path(&p, &c.paint(), FillRule::Winding, Transform::identity(), None);
    }
}

impl TextInput {
    pub fn new(value: impl Into<String>, placeholder: &'static str, numeric: bool, multiline: bool) -> Self {
        let value = value.into();
        let n = value.chars().count();
        TextInput {
            value,
            placeholder,
            numeric,
            multiline,
            cursor: n,
            anchor: n,
            scroll: 0.0,
            goal_x: None,
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            pending_high: None,
            geom: None,
            drag: false,
        }
    }

    /// Replaces the whole text (a folder from Browse…, a screen reader's SetValue). Undoable.
    pub fn set(&mut self, s: impl Into<String>) {
        let s = self.filter(&s.into());
        if s == self.value {
            return;
        }
        self.checkpoint(false);
        self.value = s;
        self.cursor = self.len();
        self.anchor = self.cursor;
        self.goal_x = None;
    }

    fn len(&self) -> usize {
        self.value.chars().count()
    }

    fn byte(&self, ci: usize) -> usize {
        self.value.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(self.value.len())
    }

    fn chars(&self) -> Vec<char> {
        self.value.chars().collect()
    }

    /// Selection as `(start, end)` char indices (equal when nothing is selected).
    pub fn selection(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }

    pub fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    pub fn selected_text(&self) -> String {
        let (a, b) = self.selection();
        self.value[self.byte(a)..self.byte(b)].to_string()
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.len();
        self.typing = false;
        self.goal_x = None;
    }

    pub fn dragging(&self) -> bool {
        self.drag
    }

    /// What this box accepts from a paste or `set`.
    fn filter(&self, s: &str) -> String {
        if self.numeric {
            s.chars().filter(|c| c.is_ascii_digit()).collect()
        } else if self.multiline {
            s.replace("\r\n", "\n").replace('\r', "\n").chars().filter(|&c| c == '\n' || c == '\t' || !c.is_control()).collect()
        } else {
            s.chars().filter(|c| !c.is_control()).collect()
        }
    }

    fn snap(&self) -> Snap {
        Snap { value: self.value.clone(), cursor: self.cursor, anchor: self.anchor }
    }

    fn restore(&mut self, s: Snap) {
        self.value = s.value;
        self.cursor = s.cursor.min(self.len());
        self.anchor = s.anchor.min(self.len());
        self.typing = false;
        self.goal_x = None;
    }

    /// Records an undo step before an edit (consecutive typing shares one step).
    fn checkpoint(&mut self, typing: bool) {
        if !(typing && self.typing) {
            self.undo.push(self.snap());
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.typing = typing;
        self.redo.clear();
    }

    fn replace_selection(&mut self, s: &str, typing: bool) -> Change {
        if s.is_empty() && !self.has_selection() {
            return Change::Moved;
        }
        self.checkpoint(typing);
        let (a, b) = self.selection();
        let (ba, bb) = (self.byte(a), self.byte(b));
        self.value.replace_range(ba..bb, s);
        self.cursor = a + s.chars().count();
        self.anchor = self.cursor;
        self.goal_x = None;
        Change::Edited
    }

    fn move_to(&mut self, i: usize, extend: bool) -> Change {
        self.cursor = i.min(self.len());
        if !extend {
            self.anchor = self.cursor;
        }
        self.typing = false;
        Change::Moved
    }

    fn undo(&mut self) -> Change {
        match self.undo.pop() {
            Some(s) => {
                self.redo.push(self.snap());
                self.restore(s);
                Change::Edited
            }
            None => Change::Moved,
        }
    }

    fn redo(&mut self) -> Change {
        match self.redo.pop() {
            Some(s) => {
                self.undo.push(self.snap());
                self.restore(s);
                Change::Edited
            }
            None => Change::Moved,
        }
    }

    fn word_left(&self, i: usize) -> usize {
        let ch = self.chars();
        let mut i = i.min(ch.len());
        while i > 0 && class(ch[i - 1]) == 0 {
            i -= 1;
        }
        if i > 0 {
            let k = class(ch[i - 1]);
            while i > 0 && class(ch[i - 1]) == k {
                i -= 1;
            }
        }
        i
    }

    fn word_right(&self, i: usize) -> usize {
        let ch = self.chars();
        let n = ch.len();
        let mut i = i.min(n);
        if i < n && class(ch[i]) != 0 {
            let k = class(ch[i]);
            while i < n && class(ch[i]) == k {
                i += 1;
            }
        }
        while i < n && class(ch[i]) == 0 && ch[i] != '\n' {
            i += 1;
        }
        i
    }

    /// The word (or run of spaces / punctuation) around char `i`, for double-click.
    fn word_at(&self, i: usize) -> (usize, usize) {
        let ch = self.chars();
        if ch.is_empty() {
            return (0, 0);
        }
        let i = i.min(ch.len() - 1);
        let k = class(ch[i]);
        let (mut a, mut b) = (i, i + 1);
        while a > 0 && class(ch[a - 1]) == k && ch[a - 1] != '\n' {
            a -= 1;
        }
        while b < ch.len() && class(ch[b]) == k && ch[b] != '\n' {
            b += 1;
        }
        (a, b)
    }

    /// Wraps the text into lines of at most `max_w` px (breaking after spaces, or inside a
    /// word that doesn't fit), as char ranges. Explicit newlines always break.
    fn wrap(&self, fs: f32, max_w: f32) -> Vec<(usize, usize)> {
        let ch = self.chars();
        let mut out = Vec::new();
        let mut ps = 0;
        loop {
            let pe = ch[ps..].iter().position(|&c| c == '\n').map(|p| ps + p).unwrap_or(ch.len());
            if pe == ps {
                out.push((ps, ps));
            } else {
                let para: String = ch[ps..pe].iter().collect();
                let pos = text::caret_positions(&para, fs, false);
                let mut ls = ps;
                while ls < pe {
                    let base = pos[ls - ps];
                    let mut e = ls + 1;
                    while e < pe && pos[e + 1 - ps] - base <= max_w {
                        e += 1;
                    }
                    if e >= pe {
                        out.push((ls, pe));
                        break;
                    }
                    let brk = (ls + 1..=e).rev().find(|&i| ch[i - 1].is_whitespace()).unwrap_or(e);
                    out.push((ls, brk));
                    ls = brk;
                }
            }
            if pe >= ch.len() {
                break;
            }
            ps = pe + 1;
        }
        if out.is_empty() {
            out.push((0, 0));
        }
        out
    }

    fn line_text(&self, s: usize, e: usize) -> String {
        self.value[self.byte(s)..self.byte(e)].to_string()
    }

    /// Clamps a caret index at the end of a soft-wrapped line to that line (otherwise it
    /// would be drawn at the start of the next one).
    fn clamp_to_line(lines: &[(usize, usize)], row: usize, i: usize) -> usize {
        let (s, e) = lines[row];
        if i == e && e > s && lines.get(row + 1).is_some_and(|n| n.0 == e) {
            e - 1
        } else {
            i
        }
    }

    fn vertical(&mut self, down: bool, extend: bool) -> Change {
        let Some(g) = self.geom.clone() else { return Change::Moved };
        if g.lines.is_empty() {
            return Change::Moved;
        }
        let row = line_of(&g.lines, self.cursor);
        let (s, e) = g.lines[row];
        let x = self.goal_x.unwrap_or_else(|| {
            let pos = text::caret_positions(&self.line_text(s, e), g.fs, false);
            pos[self.cursor.saturating_sub(s).min(pos.len() - 1)]
        });
        let target = if down { Some(row + 1).filter(|&r| r < g.lines.len()) } else { row.checked_sub(1) };
        let idx = match target {
            None if down => self.len(),
            None => 0,
            Some(t) => {
                let (ts, te) = g.lines[t];
                let pos = text::caret_positions(&self.line_text(ts, te), g.fs, false);
                Self::clamp_to_line(&g.lines, t, ts + nearest(&pos, x))
            }
        };
        self.move_to(idx, extend);
        self.goal_x = Some(x);
        Change::Moved
    }

    fn line_bounds(&self) -> Option<(usize, usize)> {
        let g = self.geom.as_ref().filter(|g| !g.lines.is_empty())?;
        let row = line_of(&g.lines, self.cursor);
        let (s, e) = g.lines[row];
        Some((s, Self::clamp_to_line(&g.lines, row, e)))
    }

    /// Handles `WM_KEYDOWN` while the box has focus.
    pub fn on_key(&mut self, vk: u32, ctrl: bool, shift: bool) -> Change {
        const BACK: u32 = 0x08;
        const END: u32 = 0x23;
        const HOME: u32 = 0x24;
        const LEFT: u32 = 0x25;
        const UP: u32 = 0x26;
        const RIGHT: u32 = 0x27;
        const DOWN: u32 = 0x28;
        const DEL: u32 = 0x2E;
        if ctrl {
            match vk {
                0x41 => {
                    self.select_all();
                    return Change::Moved;
                }
                0x43 => {
                    if self.has_selection() {
                        let _ = crate::clipboard::set_text(&self.selected_text());
                    }
                    return Change::Moved;
                }
                0x58 => {
                    if !self.has_selection() {
                        return Change::Moved;
                    }
                    let _ = crate::clipboard::set_text(&self.selected_text());
                    return self.replace_selection("", false);
                }
                0x56 => {
                    let clip = crate::clipboard::get_text().map(|c| self.filter(&c)).unwrap_or_default();
                    if clip.is_empty() {
                        return Change::Moved;
                    }
                    return self.replace_selection(&clip, false);
                }
                0x5A if shift => return self.redo(),
                0x5A => return self.undo(),
                0x59 => return self.redo(),
                _ => {}
            }
        }
        match vk {
            LEFT => {
                self.goal_x = None;
                if self.has_selection() && !shift {
                    let (a, _) = self.selection();
                    return self.move_to(a, false);
                }
                let i = if ctrl { self.word_left(self.cursor) } else { self.cursor.saturating_sub(1) };
                self.move_to(i, shift)
            }
            RIGHT => {
                self.goal_x = None;
                if self.has_selection() && !shift {
                    let (_, b) = self.selection();
                    return self.move_to(b, false);
                }
                let i = if ctrl { self.word_right(self.cursor) } else { self.cursor + 1 };
                self.move_to(i, shift)
            }
            HOME => {
                self.goal_x = None;
                let i = if ctrl || !self.multiline { 0 } else { self.line_bounds().map_or(0, |b| b.0) };
                self.move_to(i, shift)
            }
            END => {
                self.goal_x = None;
                let i = if ctrl || !self.multiline { self.len() } else { self.line_bounds().map_or(self.len(), |b| b.1) };
                self.move_to(i, shift)
            }
            UP | DOWN if self.multiline => self.vertical(vk == DOWN, shift),
            BACK => {
                if !self.has_selection() {
                    if self.cursor == 0 {
                        return Change::Moved;
                    }
                    self.anchor = if ctrl { self.word_left(self.cursor) } else { self.cursor - 1 };
                }
                self.replace_selection("", false)
            }
            DEL => {
                if !self.has_selection() {
                    if self.cursor >= self.len() {
                        return Change::Moved;
                    }
                    self.anchor = if ctrl { self.word_right(self.cursor) } else { self.cursor + 1 };
                }
                self.replace_selection("", false)
            }
            _ => Change::None,
        }
    }

    /// Handles one `WM_CHAR` UTF-16 code unit (emoji arrive as a surrogate pair).
    pub fn on_char(&mut self, unit: u16) -> Change {
        let ch = match unit {
            0xD800..=0xDBFF => {
                self.pending_high = Some(unit);
                return Change::Moved;
            }
            0xDC00..=0xDFFF => match self.pending_high.take() {
                Some(hi) => match char::decode_utf16([hi, unit]).next() {
                    Some(Ok(c)) => c,
                    _ => return Change::Moved,
                },
                None => return Change::Moved,
            },
            u => {
                self.pending_high = None;
                match char::from_u32(u as u32) {
                    Some(c) => c,
                    None => return Change::None,
                }
            }
        };
        if ch == '\r' {
            return if self.multiline { self.replace_selection("\n", false) } else { Change::None };
        }
        if ch.is_control() || (self.numeric && !ch.is_ascii_digit()) {
            return Change::None;
        }
        let mut buf = [0u8; 4];
        self.replace_selection(ch.encode_utf8(&mut buf), true)
    }

    /// The char boundary under client point `(x, y)`, from the last drawn frame.
    fn hit(&self, x: i32, y: i32) -> usize {
        let Some(g) = &self.geom else { return self.cursor };
        if !self.multiline {
            let pos = text::caret_positions(&self.value, g.fs, false);
            return nearest(&pos, x as f32 - g.x + self.scroll);
        }
        if g.lines.is_empty() {
            return 0;
        }
        let row = (((y as f32 - g.y + self.scroll) / g.lh).floor().max(0.0) as usize).min(g.lines.len() - 1);
        let (s, e) = g.lines[row];
        let pos = text::caret_positions(&self.line_text(s, e), g.fs, false);
        Self::clamp_to_line(&g.lines, row, s + nearest(&pos, x as f32 - g.x))
    }

    /// Left button pressed in the box: place the caret (Shift extends the selection) or,
    /// on a double-click, select the word.
    pub fn mouse_down(&mut self, x: i32, y: i32, shift: bool, double: bool) -> Change {
        let i = self.hit(x, y);
        self.typing = false;
        self.goal_x = None;
        if double {
            let (a, b) = self.word_at(i);
            self.anchor = a;
            self.cursor = b;
            self.drag = false;
        } else {
            self.cursor = i;
            if !shift {
                self.anchor = i;
            }
            self.drag = true;
        }
        Change::Moved
    }

    /// Mouse moved while the button is held after [`TextInput::mouse_down`].
    pub fn mouse_move(&mut self, x: i32, y: i32) -> Change {
        if !self.drag {
            return Change::None;
        }
        let i = self.hit(x, y);
        if i == self.cursor {
            return Change::None;
        }
        self.cursor = i;
        Change::Moved
    }

    pub fn mouse_up(&mut self) {
        self.drag = false;
    }

    /// Caret position (x, y, height) in client coordinates, for the IME windows.
    pub fn caret_point(&self) -> Option<(i32, i32, i32)> {
        let g = self.geom.as_ref()?;
        if !self.multiline {
            let pos = text::caret_positions(&self.value, g.fs, false);
            let x = g.x + pos[self.cursor.min(pos.len() - 1)] - self.scroll;
            return Some((x as i32, g.y as i32, g.h as i32));
        }
        let row = line_of(&g.lines, self.cursor);
        let (s, e) = *g.lines.get(row)?;
        let pos = text::caret_positions(&self.line_text(s, e), g.fs, false);
        let x = g.x + pos[self.cursor.saturating_sub(s).min(pos.len() - 1)];
        let y = g.y + row as f32 * g.lh - self.scroll;
        Some((x as i32, y as i32, g.lh as i32))
    }

    /// Draws the box. `&mut` because drawing keeps the caret scrolled into view and records
    /// where the text went (for clicks, Up / Down and the IME).
    pub fn draw(&mut self, pm: &mut PixmapMut, t: &Theme, r: Rect, focused: bool, hovered: bool, scale: f32) {
        let id = Transform::identity();
        let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        if let Some(p) = rounded_rect(x, y, w, h, 7.0 * scale) {
            pm.fill_path(&p, &t.input_bg.paint(), FillRule::Winding, id, None);
            let (bc, bw) = if focused {
                (t.accent, 1.6 * scale)
            } else if hovered {
                (t.subtext.with_alpha(160), 1.0)
            } else {
                (t.border, 1.0)
            };
            pm.stroke_path(&p, &bc.paint(), &Stroke { width: bw, ..Default::default() }, id, None);
        }
        let pad_x = 10.0 * scale;
        let fs = 13.0 * scale;
        let (ix, iw) = (x + pad_x, (w - pad_x * 2.0).max(10.0));
        let caret_w = (2.0 * scale).round().max(1.0);
        if self.multiline {
            self.draw_multi(pm, t, (ix, y, iw, h), focused, fs, caret_w, scale);
        } else {
            self.draw_single(pm, t, (ix, y, iw, h), focused, fs, caret_w, scale);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_single(&mut self, pm: &mut PixmapMut, t: &Theme, b: (f32, f32, f32, f32), focused: bool, fs: f32, caret_w: f32, scale: f32) {
        let (ix, y, iw, h) = b;
        let (iy, ih) = (y + 3.0 * scale, (h - 6.0 * scale).max(1.0));
        self.geom = Some(Geom { x: ix, y: y + 6.0 * scale, h: h - 12.0 * scale, fs, lh: 0.0, lines: Vec::new() });
        let text_h = text::layout("Ag", fs, false).height;
        let ty = y + (h - text_h) / 2.0;
        let caret = |pm: &mut PixmapMut, cx: f32| fill(pm, cx.round(), (y + 6.0 * scale).round(), caret_w, (h - 12.0 * scale).round().max(2.0), 0.5, t.accent);

        if self.value.is_empty() {
            self.scroll = 0.0;
            if !self.placeholder.is_empty() {
                let ph = ellipsize(self.placeholder, fs, false, iw);
                text::draw(pm, &ph, ix, ty, fs, false, t.subtext.with_alpha(150), None);
            }
            if focused {
                caret(pm, ix);
            }
            return;
        }
        let pos = text::caret_positions(&self.value, fs, false);
        let total = *pos.last().unwrap_or(&0.0);
        if !focused {
            self.scroll = 0.0;
            let shown = ellipsize(&self.value, fs, false, iw);
            text::draw(pm, &shown, ix, ty, fs, false, t.text, None);
            return;
        }
        // Keep the caret inside the box.
        let cx = pos[self.cursor.min(pos.len() - 1)];
        if cx - self.scroll > iw - caret_w {
            self.scroll = cx - iw + caret_w;
        }
        if cx < self.scroll {
            self.scroll = cx;
        }
        self.scroll = self.scroll.clamp(0.0, (total - iw + caret_w).max(0.0));
        if self.has_selection() {
            let (a, b) = self.selection();
            let sx = (ix + pos[a] - self.scroll).max(ix);
            let ex = (ix + pos[b] - self.scroll).min(ix + iw);
            if ex > sx {
                fill(pm, sx, y + 5.0 * scale, ex - sx, h - 10.0 * scale, 2.0 * scale, t.accent_soft.with_alpha(110));
            }
        }
        draw_clipped(pm, &self.value, ix - self.scroll, ty, (ix, iy, iw, ih), fs, t.text);
        caret(pm, ix + cx - self.scroll);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_multi(&mut self, pm: &mut PixmapMut, t: &Theme, b: (f32, f32, f32, f32), focused: bool, fs: f32, caret_w: f32, scale: f32) {
        let (ix, y, iw, h) = b;
        let pad_y = 7.0 * scale;
        let (iy, ih) = (y + pad_y, (h - pad_y * 2.0).max(1.0));
        let lh = text::layout("Ag", fs, false).line_height * 1.15;
        let lines = self.wrap(fs, iw - caret_w);
        self.geom = Some(Geom { x: ix, y: iy, h: ih, fs, lh, lines: lines.clone() });
        if self.value.is_empty() {
            self.scroll = 0.0;
            if !self.placeholder.is_empty() {
                let ph = ellipsize(self.placeholder, fs, false, iw);
                text::draw(pm, &ph, ix, iy, fs, false, t.subtext.with_alpha(150), None);
            }
            if focused {
                fill(pm, ix.round(), iy.round(), caret_w, (lh * 0.9).round(), 0.5, t.accent);
            }
            return;
        }
        let row = line_of(&lines, self.cursor);
        let total_h = lines.len() as f32 * lh;
        if focused {
            let top = row as f32 * lh;
            if top < self.scroll {
                self.scroll = top;
            }
            if top + lh > self.scroll + ih {
                self.scroll = top + lh - ih;
            }
        }
        self.scroll = self.scroll.clamp(0.0, (total_h - ih).max(0.0));
        let (sa, sb) = self.selection();
        let ch = self.chars();
        for (i, &(s, e)) in lines.iter().enumerate() {
            let ly = iy + i as f32 * lh - self.scroll;
            if ly + lh < iy || ly > iy + ih {
                continue;
            }
            let line = self.line_text(s, e);
            let pos = text::caret_positions(&line, fs, false);
            if focused && sa < sb {
                // Selected part of this line; a selected newline shows as a small block.
                let (a, b2) = (sa.max(s), sb.min(e));
                let newline_selected = e < ch.len() && ch[e] == '\n' && sa <= e && sb > e;
                if a < b2 || newline_selected {
                    let x0 = ix + pos[a.saturating_sub(s).min(pos.len() - 1)];
                    let mut x1 = ix + pos[b2.saturating_sub(s).min(pos.len() - 1)];
                    if newline_selected {
                        x1 += 5.0 * scale;
                    }
                    let (top, bot) = (ly.max(iy), (ly + lh).min(iy + ih));
                    if bot > top && x1 > x0 {
                        fill(pm, x0, top, (x1 - x0).min(ix + iw - x0), bot - top, 2.0 * scale, t.accent_soft.with_alpha(110));
                    }
                }
            }
            draw_clipped(pm, &line, ix, ly, (ix, iy, iw, ih), fs, t.text);
            if focused && i == row {
                let cx = ix + pos[self.cursor.saturating_sub(s).min(pos.len() - 1)];
                let (top, bot) = (ly.max(iy), (ly + lh * 0.9).min(iy + ih));
                if bot > top {
                    fill(pm, cx.round(), top.round(), caret_w, (bot - top).round().max(1.0), 0.5, t.accent);
                }
            }
        }
    }
}

/// Moves the IME composition and candidate windows to the caret (`(x, y, height)` in
/// client coordinates, from [`TextInput::caret_point`]). Call on `WM_IME_STARTCOMPOSITION`
/// and `WM_IME_COMPOSITION`, then let `DefWindowProc` handle the message.
pub fn place_ime(hwnd: HWND, caret: (i32, i32, i32)) {
    let (x, y, h) = caret;
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_invalid() {
            return;
        }
        let cf = COMPOSITIONFORM { dwStyle: CFS_POINT, ptCurrentPos: POINT { x, y }, rcArea: RECT::default() };
        let _ = ImmSetCompositionWindow(himc, &cf);
        let cand = CANDIDATEFORM { dwIndex: 0, dwStyle: CFS_CANDIDATEPOS, ptCurrentPos: POINT { x, y: y + h }, rcArea: RECT::default() };
        let _ = ImmSetCandidateWindow(himc, &cand);
        let _ = ImmReleaseContext(hwnd, himc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ti(s: &str) -> TextInput {
        TextInput::new(s, "", false, false)
    }

    fn typ(t: &mut TextInput, s: &str) {
        for u in s.encode_utf16() {
            t.on_char(u);
        }
    }

    #[test]
    fn caret_moves_and_inserts_where_it_is() {
        let mut t = ti("hello world");
        t.on_key(0x24, false, false); // Home
        typ(&mut t, ">");
        assert_eq!(t.value, ">hello world");
        t.on_key(0x27, true, false); // Ctrl+Right: start of the next word, like Windows
        typ(&mut t, "big ");
        assert_eq!(t.value, ">hello big world");
        t.on_key(0x23, false, false); // End
        t.on_key(0x08, true, false); // Ctrl+Backspace deletes "world"
        assert_eq!(t.value, ">hello big ");
        t.on_key(0x25, true, true); // Ctrl+Shift+Left selects "big "
        assert_eq!(t.selected_text(), "big ");
    }

    #[test]
    fn selection_replace_copy_semantics() {
        let mut t = ti("abc def");
        t.on_key(0x24, false, false);
        t.on_key(0x27, false, true); // Shift+Right ×3 selects "abc"
        t.on_key(0x27, false, true);
        t.on_key(0x27, false, true);
        assert_eq!(t.selected_text(), "abc");
        typ(&mut t, "X");
        assert_eq!(t.value, "X def");
        // Ctrl+X with no selection must not wipe the field.
        assert_eq!(t.on_key(0x58, true, false), Change::Moved);
        assert_eq!(t.value, "X def");
        t.on_key(0x41, true, false); // Ctrl+A
        assert_eq!(t.selected_text(), "X def");
        t.on_key(0x2E, false, false); // Delete removes the selection
        assert_eq!(t.value, "");
    }

    #[test]
    fn undo_groups_typing_and_redo_restores() {
        let mut t = ti("");
        typ(&mut t, "abc");
        t.on_key(0x25, false, false); // Left (ends the typing run)
        typ(&mut t, "Z");
        assert_eq!(t.value, "abZc");
        t.on_key(0x5A, true, false);
        assert_eq!(t.value, "abc", "undo the Z");
        t.on_key(0x5A, true, false);
        assert_eq!(t.value, "", "the typed run undoes as one step");
        t.on_key(0x59, true, false);
        assert_eq!(t.value, "abc", "redo");
    }

    #[test]
    fn emoji_surrogates_and_numeric_filter() {
        let mut t = ti("");
        typ(&mut t, "hi 😀");
        assert_eq!(t.value, "hi 😀");
        t.on_key(0x08, false, false);
        assert_eq!(t.value, "hi ", "backspace removes the whole emoji");
        let mut n = TextInput::new("", "", true, false);
        typ(&mut n, "4a2");
        assert_eq!(n.value, "42");
    }

    #[test]
    fn multiline_enter_and_word_select() {
        let mut t = TextInput::new("", "", false, true);
        typ(&mut t, "one two\r");
        typ(&mut t, "three");
        assert_eq!(t.value, "one two\nthree");
        let (a, b) = t.word_at(5);
        assert_eq!(&t.value[a..b], "two");
    }

    #[test]
    fn wrapped_lines_cover_the_text() {
        if !text::available() {
            return;
        }
        let t = TextInput::new("aaaa bbbb cccc\n\ndddd", "", false, true);
        let lines = t.wrap(13.0, 60.0);
        assert!(lines.len() >= 4, "{lines:?}");
        // Lines are in order, contiguous apart from the newline chars, and cover everything.
        assert_eq!(lines[0].0, 0);
        assert_eq!(lines.last().unwrap().1, t.value.chars().count());
        for w in lines.windows(2) {
            assert!(w[1].0 == w[0].1 || w[1].0 == w[0].1 + 1, "{lines:?}");
        }
        assert_eq!(line_of(&lines, 0), 0);
    }
}
