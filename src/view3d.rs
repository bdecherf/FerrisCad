//! A simple 3D view of the board, drawn in software with egui triangles: flat shading,
//! back-face culling and depth sorting (painter's algorithm). No 3D model files needed:
//! each part gets a body shaped from its kind and footprint.

use crate::app::App;
use crate::checks::PadItem;
use crate::model::*;
use eframe::egui::{self, epaint::Mesh, Align2, Color32, FontId, PointerButton, Pos2, Rect, Sense, Shape, Vec2};

const BOARD_T: f32 = 1.6;

#[derive(Clone, Copy, Debug)]
pub struct Cam3D {
    /// Turn around the vertical axis (radians).
    pub yaw: f32,
    /// Tilt from the top view (0 = straight down, PI = from below).
    pub pitch: f32,
    /// Zoom (1 = the board fills the view).
    pub zoom: f32,
    pub pan: Vec2,
    pub show_parts: bool,
    pub show_tracks: bool,
    pub show_silk: bool,
}

impl Default for Cam3D {
    fn default() -> Self {
        Cam3D { yaw: -0.5, pitch: 0.95, zoom: 1.0, pan: Vec2::ZERO, show_parts: true, show_tracks: true, show_silk: true }
    }
}

type V3 = [f32; 3];

struct Tri {
    p: [V3; 3],
    n: V3,
    c: Color32,
}

#[derive(Default)]
struct Scene {
    tris: Vec<Tri>,
    /// Where the printed details and the part bodies start in `tris`
    /// (before: board slab; details are kept in order; bodies are depth-sorted).
    details_from: usize,
    parts_from: usize,
}

/// Board coordinates (mm, y down) to 3D world (y up).
fn w(x: f32, y: f32, z: f32) -> V3 {
    [x, -y, z]
}

impl Scene {
    fn quad(&mut self, a: V3, b: V3, c: V3, d: V3, n: V3, col: Color32) {
        self.tris.push(Tri { p: [a, b, c], n, c: col });
        self.tris.push(Tri { p: [a, c, d], n, c: col });
    }

    /// Axis-aligned box from board rectangle (x0,y0)-(x1,y1) and heights z0..z1.
    fn cuboid(&mut self, r: (f32, f32, f32, f32), z0: f32, z1: f32, col: Color32) {
        let (x0, y0, x1, y1) = (r.0.min(r.2), r.1.min(r.3), r.0.max(r.2), r.1.max(r.3));
        let p = |x: f32, y: f32, z: f32| w(x, y, z);
        self.quad(p(x0, y0, z1), p(x1, y0, z1), p(x1, y1, z1), p(x0, y1, z1), [0.0, 0.0, 1.0], col);
        self.quad(p(x0, y0, z0), p(x0, y1, z0), p(x1, y1, z0), p(x1, y0, z0), [0.0, 0.0, -1.0], col);
        self.quad(p(x0, y1, z0), p(x0, y1, z1), p(x1, y1, z1), p(x1, y1, z0), [0.0, -1.0, 0.0], col);
        self.quad(p(x0, y0, z0), p(x1, y0, z0), p(x1, y0, z1), p(x0, y0, z1), [0.0, 1.0, 0.0], col);
        self.quad(p(x0, y0, z0), p(x0, y0, z1), p(x0, y1, z1), p(x0, y1, z0), [-1.0, 0.0, 0.0], col);
        self.quad(p(x1, y0, z0), p(x1, y1, z0), p(x1, y1, z1), p(x1, y0, z1), [1.0, 0.0, 0.0], col);
    }

    fn cylinder(&mut self, cx: f32, cy: f32, r: f32, z0: f32, z1: f32, side: Color32, top: Color32) {
        let n = 20;
        let pt = |i: usize, z: f32| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            w(cx + r * a.cos(), cy + r * a.sin(), z)
        };
        for i in 0..n {
            let a = (i as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
            let nrm = [a.cos(), -a.sin(), 0.0];
            self.quad(pt(i, z0), pt(i + 1, z0), pt(i + 1, z1), pt(i, z1), nrm, side);
            self.tris.push(Tri { p: [w(cx, cy, z1), pt(i, z1), pt(i + 1, z1)], n: [0.0, 0.0, 1.0], c: top });
        }
    }

