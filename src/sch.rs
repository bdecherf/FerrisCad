//! Schematic editor canvas: drawing symbols and wires, placing parts, drawing wires.

use crate::app::*;
use crate::checks::{dist_pt_seg, Severity};
use crate::model::*;
use eframe::egui::{self, Align2, Color32, FontId, Painter, PointerButton, Pos2, Rect, Sense, Shape, Stroke, Vec2};

pub const SCH_BG: Color32 = Color32::from_rgb(251, 251, 247);
const GRID_DOT: Color32 = Color32::from_rgb(196, 198, 210);
const WIRE: Color32 = Color32::from_rgb(0, 138, 108);
const BODY: Color32 = Color32::from_rgb(38, 62, 128);
const FILL: Color32 = Color32::from_rgb(234, 240, 252);
const REF: Color32 = Color32::from_rgb(0, 102, 160);
const VALUE: Color32 = Color32::from_rgb(90, 90, 96);
pub const SELECT: Color32 = Color32::from_rgb(255, 140, 0);
const HILITE: Color32 = Color32::from_rgb(230, 60, 170);
const POWER: Color32 = Color32::from_rgb(170, 40, 40);

pub fn snap_i(v: f32) -> i32 {
    ((v / GRID as f32).round() as i32) * GRID
}

pub struct Sym<'a> {
    pub p: &'a Painter,
    pub v: View,
    pub min: Pos2,
    pub o: (f32, f32),
    pub r: u8,
    pub st: Stroke,
    pub fill: Color32,
    /// When set, shapes are recorded (in the symbol's own coordinates) instead of painted.
    pub rec: Option<&'a std::cell::RefCell<Vec<Prim>>>,
}

/// A recorded drawing primitive, in symbol coordinates (schematic units, y down).
#[derive(Clone, Debug)]
pub enum Prim {
    Line(Vec<(f32, f32)>, bool),
    Circle(f32, f32, f32, bool),
    Text(f32, f32, String),
}

impl Sym<'_> {
    fn record(&self, p: Prim) -> bool {
        match self.rec {
            Some(r) => {
                r.borrow_mut().push(p);
                true
            }
            None => false,
        }
    }
    pub fn pt(&self, x: f32, y: f32) -> Pos2 {
        let (rx, ry) = rot_f((x, y), self.r);
        self.v.s(self.min, (self.o.0 + rx, self.o.1 + ry))
    }
    pub fn l(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        if self.record(Prim::Line(vec![(x0, y0), (x1, y1)], false)) {
            return;
        }
        self.p.line_segment([self.pt(x0, y0), self.pt(x1, y1)], self.st);
    }
    pub fn thick(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        if self.record(Prim::Line(vec![(x0, y0), (x1, y1)], false)) {
            return;
        }
        self.p.line_segment([self.pt(x0, y0), self.pt(x1, y1)], Stroke::new(self.st.width * 2.0, self.st.color));
    }
    pub fn pl(&self, pts: &[(f32, f32)]) {
        if self.record(Prim::Line(pts.to_vec(), false)) {
            return;
        }
        self.p.add(Shape::line(pts.iter().map(|&(x, y)| self.pt(x, y)).collect(), self.st));
    }
    pub fn poly(&self, pts: &[(f32, f32)], fill: Color32) {
        let mut closed = pts.to_vec();
        if let Some(f) = pts.first() {
            closed.push(*f);
        }
        if self.record(Prim::Line(closed, fill.a() > 0)) {
            return;
        }
        self.p.add(Shape::convex_polygon(pts.iter().map(|&(x, y)| self.pt(x, y)).collect(), fill, self.st));
    }
    pub fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.poly(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)], self.fill);
    }
    pub fn circ(&self, x: f32, y: f32, r: f32, fill: Color32) {
        if self.record(Prim::Circle(x, y, r, fill.a() > 0)) {
            return;
        }
        self.p.circle(self.pt(x, y), r * self.v.zoom, fill, self.st);
    }
    pub fn arc(&self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
        let n = 10;
        let pts: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f32 / n as f32;
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        self.pl(&pts);
    }
    pub fn text(&self, x: f32, y: f32, s: &str, size: f32, color: Color32, align: Align2) {
        if self.record(Prim::Text(x, y, s.to_string())) {
            return;
        }
        let px = size * self.v.zoom;
        if px >= 4.0 {
            self.p.text(self.pt(x, y), align, s, FontId::proportional(px), color);
        }
    }
    pub fn arrow(&self, x0: f32, y0: f32, x1: f32, y1: f32, head: f32) {
        self.l(x0, y0, x1, y1);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len = (dx * dx + dy * dy).sqrt().max(0.01);
        let (ux, uy) = (dx / len, dy / len);
        let (bx, by) = (x1 - ux * head, y1 - uy * head);
        let (nx, ny) = (-uy * head * 0.5, ux * head * 0.5);
        self.poly(&[(x1, y1), (bx + nx, by + ny), (bx - nx, by - ny)], self.st.color);
    }
}

/// Draw one schematic symbol.
#[allow(clippy::too_many_arguments)]
pub fn draw_symbol(
    p: &Painter,
    v: View,
    min: Pos2,
    kind: Kind,
    reference: &str,
    value: &str,
    pos: (i32, i32),
    rot: u8,
    alpha: f32,
    sel: bool,
    cu: Option<&Custom>,
) {
    draw_symbol_rec(p, v, min, kind, reference, value, pos, rot, alpha, sel, cu, None);
}

