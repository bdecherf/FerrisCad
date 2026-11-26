//! Board layout canvas: footprints, pads, tracks, vias, ratsnest and interactive routing.

use crate::app::*;
use crate::checks::{dist_pt_seg, Severity};
use crate::model::*;
use eframe::egui::{self, Align2, Color32, FontId, PointerButton, Pos2, Rect, Sense, Shape, Stroke, Vec2};

pub const F_CU: Color32 = Color32::from_rgb(214, 66, 66);
pub const B_CU: Color32 = Color32::from_rgb(72, 140, 232);
const INNER_COLORS: [Color32; 6] = [
    Color32::from_rgb(222, 190, 60),
    Color32::from_rgb(90, 200, 120),
    Color32::from_rgb(200, 100, 220),
    Color32::from_rgb(70, 200, 210),
    Color32::from_rgb(240, 140, 60),
    Color32::from_rgb(160, 160, 240),
];

/// Display colour of a copper layer.
pub fn layer_color(l: Layer) -> Color32 {
    match l {
        Layer::Front => F_CU,
        Layer::Back => B_CU,
        Layer::Inner(n) => INNER_COLORS[(n.max(1) as usize - 1) % INNER_COLORS.len()],
    }
}
const BG: Color32 = Color32::from_rgb(22, 24, 30);
const BOARD: Color32 = Color32::from_rgb(18, 52, 38);
const EDGE: Color32 = Color32::from_rgb(232, 200, 70);
const SILK: Color32 = Color32::from_rgb(232, 232, 214);
const PAD_SMD: Color32 = Color32::from_rgb(236, 110, 96);
const PAD_THT: Color32 = Color32::from_rgb(212, 172, 84);
const RATS: Color32 = Color32::from_rgb(170, 200, 255);
const SELECT: Color32 = Color32::from_rgb(255, 255, 255);
const HILITE: Color32 = Color32::from_rgb(255, 240, 120);

fn snap_p(w: (f32, f32)) -> (f32, f32) {
    (snap_f(w.0, 0.25), snap_f(w.1, 0.25))
}

pub fn route45(from: (f32, f32), to: (f32, f32)) -> Vec<(f32, f32)> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let (ax, ay) = (dx.abs(), dy.abs());
    if ax < 1e-4 || ay < 1e-4 || (ax - ay).abs() < 1e-4 {
        return vec![to];
    }
    let mid = if ax > ay { (from.0 + dx.signum() * (ax - ay), from.1) } else { (from.0, from.1 + dy.signum() * (ay - ax)) };
    vec![mid, to]
}

fn active_layer(app: &App) -> Layer {
    app.route_layer
}

pub type ZoneTris = Vec<(Option<Layer>, Vec<[(f32, f32); 3]>)>;

