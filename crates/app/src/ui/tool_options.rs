//! The editor's tool-options strip, modelled on ShareX's image editor: it shows
//! only the options of the current tool (or of the selected item's tool), and a
//! change applies to new items and to the selected one.

use snapit_annotate::{ArrowHeads, Dash, Item, Shape, StepType, Style, Tool};
use snapit_gfx::effects::rounded_rect;
use snapit_gfx::text;
use snapit_gfx::tiny_skia::{FillRule, LineCap, PathBuilder, PixmapMut, Stroke, StrokeDash, Transform};
use snapit_gfx::{Rect, Rgba};

use super::theme::Theme;
use super::toolbar::draw_tip;
use crate::settings::ToolOptions;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Opt {
    /// Border / stroke colour (the body colour for text, balloons and steps).
    Color,
    FillColor,
    Highlight,
    Width,
    FontSize,
    Radius,
    Blur,
    Pixel,
    Zoom,
    Dim,
    Dash,
    Heads,
    StepType,
    SpotShape,
    Shadow,
    Background,
}

/// The options shown for each tool, in ShareX's order (colour, size, fill, …, shadow).
pub fn options_for(tool: Tool) -> &'static [Opt] {
    use Opt::*;
    match tool {
        Tool::Rect => &[Color, Width, FillColor, Radius, Dash, Shadow],
        Tool::Ellipse => &[Color, Width, FillColor, Dash, Shadow],
        Tool::Line | Tool::Pen => &[Color, Width, Dash, Shadow],
        Tool::Arrow => &[Color, Width, Heads, Dash, Shadow],
        Tool::Highlight => &[Highlight],
        Tool::Text => &[Color, FontSize, Background, Shadow],
        Tool::Balloon => &[Color, FontSize, Shadow],
        Tool::Step => &[Color, FontSize, StepType, Shadow],
        Tool::Blur => &[Blur],
        Tool::Pixelate => &[Pixel],
        Tool::Magnify => &[Zoom, Color, Width, Dash, Shadow],
        Tool::Spotlight => &[Dim, SpotShape],
        Tool::Pointer | Tool::Eraser | Tool::Sticker(_) => &[],
    }
}

fn label(o: Opt, tool: Tool) -> &'static str {
    match o {
        Opt::Color => match tool {
            Tool::Text | Tool::Balloon | Tool::Step => "Colour",
            _ => "Border",
        },
        Opt::Highlight => "Colour",
        Opt::FillColor => "Fill",
        Opt::Width => "Size",
        Opt::FontSize => {
            if tool == Tool::Step {
                "Size"
            } else {
                "Text size"
            }
        }
        Opt::Radius => "Corners",
        Opt::Blur => "Blur radius",
        Opt::Pixel => "Pixel size",
        Opt::Zoom => "Zoom",
        Opt::Dim => "Dim",
        Opt::Dash => "Line",
        Opt::Heads => "Heads",
        Opt::StepType => "Type",
        Opt::SpotShape => "Shape",
        Opt::Shadow => "Shadow",
        Opt::Background => "Background",
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum OptHit {
    Swatch(Opt),
    Step(Opt, i32),
    Choice(Opt, usize),
    Toggle(Opt),
}

/// Everything the strip shows and edits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Values {
    pub style: Style,
    pub opts: ToolOptions,
    /// Highlighter colour.
    pub marker: Rgba,
}

impl Values {
    pub fn number(&self, o: Opt) -> f32 {
        match o {
            Opt::Width => self.style.width,
            Opt::FontSize => self.style.font_size,
            Opt::Radius => self.opts.corner_radius,
            Opt::Blur => self.opts.blur_radius,
            Opt::Pixel => self.opts.pixel_size,
            Opt::Zoom => self.opts.zoom,
            Opt::Dim => self.opts.spot_dim,
            _ => 0.0,
        }
    }

    fn set_number(&mut self, o: Opt, v: f32) {
        match o {
            Opt::Width => self.style.width = v,
            Opt::FontSize => self.style.font_size = v,
            Opt::Radius => self.opts.corner_radius = v,
            Opt::Blur => self.opts.blur_radius = v,
            Opt::Pixel => self.opts.pixel_size = v,
            Opt::Zoom => self.opts.zoom = v,
            Opt::Dim => self.opts.spot_dim = v,
            _ => {}
        }
    }