/// The drawing of a built-in symbol as primitives (for the KiCad export).
pub fn record_builtin(kind: Kind, value: &str) -> Vec<Prim> {
    let cell = std::cell::RefCell::new(vec![]);
    let ctx = egui::Context::default();
    let painter = Painter::new(ctx, egui::LayerId::background(), Rect::EVERYTHING);
    let view = View { offset: Vec2::ZERO, zoom: 1.0 };
    draw_symbol_rec(&painter, view, Pos2::ZERO, kind, "", value, (0, 0), 0, 1.0, false, None, Some(&cell));
    cell.into_inner()
}

#[allow(clippy::too_many_arguments)]
fn draw_symbol_rec(
    p: &Painter,
    v: View,
    min: Pos2,
    kind: Kind,
    reference: &str,
    value: &str,
    pos: (i32, i32),
    rot: u8,
    alpha: f32,
    sel: bool,
    cu: Option<&Custom>,
    rec: Option<&std::cell::RefCell<Vec<Prim>>>,
) {
    let a = |c: Color32| c.gamma_multiply(alpha);
    let w = (1.4 * v.zoom / 1.6).clamp(0.8, 3.0);
    let body = if kind.is_power() { if kind == Kind::Gnd { a(Color32::from_rgb(40, 40, 48)) } else { a(POWER) } } else { a(BODY) };
    let s = Sym { p, v, min, o: (pos.0 as f32, pos.1 as f32), r: rot, st: Stroke::new(w, body), fill: a(FILL), rec };
    let inf = info(kind);
    if let Some(cu) = cu {
        draw_custom(&s, cu, kind, reference, value, sel, a(REF), a(VALUE));
        return;
    }
    if sel {
        let (x0, y0, x1, y1) = inf.body;
        let c0 = s.pt(x0 as f32 - 4.0, y0 as f32 - 4.0);
        let c1 = s.pt(x1 as f32 + 4.0, y1 as f32 + 4.0);
        p.rect_filled(Rect::from_two_pos(c0, c1), 3.0, Color32::from_rgba_unmultiplied(255, 170, 40, 50));
        p.rect_stroke(Rect::from_two_pos(c0, c1), 3.0, Stroke::new(1.5_f32, SELECT));
    }
    match kind {
        Kind::Resistor => {
            s.l(0.0, -30.0, 0.0, -20.0);
            s.rect(-7.0, -20.0, 7.0, 20.0);
            s.l(0.0, 20.0, 0.0, 30.0);
        }
        Kind::Capacitor => {
            s.l(0.0, -30.0, 0.0, -4.0);
            s.l(0.0, 4.0, 0.0, 30.0);
            s.thick(-13.0, -4.0, 13.0, -4.0);
            s.thick(-13.0, 4.0, 13.0, 4.0);
        }
        Kind::CapPolarized => {
            s.l(0.0, -30.0, 0.0, -4.0);
            s.l(0.0, 5.0, 0.0, 30.0);
            s.thick(-13.0, -4.0, 13.0, -4.0);
            s.pl(&[(-13.0, 9.0), (-7.0, 5.8), (0.0, 4.8), (7.0, 5.8), (13.0, 9.0)]);
            s.l(-13.0, -12.0, -7.0, -12.0);
            s.l(-10.0, -15.0, -10.0, -9.0);
        }
        Kind::Inductor => {
            s.l(0.0, -30.0, 0.0, -20.0);
            for i in 0..4 {
                s.arc(0.0, -15.0 + 10.0 * i as f32, 5.0, -std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
            }
            s.l(0.0, 20.0, 0.0, 30.0);
        }
        Kind::Diode | Kind::Led => {
            s.l(0.0, -30.0, 0.0, -8.0);
            s.poly(&[(-10.0, -8.0), (10.0, -8.0), (0.0, 8.0)], s.fill);
            s.thick(-10.0, 8.0, 10.0, 8.0);
            s.l(0.0, 8.0, 0.0, 30.0);
            if kind == Kind::Led {
                s.arrow(11.0, -3.0, 19.0, -11.0, 4.0);
                s.arrow(11.0, 4.0, 19.0, -4.0, 4.0);
            }
        }
        Kind::Battery => {
            s.l(0.0, -30.0, 0.0, -8.0);
            s.thick(-14.0, -8.0, 14.0, -8.0);
            s.l(-7.0, -3.0, 7.0, -3.0);
            s.thick(-14.0, 2.0, 14.0, 2.0);
            s.l(-7.0, 7.0, 7.0, 7.0);
            s.l(0.0, 7.0, 0.0, 30.0);
            s.l(-20.0, -16.0, -14.0, -16.0);
            s.l(-17.0, -19.0, -17.0, -13.0);
        }
        Kind::Switch => {
            s.l(0.0, -30.0, 0.0, -12.0);
            s.l(0.0, 12.0, 0.0, 30.0);
            s.circ(0.0, -12.0, 2.5, Color32::TRANSPARENT);
            s.circ(0.0, 12.0, 2.5, Color32::TRANSPARENT);
            if value.to_lowercase().contains("open") {
                s.l(0.0, 9.5, -11.0, -8.0);
            } else {
                s.l(0.0, 9.5, 2.0, -9.5);
            }
        }
        Kind::Connector2 => {
            s.rect(-10.0, -20.0, 10.0, 20.0);
            s.l(-30.0, -10.0, -10.0, -10.0);
            s.l(-30.0, 10.0, -10.0, 10.0);
            s.text(-4.0, -10.0, "1", 7.0, a(VALUE), Align2::LEFT_CENTER);
            s.text(-4.0, 10.0, "2", 7.0, a(VALUE), Align2::LEFT_CENTER);
        }
        Kind::Npn | Kind::Pnp => {
            s.circ(2.0, 0.0, 19.0, Color32::TRANSPARENT);
            s.l(-30.0, 0.0, -8.0, 0.0);
            s.thick(-8.0, -12.0, -8.0, 12.0);
            s.l(10.0, -18.0, 10.0, -30.0);
            s.l(10.0, 18.0, 10.0, 30.0);
            if kind == Kind::Npn {
                s.l(-8.0, -6.0, 10.0, -18.0);
                s.arrow(-8.0, 6.0, 10.0, 18.0, 6.0);
            } else {
                s.arrow(10.0, -18.0, -7.0, -6.5, 6.0);
                s.l(-8.0, 6.0, 10.0, 18.0);
            }
        }
        Kind::Nmos => {
            s.circ(2.0, 0.0, 19.0, Color32::TRANSPARENT);
            s.l(-30.0, 0.0, -12.0, 0.0);
            s.l(-12.0, -12.0, -12.0, 12.0);
            s.thick(-6.0, -14.0, -6.0, -6.0);
            s.thick(-6.0, -3.0, -6.0, 3.0);
            s.thick(-6.0, 6.0, -6.0, 14.0);
            s.pl(&[(-6.0, -10.0), (10.0, -10.0), (10.0, -30.0)]);
            s.pl(&[(-6.0, 10.0), (10.0, 10.0), (10.0, 30.0)]);
            s.l(10.0, 0.0, 10.0, 10.0);
            s.arrow(10.0, 0.0, -5.0, 0.0, 5.0);
        }
        Kind::OpAmp => {
            s.poly(&[(-30.0, -30.0), (30.0, 0.0), (-30.0, 30.0)], s.fill);
            s.l(-40.0, -10.0, -30.0, -10.0);
            s.l(-40.0, 10.0, -30.0, 10.0);
            s.l(30.0, 0.0, 40.0, 0.0);
            s.l(0.0, -30.0, 0.0, -15.0);
            s.l(0.0, 15.0, 0.0, 30.0);
            s.text(-25.0, -10.0, "−", 11.0, a(BODY), Align2::LEFT_CENTER);
            s.text(-25.0, 10.0, "+", 11.0, a(BODY), Align2::LEFT_CENTER);
        }
        Kind::Mcu8 => {
            s.rect(-40.0, -40.0, 40.0, 40.0);
            for pd in &inf.pins {
                let (px, py) = (pd.pos.0 as f32, pd.pos.1 as f32);
                let edge = if px < 0.0 { -40.0 } else { 40.0 };
                s.l(px, py, edge, py);
                let (tx, al) = if px < 0.0 { (-36.0, Align2::LEFT_CENTER) } else { (36.0, Align2::RIGHT_CENTER) };
                s.text(tx, py, &pd.name, 7.0, a(BODY), al);
                s.text((px + edge) / 2.0, py - 2.0, &pd.pad, 6.0, a(VALUE), Align2::CENTER_BOTTOM);
            }
        }
        Kind::Regulator => {
            s.rect(-30.0, -20.0, 30.0, 20.0);
            s.l(-50.0, 0.0, -30.0, 0.0);
            s.l(30.0, 0.0, 50.0, 0.0);
            s.l(0.0, 20.0, 0.0, 40.0);
            s.text(-27.0, 0.0, "IN", 7.0, a(BODY), Align2::LEFT_CENTER);
            s.text(27.0, 0.0, "OUT", 7.0, a(BODY), Align2::RIGHT_CENTER);
            s.text(0.0, 17.0, "GND", 7.0, a(BODY), Align2::CENTER_BOTTOM);
        }
        Kind::Generic => {
            s.rect(-10.0, -10.0, 10.0, 10.0);
        }
        Kind::Gnd => {
            s.l(0.0, 0.0, 0.0, 8.0);
            s.thick(-10.0, 8.0, 10.0, 8.0);
            s.l(-6.0, 12.0, 6.0, 12.0);
            s.l(-2.0, 16.0, 2.0, 16.0);
        }
        Kind::Power => {
            s.l(0.0, 0.0, 0.0, -10.0);
            s.thick(-10.0, -10.0, 10.0, -10.0);
            s.text(0.0, -13.0, value, 9.0, a(POWER), Align2::CENTER_BOTTOM);
        }
    }
    if kind.is_power() || rec.is_some() {
        return;
    }
    // Reference and value text.
    let (x0, y0, x1, y1) = inf.body;
    let c0 = s.pt(x0 as f32, y0 as f32);
    let c1 = s.pt(x1 as f32, y1 as f32);
    let bb = Rect::from_two_pos(c0, c1);
    let fs = 9.0 * v.zoom;
    if fs < 4.0 {
        return;
    }
    let two_pin = inf.pins.len() <= 2 && kind != Kind::Connector2;
    if two_pin && bb.height() >= bb.width() {
        p.text(Pos2::new(bb.right() + 4.0, bb.center().y - fs * 0.6), Align2::LEFT_CENTER, reference, FontId::proportional(fs), a(REF));
        p.text(Pos2::new(bb.right() + 4.0, bb.center().y + fs * 0.6), Align2::LEFT_CENTER, value, FontId::proportional(fs), a(VALUE));
    } else if two_pin || matches!(kind, Kind::Npn | Kind::Pnp | Kind::Nmos) {
        if matches!(kind, Kind::Npn | Kind::Pnp | Kind::Nmos) && bb.height() >= bb.width() {
            p.text(Pos2::new(bb.right() + 6.0, bb.center().y - fs * 0.6), Align2::LEFT_CENTER, reference, FontId::proportional(fs), a(REF));
            p.text(Pos2::new(bb.right() + 6.0, bb.center().y + fs * 0.6), Align2::LEFT_CENTER, value, FontId::proportional(fs), a(VALUE));
        } else {
            p.text(Pos2::new(bb.center().x, bb.top() - 2.0), Align2::CENTER_BOTTOM, reference, FontId::proportional(fs), a(REF));
            p.text(Pos2::new(bb.center().x, bb.bottom() + 2.0), Align2::CENTER_TOP, value, FontId::proportional(fs), a(VALUE));
        }
    } else {
        p.text(Pos2::new(bb.left(), bb.top() - 2.0), Align2::LEFT_BOTTOM, reference, FontId::proportional(fs), a(REF));
        p.text(Pos2::new(bb.left(), bb.bottom() + 2.0), Align2::LEFT_TOP, value, FontId::proportional(fs), a(VALUE));
    }
}

/// Draw an imported symbol from its own geometry.
#[allow(clippy::too_many_arguments)]
fn draw_custom(s: &Sym, cu: &Custom, kind: Kind, reference: &str, value: &str, sel: bool, ref_col: Color32, val_col: Color32) {
    if sel {
        for &(x0, y0, x1, y1) in &cu.boxes {
            let r = Rect::from_two_pos(s.pt(x0 as f32 - 3.0, y0 as f32 - 3.0), s.pt(x1 as f32 + 3.0, y1 as f32 + 3.0));
            s.p.rect_filled(r, 3.0, Color32::from_rgba_unmultiplied(255, 170, 40, 50));
            s.p.rect_stroke(r, 3.0, Stroke::new(1.5_f32, SELECT));
        }
    }
    for (pts, filled) in &cu.polys {
        if *filled && pts.len() >= 3 && pts.len() <= 5 {
            s.poly(pts, s.fill);
        } else {
            s.pl(pts);
        }
    }
    for &(x, y, r, filled) in &cu.circles {
        s.circ(x, y, r, if filled { s.fill } else { Color32::TRANSPARENT });
    }
    for (x, y, t) in &cu.texts {
        s.text(*x, *y, t, 6.5, s.st.color, Align2::CENTER_CENTER);
    }
    if kind.is_power() {
        if kind == Kind::Power {
            let at = cu.val_pos.unwrap_or((0.0, -16.0));
            s.text(at.0, at.1, value, 9.0, s.st.color, Align2::CENTER_CENTER);
        }
        return;
    }
    if let Some(at) = cu.ref_pos {
        s.text(at.0, at.1, reference, 9.0, ref_col, Align2::CENTER_CENTER);
    }
    if let Some(at) = cu.val_pos {
        s.text(at.0, at.1, value, 9.0, val_col, Align2::CENTER_CENTER);
    }
}

impl App {
    pub fn sch_hit(&self, w: (f32, f32), tol: f32) -> Sel {
        for (i, l) in self.project.labels.iter().enumerate().rev() {
            let (x, y) = (l.pos.0 as f32, l.pos.1 as f32);
            let len = l.text.len() as f32 * 6.0 + 6.0;
            if w.0 >= x - tol && w.0 <= x + len && w.1 >= y - 14.0 && w.1 <= y + tol {
                return Sel::Label(i);
            }
        }
        for c in self.project.components.iter().rev() {
            for b in c.sch_boxes() {
                let inner = (b.0 as f32 + 3.0, b.1 as f32 + 3.0, b.2 as f32 - 3.0, b.3 as f32 - 3.0);
                if w.0 >= inner.0 - tol && w.0 <= inner.2 + tol && w.1 >= inner.1 - tol && w.1 <= inner.3 + tol {
                    return Sel::Comp(c.id);
                }
            }
        }
        for (i, wr) in self.project.wires.iter().enumerate() {
            let d = dist_pt_seg(w, (wr.a.0 as f32, wr.a.1 as f32), (wr.b.0 as f32, wr.b.1 as f32));
            if d < tol {
                return Sel::Wire(i);
            }
        }
        self.note_hit(w, tol)
    }

    fn is_connection_point(&self, q: (i32, i32)) -> bool {
        for c in &self.project.components {
            for pd in c.info().pins.iter() {
                if c.pin_pos(pd) == q {
                    return true;
                }
            }
        }
        self.project.wires.iter().any(|w| on_segment(q, w.a, w.b))
    }

    fn wire_click(&mut self, s: (i32, i32)) {
        let conn = self.is_connection_point(s);
        let mut finish = false;
        match &mut self.wire_draw {
            None => self.wire_draw = Some(vec![s]),
            Some(pts) => {
                let last = *pts.last().unwrap();
                if s == last {
                    finish = true;
                } else {
                    if last.0 != s.0 && last.1 != s.1 {
                        pts.push((s.0, last.1));
                    }
                    pts.push(s);
                    finish = conn;
                }
            }
        }
        if finish {
            self.finish_wire();
        }
    }

    pub fn finish_wire(&mut self) {
        let Some(pts) = self.wire_draw.take() else { return };
        if pts.len() < 2 {
            return;
        }
        self.checkpoint();
        for w in pts.windows(2) {
            if w[0] != w[1] {
                self.project.wires.push(Wire { a: w[0], b: w[1] });
            }
        }
        self.status = "Wire added.".into();
    }

    fn sch_hover_info(&self, w: (f32, f32), tol: f32) -> String {
        let fmtv = |n: usize| self.sim.net_v.get(n).copied().flatten().map(|v| format!(" · {}", fmt_si(v, "V"))).unwrap_or_default();
        for (ci, c) in self.project.components.iter().enumerate() {
            for (pi, pd) in c.info().pins.iter().enumerate() {
                let q = c.pin_pos(pd);
                if (q.0 as f32 - w.0).abs() <= tol && (q.1 as f32 - w.1).abs() <= tol {
                    if let Some(n) = self.nl.net_of(ci, pi) {
                        if c.kind.is_power() {
                            return format!("{} supply{}", self.nl.nets[n].name, fmtv(n));
                        }
                        return format!("{} pin {}, net {}{}", c.reference, pd.name, self.nl.nets[n].name, fmtv(n));
                    }
                }
            }
        }
        match self.sch_hit(w, tol) {
            Sel::Comp(id) => {
                let c = &self.project.components[self.project.find(id).unwrap()];
                if c.kind.is_power() {
                    return format!("Power symbol {}", c.value);
                }
                let note = self.sim.notes.get(&id).map(|n| format!(" · {}", n)).unwrap_or_default();
                format!("{} ({} {}){}", c.reference, c.info().label, c.value, note)
            }
            Sel::Wire(i) => match self.nl.wire_net.get(i) {
                Some(&n) => format!("Wire on net {}{}", self.nl.nets[n].name, fmtv(n)),
                None => String::new(),
            },
            Sel::Label(i) => format!("Net label {}", self.project.labels[i].text),
            Sel::Note(i) => format!("{} (annotation)", self.project.notes[i].kind.name()),
            _ => String::new(),
        }
    }

    pub fn sch_canvas(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        self.canvas_rect = rect;
        if self.fit_sch {
            self.fit_view(Mode::Schematic, rect);
            self.fit_sch = false;
        }
        zoom_pan(&mut self.sch_view, &resp, ui, rect, 0.2, 8.0);
        let view = self.sch_view;
        let min = rect.min;
        let mouse_w = resp.hover_pos().map(|p| view.w(min, p));
        let snapped = mouse_w.map(|(x, y)| (snap_i(x), snap_i(y)));
        let tol = (6.0 / view.zoom).max(2.0);
        if let Some(w) = mouse_w {
            self.cursor = Some(w);
            self.hover_info = self.sch_hover_info(w, tol);
        }

        // ---- input
        match self.sch_tool {
            SchTool::Select => {
                let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                if resp.drag_started_by(PointerButton::Primary) {
                    if let Some(origin) = ui.input(|i| i.pointer.press_origin()) {
                        let w = view.w(min, origin);
                        let start = (snap_i(w.0), snap_i(w.1));
                        let hit0 = self.sch_hit(w, tol);
                        // Grab a wire end: drag it (and anything ending at the same point).
                        let near_end = self.project.wires.iter().flat_map(|wr| [wr.a, wr.b]).find(|q| {
                            (q.0 as f32 - w.0).abs() <= tol && (q.1 as f32 - w.1).abs() <= tol
                        });
                        let on_part = matches!(hit0, Sel::Comp(_) | Sel::Label(_));
                        let end_grab = near_end.filter(|_| !on_part || self.sch_hit(w, tol * 0.3) == Sel::None);
                        if !additive && self.grab_note_handle(w, tol) {
                            // Resizing a line or a box.
                        } else if !self.group.is_empty() && self.is_sel(hit0) {
                            // Move the whole selection.
                            self.start_sch_group_drag(start);
                        } else if additive || (hit0 == Sel::None && end_grab.is_none()) {
                            // Selection box.
                            self.drag = Some(Drag::Band { start: w, cur: w, add: additive });
                        } else if let Some(q) = end_grab {
                            self.group.clear();
                            self.checkpoint();
                            let ends: Vec<(usize, bool)> = self
                                .project
                                .wires
                                .iter()
                                .enumerate()
                                .flat_map(|(i, wr)| {
                                    let mut v = vec![];
                                    if wr.a == q {
                                        v.push((i, true));
                                    }
                                    if wr.b == q {
                                        v.push((i, false));
                                    }
                                    v
                                })
                                .collect();
                            self.drag = Some(Drag::SchWireEnd { start: q, ends });
                        } else {
                            self.group.clear();
                            match hit0 {
                                Sel::Comp(id) => {
                                    self.checkpoint();
                                    let ci = self.project.find(id).unwrap();
                                    // Pins touching another part's pin get a wire that stretches.
                                    let mine: Vec<(i32, i32)> = {
                                        let c = &self.project.components[ci];
                                        c.info().pins.iter().map(|pd| c.pin_pos(pd)).collect()
                                    };
                                    for q in mine {
                                        let touching = self.project.components.iter().enumerate().any(|(cj, o)| {
                                            cj != ci && o.info().pins.iter().any(|pd| o.pin_pos(pd) == q)
                                        });
                                        let wired = self.project.wires.iter().any(|wr| wr.a == q || wr.b == q);
                                        if touching && !wired {
                                            self.project.wires.push(Wire { a: q, b: q });
                                        }
                                    }
                                    let c = &self.project.components[ci];
                                    let mut attached = vec![];
                                    for (pi, pd) in c.info().pins.iter().enumerate() {
                                        let q = c.pin_pos(pd);
                                        for (wi, wr) in self.project.wires.iter().enumerate() {
                                            if wr.a == q && wr.b != q {
                                                attached.push((wi, true, pi));
                                            }
                                            if wr.b == q {
                                                attached.push((wi, false, pi));
                                            }
                                        }
                                    }
                                    let orig = c.pos;
                                    let marks = self.nc_marks(&[id].into());
                                    self.drag = Some(Drag::SchComp { id, start, orig, attached, marks });
                                    self.sel = Sel::Comp(id);
                                }
                                Sel::Label(idx) => {
                                    self.checkpoint();
                                    self.drag = Some(Drag::SchLabel { idx, start, orig: self.project.labels[idx].pos });
                                    self.sel = Sel::Label(idx);
                                }
                                Sel::Note(i) => {
                                    self.sel = Sel::Note(i);
                                    self.start_sch_group_drag(start);
                                }
                                Sel::Wire(i) => {
                                    // Move the whole wire; wires joined to its ends follow.
                                    self.sel = Sel::Wire(i);
                                    self.start_sch_group_drag(start);
                                }
                                other => self.sel = other,
                            }
                        }
                    }
                }
                if resp.dragged_by(PointerButton::Primary) {
                    if let (Some(d), Some(s)) = (self.drag.clone(), snapped) {
                        match d {
                            Drag::Band { start, add, .. } => {
                                if let Some(w) = mouse_w {
                                    self.drag = Some(Drag::Band { start, cur: w, add });
                                }
                            }
                            Drag::SchGroup { .. } => self.drag_sch_group(&d, s),
                            Drag::SchComp { id, start, orig, attached, marks } => {
                                if let Some(ci) = self.project.find(id) {
                                    let np = (orig.0 + s.0 - start.0, orig.1 + s.1 - start.1);
                                    if self.project.components[ci].pos != np {
                                        self.project.components[ci].pos = np;
                                        self.place_nc_marks(&marks);
                                        let c = self.project.components[ci].clone();
                                        let inf = c.info();
                                        for (wi, is_a, pi) in attached {
                                            let q = c.pin_pos(&inf.pins[pi]);
                                            if let Some(w) = self.project.wires.get_mut(wi) {
                                                if is_a {
                                                    w.a = q
                                                } else {
                                                    w.b = q
                                                }
                                            }
                                        }
                                        self.analysis_dirty = true;
                                    }
                                }
                            }
                            Drag::SchWireEnd { ends, .. } => {
                                for (wi, is_a) in ends {
                                    if let Some(w) = self.project.wires.get_mut(wi) {
                                        if is_a {
                                            w.a = s
                                        } else {
                                            w.b = s
                                        }
                                    }
                                }
                                self.analysis_dirty = true;
                            }
                            Drag::SchLabel { idx, start, orig } => {
                                if let Some(l) = self.project.labels.get_mut(idx) {
                                    l.pos = (orig.0 + s.0 - start.0, orig.1 + s.1 - start.1);
                                    self.analysis_dirty = true;
                                }
                            }
                            Drag::NoteResize { idx, fixed, is_a } => self.resize_note(idx, fixed, is_a, s),
                            _ => {}
                        }
                    }
                }
                if resp.drag_stopped() {
                    match self.drag.take() {
                        Some(Drag::Band { start, cur, add }) => {
                            let mut items = if add { self.selection() } else { vec![] };
                            for it in self.band_items(start, cur) {
                                if !items.contains(&it) {
                                    items.push(it);
                                }
                            }
                            let n = items.len();
                            self.set_selection(items);
                            if n > 1 {
                                self.status = format!("{} items selected. Drag one of them to move them all.", n);
                            }
                        }
                        Some(_) => {
                            self.project.wires.retain(|w| w.a != w.b);
                            self.project.notes.retain(|n| !n.is_degenerate());
                            self.analysis_dirty = true;
                        }
                        None => {}
                    }
                }
                if resp.double_clicked() {
                    if let Some(w) = mouse_w {
                        let hit = self.sch_hit(w, tol);
                        if matches!(hit, Sel::Comp(_) | Sel::Note(_)) {
                            self.set_selection(vec![hit]);
                            self.focus_value = true;
                        }
                    }
                }
                if resp.clicked() {
                    if let Some(w) = mouse_w {
                        let hit = self.sch_hit(w, tol);
                        if additive {
                            self.toggle_selected(hit);
                        } else {
                            self.set_selection(vec![hit]);
                            self.highlight_net = match self.sel {
                                Sel::Wire(i) => self.nl.wire_net.get(i).copied(),
                                Sel::Label(i) => self.nl.label_net.get(i).copied(),
                                _ => None,
                            };
                        }
                    }
                }
            }
            SchTool::Note(kind) => self.note_tool_input(kind, &resp, ui, view, min, snapped),
            SchTool::Wire => {
                if resp.clicked() {
                    if let Some(s) = snapped {
                        self.wire_click(s);
                    }
                }
                if resp.secondary_clicked() {
                    self.finish_wire();
                }
            }
            SchTool::NoConnect => {
                if resp.clicked() {
                    if let Some((mx, my)) = mouse_w {
                        let near = |q: (i32, i32)| ((q.0 as f32 - mx).powi(2) + (q.1 as f32 - my).powi(2)).sqrt();
                        let reach = tol.max(8.0);
                        // The closest pin, or an existing mark, within reach.
                        let pin = self
                            .project
                            .components
                            .iter()
                            .filter(|c| !c.kind.is_power())
                            .flat_map(|c| c.info().pins.iter().map(|pd| c.pin_pos(pd)).collect::<Vec<_>>())
                            .filter(|&q| near(q) <= reach)
                            .min_by(|a, b| near(*a).partial_cmp(&near(*b)).unwrap());
                        let mark = self.project.no_connects.iter().position(|&q| near(q) <= reach);
                        match (pin, mark) {
                            (_, Some(i)) if pin.map_or(true, |p| self.project.no_connects[i] == p) => {
                                self.checkpoint();
                                self.project.no_connects.remove(i);
                                self.status = "No-connect mark removed.".into();
                            }
                            (Some(p), _) => {
                                self.checkpoint();
                                self.project.no_connects.push(p);
                                self.status = "Pin marked as not connected.".into();
                            }
                            _ => self.status = "Click on a pin.".into(),
                        }
                    }
                }
            }
            SchTool::Label => {
                if resp.clicked() {
                    if let Some(s) = snapped {
                        self.checkpoint();
                        let text = self.label_text.trim().to_string();
                        if !text.is_empty() {
                            self.project.labels.push(Label { pos: s, text: text.clone() });
                            self.label_text = bump_name(&text);
                        }
                    }
                }
            }
            SchTool::Place(k) => {
                if resp.clicked() {
                    if let Some(s) = snapped {
                        self.checkpoint();
                        let id = self.project.add_component(k, s, self.ghost_rot);
                        if !k.is_power() {
                            self.sel = Sel::Comp(id);
                            self.sch_tool = SchTool::Select;
                            let r = self.project.components.last().map(|c| c.reference.clone()).unwrap_or_default();
                            self.status = format!("Placed {}. Draw wires to connect it (W).", r);
                        }
                    }
                }
            }
        }
        if resp.secondary_clicked() && !matches!(self.sch_tool, SchTool::Wire | SchTool::Note(_)) {
            self.sch_tool = SchTool::Select;
        }

        // ---- drawing
        let painter = painter.with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, SCH_BG);
        let z = view.zoom;
        let step = if 10.0 * z >= 9.0 {
            10
        } else if 50.0 * z >= 9.0 {
            50
        } else {
            100
        };
        let (wx0, wy0) = view.w(min, rect.min);
        let (wx1, wy1) = view.w(min, rect.max);
        let (gx0, gx1) = ((wx0 / step as f32).floor() as i32, (wx1 / step as f32).ceil() as i32);
        let (gy0, gy1) = ((wy0 / step as f32).floor() as i32, (wy1 / step as f32).ceil() as i32);
        if (gx1 - gx0) * (gy1 - gy0) < 40000 {
            for gx in gx0..=gx1 {
                for gy in gy0..=gy1 {
                    let p = view.s(min, ((gx * step) as f32, (gy * step) as f32));
                    painter.rect_filled(Rect::from_center_size(p, Vec2::splat(1.5)), 0.0, GRID_DOT);
                }
            }
        }
        // Annotations go underneath the circuit.
        for boxes in [true, false] {
            for (i, n) in self.project.notes.iter().enumerate() {
                if (n.kind == NoteKind::Box) == boxes {
                    crate::annot::draw_note(&painter, view, min, n, self.is_sel(Sel::Note(i)), false);
                }
            }
        }
        let ww = (1.6 * z / 1.6).clamp(1.0, 3.5);
        for (i, w) in self.project.wires.iter().enumerate() {
            let net = self.nl.wire_net.get(i).copied();
            let hl = net.is_some() && net == self.highlight_net;
            let selected = self.is_sel(Sel::Wire(i));
            let (col, wd) = if selected {
                (SELECT, ww * 2.0)
            } else if hl {
                (HILITE, ww * 1.8)
            } else {
                (WIRE, ww)
            };
            painter.line_segment(
                [view.s(min, (w.a.0 as f32, w.a.1 as f32)), view.s(min, (w.b.0 as f32, w.b.1 as f32))],
                Stroke::new(wd, col),
            );
        }
        for j in &self.nl.junctions {
            painter.circle_filled(view.s(min, (j.0 as f32, j.1 as f32)), (2.6 * z).max(2.5), WIRE);
        }
        let mut problem_comps: std::collections::HashMap<u32, Severity> = std::collections::HashMap::new();
        for is in self.erc.iter().chain(self.sim.issues.iter()) {
            if is.severity == Severity::Info {
                continue;
            }
            for &id in &is.comps {
                let e = problem_comps.entry(id).or_insert(is.severity);
                if is.severity < *e {
                    *e = is.severity;
                }
            }
        }
        for (ci, c) in self.project.components.iter().enumerate() {
            let sel = self.is_sel(Sel::Comp(c.id));
            draw_symbol(&painter, view, min, c.kind, &c.reference, &c.value, c.pos, c.rot, 1.0, sel, c.custom.as_deref());
            // Pins: highlight net, mark unconnected ones.
            for (pi, pd) in c.info().pins.iter().enumerate() {
                let q = c.pin_pos(pd);
                let sp = view.s(min, (q.0 as f32, q.1 as f32));
                let net = self.nl.net_of(ci, pi);
                if let Some(n) = net {
                    let netr = &self.nl.nets[n];
                    if netr.pins.len() <= 1 && netr.labels.is_empty() && !netr.has_wire {
                        painter.circle_stroke(sp, (2.5 * z).max(2.5), Stroke::new(1.2_f32, Color32::from_rgb(200, 60, 60)));
                    }
                    if Some(n) == self.highlight_net {
                        painter.circle_filled(sp, (3.0 * z).max(3.0), HILITE);
                    }
                }
            }
            if let Some(sev) = problem_comps.get(&c.id) {
                let b = c.sch_bbox();
                let p = view.s(min, (b.2 as f32, b.1 as f32));
                let col = if *sev == Severity::Error { Color32::from_rgb(210, 40, 40) } else { Color32::from_rgb(230, 150, 0) };
                let r = (6.0 * z / 1.6).clamp(5.0, 10.0);
                painter.circle_filled(p, r, col);
                painter.text(p, Align2::CENTER_CENTER, "!", FontId::proportional(r * 1.5), Color32::WHITE);
            }
        }
        for nc in &self.project.no_connects {
            let p = view.s(min, (nc.0 as f32, nc.1 as f32));
            let d = (4.0 * z).max(3.0);
            let st = Stroke::new(1.5_f32, Color32::from_rgb(60, 60, 200));
            painter.line_segment([p - Vec2::splat(d), p + Vec2::splat(d)], st);
            painter.line_segment([p + Vec2::new(-d, d), p + Vec2::new(d, -d)], st);
        }
        for (i, l) in self.project.labels.iter().enumerate() {
            let p = view.s(min, (l.pos.0 as f32, l.pos.1 as f32));
            let fs = (9.0 * z).max(5.0);
            let sel = self.is_sel(Sel::Label(i));
            let col = if sel { SELECT } else { Color32::from_rgb(20, 20, 30) };
            let galley = painter.layout_no_wrap(l.text.clone(), FontId::monospace(fs), col);
            let tp = Pos2::new(p.x + 2.0, p.y - 2.0 - galley.size().y);
            painter.line_segment([p, Pos2::new(p.x, tp.y + galley.size().y * 0.3)], Stroke::new(1.0_f32, col));
            painter.galley(tp, galley, col);
            painter.circle_filled(p, 2.0, col);
        }
        // Voltage tags.
        if self.show_voltages && self.sim.ok {
            for (n, net) in self.nl.nets.iter().enumerate() {
                if net.power.iter().any(|x| is_ground_name(x)) || net.pins.len() < 2 {
                    continue;
                }
                let Some(v) = self.sim.net_v.get(n).copied().flatten() else { continue };
                if v.abs() < 0.005 {
                    continue; // 0 V tags only add clutter
                }
                let anchor = self
                    .project
                    .wires
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| self.nl.wire_net.get(*i) == Some(&n))
                    .max_by_key(|(_, w)| (w.a.0 - w.b.0).abs() + (w.a.1 - w.b.1).abs())
                    .map(|(_, w)| ((w.a.0 + w.b.0) as f32 / 2.0, (w.a.1 + w.b.1) as f32 / 2.0))
                    .or_else(|| {
                        net.pins
                            .iter()
                            .find(|(ci, _)| self.project.components.get(*ci).map_or(false, |c| !c.kind.is_power()))
                            .and_then(|&(ci, pi)| {
                                let c = self.project.components.get(ci)?;
                                let q = c.pin_pos(c.info().pins.get(pi)?);
                                Some((q.0 as f32, q.1 as f32))
                            })
                    });
                let Some(a) = anchor else { continue };
                let p = view.s(min, a) + Vec2::new(4.0, 4.0);
                let fs = (7.5 * z).clamp(9.0, 13.0);
                let galley = painter.layout_no_wrap(if v.abs() < 0.005 { "0 V".to_string() } else { fmt_si(v, "V") }, FontId::proportional(fs), Color32::from_rgb(120, 70, 0));
                let r = Rect::from_min_size(p, galley.size() + Vec2::new(8.0, 4.0));
                painter.rect(r, 6.0, Color32::from_rgb(255, 238, 186), Stroke::new(1.0_f32, Color32::from_rgb(225, 170, 50)));
                painter.galley(p + Vec2::new(4.0, 2.0), galley, Color32::from_rgb(120, 70, 0));
            }
        }
        // Tool previews.
        if let Some(s) = snapped {
            match self.sch_tool {
                SchTool::Place(k) => {
                    let inf = info(k);
                    draw_symbol(&painter, view, min, k, inf.prefix, inf.default_value, s, self.ghost_rot, 0.45, false, None);
                }
                SchTool::Wire => {
                    let sp = view.s(min, (s.0 as f32, s.1 as f32));
                    painter.circle_stroke(sp, 4.0, Stroke::new(1.0_f32, WIRE));
                    if let Some(pts) = &self.wire_draw {
                        let mut all: Vec<(i32, i32)> = pts.clone();
                        let last = *pts.last().unwrap();
                        if last.0 != s.0 && last.1 != s.1 {
                            all.push((s.0, last.1));
                        }
                        all.push(s);
                        let line: Vec<Pos2> = all.iter().map(|q| view.s(min, (q.0 as f32, q.1 as f32))).collect();
                        painter.add(Shape::line(line, Stroke::new(ww, WIRE.gamma_multiply(0.7))));
                    }
                }
                SchTool::NoConnect => {}
                SchTool::Note(kind) => self.draw_note_preview(kind, &painter, view, min, s),
                SchTool::Label => {
                    let sp = view.s(min, (s.0 as f32, s.1 as f32));
                    painter.text(sp + Vec2::new(2.0, -2.0), Align2::LEFT_BOTTOM, &self.label_text, FontId::monospace((9.0 * z).max(8.0)), Color32::from_gray(120));
                }
                _ => {}
            }
        }
        if let Some(Drag::Band { start, cur, .. }) = &self.drag {
            draw_band(&painter, view.s(min, *start), view.s(min, *cur));
        }
        // Corner caption.
        painter.text(
            rect.left_top() + Vec2::new(10.0, 8.0),
            Align2::LEFT_TOP,
            format!("Schematic — {}", self.project.name),
            FontId::proportional(13.0),
            Color32::from_rgb(120, 124, 140),
        );
    }
}

fn bump_name(s: &str) -> String {
    let digits: String = s.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    if digits.is_empty() {
        return s.to_string();
    }
    let base = &s[..s.len() - digits.len()];
    let n: u64 = digits.parse().unwrap_or(0);
    format!("{}{}", base, n + 1)
}

/// Selection box: blue = items fully inside (dragged to the right), green = items touched (to the left).
pub fn draw_band(painter: &Painter, a: Pos2, b: Pos2) {
    let touching = b.x < a.x;
    let col = if touching { Color32::from_rgb(40, 150, 80) } else { Color32::from_rgb(40, 100, 200) };
    let r = Rect::from_two_pos(a, b);
    painter.rect_filled(r, 0.0, col.gamma_multiply(0.12));
    painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, col));
}
