//! Multiple selection: Shift/Ctrl+click, rubber-band box, Ctrl+A, and moving or removing
//! several parts, wires, labels, tracks and vias at once.

use crate::app::{App, Drag, Mode, Sel};
use crate::checks::seg_rect_dist;
use crate::model::*;
use std::collections::HashSet;

fn rect_of(a: (f32, f32), b: (f32, f32)) -> (f32, f32, f32, f32) {
    (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

fn inside(p: (f32, f32), r: (f32, f32, f32, f32)) -> bool {
    p.0 >= r.0 && p.0 <= r.2 && p.1 >= r.1 && p.1 <= r.3
}

fn box_in(b: (f32, f32, f32, f32), r: (f32, f32, f32, f32)) -> bool {
    b.0 >= r.0 && b.1 >= r.1 && b.2 <= r.2 && b.3 <= r.3
}

fn box_touch(b: (f32, f32, f32, f32), r: (f32, f32, f32, f32)) -> bool {
    b.0 <= r.2 && r.0 <= b.2 && b.1 <= r.3 && r.1 <= b.3
}

fn fi(p: (i32, i32)) -> (f32, f32) {
    (p.0 as f32, p.1 as f32)
}

impl App {
    pub fn is_sel(&self, s: Sel) -> bool {
        s != Sel::None && (self.sel == s || self.group.contains(&s))
    }

    /// Everything selected (the group, or the single selection).
    pub fn selection(&self) -> Vec<Sel> {
        if !self.group.is_empty() {
            self.group.clone()
        } else if self.sel != Sel::None {
            vec![self.sel]
        } else {
            vec![]
        }
    }

    pub fn set_selection(&mut self, items: Vec<Sel>) {
        let mut v: Vec<Sel> = vec![];
        for s in items {
            if s != Sel::None && !v.contains(&s) {
                v.push(s);
            }
        }
        self.sel = v.first().copied().unwrap_or(Sel::None);
        self.group = if v.len() > 1 { v } else { vec![] };
    }

    /// Shift/Ctrl+click: add or remove one item.
    pub fn toggle_selected(&mut self, s: Sel) {
        if s == Sel::None {
            return;
        }
        let mut v = self.selection();
        if let Some(i) = v.iter().position(|x| *x == s) {
            v.remove(i);
        } else {
            v.push(s);
        }
        self.set_selection(v);
    }

    pub fn select_all(&mut self) {
        let mut v = vec![];
        match self.mode {
            Mode::Schematic => {
                v.extend(self.project.components.iter().map(|c| Sel::Comp(c.id)));
                v.extend((0..self.project.wires.len()).map(Sel::Wire));
                v.extend((0..self.project.labels.len()).map(Sel::Label));
            }
            Mode::Pcb | Mode::View3D => {
                v.extend(self.project.components.iter().filter(|c| c.has_fp()).map(|c| Sel::Comp(c.id)));
                v.extend((0..self.project.tracks.len()).map(Sel::Track));
                v.extend((0..self.project.vias.len()).map(Sel::Via));
            }
        }
        let n = v.len();
        self.set_selection(v);
        self.status = format!("{} items selected.", n);
    }

    /// Drop selected items that no longer exist.
    pub fn prune_selection(&mut self) {
        let ok = |s: &Sel, p: &Project| match *s {
            Sel::Comp(id) => p.find(id).is_some(),
            Sel::Wire(i) => i < p.wires.len(),
            Sel::Label(i) => i < p.labels.len(),
            Sel::Track(i) => i < p.tracks.len(),
            Sel::Via(i) => i < p.vias.len(),
            Sel::None => false,
        };
        let p = &self.project;
        self.group.retain(|s| ok(s, p));
        if self.group.len() == 1 {
            self.sel = self.group[0];
            self.group.clear();
        }
    }

    /// Items inside the box (left-to-right) or touching it (right-to-left).
    pub fn band_items(&self, a: (f32, f32), b: (f32, f32)) -> Vec<Sel> {
        let r = rect_of(a, b);
        let touching = b.0 < a.0;
        let mut v = vec![];
        match self.mode {
            Mode::Schematic => {
                for c in &self.project.components {
                    let bb = c.sch_boxes().iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |m, x| {
                        (m.0.min(x.0 as f32), m.1.min(x.1 as f32), m.2.max(x.2 as f32), m.3.max(x.3 as f32))
                    });
                    if (touching && box_touch(bb, r)) || (!touching && box_in(bb, r)) {
                        v.push(Sel::Comp(c.id));
                    }
                }
                for (i, w) in self.project.wires.iter().enumerate() {
                    let (p, q) = (fi(w.a), fi(w.b));
                    if (inside(p, r) && inside(q, r)) || (touching && seg_rect_dist(p, q, r) <= 0.0) {
                        v.push(Sel::Wire(i));
                    }
                }
                for (i, l) in self.project.labels.iter().enumerate() {
                    if inside(fi(l.pos), r) {
                        v.push(Sel::Label(i));
                    }
                }
            }
            Mode::Pcb | Mode::View3D => {
                for c in self.project.components.iter().filter(|c| c.has_fp()) {
                    let bb = c.pcb_bbox();
                    if (touching && box_touch(bb, r)) || (!touching && box_in(bb, r)) {
                        v.push(Sel::Comp(c.id));
                    }
                }
                for (i, t) in self.project.tracks.iter().enumerate() {
                    if (inside(t.a, r) && inside(t.b, r)) || (touching && seg_rect_dist(t.a, t.b, r) <= 0.0) {
                        v.push(Sel::Track(i));
                    }
                }
                for (i, via) in self.project.vias.iter().enumerate() {
                    if inside(via.pos, r) {
                        v.push(Sel::Via(i));
                    }
                }
            }
        }
        v
    }

    /// Start moving everything selected on the schematic. Wires ending on moved pins or on
    /// moved wire ends stretch to follow.
    pub fn start_sch_group_drag(&mut self, start: (i32, i32)) {
        self.checkpoint();
        let sel = self.selection();
        let ids: HashSet<u32> = sel.iter().filter_map(|s| if let Sel::Comp(id) = s { Some(*id) } else { None }).collect();
        let wires: HashSet<usize> = sel.iter().filter_map(|s| if let Sel::Wire(i) = s { Some(*i) } else { None }).collect();
        let labels: HashSet<usize> = sel.iter().filter_map(|s| if let Sel::Label(i) = s { Some(*i) } else { None }).collect();
        let mut moving: HashSet<(i32, i32)> = HashSet::new();
        let comps: Vec<(u32, (i32, i32))> = self
            .project
            .components
            .iter()
            .filter(|c| ids.contains(&c.id))
            .map(|c| {
                for pd in c.info().pins.iter() {
                    moving.insert(c.pin_pos(pd));
                }
                (c.id, c.pos)
            })
            .collect();
        for &i in &wires {
            if let Some(w) = self.project.wires.get(i) {
                moving.insert(w.a);
                moving.insert(w.b);
            }
        }
        // Pins touching a part that stays put get a new wire, so the connection stretches.
        let fixed_pins: HashSet<(i32, i32)> = self
            .project
            .components
            .iter()
            .filter(|c| !ids.contains(&c.id))
            .flat_map(|c| c.info().pins.iter().map(|pd| c.pin_pos(pd)).collect::<Vec<_>>())
            .collect();
        let moved_pins: Vec<(i32, i32)> = self
            .project
            .components
            .iter()
            .filter(|c| ids.contains(&c.id))
            .flat_map(|c| c.info().pins.iter().map(|pd| c.pin_pos(pd)).collect::<Vec<_>>())
            .collect();
        for q in moved_pins {
            if fixed_pins.contains(&q) {
                self.project.wires.push(Wire { a: q, b: q });
            }
        }
        let wire_moves: Vec<(usize, (i32, i32), (i32, i32), bool, bool)> = self
            .project
            .wires
            .iter()
            .enumerate()
            .filter_map(|(i, w)| {
                if w.a == w.b && fixed_pins.contains(&w.a) {
                    return Some((i, w.a, w.b, false, true));
                }
                let whole = wires.contains(&i);
                let (ma, mb) = (whole || moving.contains(&w.a), whole || moving.contains(&w.b));
                (ma || mb).then_some((i, w.a, w.b, ma, mb))
            })
            .collect();
        let label_moves: Vec<(usize, (i32, i32))> = self
            .project
            .labels
            .iter()
            .enumerate()
            .filter(|(i, l)| labels.contains(i) || moving.contains(&l.pos))
            .map(|(i, l)| (i, l.pos))
            .collect();
        self.drag = Some(Drag::SchGroup { start, comps, wires: wire_moves, labels: label_moves });
    }

    pub fn drag_sch_group(&mut self, d: &Drag, s: (i32, i32)) {
        let Drag::SchGroup { start, comps, wires, labels } = d else { return };
        let dv = (s.0 - start.0, s.1 - start.1);
        let add = |p: (i32, i32)| (p.0 + dv.0, p.1 + dv.1);
        for (id, orig) in comps {
            if let Some(ci) = self.project.find(*id) {
                self.project.components[ci].pos = add(*orig);
            }
        }
        for &(i, a, b, ma, mb) in wires {
            if let Some(w) = self.project.wires.get_mut(i) {
                w.a = if ma { add(a) } else { a };
                w.b = if mb { add(b) } else { b };
            }
        }
        for &(i, orig) in labels {
            if let Some(l) = self.project.labels.get_mut(i) {
                l.pos = add(orig);
            }
        }
        self.analysis_dirty = true;
    }

    /// Start moving everything selected on the board. Track ends on moved pads follow.
    pub fn start_pcb_group_drag(&mut self, start: (f32, f32)) {
        let sel = self.selection();
        let ids: HashSet<u32> = sel.iter().filter_map(|s| if let Sel::Comp(id) = s { Some(*id) } else { None }).collect();
        let tracks: HashSet<usize> = sel.iter().filter_map(|s| if let Sel::Track(i) = s { Some(*i) } else { None }).collect();
        let vias: HashSet<usize> = sel.iter().filter_map(|s| if let Sel::Via(i) = s { Some(*i) } else { None }).collect();
        let comps: Vec<(u32, (f32, f32))> =
            self.project.components.iter().filter(|c| ids.contains(&c.id) && c.has_fp()).map(|c| (c.id, c.pcb_pos)).collect();
        let moved_pads: Vec<crate::checks::PadItem> = self
            .conn
            .pads
            .iter()
            .filter(|p| self.project.components.get(p.ci).map_or(false, |c| ids.contains(&c.id)))
            .cloned()
            .collect();
        let mut ends: Vec<(f32, f32)> = vec![];
        for &i in &tracks {
            if let Some(t) = self.project.tracks.get(i) {
                ends.push(t.a);
                ends.push(t.b);
            }
        }
        for &i in &vias {
            if let Some(v) = self.project.vias.get(i) {
                ends.push(v.pos);
            }
        }
        let near = |p: (f32, f32)| ends.iter().any(|e| (e.0 - p.0).abs() < 0.01 && (e.1 - p.1).abs() < 0.01);
        let on_pad = |p: (f32, f32)| moved_pads.iter().any(|pad| pad.contains(p));
        let track_moves: Vec<(usize, (f32, f32), (f32, f32), bool, bool)> = self
            .project
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                let whole = tracks.contains(&i);
                let ma = whole || on_pad(t.a) || near(t.a);
                let mb = whole || on_pad(t.b) || near(t.b);
                (ma || mb).then_some((i, t.a, t.b, ma, mb))
            })
            .collect();
        let via_moves: Vec<(usize, (f32, f32))> =
            self.project.vias.iter().enumerate().filter(|(i, v)| vias.contains(i) || near(v.pos)).map(|(i, v)| (i, v.pos)).collect();
        self.checkpoint();
        self.drag = Some(Drag::PcbGroup { start, comps, tracks: track_moves, vias: via_moves });
    }

    pub fn drag_pcb_group(&mut self, d: &Drag, s: (f32, f32)) {
        let Drag::PcbGroup { start, comps, tracks, vias } = d else { return };
        let dv = (s.0 - start.0, s.1 - start.1);
        let add = |p: (f32, f32)| (p.0 + dv.0, p.1 + dv.1);
        for (id, orig) in comps {
            if let Some(ci) = self.project.find(*id) {
                self.project.components[ci].pcb_pos = add(*orig);
            }
        }
        for &(i, a, b, ma, mb) in tracks {
            if let Some(t) = self.project.tracks.get_mut(i) {
                t.a = if ma { add(a) } else { a };
                t.b = if mb { add(b) } else { b };
            }
        }
        for &(i, orig) in vias {
            if let Some(v) = self.project.vias.get_mut(i) {
                v.pos = add(orig);
            }
        }
        self.analysis_dirty = true;
    }

    /// Remove every selected item (one undo step).
    pub fn delete_group(&mut self) {
        let sel = self.selection();
        if sel.is_empty() {
            return;
        }
        self.checkpoint();
        let ids: HashSet<u32> = sel.iter().filter_map(|s| if let Sel::Comp(id) = s { Some(*id) } else { None }).collect();
        let take = |f: fn(&Sel) -> Option<usize>| -> HashSet<usize> { sel.iter().filter_map(f).collect() };
        let wires = take(|s| if let Sel::Wire(i) = s { Some(*i) } else { None });
        let labels = take(|s| if let Sel::Label(i) = s { Some(*i) } else { None });
        let tracks = take(|s| if let Sel::Track(i) = s { Some(*i) } else { None });
        let vias = take(|s| if let Sel::Via(i) = s { Some(*i) } else { None });
        self.project.components.retain(|c| !ids.contains(&c.id));
        let mut i = 0;
        self.project.wires.retain(|_| {
            i += 1;
            !wires.contains(&(i - 1))
        });
        let mut i = 0;
        self.project.labels.retain(|_| {
            i += 1;
            !labels.contains(&(i - 1))
        });
        let mut i = 0;
        self.project.tracks.retain(|_| {
            i += 1;
            !tracks.contains(&(i - 1))
        });
        let mut i = 0;
        self.project.vias.retain(|_| {
            i += 1;
            !vias.contains(&(i - 1))
        });
        self.status = format!("Removed {} items.", sel.len());
        self.sel = Sel::None;
        self.group.clear();
    }
}
