//! Schematic annotations: lines, text and text boxes used to separate the sections of a circuit
//! and to write notes. They are drawings only: they never connect anything.

use crate::app::{App, Drag, SchTool, Sel, View};
use crate::checks::dist_pt_seg;
use crate::model::{Note, NoteColor, NoteKind, GRID};
use eframe::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, RichText, Shape, Stroke, Vec2};

/// Space between a box's border and its text.
pub const PAD: f32 = 6.0;
pub const SIZES: [(&str, i32); 4] = [("Small", 10), ("Normal", 14), ("Large", 20), ("Title", 30)];

pub fn color(c: NoteColor) -> Color32 {
    let (r, g, b) = c.rgb();
    Color32::from_rgb(r, g, b)
}

fn fp(p: (i32, i32)) -> (f32, f32) {
    (p.0 as f32, p.1 as f32)
}

/// Is the point on the note? Lines by their stroke, text and filled boxes anywhere on them,
/// empty frames only by their border or their text (so a circuit inside stays easy to select).
fn hit(n: &Note, w: (f32, f32), tol: f32) -> bool {
    let (x0, y0, x1, y1) = n.bbox();
    let within = |r: (f32, f32, f32, f32)| w.0 >= r.0 - tol && w.0 <= r.2 + tol && w.1 >= r.1 - tol && w.1 <= r.3 + tol;
    let b = (x0 as f32, y0 as f32, x1 as f32, y1 as f32);
    match n.kind {
        NoteKind::Line => dist_pt_seg(w, fp(n.a), fp(n.b)) < tol,
        NoteKind::Text => within(b),
        NoteKind::Box => {
            if !within(b) {
                return false;
            }
            let inside = w.0 > b.0 + tol && w.0 < b.2 - tol && w.1 > b.1 + tol && w.1 < b.3 - tol;
            let (tw, th) = n.text_extent();
            n.filled || !inside || (!n.text.is_empty() && within((b.0, b.1, (b.0 + PAD + tw).min(b.2), (b.1 + PAD + th).min(b.3))))
        }
    }
}

/// The draggable points of a selected note: (point that moves, point that stays, moves end `a`).
fn handles(n: &Note) -> Vec<((i32, i32), (i32, i32), bool)> {
    match n.kind {
        NoteKind::Line => vec![(n.a, n.b, true), (n.b, n.a, false)],
        NoteKind::Box => {
            let (a, b) = (n.a, n.b);
            vec![(a, b, true), (b, a, false), ((b.0, a.1), (a.0, b.1), false), ((a.0, b.1), (b.0, a.1), false)]
        }
        NoteKind::Text => vec![],
    }
}

fn dashed_line(painter: &Painter, pts: &[Pos2], st: Stroke, dashed: bool, z: f32) {
    if dashed {
        painter.extend(Shape::dashed_line(pts, st, (8.0 * z).max(3.0), (5.0 * z).max(2.0)));
    } else {
        painter.add(Shape::line(pts.to_vec(), st));
    }
}

pub fn draw_note(painter: &Painter, view: View, min: Pos2, n: &Note, selected: bool, ghost: bool) {
    let z = view.zoom;
    let base = color(n.color);
    let col = if selected { crate::sch::SELECT } else if ghost { base.gamma_multiply(0.55) } else { base };
    let width = (1.2 * z).clamp(1.0, 3.0) * if selected { 1.6 } else { 1.0 };
    let st = Stroke::new(width, col);
    let font = FontId::proportional(n.size as f32 * z);
    let readable = n.size as f32 * z >= 2.0;
    match n.kind {
        NoteKind::Line => dashed_line(painter, &[view.s(min, fp(n.a)), view.s(min, fp(n.b))], st, n.dashed, z),
        NoteKind::Text => {
            let p = view.s(min, fp(n.a));
            if n.text.is_empty() {
                if readable {
                    painter.text(p, Align2::LEFT_TOP, "(empty text)", font, Color32::from_gray(150));
                }
            } else if readable {
                painter.text(p, Align2::LEFT_TOP, &n.text, font, col);
            }
            if selected {
                let (x0, y0, x1, y1) = n.bbox();
                let r = Rect::from_two_pos(view.s(min, (x0 as f32, y0 as f32)), view.s(min, (x1 as f32, y1 as f32)));
                painter.rect_stroke(r.expand(3.0), 2.0, Stroke::new(1.0_f32, col));
            }
        }
        NoteKind::Box => {
            let r = Rect::from_two_pos(view.s(min, fp(n.a)), view.s(min, fp(n.b)));
            if n.filled {
                let (red, green, blue) = n.color.fill();
                painter.rect_filled(r, 0.0, Color32::from_rgb(red, green, blue));
            }
            let k = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
            dashed_line(painter, &k, st, n.dashed, z);
            if !n.text.is_empty() && readable {
                let inner = r.shrink(PAD * z);
                if inner.is_positive() {
                    let galley = painter.layout(n.text.clone(), font, col, inner.width());
                    painter.with_clip_rect(r.intersect(painter.clip_rect())).galley(inner.min, galley, col);
                }
            }
        }
    }
    if selected {
        for (q, _, _) in handles(n) {
            let h = Rect::from_center_size(view.s(min, fp(q)), Vec2::splat(7.0));
            painter.rect(h, 1.0, Color32::WHITE, Stroke::new(1.5_f32, col));
        }
    }
}