    /// Dome (cone approximation) on top of an LED.
    fn dome(&mut self, cx: f32, cy: f32, r: f32, z0: f32, h: f32, col: Color32) {
        let n = 20;
        for i in 0..n {
            let a0 = i as f32 / n as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
            let am = (a0 + a1) / 2.0;
            let s = (h * h + r * r).sqrt();
            let nrm = [am.cos() * h / s, -am.sin() * h / s, r / s];
            self.tris.push(Tri {
                p: [w(cx + r * a0.cos(), cy + r * a0.sin(), z0), w(cx + r * a1.cos(), cy + r * a1.sin(), z0), w(cx, cy, z0 + h)],
                n: nrm,
                c: col,
            });
        }
    }

    /// Flat shape lying on the top (up = true) or bottom face.
    fn flat_rect(&mut self, r: (f32, f32, f32, f32), z: f32, up: bool, col: Color32) {
        let (a, b, c, d) = (w(r.0, r.1, z), w(r.2, r.1, z), w(r.2, r.3, z), w(r.0, r.3, z));
        if up {
            self.quad(a, b, c, d, [0.0, 0.0, 1.0], col);
        } else {
            self.quad(a, d, c, b, [0.0, 0.0, -1.0], col);
        }
    }

    fn flat_disc(&mut self, cx: f32, cy: f32, r: f32, z: f32, up: bool, col: Color32) {
        let n = 16;
        for i in 0..n {
            let a0 = i as f32 / n as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
            let (p0, p1) = (w(cx + r * a0.cos(), cy + r * a0.sin(), z), w(cx + r * a1.cos(), cy + r * a1.sin(), z));
            let c = w(cx, cy, z);
            if up {
                self.tris.push(Tri { p: [c, p0, p1], n: [0.0, 0.0, 1.0], c: col });
            } else {
                self.tris.push(Tri { p: [c, p1, p0], n: [0.0, 0.0, -1.0], c: col });
            }
        }
    }

    fn flat_line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, z: f32, up: bool, col: Color32) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l = (dx * dx + dy * dy).sqrt();
        if l < 1e-4 {
            return;
        }
        let (nx, ny) = (-dy / l * width / 2.0, dx / l * width / 2.0);
        let (p0, p1, p2, p3) = (w(a.0 + nx, a.1 + ny, z), w(b.0 + nx, b.1 + ny, z), w(b.0 - nx, b.1 - ny, z), w(a.0 - nx, a.1 - ny, z));
        if up {
            self.quad(p0, p3, p2, p1, [0.0, 0.0, 1.0], col);
        } else {
            self.quad(p0, p1, p2, p3, [0.0, 0.0, -1.0], col);
        }
    }
}

fn led_color(v: &str) -> Color32 {
    let v = v.to_lowercase();
    if v.contains("green") {
        Color32::from_rgb(60, 220, 90)
    } else if v.contains("blue") {
        Color32::from_rgb(70, 120, 255)
    } else if v.contains("yellow") {
        Color32::from_rgb(250, 220, 60)
    } else if v.contains("white") {
        Color32::from_rgb(240, 240, 235)
    } else if v.contains("orange") {
        Color32::from_rgb(255, 150, 40)
    } else {
        Color32::from_rgb(230, 40, 40)
    }
}

const BLACK: Color32 = Color32::from_rgb(32, 32, 36);
const METAL: Color32 = Color32::from_rgb(200, 200, 206);
const LEG: Color32 = Color32::from_rgb(170, 170, 176);