    fn value_text(&self, o: Opt) -> String {
        let v = self.number(o);
        match o {
            Opt::Zoom => format!("{:.0}%", v * 100.0),
            Opt::Dim => format!("{v:.0}%"),
            _ => format!("{v:.0}"),
        }
    }

    pub fn choice(&self, o: Opt) -> usize {
        match o {
            Opt::Dash => Dash::ALL.iter().position(|d| *d == self.opts.dash).unwrap_or(0),
            Opt::Heads => ArrowHeads::ALL.iter().position(|h| *h == self.opts.heads).unwrap_or(0),
            Opt::StepType => StepType::ALL.iter().position(|s| *s == self.opts.step_type).unwrap_or(0),
            Opt::SpotShape => self.opts.spot_ellipse as usize,
            _ => 0,
        }
    }

    pub fn toggled(&self, o: Opt) -> bool {
        match o {
            Opt::Shadow => self.opts.shadow,
            Opt::Background => self.opts.text_background,
            _ => false,
        }
    }

    /// The colour a swatch shows; `None` means "no fill".
    pub fn swatch(&self, o: Opt) -> Option<Rgba> {
        match o {
            Opt::Color => Some(self.style.color),
            Opt::Highlight => Some(self.marker),
            Opt::FillColor => self.opts.fill.then_some(self.opts.fill_color),
            _ => None,
        }
    }

    /// Sets a swatch's colour (`None` turns the fill off).
    pub fn set_swatch(&mut self, o: Opt, c: Option<Rgba>) {
        match (o, c) {
            (Opt::Color, Some(c)) => self.style.color = c,
            (Opt::Highlight, Some(c)) => self.marker = c,
            (Opt::FillColor, Some(c)) => {
                self.opts.fill = true;
                self.opts.fill_color = c;
            }
            (Opt::FillColor, None) => self.opts.fill = false,
            _ => {}
        }
    }

    /// One click on a stepper's − / + (or a wheel notch). Returns whether anything changed.
    pub fn step(&mut self, o: Opt, dir: i32, tool: Tool) -> bool {
        let (min, max, inc) = range(o, tool);
        let cur = self.number(o);
        let n = ((cur / inc).round() * inc + inc * dir as f32).clamp(min, max);
        if (n - cur).abs() < 1e-4 {
            return false;
        }
        self.set_number(o, n);
        true
    }

    pub fn choose(&mut self, o: Opt, i: usize) {
        match o {
            Opt::Dash => self.opts.dash = Dash::ALL[i.min(Dash::ALL.len() - 1)],
            Opt::Heads => self.opts.heads = ArrowHeads::ALL[i.min(ArrowHeads::ALL.len() - 1)],
            Opt::StepType => self.opts.step_type = StepType::ALL[i.min(StepType::ALL.len() - 1)],
            Opt::SpotShape => self.opts.spot_ellipse = i == 1,
            _ => {}
        }
    }

    pub fn toggle(&mut self, o: Opt) {
        match o {
            Opt::Shadow => self.opts.shadow = !self.opts.shadow,
            Opt::Background => self.opts.text_background = !self.opts.text_background,
            _ => {}
        }
    }
}

impl Values {
    /// Saves these options, so the editor and quick edit both start from them.
    pub fn save(&self) {
        let v = *self;
        crate::app::update_settings(|s| {
            s.annotate.color = v.style.color;
            s.annotate.highlight = v.marker;
            s.annotate.width = v.style.width;
            s.annotate.font_size = v.style.font_size;
            s.annotate.tools = v.opts;
        });
    }

    /// Copies the shared, saved options into `self`, keeping the rest of the style.
    pub fn take_saved(&mut self, a: &crate::settings::Annotate) {
        self.style.color = a.color;
        self.style.width = a.width;
        self.style.font_size = a.font_size;
        self.opts = a.tools;
        self.marker = a.highlight;
    }