impl App {
    pub fn note_hit(&self, w: (f32, f32), tol: f32) -> Sel {
        // Lines and text sit on top of boxes.
        for boxes in [false, true] {
            for (i, n) in self.project.notes.iter().enumerate().rev() {
                if (n.kind == NoteKind::Box) == boxes && hit(n, w, tol) {
                    return Sel::Note(i);
                }
            }
        }
        Sel::None
    }

    /// Start resizing if `w` is on a handle of the one selected note.
    pub fn grab_note_handle(&mut self, w: (f32, f32), tol: f32) -> bool {
        let (true, Sel::Note(idx)) = (self.group.is_empty(), self.sel) else { return false };
        let Some(n) = self.project.notes.get(idx) else { return false };
        let found = handles(n).into_iter().find(|(q, _, _)| (q.0 as f32 - w.0).abs() <= tol && (q.1 as f32 - w.1).abs() <= tol);
        let Some((_, fixed, is_a)) = found else { return false };
        self.checkpoint();
        self.drag = Some(Drag::NoteResize { idx, fixed, is_a });
        true
    }

    pub fn resize_note(&mut self, idx: usize, fixed: (i32, i32), is_a: bool, s: (i32, i32)) {
        let Some(n) = self.project.notes.get_mut(idx) else { return };
        match n.kind {
            // A box is kept as (top-left, bottom-right).
            NoteKind::Box => {
                n.a = (fixed.0.min(s.0), fixed.1.min(s.1));
                n.b = (fixed.0.max(s.0), fixed.1.max(s.1));
            }
            _ if is_a => n.a = s,
            _ => n.b = s,
        }
        self.analysis_dirty = true;
    }

    /// A new note with the current style.
    fn styled_note(&self, kind: NoteKind, a: (i32, i32), b: (i32, i32)) -> Note {
        let mut n = Note { kind, a, b, text: String::new(), ..self.note_style.clone() };
        n.normalize();
        n
    }

    fn add_note(&mut self, n: Note) {
        self.checkpoint();
        self.project.notes.push(n);
    }

    /// Add a box or text, select it and put the cursor in its text field.
    fn add_and_edit(&mut self, n: Note) {
        self.add_note(n);
        self.set_selection(vec![Sel::Note(self.project.notes.len() - 1)]);
        self.sch_tool = SchTool::Select;
        self.focus_value = true;
        self.status = "Type the text in the Properties panel.".into();
    }

    /// Mouse input for the Line, Box and Text tools.
    pub fn note_tool_input(&mut self, kind: NoteKind, resp: &egui::Response, ui: &egui::Ui, view: View, min: Pos2, s: Option<(i32, i32)>) {
        let draft = self.note_draft.filter(|d| d.0 == kind).map(|d| d.1);
        match kind {
            NoteKind::Text => {
                if resp.clicked() {
                    if let Some(s) = s {
                        let n = self.styled_note(kind, s, s);
                        self.add_and_edit(n);
                    }
                }
            }
            NoteKind::Line => {
                if resp.clicked() {
                    if let Some(s) = s {
                        match draft {
                            Some(a) if a != s => {
                                let n = self.styled_note(kind, a, s);
                                self.add_note(n);
                                self.note_draft = Some((kind, s));
                            }
                            Some(_) => self.note_draft = None,
                            None => self.note_draft = Some((kind, s)),
                        }
                    }
                }
                if resp.secondary_clicked() {
                    if self.note_draft.take().is_none() {
                        self.sch_tool = SchTool::Select;
                    }
                }
            }
            NoteKind::Box => {
                // Either click two opposite corners, or press and drag.
                if resp.drag_started_by(egui::PointerButton::Primary) && draft.is_none() {
                    if let Some(o) = ui.input(|i| i.pointer.press_origin()) {
                        let w = view.w(min, o);
                        self.note_draft = Some((kind, (crate::sch::snap_i(w.0), crate::sch::snap_i(w.1))));
                    }
                }
                let done = resp.clicked() || resp.drag_stopped_by(egui::PointerButton::Primary);
                if let (true, Some(s)) = (done, s) {
                    match self.note_draft.filter(|d| d.0 == kind) {
                        Some((_, a)) if a.0 != s.0 && a.1 != s.1 => {
                            self.note_draft = None;
                            let n = self.styled_note(kind, a, s);
                            self.add_and_edit(n);
                        }
                        Some(_) => {}
                        None => self.note_draft = Some((kind, s)),
                    }
                }
                if resp.secondary_clicked() {
                    self.note_draft = None;
                    self.sch_tool = SchTool::Select;
                }
            }
        }
    }