/// The part's body in 3D, built from its kind and footprint.
fn add_part(sc: &mut Scene, c: &Component, pads: &[&PadItem]) {
    let inf = c.info();
    let z = BOARD_T;
    // Local footprint rectangle -> board rectangle (rotation is in quarter turns).
    let rect = |r: (f32, f32, f32, f32)| {
        let a = rot_f((r.0, r.1), c.pcb_rot);
        let b = rot_f((r.2, r.3), c.pcb_rot);
        (c.pcb_pos.0 + a.0, c.pcb_pos.1 + a.1, c.pcb_pos.0 + b.0, c.pcb_pos.1 + b.1)
    };
    let mut pb = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pads {
        let r = p.rect();
        pb = (pb.0.min(r.0), pb.1.min(r.1), pb.2.max(r.2), pb.3.max(r.3));
    }
    if pb.0 > pb.2 {
        return;
    }
    let mid = ((pb.0 + pb.2) / 2.0, (pb.1 + pb.3) / 2.0);
    let all_tht = pads.iter().all(|p| p.tht);
    let fp = inf.footprint.to_uppercase();
    // Two-pad SMD parts: a small block between the pads, with metal ends.
    let two_pad_smd = |sc: &mut Scene, body: Color32, h: f32, ends: bool| {
        let horizontal = (pb.2 - pb.0) >= (pb.3 - pb.1);
        let r = if horizontal {
            (pb.0 + 0.1, pb.1 + 0.1, pb.2 - 0.1, pb.3 - 0.1)
        } else {
            (pb.0 + 0.1, pb.1 + 0.1, pb.2 - 0.1, pb.3 - 0.1)
        };
        sc.cuboid(r, z, z + h, body);
        if ends {
            for p in pads {
                let pr = p.rect();
                let e = if horizontal {
                    (pr.0.max(r.0), r.1, pr.2.min(r.2), r.3)
                } else {
                    (r.0, pr.1.max(r.1), r.2, pr.3.min(r.3))
                };
                sc.cuboid(e, z, z + h + 0.02, METAL);
            }
        }
    };
    match c.kind {
        Kind::Led if all_tht => {
            let col = led_color(&c.value);
            sc.cylinder(mid.0, mid.1, 2.6, z + 1.0, z + 2.0, col.gamma_multiply(0.85), col);
            sc.cylinder(mid.0, mid.1, 2.4, z + 2.0, z + 7.0, col.gamma_multiply(0.85), col);
            sc.dome(mid.0, mid.1, 2.4, z + 7.0, 2.2, col);
            for p in pads {
                sc.cuboid((p.pos.0 - 0.25, p.pos.1 - 0.25, p.pos.0 + 0.25, p.pos.1 + 0.25), z, z + 1.0, LEG);
            }
        }
        Kind::Led => {
            two_pad_smd(sc, Color32::from_rgb(235, 235, 225), 0.6, false);
            sc.flat_rect((mid.0 - 0.4, mid.1 - 0.4, mid.0 + 0.4, mid.1 + 0.4), z + 0.61, true, led_color(&c.value));
        }
        Kind::CapPolarized if all_tht => {
            let r = ((pb.2 - pb.0).max(pb.3 - pb.1) / 2.0 + 0.6).clamp(2.0, 9.0);
            sc.cylinder(mid.0, mid.1, r, z + 0.3, z + 2.2 * r + 2.0, Color32::from_rgb(30, 40, 110), METAL);
        }
        Kind::Resistor if !all_tht => two_pad_smd(sc, BLACK, 0.45, true),
        Kind::Capacitor | Kind::CapPolarized if !all_tht => two_pad_smd(sc, Color32::from_rgb(190, 145, 95), 0.8, true),
        Kind::Inductor if !all_tht => two_pad_smd(sc, Color32::from_rgb(70, 70, 76), 1.6, true),
        Kind::Diode if !all_tht => {
            two_pad_smd(sc, BLACK, 0.9, false);
        }
        Kind::Regulator if fp.contains("TO-220") || fp.contains("TO220") => {
            // Standing TO-220: body above the legs, metal tab on top.
            let horizontal = (pb.2 - pb.0) >= (pb.3 - pb.1);
            let (bx, by) = if horizontal { (5.1, 2.3) } else { (2.3, 5.1) };
            sc.cuboid((mid.0 - bx, mid.1 - by, mid.0 + bx, mid.1 + by), z + 3.0, z + 12.2, BLACK);
            let tab = if horizontal { (mid.0 - bx, mid.1 - by, mid.0 + bx, mid.1 - by + 1.3) } else { (mid.0 - bx, mid.1 - by, mid.0 - bx + 1.3, mid.1 + by) };
            sc.cuboid(tab, z + 12.2, z + 18.5, METAL);
            for p in pads {
                sc.cuboid((p.pos.0 - 0.4, p.pos.1 - 0.25, p.pos.0 + 0.4, p.pos.1 + 0.25), z, z + 3.0, LEG);
            }
        }
        Kind::Battery => {
            let b = c.pcb_bbox();
            sc.cuboid((b.0 + 0.5, b.1 + 0.5, b.2 - 0.5, b.3 - 0.5), z, z + 3.0, BLACK);
            sc.cuboid((b.0 + 1.5, b.1 + 1.0, b.2 - 1.5, b.3 - 1.0), z + 3.0, z + 16.0, Color32::from_rgb(40, 80, 170));
        }
        Kind::Switch => {
            let b = rect(inf.fp_body);
            sc.cuboid(b, z, z + 3.5, BLACK);
            let cx = ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0);
            sc.cylinder(cx.0, cx.1, 1.75, z + 3.5, z + 5.0, Color32::from_rgb(60, 60, 66), Color32::from_rgb(80, 80, 88));
        }
        Kind::Connector2 => {
            sc.cuboid((pb.0 - 0.2, pb.1 - 0.2, pb.2 + 0.2, pb.3 + 0.2), z, z + 2.5, BLACK);
            for p in pads {
                sc.cuboid((p.pos.0 - 0.32, p.pos.1 - 0.32, p.pos.0 + 0.32, p.pos.1 + 0.32), z + 2.5, z + 8.5, Color32::from_rgb(215, 180, 80));
            }
        }
        _ => {
            // Chips and anything else: a block over the pads, with legs for gull-wing packages.
            let many = pads.len() >= 3;
            if all_tht {
                let col = if fp.contains("CRYSTAL") || fp.contains("USB") { METAL } else { Color32::from_rgb(55, 55, 60) };
                sc.cuboid((pb.0 + 0.3, pb.1 + 0.3, pb.2 - 0.3, pb.3 - 0.3), z + 0.5, z + 4.0, col);
            } else if many {
                // Body between the pad rows.
                let b = if c.custom.as_ref().map_or(true, |cu| cu.pads.is_empty()) { rect(inf.fp_body) } else { shrink_to_body(&pb, pads) };
                let col = if fp.contains("CRYSTAL") || fp.contains("USB") { METAL } else { BLACK };
                sc.cuboid(b, z + 0.15, z + 1.5, col);
                sc.flat_disc(b.0 + 0.6, b.1 + 0.6, 0.25, z + 1.51, true, Color32::from_rgb(90, 90, 96));
                for p in pads {
                    let r = p.rect();
                    sc.cuboid((r.0 + 0.1, r.1 + 0.1, r.2 - 0.1, r.3 - 0.1), z, z + 0.25, LEG);
                }
            } else {
                two_pad_smd(sc, BLACK, 0.8, true);
            }
        }
    }
}