    /// The style a new item of `tool` gets from these options.
    pub fn item_style(&self, tool: Tool) -> Style {
        let o = &self.opts;
        let mut st = Style {
            fill: false,
            fill_color: o.fill_color,
            corner_radius: o.corner_radius,
            dash: o.dash,
            shadow: o.shadow,
            heads: o.heads,
            step_type: o.step_type,
            strength: 0.0,
            ..self.style
        };
        match tool {
            Tool::Rect | Tool::Ellipse => st.fill = o.fill,
            // A text item's `fill` is its background.
            Tool::Text => st.fill = o.text_background,
            Tool::Highlight => st.color = self.marker,
            Tool::Blur => st.strength = o.blur_radius,
            Tool::Pixelate => st.strength = o.pixel_size,
            Tool::Spotlight => st.strength = o.spot_dim,
            _ => {}
        }
        st
    }

    /// Options that live in the shape rather than the style, for a just-created item.
    pub fn finish_new_item(&self, it: &mut Item) {
        match &mut it.shape {
            Shape::Magnify { zoom, .. } => *zoom = self.opts.zoom,
            Shape::Spotlight { ellipse, .. } => *ellipse = self.opts.spot_ellipse,
            _ => {}
        }
    }

    /// Takes the options from an item, so selecting it shows (and edits) its own options.
    pub fn load_from_item(&mut self, it: &Item) {
        let st = it.style;
        let o = &mut self.opts;
        match &it.shape {
            Shape::Highlight(_) => self.marker = st.color,
            Shape::Blur(_) => o.blur_radius = if st.strength > 0.0 { st.strength } else { o.blur_radius },
            Shape::Pixelate(_) => o.pixel_size = if st.strength > 0.0 { st.strength } else { o.pixel_size },
            Shape::Spotlight { dim, ellipse, .. } => {
                o.spot_dim = (*dim as f32 / 2.55).round();
                o.spot_ellipse = *ellipse;
            }
            Shape::Text { background, .. } => {
                o.text_background = *background;
                self.style.color = st.color;
                self.style.font_size = st.font_size;
                o.shadow = st.shadow;
            }
            Shape::Image { .. } | Shape::Sticker { .. } | Shape::Eraser(..) => {}
            shape => {
                self.style.color = st.color;
                self.style.width = st.width;
                self.style.font_size = st.font_size;
                if matches!(shape, Shape::Rect(_) | Shape::Ellipse(_)) {
                    o.fill = st.fill;
                    o.fill_color = st.fill_color;
                }
                o.corner_radius = st.corner_radius;
                o.dash = st.dash;
                o.shadow = st.shadow;
                o.heads = st.heads;
                o.step_type = st.step_type;
                if let Shape::Magnify { zoom, .. } = shape {
                    o.zoom = *zoom;
                }
            }
        }
    }

    /// Applies one changed option to an existing item.
    pub fn apply_to_item(&self, changed: Opt, it: &mut Item) {
        let Some(tool) = it.tool() else { return };
        let new = self.item_style(tool);
        let opts = self.opts;
        match changed {
            Opt::Color => it.style.color = new.color,
            Opt::Highlight => it.style.color = self.marker,
            Opt::FillColor => {
                it.style.fill = new.fill;
                it.style.fill_color = new.fill_color;
            }
            Opt::Width => it.style.width = new.width,
            Opt::FontSize => it.style.font_size = new.font_size,
            Opt::Radius => it.style.corner_radius = new.corner_radius,
            Opt::Dash => it.style.dash = new.dash,
            Opt::Shadow => it.style.shadow = new.shadow,
            Opt::Heads => it.style.heads = new.heads,
            Opt::StepType => it.style.step_type = new.step_type,
            Opt::Blur | Opt::Pixel => it.style.strength = new.strength,
            Opt::Background => {
                it.style.fill = opts.text_background;
                if let Shape::Text { background, .. } = &mut it.shape {
                    *background = opts.text_background;
                }
            }
            Opt::Zoom => {
                if let Shape::Magnify { zoom, .. } = &mut it.shape {
                    *zoom = opts.zoom;
                }
            }
            Opt::Dim => {
                if let Shape::Spotlight { dim, .. } = &mut it.shape {
                    *dim = (opts.spot_dim * 2.55).clamp(0.0, 255.0) as u8;
                }
            }
            Opt::SpotShape => {
                if let Shape::Spotlight { ellipse, .. } = &mut it.shape {
                    *ellipse = opts.spot_ellipse;
                }
            }
        }
    }
}