fn is_convex(poly: &[(f32, f32)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut sign = 0.0f32;
    for i in 0..n {
        let (a, b, c) = (poly[i], poly[(i + 1) % n], poly[(i + 2) % n]);
        let cross = (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0);
        if cross.abs() < 1e-9 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// Fill any polygon (concave, or with holes joined by cut lines as KiCad saves zones) with
/// thin horizontal strips, using the even-odd rule.
fn scan_fill(poly: &[(f32, f32)], out: &mut Vec<[(f32, f32); 3]>) {
    let (y0, y1) = poly.iter().fold((f32::MAX, f32::MIN), |m, p| (m.0.min(p.1), m.1.max(p.1)));
    let step = 0.05f32;
    let n = poly.len();
    let mut y = y0;
    while y < y1 {
        let yc = y + step / 2.0;
        let mut xs: Vec<f32> = vec![];
        for i in 0..n {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            if (a.1 > yc) != (b.1 > yc) {
                xs.push(a.0 + (yc - a.1) / (b.1 - a.1) * (b.0 - a.0));
            }
        }
        xs.sort_by(|p, q| p.partial_cmp(q).unwrap_or(std::cmp::Ordering::Equal));
        let ytop = (y + step).min(y1);
        for pair in xs.chunks(2) {
            if let [xa, xb] = pair {
                if xb - xa > 1e-4 {
                    out.push([(*xa, y), (*xb, y), (*xb, ytop)]);
                    out.push([(*xa, y), (*xb, ytop), (*xa, ytop)]);
                }
            }
        }
        y += step;
    }
}

/// Triangles filling every copper zone, computed once per zone change.
fn zone_fill(app: &mut App) -> std::sync::Arc<ZoneTris> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    app.project.zones.len().hash(&mut h);
    for z in &app.project.zones {
        z.layer.hash(&mut h);
        z.polys.len().hash(&mut h);
        for p in &z.polys {
            p.len().hash(&mut h);
            for q in p.iter().step_by((p.len() / 8).max(1)) {
                q.0.to_bits().hash(&mut h);
                q.1.to_bits().hash(&mut h);
            }
        }
    }
    let key = h.finish();
    if let Some((k, tris)) = &app.zone_cache {
        if *k == key {
            return tris.clone();
        }
    }
    let mut v: ZoneTris = vec![];
    for z in &app.project.zones {
        let mut tris = vec![];
        for poly in &z.polys {
            if poly.len() < 3 {
                continue;
            }
            if is_convex(poly) {
                for i in 1..poly.len() - 1 {
                    tris.push([poly[0], poly[i], poly[i + 1]]);
                }
            } else {
                scan_fill(poly, &mut tris);
            }
        }
        v.push((z.layer, tris));
    }
    let arc = std::sync::Arc::new(v);
    app.zone_cache = Some((key, arc.clone()));
    arc
}

/// Zone triangles for other views (3D).
pub fn zone_triangles(app: &mut App) -> std::sync::Arc<ZoneTris> {
    zone_fill(app)
}

impl App {
    fn pad_at(&self, w: (f32, f32)) -> Option<usize> {
        self.conn.pads.iter().position(|p| p.contains(w))
    }

    fn pcb_hit(&self, w: (f32, f32), tol: f32) -> Sel {
        for (i, t) in self.project.texts.iter().enumerate().rev() {
            let b = crate::font::bbox(t);
            if w.0 >= b.0 - tol && w.0 <= b.2 + tol && w.1 >= b.1 - tol && w.1 <= b.3 + tol {
                return Sel::Text(i);
            }
        }
        for (i, v) in self.project.vias.iter().enumerate() {
            if ((w.0 - v.pos.0).powi(2) + (w.1 - v.pos.1).powi(2)).sqrt() <= v.size / 2.0 + tol {
                return Sel::Via(i);
            }
        }
        for (i, t) in self.project.tracks.iter().enumerate().rev() {
            if dist_pt_seg(w, t.a, t.b) <= t.width / 2.0 + tol {
                return Sel::Track(i);
            }
        }
        // Zone outlines (their edge), before the footprints that may sit on top of them.
        for (i, z) in self.project.zones.iter().enumerate().rev() {
            if z.layer.map_or(false, |l| self.hidden_layers.contains(&l)) {
                continue;
            }
            for o in &z.outlines {
                if (0..o.len()).any(|k| dist_pt_seg(w, o[k], o[(k + 1) % o.len()]) <= tol) {
                    return Sel::Zone(i);
                }
            }
        }
        for c in self.project.components.iter().rev().filter(|c| c.has_fp()) {
            let b = c.pcb_bbox();
            if w.0 >= b.0 && w.0 <= b.2 && w.1 >= b.1 && w.1 <= b.3 {
                return Sel::Comp(c.id);
            }
        }
        // Inside a zone: the active layer's zones first.
        let active = active_layer(self);
        for on_active in [true, false] {
            for (i, z) in self.project.zones.iter().enumerate().rev() {
                let shown = z.layer.map_or(true, |l| !self.hidden_layers.contains(&l));
                if shown && (z.layer == Some(active)) == on_active && z.contains(w) {
                    return Sel::Zone(i);
                }
            }
        }
        Sel::None
    }

    /// The outline corner of zone `zi` within `r` of `w`: (outline, vertex).
    fn zone_vertex_at(&self, zi: usize, w: (f32, f32), r: f32) -> Option<(usize, usize)> {
        let z = self.project.zones.get(zi)?;
        for (oi, o) in z.outlines.iter().enumerate() {
            for (vi, q) in o.iter().enumerate() {
                if ((q.0 - w.0).powi(2) + (q.1 - w.1).powi(2)).sqrt() <= r {
                    return Some((oi, vi));
                }
            }
        }
        None
    }

    /// Close the zone being drawn.
    pub fn finish_zone(&mut self) {
        let pts = std::mem::take(&mut self.zone_draft);
        if pts.len() < 3 {
            self.status = "A zone needs at least 3 corners.".into();
            return;
        }
        self.checkpoint();
        let layer = self.route_layer;
        self.project.zones.push(Zone::new(layer, &self.zone_net.clone(), pts));
        self.sel = Sel::Zone(self.project.zones.len() - 1);
        self.group.clear();
        self.pcb_tool = PcbTool::Select;
        self.status = format!(
            "Zone on {} ({}) created. Drag its corners, double-click an edge to add one. KiCad recomputes the real fill with clearances (B).",
            layer.name(),
            self.zone_net
        );
    }

    pub fn zone_properties(&mut self, ui: &mut egui::Ui, i: usize) {
        ui.label(egui::RichText::new("Copper zone").strong().size(15.0));
        let Some(z) = self.project.zones.get(i).cloned() else { return };
        let mut names: Vec<String> = self.nl.nets.iter().filter(|n| !n.pins.is_empty()).map(|n| n.name.clone()).collect();
        names.sort();
        names.dedup();
        let mut net = z.net.clone();
        ui.horizontal(|ui| {
            ui.label("Net");
            egui::ComboBox::from_id_salt("zone_net_sel").selected_text(if net.is_empty() { "(none)".to_string() } else { net.clone() }).show_ui(ui, |ui| {
                ui.selectable_value(&mut net, String::new(), "(none)");
                for n in &names {
                    ui.selectable_value(&mut net, n.clone(), n);
                }
            });
        });
        let mut layer = z.layer;
        ui.horizontal_wrapped(|ui| {
            ui.label("Layer");
            for l in self.project.board.copper() {
                if ui.selectable_label(layer == Some(l), l.short()).clicked() {
                    layer = Some(l);
                }
            }
        });
        if net != z.net || layer != z.layer {
            self.checkpoint();
            let zz = &mut self.project.zones[i];
            zz.net = net;
            zz.layer = layer;
            if !zz.outlines.is_empty() {
                zz.outline_changed();
            }
        }
        let area: f32 = z
            .outlines
            .iter()
            .map(|o| (0..o.len()).map(|k| o[k].0 * o[(k + 1) % o.len()].1 - o[(k + 1) % o.len()].0 * o[k].1).sum::<f32>().abs() / 2.0)
            .sum();
        if z.outlines.is_empty() {
            ui.label(egui::RichText::new("Filled copper imported from KiCad (the outline wasn't kept).").small().color(Color32::GRAY));
            if ui.button("Redraw as the board outline").on_hover_text("Replaces the fill by a zone covering the whole board").clicked() {
                self.checkpoint();
                let b = self.project.board.clone();
                let zz = &mut self.project.zones[i];
                zz.outlines = vec![vec![(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)]];
                zz.outline_changed();
            }
        } else {
            ui.label(format!("{} corners, {:.0} mm²", z.outlines.iter().map(|o| o.len()).sum::<usize>(), area));
            ui.label(
                egui::RichText::new(if z.drawn {
                    "Drag a corner to move it, drag the zone to move it, double-click an edge to add a corner, right-click a corner to remove it. The fill is the plain outline here: other nets are kept clear in KiCad (B) and in the Gerbers."
                } else {
                    "Filled by KiCad. Editing the outline replaces that fill by the plain outline."
                })
                .small()
                .color(Color32::GRAY),
            );
        }
        if ui.button("🗑 Delete zone").clicked() {
            self.delete_selection();
        }
    }

    fn route_click(&mut self, w: (f32, f32)) {
        let pad = self.pad_at(w);
        let target = pad.map(|k| self.conn.pads[k].pos).unwrap_or_else(|| {
            // Snap to the end of an existing track.
            for t in &self.project.tracks {
                for q in [t.a, t.b] {
                    if ((q.0 - w.0).powi(2) + (q.1 - w.1).powi(2)).sqrt() < t.width {
                        return q;
                    }
                }
            }
            snap_p(w)
        });
        let layer = self.route_layer;
        let mut finish = false;
        match &mut self.route {
            None => {
                self.route = Some(RouteState { pts: vec![target], layers: vec![], vias: vec![] });
                if let Some(k) = pad {
                    self.highlight_net = self.conn.pads[k].net;
                }
            }
            Some(r) => {
                let last = *r.pts.last().unwrap();
                if (target.0 - last.0).abs() < 1e-3 && (target.1 - last.1).abs() < 1e-3 {
                    finish = true;
                } else {
                    for q in route45(last, target) {
                        r.pts.push(q);
                        r.layers.push(layer);
                    }
                    finish = pad.is_some();
                }
            }
        }
        if finish {
            self.finish_route();
        }
    }

    pub fn finish_route(&mut self) {
        let Some(r) = self.route.take() else { return };
        self.highlight_net = None;
        if r.pts.len() < 2 {
            return;
        }
        self.checkpoint();
        for (i, w) in r.pts.windows(2).enumerate() {
            self.project.tracks.push(Track { a: w[0], b: w[1], width: self.track_width, layer: r.layers[i] });
        }
        for v in r.vias {
            self.project.vias.push(Via { pos: v, size: 0.8 });
        }
        self.status = "Track added.".into();
    }

    fn pcb_hover_info(&self, w: (f32, f32), tol: f32) -> String {
        if let Sel::Text(i) = self.pcb_hit(w, tol) {
            let t = &self.project.texts[i];
            return format!("Text \"{}\" · {} · {} mm", t.text, t.layer.name(), t.size);
        }
        if let Some(k) = self.pad_at(w) {
            let pad = &self.conn.pads[k];
            let c = &self.project.components[pad.ci];
            let pd = &c.info().pins[pad.pi];
            let net = pad.net.map(|n| self.nl.nets[n].name.clone()).unwrap_or_default();
            return format!("{} pad {} ({}) · net {}", c.reference, pd.pad, pd.name, net);
        }
        match self.pcb_hit(w, tol) {
            Sel::Track(i) => {
                let t = &self.project.tracks[i];
                let net = self.conn.track_net(i).map(|n| self.nl.nets[n].name.clone()).unwrap_or_else(|| "(none)".into());
                format!("Track · net {} · {} mm · {}", net, t.width, t.layer.name())
            }
            Sel::Via(i) => {
                let net = self.conn.via_net(i).map(|n| self.nl.nets[n].name.clone()).unwrap_or_else(|| "(none)".into());
                format!("Via · net {}", net)
            }
            Sel::Comp(id) => {
                let c = &self.project.components[self.project.find(id).unwrap()];
                format!("{} · {} · {}", c.reference, c.value, c.info().footprint)
            }
            _ => String::new(),
        }
    }

    pub fn pcb_canvas(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        self.canvas_rect = rect;
        if self.fit_pcb {
            self.fit_view(Mode::Pcb, rect);
            self.fit_pcb = false;
        }
        zoom_pan(&mut self.pcb_view, &resp, ui, rect, 1.0, 150.0);
        let view = self.pcb_view;
        let min = rect.min;
        let mouse_w = resp.hover_pos().map(|p| view.w(min, p));
        let tol = 4.0 / view.zoom;
        if let Some(w) = mouse_w {
            self.cursor = Some(w);
            self.hover_info = self.pcb_hover_info(w, tol);
        }

        // ---- input
        match self.pcb_tool {
            PcbTool::Select => {
                let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                if resp.drag_started_by(PointerButton::Primary) {
                    if let Some(origin) = ui.input(|i| i.pointer.press_origin()) {
                        let w = view.w(min, origin);
                        let hit = self.pcb_hit(w, tol);
                        let comp_hit = self
                            .project
                            .components
                            .iter()
                            .rev()
                            .filter(|c| c.has_fp())
                            .find(|c| {
                                let b = c.pcb_bbox();
                                w.0 >= b.0 && w.0 <= b.2 && w.1 >= b.1 && w.1 <= b.3
                            })
                            .map(|c| (c.id, c.pcb_pos));
                        let in_group = !self.group.is_empty()
                            && (self.is_sel(hit) || comp_hit.map_or(false, |(id, _)| self.is_sel(Sel::Comp(id))));
                        let zone_corner = match self.sel {
                            Sel::Zone(zi) if self.group.is_empty() => self.zone_vertex_at(zi, w, tol * 2.0).map(|(oi, vi)| (zi, oi, vi)),
                            _ => None,
                        };
                        let zone_hit = matches!(hit, Sel::Zone(_)) && hit == self.sel && comp_hit.is_none();
                        if let Some((zi, oi, vi)) = zone_corner {
                            self.checkpoint();
                            self.drag = Some(Drag::ZoneVertex { zi, oi, vi });
                        } else if zone_hit {
                            if let Sel::Zone(zi) = hit {
                                self.checkpoint();
                                self.drag = Some(Drag::ZoneMove { zi, start: snap_p(w), orig: self.project.zones[zi].outlines.clone() });
                            }
                        } else if in_group {
                            self.start_pcb_group_drag(snap_p(w));
                        } else if additive || ((hit == Sel::None || matches!(hit, Sel::Zone(_))) && comp_hit.is_none()) {
                            self.drag = Some(Drag::Band { start: w, cur: w, add: additive });
                        } else if let Some((id, orig)) = comp_hit {
                            self.group.clear();
                            self.sel = Sel::Comp(id);
                            // Moving a footprint also drags the track ends on its pads.
                            self.start_pcb_group_drag(snap_p(w));
                            let _ = orig;
                        } else {
                            self.group.clear();
                            self.sel = hit;
                            if matches!(hit, Sel::Track(_) | Sel::Via(_) | Sel::Text(_)) {
                                self.start_pcb_group_drag(snap_p(w));
                            }
                        }
                    }
                }
                if resp.dragged_by(PointerButton::Primary) {
                    if let (Some(d), Some(w)) = (self.drag.clone(), mouse_w) {
                        match d {
                            Drag::Band { start, add, .. } => self.drag = Some(Drag::Band { start, cur: w, add }),
                            Drag::PcbGroup { .. } => self.drag_pcb_group(&d, snap_p(w)),
                            Drag::ZoneVertex { zi, oi, vi } => {
                                if let Some(q) = self.project.zones.get_mut(zi).and_then(|z| z.outlines.get_mut(oi)).and_then(|o| o.get_mut(vi)) {
                                    *q = snap_p(w);
                                }
                                if let Some(z) = self.project.zones.get_mut(zi) {
                                    z.outline_changed();
                                }
                                self.analysis_dirty = true;
                            }
                            Drag::ZoneMove { zi, start, orig } => {
                                let s = snap_p(w);
                                if let Some(z) = self.project.zones.get_mut(zi) {
                                    z.outlines = orig.iter().map(|o| o.iter().map(|q| (q.0 + s.0 - start.0, q.1 + s.1 - start.1)).collect()).collect();
                                    z.outline_changed();
                                }
                                self.analysis_dirty = true;
                            }
                            Drag::PcbComp { id, start, orig } => {
                                let s = snap_p(w);
                                if let Some(ci) = self.project.find(id) {
                                    let np = (orig.0 + s.0 - start.0, orig.1 + s.1 - start.1);
                                    if self.project.components[ci].pcb_pos != np {
                                        self.project.components[ci].pcb_pos = np;
                                        self.analysis_dirty = true;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                if resp.drag_stopped() {
                    if let Some(Drag::Band { start, cur, add }) = self.drag.take() {
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
                }
                if resp.clicked() {
                    if let Some(w) = mouse_w {
                        let hit = self.pcb_hit(w, tol);
                        if additive {
                            self.toggle_selected(hit);
                        } else {
                            self.set_selection(vec![hit]);
                            self.highlight_net = match self.sel {
                                Sel::Track(i) => self.conn.track_net(i),
                                Sel::Via(i) => self.conn.via_net(i),
                                _ => self.pad_at(w).and_then(|k| self.conn.pads[k].net),
                            };
                        }
                    }
                }
            }
            PcbTool::Route => {
                if resp.clicked() {
                    if let Some(w) = mouse_w {
                        self.route_click(w);
                    }
                }
                if resp.double_clicked() || resp.secondary_clicked() {
                    self.finish_route();
                }
            }
            PcbTool::Via => {
                if resp.clicked() {
                    if let Some(w) = mouse_w {
                        self.checkpoint();
                        self.project.vias.push(Via { pos: snap_p(w), size: 0.8 });
                    }
                }
            }
            PcbTool::Zone => {
                if resp.clicked() {
                    if let Some(w) = mouse_w {
                        let q = snap_p(w);
                        if self.zone_draft.len() >= 3 && ((q.0 - self.zone_draft[0].0).powi(2) + (q.1 - self.zone_draft[0].1).powi(2)).sqrt() < tol * 2.0 {
                            self.finish_zone();
                        } else if self.zone_draft.last() != Some(&q) {
                            self.zone_draft.push(q);
                        }
                    }
                }
                if resp.double_clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.finish_zone();
                }
            }
            PcbTool::Text => {
                if resp.clicked() {
                    if let Some(w) = mouse_w {
                        let text = self.text_input.trim().to_string();
                        if text.is_empty() {
                            self.status = "Type the text first (left panel).".into();
                        } else {
                            self.checkpoint();
                            self.project.texts.push(BoardText { text, pos: snap_p(w), size: self.text_size, rot: 0, layer: self.text_layer });
                            self.sel = Sel::Text(self.project.texts.len() - 1);
                            self.group.clear();
                            self.status = format!("Text placed on {}. Drag it to move it, R to rotate.", self.text_layer.name());
                        }
                    }
                }
            }
        }
        if resp.secondary_clicked() && self.pcb_tool == PcbTool::Zone && !self.zone_draft.is_empty() {
            // Right-click closes the zone, or drops it when it has fewer than 3 corners.
            if self.zone_draft.len() >= 3 {
                self.finish_zone();
            } else {
                self.zone_draft.clear();
            }
        } else if resp.secondary_clicked() && self.pcb_tool == PcbTool::Select {
            // Right-click on a corner of the selected zone removes it.
            if let (Sel::Zone(zi), Some(w)) = (self.sel, mouse_w) {
                if let Some((oi, vi)) = self.zone_vertex_at(zi, w, tol * 2.0) {
                    if self.project.zones[zi].outlines[oi].len() > 3 {
                        self.checkpoint();
                        self.project.zones[zi].outlines[oi].remove(vi);
                        self.project.zones[zi].outline_changed();
                    }
                }
            }
        } else if resp.secondary_clicked() && self.pcb_tool != PcbTool::Route {
            self.pcb_tool = PcbTool::Select;
        }
        // Double-click on an edge of the selected zone adds a corner.
        if self.pcb_tool == PcbTool::Select && resp.double_clicked() {
            if let (Sel::Zone(zi), Some(w)) = (self.sel, mouse_w) {
                let found = self.project.zones[zi].outlines.iter().enumerate().find_map(|(oi, o)| {
                    (0..o.len()).find(|&k| dist_pt_seg(w, o[k], o[(k + 1) % o.len()]) <= tol * 2.0).map(|k| (oi, k))
                });
                if let Some((oi, k)) = found {
                    self.checkpoint();
                    self.project.zones[zi].outlines[oi].insert(k + 1, snap_p(w));
                    self.project.zones[zi].outline_changed();
                }
            }
        }

        // ---- drawing
        let painter = painter.with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, BG);
        let z = view.zoom;
        let b = &self.project.board;
        let br = Rect::from_two_pos(view.s(min, (b.x0, b.y0)), view.s(min, (b.x1, b.y1)));
        painter.rect_filled(br, 0.0, BOARD);
        // grid
        let step = if z >= 9.0 { 1.0 } else if z * 5.0 >= 9.0 { 5.0 } else { 10.0 };
        let (wx0, wy0) = view.w(min, rect.min);
        let (wx1, wy1) = view.w(min, rect.max);
        let (gx0, gx1) = ((wx0 / step).floor() as i32, (wx1 / step).ceil() as i32);
        let (gy0, gy1) = ((wy0 / step).floor() as i32, (wy1 / step).ceil() as i32);
        if (gx1 - gx0) * (gy1 - gy0) < 40000 {
            for gx in gx0..=gx1 {
                for gy in gy0..=gy1 {
                    let p = view.s(min, (gx as f32 * step, gy as f32 * step));
                    painter.rect_filled(Rect::from_center_size(p, Vec2::splat(1.2)), 0.0, Color32::from_rgba_unmultiplied(160, 170, 190, 60));
                }
            }
        }
        painter.rect_stroke(br, 0.0, Stroke::new((0.15 * z).max(1.5), EDGE));
        // copper zones, filled (inactive layers fainter)
        let zones = zone_fill(self);
        let mut mesh = egui::epaint::Mesh::default();
        for (layer, tris) in zones.iter() {
            if layer.map_or(false, |l| self.hidden_layers.contains(&l)) {
                continue;
            }
            let strength = if *layer == Some(active_layer(self)) { 0.45 } else { 0.25 };
            let col = layer.map(layer_color).unwrap_or(Color32::from_rgb(160, 160, 90)).gamma_multiply(strength);
            for t in tris {
                let i = mesh.vertices.len() as u32;
                for q in t {
                    mesh.colored_vertex(view.s(min, *q), col);
                }
                mesh.add_triangle(i, i + 1, i + 2);
            }
            if mesh.vertices.len() > 50_000 {
                painter.add(Shape::mesh(std::mem::take(&mut mesh)));
            }
        }
        painter.add(Shape::mesh(mesh));
        // zone outlines, net names, and the corners of the selected zone
        for (zi, zn) in self.project.zones.iter().enumerate() {
            if zn.layer.map_or(false, |l| self.hidden_layers.contains(&l)) {
                continue;
            }
            let selected = self.sel == Sel::Zone(zi);
            let col = if selected { SELECT } else { zn.layer.map(layer_color).unwrap_or(Color32::from_rgb(160, 160, 90)) };
            for o in &zn.outlines {
                let mut pts: Vec<Pos2> = o.iter().map(|&q2| view.s(min, q2)).collect();
                if let Some(f0) = pts.first().copied() {
                    pts.push(f0);
                }
                painter.add(Shape::line(pts, Stroke::new(if selected { 2.0 } else { 1.0 }, col.gamma_multiply(if selected { 1.0 } else { 0.8 }))));
                if selected {
                    for &q2 in o {
                        painter.rect_filled(Rect::from_center_size(view.s(min, q2), Vec2::splat(7.0)), 1.0, SELECT);
                    }
                }
                if !zn.net.is_empty() && z > 3.0 {
                    let c = (o.iter().map(|q| q.0).sum::<f32>() / o.len() as f32, o.iter().map(|q| q.1).sum::<f32>() / o.len() as f32);
                    painter.text(view.s(min, c), Align2::CENTER_CENTER, &zn.net, FontId::proportional(11.0), col.gamma_multiply(0.9));
                }
            }
        }
        if !self.zone_draft.is_empty() {
            let mut pts: Vec<Pos2> = self.zone_draft.iter().map(|&q2| view.s(min, q2)).collect();
            if let Some(w) = mouse_w {
                pts.push(view.s(min, snap_p(w)));
            }
            let col = layer_color(self.route_layer);
            painter.add(Shape::line(pts.clone(), Stroke::new(1.5, col)));
            if self.zone_draft.len() >= 2 {
                if let (Some(a), Some(b)) = (pts.last(), pts.first()) {
                    painter.line_segment([*a, *b], Stroke::new(1.0, col.gamma_multiply(0.5)));
                }
            }
            for (k, &q2) in self.zone_draft.iter().enumerate() {
                painter.circle_filled(view.s(min, q2), if k == 0 { 5.0 } else { 3.0 }, col);
            }
        }
        // tracks: back to front, the active layer last (on top)
        let active = self.route_layer;
        let mut order: Vec<Layer> = self.project.board.copper().into_iter().rev().filter(|l| *l != active).collect();
        // Tracks on a layer the board no longer has are still drawn so they can be fixed.
        for t in &self.project.tracks {
            if !order.contains(&t.layer) && t.layer != active {
                order.push(t.layer);
            }
        }
        order.push(active);
        for layer in order {
            if self.hidden_layers.contains(&layer) && layer != active {
                continue;
            }
            for (i, t) in self.project.tracks.iter().enumerate() {
                if t.layer != layer {
                    continue;
                }
                let base = layer_color(layer);
                let net = self.conn.track_net(i);
                let col = if self.is_sel(Sel::Track(i)) {
                    SELECT
                } else if net.is_some() && net == self.highlight_net {
                    HILITE
                } else if layer != active {
                    base.gamma_multiply(0.55)
                } else {
                    base
                };
                let (a, bb) = (view.s(min, t.a), view.s(min, t.b));
                let wd = (t.width * z).max(1.0);
                painter.line_segment([a, bb], Stroke::new(wd, col));
                painter.circle_filled(a, wd / 2.0, col);
                painter.circle_filled(bb, wd / 2.0, col);
            }
        }
        // footprints
        for c in self.project.components.iter().filter(|c| c.has_fp()) {
            let inf = c.info();
            let (x0, y0, x1, y1) = inf.fp_body;
            let selected = self.is_sel(Sel::Comp(c.id));
            let silk = if selected { SELECT } else { SILK };
            let sw = (0.15 * z).max(1.0);
            let xf = |q: (f32, f32)| {
                let r = rot_f(q, c.pcb_rot);
                view.s(min, (c.pcb_pos.0 + r.0, c.pcb_pos.1 + r.1))
            };
            if let Some(cu) = c.custom.as_ref().filter(|cu| !cu.pads.is_empty()) {
                for (a, b) in &cu.silk {
                    painter.line_segment([xf(*a), xf(*b)], Stroke::new(sw, silk));
                }
                for &(x, y, r) in &cu.silk_circles {
                    painter.circle_stroke(xf((x, y)), r * z, Stroke::new(sw, silk));
                }
                if cu.silk.is_empty() && cu.silk_circles.is_empty() {
                    let (x0, y0, x1, y1) = inf.fp_body;
                    let pts: Vec<Pos2> = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].iter().map(|&q| xf(q)).collect();
                    painter.add(Shape::closed_line(pts, Stroke::new(sw, silk)));
                }
            } else if inf.fp_round {
                painter.circle_stroke(view.s(min, c.pcb_pos), (x1 - x0) / 2.0 * z, Stroke::new(sw, silk));
            } else {
                let corners: Vec<Pos2> = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
                    .iter()
                    .map(|&q| {
                        let r = rot_f(q, c.pcb_rot);
                        view.s(min, (c.pcb_pos.0 + r.0, c.pcb_pos.1 + r.1))
                    })
                    .collect();
                painter.add(Shape::closed_line(corners, Stroke::new(sw, silk)));
            }
            if selected {
                let bb = c.pcb_bbox();
                painter.rect_stroke(
                    Rect::from_two_pos(view.s(min, (bb.0 - 0.3, bb.1 - 0.3)), view.s(min, (bb.2 + 0.3, bb.3 + 0.3))),
                    2.0,
                    Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 120)),
                );
            }
            // pin 1 marker
            if let Some(p1) = inf.pins.iter().find(|p| p.pad == "1") {
                let q = c.pad_pos(p1);
                let s = c.pad_size(p1);
                painter.circle_filled(view.s(min, (q.0 - s.0 / 2.0 - 0.4, q.1 - s.1 / 2.0 - 0.4)), (0.18 * z).max(1.0), silk);
            }
        }
        for pad in &self.conn.pads {
            let c = &self.project.components[pad.ci];
            let col = if pad.net.is_some() && pad.net == self.highlight_net {
                HILITE
            } else if pad.tht {
                PAD_THT
            } else {
                PAD_SMD
            };
            let center = view.s(min, pad.pos);
            if pad.round {
                painter.circle_filled(center, pad.size.0.min(pad.size.1) / 2.0 * z, col);
            } else {
                let r = pad.rect();
                painter.rect_filled(Rect::from_two_pos(view.s(min, (r.0, r.1)), view.s(min, (r.2, r.3))), 0.1 * z, col);
            }
            if pad.tht {
                painter.circle_filled(center, pad.size.0.min(pad.size.1) * 0.28 * z, Color32::from_rgb(30, 30, 30));
            }
            if z > 16.0 {
                if let Some(n) = pad.net {
                    let name = &self.nl.nets[n].name;
                    let short: String = name.chars().take(10).collect();
                    painter.text(center, Align2::CENTER_CENTER, short, FontId::proportional((0.45 * z).min(12.0)), Color32::from_rgb(20, 20, 20));
                }
            }
            let _ = c;
        }
        for (i, v) in self.project.vias.iter().enumerate() {
            let p = view.s(min, v.pos);
            let net = self.conn.via_net(i);
            let col = if self.is_sel(Sel::Via(i)) {
                SELECT
            } else if net.is_some() && net == self.highlight_net {
                HILITE
            } else {
                Color32::from_rgb(190, 190, 196)
            };
            painter.circle_filled(p, v.size / 2.0 * z, col);
            painter.circle_filled(p, v.size * 0.2 * z, Color32::from_rgb(30, 30, 30));
        }
        // board texts (silkscreen and copper)
        for (i, t) in self.project.texts.iter().enumerate() {
            let col = match t.layer {
                TextLayer::SilkFront => SILK,
                TextLayer::SilkBack => SILK.gamma_multiply(0.55),
                TextLayer::Copper(l) => {
                    if self.hidden_layers.contains(&l) {
                        continue;
                    }
                    layer_color(l)
                }
            };
            let col = if self.is_sel(Sel::Text(i)) { SELECT } else { col };
            let wd = (crate::font::stroke_width(t.size) * z).max(1.0);
            for stroke in crate::font::board_strokes(t) {
                let pts: Vec<Pos2> = stroke.iter().map(|&q2| view.s(min, q2)).collect();
                painter.add(Shape::line(pts, Stroke::new(wd, col)));
            }
            if self.is_sel(Sel::Text(i)) {
                let b = crate::font::bbox(t);
                painter.rect_stroke(Rect::from_two_pos(view.s(min, (b.0, b.1)), view.s(min, (b.2, b.3))), 2.0, Stroke::new(1.0_f32, SELECT.gamma_multiply(0.5)));
            }
        }
        // reference text
        for c in self.project.components.iter().filter(|c| c.has_fp()) {
            let fs = (1.0 * z).clamp(7.0, 28.0);
            let bb = c.pcb_bbox();
            let p = view.s(min, ((bb.0 + bb.2) / 2.0, bb.1));
            painter.text(p - Vec2::new(0.0, 2.0), Align2::CENTER_BOTTOM, &c.reference, FontId::proportional(fs), SILK);
        }
        // ratsnest
        for (a, bq) in &self.conn.ratsnest {
            painter.line_segment([view.s(min, *a), view.s(min, *bq)], Stroke::new(1.0_f32, RATS));
        }
        // DRC error markers
        for is in &self.drc {
            if is.severity != Severity::Error {
                continue;
            }
            if let Some(p) = is.pcb_pos {
                let sp = view.s(min, p);
                let r = (1.2 * z).max(8.0);
                painter.circle_stroke(sp, r, Stroke::new(2.0_f32, Color32::from_rgb(255, 60, 60)));
                painter.text(sp + Vec2::new(r, -r), Align2::LEFT_BOTTOM, "!", FontId::proportional(14.0), Color32::from_rgb(255, 80, 80));
            }
        }
        // route preview
        if let (Some(r), Some(w)) = (&self.route, mouse_w) {
            let col = layer_color(self.route_layer);
            for (i, s) in r.pts.windows(2).enumerate() {
                let c = layer_color(r.layers[i]);
                painter.line_segment([view.s(min, s[0]), view.s(min, s[1])], Stroke::new((self.track_width * z).max(1.0), c));
            }
            let last = *r.pts.last().unwrap();
            let target = self.pad_at(w).map(|k| self.conn.pads[k].pos).unwrap_or_else(|| snap_p(w));
            let mut prev = last;
            for q in route45(last, target) {
                painter.line_segment([view.s(min, prev), view.s(min, q)], Stroke::new((self.track_width * z).max(1.0), col.gamma_multiply(0.6)));
                prev = q;
            }
            for v in &r.vias {
                painter.circle_filled(view.s(min, *v), 0.4 * z, Color32::from_rgb(190, 190, 196));
            }
        }
        if self.pcb_tool == PcbTool::Text && !self.text_input.trim().is_empty() {
            if let Some(w) = mouse_w {
                let ghost = BoardText { text: self.text_input.clone(), pos: snap_p(w), size: self.text_size, rot: 0, layer: self.text_layer };
                let wd = (crate::font::stroke_width(ghost.size) * z).max(1.0);
                for stroke in crate::font::board_strokes(&ghost) {
                    let pts: Vec<Pos2> = stroke.iter().map(|&q2| view.s(min, q2)).collect();
                    painter.add(Shape::line(pts, Stroke::new(wd, SILK.gamma_multiply(0.5))));
                }
            }
        }
        if self.pcb_tool == PcbTool::Via {
            if let Some(w) = mouse_w {
                painter.circle_stroke(view.s(min, snap_p(w)), 0.4 * z, Stroke::new(1.0_f32, Color32::from_rgb(190, 190, 196)));
            }
        }
        if let Some(Drag::Band { start, cur, .. }) = &self.drag {
            crate::sch::draw_band(&painter, view.s(min, *start), view.s(min, *cur));
        }
        // Legend.
        let lp = rect.left_top() + Vec2::new(10.0, 8.0);
        painter.text(lp, Align2::LEFT_TOP, format!("Board layout — {}", self.project.name), FontId::proportional(13.0), Color32::from_rgb(170, 176, 196));
        let mut items: Vec<(Color32, String)> = self
            .project
            .board
            .copper()
            .into_iter()
            .map(|l| {
                let mut name = l.name();
                if l == active {
                    name.push_str("  ◀ routing");
                } else if self.hidden_layers.contains(&l) {
                    name.push_str("  (hidden)");
                }
                (layer_color(l), name)
            })
            .collect();
        items.push((PAD_THT, "Through-hole pad".into()));
        items.push((RATS, "Not routed yet".into()));
        for (i, (c, t)) in items.iter().enumerate() {
            let y = lp.y + 22.0 + i as f32 * 16.0;
            painter.rect_filled(Rect::from_min_size(Pos2::new(lp.x, y + 3.0), Vec2::new(14.0, 8.0)), 2.0, *c);
            painter.text(Pos2::new(lp.x + 20.0, y), Align2::LEFT_TOP, t, FontId::proportional(12.0), Color32::from_rgb(170, 176, 196));
        }
        if self.conn.unrouted == 0 && !self.conn.pads.is_empty() {
            painter.text(rect.right_top() + Vec2::new(-12.0, 8.0), Align2::RIGHT_TOP, "✔ All connections routed", FontId::proportional(13.0), Color32::from_rgb(120, 220, 140));
        }
    }
}