/// The plastic body of a gull-wing package: the area between the rows of pads.
fn shrink_to_body(pb: &(f32, f32, f32, f32), pads: &[&PadItem]) -> (f32, f32, f32, f32) {
    let wide = (pb.2 - pb.0) >= (pb.3 - pb.1);
    let pad_len = pads.iter().map(|p| if wide { p.size.0 } else { p.size.1 }).fold(0.0f32, f32::max);
    if wide {
        (pb.0 + pad_len * 0.8, pb.1 + 0.2, pb.2 - pad_len * 0.8, pb.3 - 0.2)
    } else {
        (pb.0 + 0.2, pb.1 + pad_len * 0.8, pb.2 - 0.2, pb.3 - pad_len * 0.8)
    }
}

fn build_scene(app: &App) -> Scene {
    let mut sc = Scene::default();
    let p = &app.project;
    let b = &p.board;
    let cam = app.cam3d;
    let mask = Color32::from_rgb(22, 96, 52);
    let edge = Color32::from_rgb(196, 186, 140);
    // Board slab.
    let br = (b.x0, b.y0, b.x1, b.y1);
    sc.flat_rect(br, BOARD_T, true, mask);
    sc.flat_rect(br, 0.0, false, mask);
    let k = [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)];
    let nrm = [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [-1.0, 0.0, 0.0]];
    for i in 0..4 {
        let (a, c) = (k[i], k[(i + 1) % 4]);
        sc.quad(w(a.0, a.1, 0.0), w(c.0, c.1, 0.0), w(c.0, c.1, BOARD_T), w(a.0, a.1, BOARD_T), nrm[i], edge);
    }
    let top = BOARD_T + 0.01;
    sc.details_from = sc.tris.len();
    // Tracks (copper under the solder mask looks lighter green).
    if cam.show_tracks {
        let under = Color32::from_rgb(52, 140, 72);
        for t in &p.tracks {
            match t.layer {
                Layer::Front => sc.flat_line(t.a, t.b, t.width, top, true, under),
                Layer::Back => sc.flat_line(t.a, t.b, t.width, -0.01, false, under),
                Layer::Inner(_) => {}
            }
        }
    }
    // Pads (gold) and holes.
    let gold = Color32::from_rgb(214, 178, 88);
    let hole = Color32::from_rgb(20, 20, 20);
    for pad in &app.conn.pads {
        let front = pad.tht || !pad.back;
        let back = pad.tht || pad.back;
        for (on, z, up) in [(front, top + 0.01, true), (back, -0.02, false)] {
            if !on {
                continue;
            }
            if pad.round {
                sc.flat_disc(pad.pos.0, pad.pos.1, pad.size.0.min(pad.size.1) / 2.0, z, up, gold);
            } else {
                sc.flat_rect(pad.rect(), z, up, gold);
            }
            if pad.tht {
                let r = pad.size.0.min(pad.size.1) * 0.28;
                sc.flat_disc(pad.pos.0, pad.pos.1, r, z + if up { 0.01 } else { -0.01 }, up, hole);
            }
        }
    }
    for v in &p.vias {
        for (z, up) in [(top + 0.01, true), (-0.02, false)] {
            sc.flat_disc(v.pos.0, v.pos.1, v.size / 2.0, z, up, METAL);
            sc.flat_disc(v.pos.0, v.pos.1, v.size * 0.22, z + if up { 0.01 } else { -0.01 }, up, hole);
        }
    }
    // Silkscreen outlines.
    if cam.show_silk {
        let white = Color32::from_rgb(235, 235, 228);
        for c in p.components.iter().filter(|c| c.has_fp()) {
            let xf = |q: (f32, f32)| {
                let r = rot_f(q, c.pcb_rot);
                (c.pcb_pos.0 + r.0, c.pcb_pos.1 + r.1)
            };
            match c.custom.as_ref().filter(|cu| !cu.pads.is_empty()) {
                Some(cu) => {
                    for (a, b) in &cu.silk {
                        sc.flat_line(xf(*a), xf(*b), 0.15, top + 0.02, true, white);
                    }
                }
                None => {
                    let (x0, y0, x1, y1) = c.info().fp_body;
                    let q = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
                    for i in 0..4 {
                        sc.flat_line(xf(q[i]), xf(q[(i + 1) % 4]), 0.15, top + 0.02, true, white);
                    }
                }
            }
        }
    }
    // Part bodies.
    sc.parts_from = sc.tris.len();
    if cam.show_parts {
        for (ci, c) in p.components.iter().enumerate() {
            if !c.has_fp() {
                continue;
            }
            let pads: Vec<&PadItem> = app.conn.pads.iter().filter(|x| x.ci == ci).collect();
            add_part(&mut sc, c, &pads);
        }
    }
    sc
}