/// (min, max, increment) of a numeric option.
fn range(o: Opt, tool: Tool) -> (f32, f32, f32) {
    match o {
        // Filled shapes may drop their border entirely.
        Opt::Width => (if matches!(tool, Tool::Rect | Tool::Ellipse) { 0.0 } else { 1.0 }, 40.0, 1.0),
        Opt::FontSize => (8.0, 120.0, 2.0),
        Opt::Radius => (0.0, 60.0, 2.0),
        Opt::Blur | Opt::Pixel => (2.0, 60.0, 2.0),
        Opt::Zoom => (1.25, 8.0, 0.25),
        Opt::Dim => (10.0, 90.0, 5.0),
        _ => (0.0, 0.0, 1.0),
    }
}

fn choice_count(o: Opt) -> usize {
    match o {
        Opt::Dash => Dash::ALL.len(),
        Opt::Heads => ArrowHeads::ALL.len(),
        Opt::StepType => StepType::ALL.len(),
        Opt::SpotShape => 2,
        _ => 0,
    }
}

fn choice_name(o: Opt, i: usize) -> &'static str {
    match o {
        Opt::Dash => Dash::ALL[i].name(),
        Opt::Heads => ArrowHeads::ALL[i].name(),
        Opt::StepType => StepType::ALL[i].name(),
        Opt::SpotShape => ["Rectangle", "Ellipse"][i],
        _ => "",
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Part {
    Label(&'static str),
    Swatch(Opt),
    Minus(Opt),
    Value(Opt),
    Plus(Opt),
    Seg(Opt, usize),
    Toggle(Opt),
}

/// Control height, label size and spacing (unscaled).
const H: f32 = 28.0;
const FONT: f32 = 12.5;
const GAP: f32 = 16.0;
const LGAP: f32 = 6.0;
const SEG: f32 = 28.0;
const STEP_BTN: f32 = 20.0;
const VALUE_W: f32 = 40.0;

pub struct OptionsBar {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub tool: Option<Tool>,
    pub hover: Option<(i32, i32)>,
}

impl OptionsBar {
    fn parts(&self) -> Vec<(Rect, Part)> {
        let Some(tool) = self.tool else { return Vec::new() };
        let s = self.scale;
        let (h, y) = (H * s, self.y);
        let mut x = self.x;
        let mut out = Vec::new();
        let mut push = |x: &mut f32, w: f32, p: Part| {
            out.push((Rect::new(x.round() as i32, y.round() as i32, w.round() as i32, h.round() as i32), p));
            *x += w;
        };
        for (n, &o) in options_for(tool).iter().enumerate() {
            if n > 0 {
                x += GAP * s;
            }
            if matches!(o, Opt::Shadow | Opt::Background) {
                let w = text::layout(label(o, tool), FONT * s, false).width + 24.0 * s;
                push(&mut x, w, Part::Toggle(o));
                continue;
            }
            let lw = text::layout(label(o, tool), FONT * s, false).width;
            push(&mut x, lw, Part::Label(label(o, tool)));
            x += LGAP * s;
            match o {
                Opt::Color | Opt::FillColor | Opt::Highlight => push(&mut x, h, Part::Swatch(o)),
                Opt::Dash | Opt::Heads | Opt::StepType | Opt::SpotShape => {
                    for i in 0..choice_count(o) {
                        push(&mut x, SEG * s, Part::Seg(o, i));
                    }
                }
                _ => {
                    push(&mut x, STEP_BTN * s, Part::Minus(o));
                    push(&mut x, VALUE_W * s, Part::Value(o));
                    push(&mut x, STEP_BTN * s, Part::Plus(o));
                }
            }
        }
        out
    }

    pub fn width(&self) -> f32 {
        self.parts().last().map_or(0.0, |(r, _)| r.right() as f32 - self.x)
    }

    #[cfg(test)]
    fn rect(&self) -> Rect {
        Rect::new(self.x as i32, self.y as i32, self.width().ceil() as i32, (H * self.scale).ceil() as i32)
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<OptHit> {
        let (_, p) = self.parts().into_iter().find(|(r, _)| r.contains(x, y))?;
        match p {
            Part::Swatch(o) => Some(OptHit::Swatch(o)),
            Part::Minus(o) => Some(OptHit::Step(o, -1)),
            Part::Plus(o) => Some(OptHit::Step(o, 1)),
            Part::Seg(o, i) => Some(OptHit::Choice(o, i)),
            Part::Toggle(o) => Some(OptHit::Toggle(o)),
            Part::Label(_) | Part::Value(_) => None,
        }
    }

    /// The numeric option under (x, y), for the mouse wheel (label, − / value / +).
    pub fn stepper_at(&self, x: i32, y: i32) -> Option<Opt> {
        let parts = self.parts();
        let i = parts.iter().position(|(r, _)| r.contains(x, y))?;
        match parts[i].1 {
            Part::Minus(o) | Part::Value(o) | Part::Plus(o) => Some(o),
            Part::Label(_) => match parts.get(i + 1).map(|p| p.1) {
                Some(Part::Minus(o)) => Some(o),
                _ => None,
            },
            _ => None,
        }
    }

    /// Where a swatch is, to open its colour popup under it.
    pub fn swatch_rect(&self, o: Opt) -> Option<Rect> {
        self.parts().into_iter().find(|(_, p)| *p == Part::Swatch(o)).map(|(r, _)| r)
    }

    pub fn draw(&self, pm: &mut PixmapMut, t: &Theme, v: &Values) {
        let Some(tool) = self.tool else { return };
        let s = self.scale;
        let id = Transform::identity();
        let thin = Stroke { width: 1.0, ..Default::default() };
        let parts = self.parts();
        let hovered = |r: &Rect| self.hover.is_some_and(|(x, y)| r.contains(x, y));
        let centre_text = |pm: &mut PixmapMut, r: &Rect, txt: &str, c: Rgba, bold: bool| {
            let l = text::layout(txt, FONT * s, bold);
            text::draw(pm, txt, r.x as f32 + (r.w as f32 - l.width) / 2.0, r.y as f32 + (r.h as f32 - l.height) / 2.0, FONT * s, bold, c, None);
        };
        let mut tip: Option<(&str, Rect)> = None;
        for (i, (r, p)) in parts.iter().enumerate() {
            let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
            match *p {
                Part::Label(txt) => {
                    let l = text::layout(txt, FONT * s, false);
                    text::draw(pm, txt, x, y + (h - l.height) / 2.0, FONT * s, false, t.subtext, None);
                }
                Part::Swatch(o) => {
                    let inset = 3.0 * s;
                    if hovered(r) {
                        if let Some(bg) = rounded_rect(x, y, w, h, 6.0 * s) {
                            pm.fill_path(&bg, &t.hover.paint(), FillRule::Winding, id, None);
                        }
                    }
                    if let Some(sw) = rounded_rect(x + inset, y + inset, w - inset * 2.0, h - inset * 2.0, 4.0 * s) {
                        match v.swatch(o) {
                            Some(c) => pm.fill_path(&sw, &c.paint(), FillRule::Winding, id, None),
                            None => {
                                pm.fill_path(&sw, &Rgba::WHITE.paint(), FillRule::Winding, id, None);
                                let mut pb = PathBuilder::new();
                                pb.move_to(x + inset + 2.0, y + h - inset - 2.0);
                                pb.line_to(x + w - inset - 2.0, y + inset + 2.0);
                                if let Some(l) = pb.finish() {
                                    pm.stroke_path(&l, &Rgba::rgb(220, 38, 38).paint(), &Stroke { width: 2.0 * s, ..Default::default() }, id, None);
                                }
                            }
                        }
                        pm.stroke_path(&sw, &t.border.paint(), &thin, id, None);
                    }
                    if hovered(r) {
                        tip = Some((
                            match o {
                                Opt::FillColor => "Fill colour",
                                Opt::Highlight => "Highlight colour",
                                _ => "Colour",
                            },
                            *r,
                        ));
                    }
                }
                Part::Minus(_) | Part::Plus(_) => {
                    let minus = matches!(p, Part::Minus(_));
                    if let Some(bg) = rounded_rect(x, y + 2.0 * s, w, h - 4.0 * s, 5.0 * s) {
                        pm.fill_path(&bg, &(if hovered(r) { t.hover } else { t.raised }).paint(), FillRule::Winding, id, None);
                    }
                    centre_text(pm, r, if minus { "−" } else { "+" }, t.text, true);
                }
                Part::Value(o) => centre_text(pm, r, &v.value_text(o), t.text, false),
                Part::Seg(o, n) => {
                    let first = n == 0;
                    let last = !matches!(parts.get(i + 1), Some((_, Part::Seg(o2, _))) if *o2 == o);
                    // Group background, drawn once from the first cell.
                    if first {
                        let gw = SEG * s * choice_count(o) as f32;
                        if let Some(bg) = rounded_rect(x, y + 1.0, gw, h - 2.0, 6.0 * s) {
                            pm.fill_path(&bg, &t.raised.paint(), FillRule::Winding, id, None);
                        }
                    }
                    let _ = last;
                    let on = v.choice(o) == n;
                    if on || hovered(r) {
                        if let Some(c) = rounded_rect(x + 2.0, y + 3.0, w - 4.0, h - 6.0, 5.0 * s) {
                            pm.fill_path(&c, &(if on { t.accent } else { t.hover }).paint(), FillRule::Winding, id, None);
                        }
                    }
                    let fg = if on { t.accent_text } else { t.text };
                    draw_choice(pm, o, n, r, fg, s);
                    if hovered(r) {
                        tip = Some((choice_name(o, n), *r));
                    }
                }
                Part::Toggle(o) => {
                    if hovered(r) {
                        if let Some(bg) = rounded_rect(x, y, w, h, 6.0 * s) {
                            pm.fill_path(&bg, &t.hover.paint(), FillRule::Winding, id, None);
                        }
                    }
                    let b = 14.0 * s;
                    let (bx, by) = (x + 4.0 * s, y + (h - b) / 2.0);
                    let on = v.toggled(o);
                    if let Some(bp) = rounded_rect(bx, by, b, b, 3.0 * s) {
                        if on {
                            pm.fill_path(&bp, &t.accent.paint(), FillRule::Winding, id, None);
                            let mut pb = PathBuilder::new();
                            pb.move_to(bx + b * 0.22, by + b * 0.52);
                            pb.line_to(bx + b * 0.42, by + b * 0.72);
                            pb.line_to(bx + b * 0.78, by + b * 0.3);
                            if let Some(c) = pb.finish() {
                                let st = Stroke { width: 1.8 * s, line_cap: LineCap::Round, ..Default::default() };
                                pm.stroke_path(&c, &t.accent_text.paint(), &st, id, None);
                            }
                        } else {
                            pm.stroke_path(&bp, &t.subtext.paint(), &Stroke { width: 1.2 * s, ..Default::default() }, id, None);
                        }
                    }
                    let txt = label(o, tool);
                    let l = text::layout(txt, FONT * s, false);
                    text::draw(pm, txt, bx + b + 6.0 * s, y + (h - l.height) / 2.0, FONT * s, false, t.text, None);
                }
            }
        }
        if let Some((txt, r)) = tip {
            draw_tip(pm, t, txt, r.x as f32 + r.w as f32 / 2.0, r.bottom() as f32 + 6.0 * s, s);
        }
    }
}

/// The glyph on a choice cell: a dash sample, an arrow, a step label or a shape.
fn draw_choice(pm: &mut PixmapMut, o: Opt, n: usize, r: &Rect, c: Rgba, s: f32) {
    let id = Transform::identity();
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let half = r.w as f32 * 0.3;
    let line = |pm: &mut PixmapMut, x0: f32, x1: f32, dash: Option<Vec<f32>>| {
        let mut pb = PathBuilder::new();
        pb.move_to(x0, cy);
        pb.line_to(x1, cy);
        if let Some(p) = pb.finish() {
            let st = Stroke { width: 2.0 * s, line_cap: LineCap::Round, dash: dash.and_then(|d| StrokeDash::new(d, 0.0)), ..Default::default() };
            pm.stroke_path(&p, &c.paint(), &st, id, None);
        }
    };
    let head = |pm: &mut PixmapMut, tip: f32, dir: f32| {
        let mut pb = PathBuilder::new();
        pb.move_to(tip, cy);
        pb.line_to(tip - dir * 5.0 * s, cy - 4.0 * s);
        pb.line_to(tip - dir * 5.0 * s, cy + 4.0 * s);
        pb.close();
        if let Some(p) = pb.finish() {
            pm.fill_path(&p, &c.paint(), FillRule::Winding, id, None);
        }
    };
    match o {
        Opt::Dash => {
            let d = match Dash::ALL[n] {
                Dash::Solid => None,
                Dash::Dash => Some(vec![4.0 * s, 3.0 * s]),
                Dash::Dot => Some(vec![0.01, 3.5 * s]),
                Dash::DashDot => Some(vec![4.0 * s, 3.0 * s, 0.01, 3.0 * s]),
            };
            line(pm, cx - half, cx + half, d);
        }
        Opt::Heads => {
            let (start, end) = match ArrowHeads::ALL[n] {
                ArrowHeads::End => (false, true),
                ArrowHeads::Start => (true, false),
                ArrowHeads::Both => (true, true),
            };
            line(pm, cx - half + if start { 3.0 * s } else { 0.0 }, cx + half - if end { 3.0 * s } else { 0.0 }, None);
            if end {
                head(pm, cx + half, 1.0);
            }
            if start {
                head(pm, cx - half, -1.0);
            }
        }
        Opt::StepType => {
            let txt = StepType::ALL[n].sample();
            let size = FONT * s;
            let l = text::layout(txt, size, true);
            text::draw(pm, txt, cx - l.width / 2.0, cy - l.height / 2.0, size, true, c, None);
        }
        Opt::SpotShape => {
            let (w, h) = (r.w as f32 * 0.55, r.h as f32 * 0.42);
            if let Some(rr) = snapit_gfx::tiny_skia::Rect::from_xywh(cx - w / 2.0, cy - h / 2.0, w, h) {
                let p = if n == 1 { PathBuilder::from_oval(rr) } else { Some(PathBuilder::from_rect(rr)) };
                if let Some(p) = p {
                    pm.stroke_path(&p, &c.paint(), &Stroke { width: 1.6 * s, ..Default::default() }, id, None);
                }
            }
        }
        _ => {}
    }
}

/// Colour choices for a swatch: the palette, "no fill" (fill swatches only) and a custom colour.
pub struct ColorPopup {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub palette: Vec<Rgba>,
    pub allow_none: bool,
    pub current: Option<Rgba>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ColorHit {
    Color(Rgba),
    None,
    Custom,
}

const CELL: f32 = 24.0;
const CGAP: f32 = 4.0;
const COLS: usize = 5;

impl ColorPopup {
    fn cells(&self) -> Vec<(Rect, ColorHit)> {
        let s = self.scale;
        let (x0, y0) = (self.x + 8.0 * s, self.y + 8.0 * s);
        let step = (CELL + CGAP) * s;
        let mut v: Vec<(Rect, ColorHit)> = self
            .palette
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (cx, cy) = ((i % COLS) as f32, (i / COLS) as f32);
                (Rect::new((x0 + cx * step) as i32, (y0 + cy * step) as i32, (CELL * s) as i32, (CELL * s) as i32), ColorHit::Color(*c))
            })
            .collect();
        let rows = self.palette.len().div_ceil(COLS) as f32;
        let yb = y0 + rows * step + 2.0 * s;
        let full_w = COLS as f32 * step - CGAP * s;
        if self.allow_none {
            let half = (full_w - CGAP * s) / 2.0;
            v.push((Rect::new(x0 as i32, yb as i32, half as i32, (CELL * s) as i32), ColorHit::None));
            v.push((Rect::new((x0 + half + CGAP * s) as i32, yb as i32, half as i32, (CELL * s) as i32), ColorHit::Custom));
        } else {
            v.push((Rect::new(x0 as i32, yb as i32, full_w as i32, (CELL * s) as i32), ColorHit::Custom));
        }
        v
    }

    pub fn rect(&self) -> Rect {
        let s = self.scale;
        let cells = self.cells();
        let bottom = cells.iter().map(|(r, _)| r.bottom()).max().unwrap_or(0);
        let right = cells.iter().map(|(r, _)| r.right()).max().unwrap_or(0);
        Rect::new(self.x as i32, self.y as i32, right - self.x as i32 + (8.0 * s) as i32, bottom - self.y as i32 + (8.0 * s) as i32)
    }

    pub fn hit(&self, x: i32, y: i32) -> Option<ColorHit> {
        self.cells().into_iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| h)
    }

    pub fn draw(&self, pm: &mut PixmapMut, t: &Theme) {
        let s = self.scale;
        let id = Transform::identity();
        let thin = Stroke { width: 1.0, ..Default::default() };
        let r = self.rect();
        if let Some(bg) = rounded_rect(r.x as f32, r.y as f32, r.w as f32, r.h as f32, 9.0 * s) {
            if let Some(sh) = rounded_rect(r.x as f32 + 1.0, r.y as f32 + 3.0 * s, r.w as f32, r.h as f32, 9.0 * s) {
                pm.fill_path(&sh, &Rgba::new(0, 0, 0, 60).paint(), FillRule::Winding, id, None);
            }
            pm.fill_path(&bg, &t.panel.paint(), FillRule::Winding, id, None);
            pm.stroke_path(&bg, &t.border.paint(), &thin, id, None);
        }
        for (c, h) in self.cells() {
            let (x, y, w, hh) = (c.x as f32, c.y as f32, c.w as f32, c.h as f32);
            let selected = match h {
                ColorHit::Color(col) => self.current == Some(col),
                ColorHit::None => self.current.is_none(),
                ColorHit::Custom => false,
            };
            if selected {
                if let Some(p) = rounded_rect(x - 2.0, y - 2.0, w + 4.0, hh + 4.0, 6.0 * s) {
                    pm.stroke_path(&p, &t.accent.paint(), &Stroke { width: 2.0 * s, ..Default::default() }, id, None);
                }
            }
            let Some(p) = rounded_rect(x, y, w, hh, 5.0 * s) else { continue };
            match h {
                ColorHit::Color(col) => {
                    pm.fill_path(&p, &col.paint(), FillRule::Winding, id, None);
                    pm.stroke_path(&p, &t.border.paint(), &thin, id, None);
                }
                ColorHit::None | ColorHit::Custom => {
                    pm.fill_path(&p, &t.raised.paint(), FillRule::Winding, id, None);
                    let txt = if h == ColorHit::None { "No fill" } else { "Custom…" };
                    let size = 12.0 * s;
                    let l = text::layout(txt, size, false);
                    text::draw(pm, txt, x + (w - l.width) / 2.0, y + (hh - l.height) / 2.0, size, false, t.text, None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> Values {
        Values { style: Style::default(), opts: ToolOptions::default(), marker: Rgba::YELLOW }
    }

    #[test]
    fn every_drawing_tool_has_options_and_steppers_clamp() {
        for t in [Tool::Rect, Tool::Ellipse, Tool::Line, Tool::Arrow, Tool::Pen, Tool::Text, Tool::Step, Tool::Blur, Tool::Pixelate, Tool::Magnify, Tool::Spotlight] {
            assert!(!options_for(t).is_empty(), "{t:?}");
        }
        let mut v = values();
        v.style.width = 1.0;
        assert!(!v.step(Opt::Width, -1, Tool::Line), "lines keep a border");
        assert!(v.step(Opt::Width, -1, Tool::Rect), "rectangles may drop it");
        assert_eq!(v.style.width, 0.0);
        v.opts.zoom = 8.0;
        assert!(!v.step(Opt::Zoom, 1, Tool::Magnify));
    }

    #[test]
    fn choices_toggles_and_fill() {
        let mut v = values();
        v.choose(Opt::Heads, 2);
        assert_eq!(v.opts.heads, ArrowHeads::Both);
        assert_eq!(v.choice(Opt::Heads), 2);
        v.toggle(Opt::Shadow);
        assert!(v.toggled(Opt::Shadow));
        assert_eq!(v.swatch(Opt::FillColor), None);
        v.set_swatch(Opt::FillColor, Some(Rgba::BLACK));
        assert_eq!(v.swatch(Opt::FillColor), Some(Rgba::BLACK));
        v.set_swatch(Opt::FillColor, None);
        assert!(!v.opts.fill);
    }

    #[test]
    fn bar_layout_hits_its_controls() {
        let bar = OptionsBar { x: 10.0, y: 5.0, scale: 1.0, tool: Some(Tool::Arrow), hover: None };
        let r = bar.rect();
        assert!(r.w > 200 && r.h == 28);
        let sw = bar.swatch_rect(Opt::Color).unwrap();
        assert_eq!(bar.hit(sw.x + 2, sw.y + 2), Some(OptHit::Swatch(Opt::Color)));
        // Somewhere along the bar there is a "Both heads" segment.
        let found = (r.x..r.right()).any(|x| bar.hit(x, r.y + 14) == Some(OptHit::Choice(Opt::Heads, 2)));
        assert!(found);
        assert_eq!(OptionsBar { tool: Some(Tool::Pointer), ..bar }.width(), 0.0);
    }
}