    pub fn draw_note_preview(&self, kind: NoteKind, painter: &Painter, view: View, min: Pos2, s: (i32, i32)) {
        painter.circle_stroke(view.s(min, fp(s)), 4.0, Stroke::new(1.0_f32, color(self.note_style.color)));
        if let Some((k, a)) = self.note_draft {
            if k == kind {
                draw_note(painter, view, min, &self.styled_note(kind, a, s), false, true);
            }
        }
    }

    pub fn duplicate_note(&mut self, i: usize) {
        let Some(mut n) = self.project.notes.get(i).cloned() else { return };
        n.shift((2 * GRID, 2 * GRID));
        self.add_note(n);
        self.set_selection(vec![Sel::Note(self.project.notes.len() - 1)]);
    }

    pub fn note_properties(&mut self, ui: &mut egui::Ui, i: usize) {
        let Some(mut n) = self.project.notes.get(i).cloned() else { return };
        ui.label(RichText::new(n.kind.name()).strong().size(15.0));
        if n.kind != NoteKind::Line {
            let hint = if n.kind == NoteKind::Box { "Notes (optional)" } else { "Text" };
            let r = ui.add(egui::TextEdit::multiline(&mut n.text).hint_text(hint).desired_rows(3).desired_width(ui.available_width()));
            if self.focus_value {
                r.request_focus();
                self.focus_value = false;
            }
            if r.gained_focus() {
                self.undo.push(self.project.clone());
            }
            if r.changed() {
                self.dirty = true;
                self.edited_since_load = true;
                self.analysis_dirty = true;
                self.project.notes[i].text = n.text.clone();
            }
        }
        if note_style_controls(ui, n.kind, &mut n) {
            self.checkpoint();
            self.project.notes[i] = n;
        }
        ui.horizontal(|ui| {
            if ui.button("🔄 Rotate").on_hover_text("R").clicked() {
                self.rotate_selection();
            }
            if ui.button("📋 Copy").on_hover_text("Ctrl+D").clicked() {
                self.duplicate_note(i);
            }
            if ui.button("🗑 Delete").on_hover_text("Del").clicked() {
                self.delete_selection();
            }
        });
    }
}

/// Colour, size, dashes and fill. Returns true when something changed.
pub fn note_style_controls(ui: &mut egui::Ui, kind: NoteKind, n: &mut Note) -> bool {
    let before = n.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label("Colour");
        for c in NoteColor::ALL {
            let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::click());
            ui.painter().rect_filled(r.shrink(2.0), 3.0, color(c));
            if n.color == c {
                ui.painter().rect_stroke(r, 4.0, Stroke::new(1.5_f32, color(c)));
            }
            if resp.on_hover_text(c.name()).clicked() {
                n.color = c;
            }
        }
    });
    if kind != NoteKind::Line {
        ui.horizontal_wrapped(|ui| {
            ui.label("Size");
            for (name, v) in SIZES {
                if ui.selectable_label(n.size == v, name).clicked() {
                    n.size = v;
                }
            }
        });
    }
    if kind != NoteKind::Text {
        ui.horizontal(|ui| {
            ui.checkbox(&mut n.dashed, "Dashed");
            if kind == NoteKind::Box {
                ui.checkbox(&mut n.filled, "Filled").on_hover_text("On: a note card. Off: a frame around part of the circuit.");
            }
        });
    }
    *n != before
}