impl App {
    pub fn view3d_canvas(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        self.canvas_rect = rect;
        let painter = painter.with_clip_rect(rect);
        // Background gradient.
        let mut bg = Mesh::default();
        let (tc, bc) = (Color32::from_rgb(58, 64, 82), Color32::from_rgb(18, 20, 26));
        bg.colored_vertex(rect.left_top(), tc);
        bg.colored_vertex(rect.right_top(), tc);
        bg.colored_vertex(rect.right_bottom(), bc);
        bg.colored_vertex(rect.left_bottom(), bc);
        bg.add_triangle(0, 1, 2);
        bg.add_triangle(0, 2, 3);
        painter.add(Shape::mesh(bg));

        // Mouse: left-drag orbits, right/middle-drag pans, wheel zooms.
        if resp.dragged_by(PointerButton::Primary) {
            let d = resp.drag_delta();
            self.cam3d.yaw -= d.x * 0.008;
            self.cam3d.pitch = (self.cam3d.pitch + d.y * 0.008).clamp(0.0, std::f32::consts::PI);
        }
        if resp.dragged_by(PointerButton::Secondary) || resp.dragged_by(PointerButton::Middle) {
            self.cam3d.pan += resp.drag_delta();
        }
        if resp.hovered() {
            let s = ui.input(|i| i.smooth_scroll_delta.y + (i.zoom_delta() - 1.0) * 200.0);
            if s != 0.0 {
                self.cam3d.zoom = (self.cam3d.zoom * (s * 0.0018).exp()).clamp(0.2, 20.0);
            }
        }
        if resp.double_clicked() {
            self.cam3d = Cam3D { show_parts: self.cam3d.show_parts, show_tracks: self.cam3d.show_tracks, show_silk: self.cam3d.show_silk, ..Default::default() };
        }

        let scene = build_scene(self);
        let b = &self.project.board;
        let center = [(b.x0 + b.x1) / 2.0, -(b.y0 + b.y1) / 2.0, BOARD_T / 2.0];
        let diag = ((b.x1 - b.x0).powi(2) + (b.y1 - b.y0).powi(2)).sqrt().max(10.0);
        let cam = self.cam3d;
        let (sy, cy) = cam.yaw.sin_cos();
        let (sp, cp) = cam.pitch.sin_cos();
        let rot = |v: V3| -> V3 {
            let x = v[0] * cy - v[1] * sy;
            let y = v[0] * sy + v[1] * cy;
            [x, y * cp + v[2] * sp, -y * sp + v[2] * cp]
        };
        let dist = diag * 2.2;
        let f = rect.width().min(rect.height()) * 0.95 * dist / diag * cam.zoom;
        let origin = rect.center() + cam.pan;
        let project = |v: V3| -> (Pos2, f32) {
            let r = rot([v[0] - center[0], v[1] - center[1], v[2] - center[2]]);
            let s = f / (dist - r[2]).max(1.0);
            (origin + Vec2::new(r[0] * s, -r[1] * s), r[2])
        };
        let light = {
            let l = [-0.35f32, 0.45, 1.0];
            let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
            [l[0] / n, l[1] / n, l[2] / n]
        };
        // Seen from below, the board covers the parts on its top side.
        let top_visible = rot([0.0, 0.0, 1.0])[2] > 0.0;
        let mut drawn: Vec<(u8, f32, [Pos2; 3], Color32)> = Vec::with_capacity(scene.tris.len());
        for (ti, t) in scene.tris.iter().enumerate() {
            let pass: u8 = if ti < scene.details_from {
                0
            } else if ti < scene.parts_from {
                1
            } else {
                2
            };
            if pass == 2 && !top_visible {
                continue;
            }
            let n = rot(t.n);
            if n[2] <= 0.0 {
                continue; // facing away
            }
            let (a, za) = project(t.p[0]);
            let (b2, zb) = project(t.p[1]);
            let (c, zc) = project(t.p[2]);
            let shade = 0.38 + 0.62 * (n[0] * light[0] + n[1] * light[1] + n[2] * light[2]).max(0.0);
            let col = Color32::from_rgb(
                (t.c.r() as f32 * shade).min(255.0) as u8,
                (t.c.g() as f32 * shade).min(255.0) as u8,
                (t.c.b() as f32 * shade).min(255.0) as u8,
            );
            // Printed details keep their order (pads before holes); the rest sorts by depth.
            let depth = if pass == 1 { 0.0 } else { (za + zb + zc) / 3.0 };
            drawn.push((pass, depth, [a, b2, c], col));
        }
        // Board, then printed details, then parts far-to-near (painter's algorithm).
        drawn.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal)));
        let mut mesh = Mesh::default();
        for (_, _, p, col) in &drawn {
            let i = mesh.vertices.len() as u32;
            for q in p {
                mesh.colored_vertex(*q, *col);
            }
            mesh.add_triangle(i, i + 1, i + 2);
            if mesh.vertices.len() > 60000 {
                painter.add(Shape::mesh(std::mem::take(&mut mesh)));
            }
        }
        painter.add(Shape::mesh(mesh));

        // Caption.
        let lp = rect.left_top() + Vec2::new(10.0, 8.0);
        painter.text(lp, Align2::LEFT_TOP, format!("3D view — {}", self.project.name), FontId::proportional(13.0), Color32::from_rgb(200, 206, 222));
        painter.text(
            rect.left_bottom() + Vec2::new(10.0, -8.0),
            Align2::LEFT_BOTTOM,
            "Drag to turn · right-drag to move · scroll to zoom · double-click to reset",
            FontId::proportional(12.0),
            Color32::from_rgb(170, 176, 196),
        );
        let _ = Rect::NOTHING;
        if resp.hovered() {
            self.hover_info = format!("{} triangles drawn", drawn.len());
        }
    }

    pub fn view3d_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("3D VIEW").small().strong().color(Color32::GRAY));
        ui.horizontal_wrapped(|ui| {
            let presets = [("Top", 0.0, 0.0), ("Front", 0.0, 1.25), ("Angle", -0.5, 0.95), ("Bottom", 0.0, std::f32::consts::PI)];
            for (name, yaw, pitch) in presets {
                if ui.button(name).clicked() {
                    self.cam3d.yaw = yaw;
                    self.cam3d.pitch = pitch;
                    self.cam3d.pan = Vec2::ZERO;
                    self.cam3d.zoom = 1.0;
                }
            }
        });
        ui.checkbox(&mut self.cam3d.show_parts, "Show parts");
        ui.checkbox(&mut self.cam3d.show_tracks, "Show tracks");
        ui.checkbox(&mut self.cam3d.show_silk, "Show silkscreen");
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Parts are drawn as simple shapes from their footprint; copper zones and inner layers aren't shown.")
                .small()
                .color(Color32::GRAY),
        );
    }
}
