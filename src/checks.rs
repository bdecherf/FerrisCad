//! Electrical rules (schematic) and design rules (board) checks.

use crate::model::*;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub severity: Severity,
    pub source: &'static str,
    pub message: String,
    pub comps: Vec<u32>,
    pub sch_pos: Option<(i32, i32)>,
    pub pcb_pos: Option<(f32, f32)>,
    /// One-click fix: set this component's value.
    pub fix: Option<(u32, String)>,
}

impl Issue {
    pub fn new(severity: Severity, source: &'static str, message: String) -> Self {
        Issue { severity, source, message, comps: vec![], sch_pos: None, pcb_pos: None, fix: None }
    }
    pub fn comp(mut self, id: u32) -> Self {
        self.comps.push(id);
        self
    }
    pub fn at_sch(mut self, p: (i32, i32)) -> Self {
        self.sch_pos = Some(p);
        self
    }
    pub fn at_pcb(mut self, p: (f32, f32)) -> Self {
        self.pcb_pos = Some(p);
        self
    }
}

fn pin_desc(p: &Project, ci: usize, pi: usize) -> String {
    let c = &p.components[ci];
    format!("{} pin {}", c.reference, c.info().pins[pi].name)
}

pub fn ground_net(nl: &Netlist) -> Option<usize> {
    nl.nets
        .iter()
        .position(|n| n.power.iter().any(|x| x == "GND") || n.labels.iter().any(|x| x == "GND"))
        .or_else(|| nl.nets.iter().position(|n| n.power.iter().any(|x| is_ground_name(x))))
}

/// Nets that get power: a supply symbol or source pin, or reached from one through
/// a switch, fuse-like inductor, resistor or diode.
pub fn supplied_nets(p: &Project, nl: &Netlist) -> Vec<bool> {
    let mut s: Vec<bool> = nl
        .nets
        .iter()
        .map(|n| {
            !n.power.is_empty()
                || n.pins.iter().any(|&(ci, pi)| {
                    let c = &p.components[ci];
                    !c.kind.is_power() && c.info().pins[pi].ptype == PinType::PowerOut
                })
        })
        .collect();
    loop {
        let mut changed = false;
        for (ci, c) in p.components.iter().enumerate() {
            let passes = match c.kind {
                Kind::Switch => switch_closed(&c.value),
                Kind::Inductor | Kind::Resistor | Kind::Diode | Kind::Led => true,
                _ => false,
            };
            if !passes {
                continue;
            }
            if let (Some(a), Some(b)) = (nl.net_of(ci, 0), nl.net_of(ci, 1)) {
                if s[a] != s[b] {
                    s[a] = true;
                    s[b] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return s;
        }
    }
}

/// Electrical rules check on the schematic.
pub fn erc(p: &Project, nl: &Netlist) -> Vec<Issue> {
    let mut out = Vec::new();
    let gnd = ground_net(nl);
    let supplied = supplied_nets(p, nl);

    // Duplicate references.
    let mut seen: HashMap<&str, u32> = HashMap::new();
    for c in p.components.iter().filter(|c| !c.kind.is_power()) {
        if seen.insert(c.reference.as_str(), c.id).is_some() {
            out.push(
                Issue::new(Severity::Error, "ERC", format!("Two parts are both called {}; give each part its own reference.", c.reference))
                    .comp(c.id)
                    .at_sch(c.pos),
            );
        }
    }

    for (ci, c) in p.components.iter().enumerate() {
        let inf = c.info();
        let nets: Vec<Option<usize>> = (0..inf.pins.len()).map(|pi| nl.net_of(ci, pi)).collect();
        // Unconnected pins.
        for (pi, pd) in inf.pins.iter().enumerate() {
            let Some(n) = nets[pi] else { continue };
            let net = &nl.nets[n];
            if net.pins.len() <= 1 && net.labels.is_empty() {
                if pd.ptype == PinType::NotConnected || p.no_connects.contains(&c.pin_pos(pd)) {
                    continue;
                }
                if c.kind.is_power() {
                    out.push(
                        Issue::new(Severity::Warning, "ERC", format!("Power symbol {} is connected to nothing.", c.value))
                            .comp(c.id)
                            .at_sch(c.pos),
                    );
                    continue;
                }
                if matches!(c.kind, Kind::Mcu8 | Kind::Generic) && pd.ptype == PinType::Bidir {
                    continue; // unused I/O is normal
                }
                let sev = if pd.ptype == PinType::PowerIn { Severity::Error } else { Severity::Warning };
                out.push(
                    Issue::new(sev, "ERC", format!("{} is not connected to anything.", pin_desc(p, ci, pi)))
                        .comp(c.id)
                        .at_sch(c.pin_pos(pd)),
                );
            }
        }
        if c.kind.is_power() {
            continue;
        }
        // Part shorted on itself.
        if nets.len() == 2 && nets[0].is_some() && nets[0] == nets[1] {
            let n = &nl.nets[nets[0].unwrap()].name;
            let sev = if c.kind == Kind::Battery { Severity::Error } else { Severity::Warning };
            out.push(
                Issue::new(sev, "ERC", format!("Both pins of {} are on the same net ({}): the part is shorted out.", c.reference, n))
                    .comp(c.id)
                    .at_sch(c.pos),
            );
        }
        // Power inputs need a source.
        for (pi, pd) in inf.pins.iter().enumerate() {
            if pd.ptype != PinType::PowerIn {
                continue;
            }
            let Some(n) = nets[pi] else { continue };
            let net = &nl.nets[n];
            if net.pins.len() <= 1 {
                continue; // already reported
            }
            if !supplied[n] {
                out.push(
                    Issue::new(
                        Severity::Error,
                        "ERC",
                        format!("{} needs power, but nothing supplies net {}.", pin_desc(p, ci, pi), net.name),
                    )
                    .comp(c.id)
                    .at_sch(c.pin_pos(pd)),
                );
            }
        }
        // LED without a current-limiting resistor.
        if c.kind == Kind::Led {
            let has_r = nets.iter().flatten().any(|&n| {
                nl.part_pins(p, n).any(|(cj, _)| cj != ci && p.components[cj].kind == Kind::Resistor)
            });
            if !has_r {
                out.push(
                    Issue::new(
                        Severity::Warning,
                        "ERC",
                        format!("LED {} has no resistor in series to limit its current.", c.reference),
                    )
                    .comp(c.id)
                    .at_sch(c.pos),
                );
            }
        }
        // Decoupling capacitors on chip supply pins.
        if matches!(c.kind, Kind::Mcu8 | Kind::OpAmp) {
            for (pi, pd) in inf.pins.iter().enumerate() {
                if pd.ptype != PinType::PowerIn || pd.name == "GND" || pd.name == "V-" {
                    continue;
                }
                let Some(n) = nets[pi] else { continue };
                if Some(n) == gnd {
                    continue;
                }
                let has_cap = nl.part_pins(p, n).any(|(cj, pj)| {
                    let k = p.components[cj].kind;
                    if !matches!(k, Kind::Capacitor | Kind::CapPolarized) {
                        return false;
                    }
                    let other = 1 - pj;
                    nl.net_of(cj, other).is_some() && nl.net_of(cj, other) == gnd
                });
                if !has_cap {
                    out.push(
                        Issue::new(
                            Severity::Warning,
                            "ERC",
                            format!(
                                "No decoupling capacitor between {} {} ({}) and GND. Add a 100n capacitor close to the pin.",
                                c.reference, pd.name, nl.nets[n].name
                            ),
                        )
                        .comp(c.id)
                        .at_sch(c.pin_pos(pd)),
                    );
                }
            }
        }
        // Polarized capacitor obviously reversed.
        if c.kind == Kind::CapPolarized && nets[0].is_some() && nets[0] == gnd && nets[1] != gnd {
            out.push(
                Issue::new(
                    Severity::Warning,
                    "ERC",
                    format!("{}'s + pin goes to GND: the capacitor looks reversed.", c.reference),
                )
                .comp(c.id)
                .at_sch(c.pos),
            );
        }
        // Transistor base/gate straight to a supply.
        if matches!(c.kind, Kind::Npn | Kind::Pnp) {
            if let Some(n) = nets[0] {
                let net = &nl.nets[n];
                let supply = net.power.iter().any(|x| x != "GND")
                    || net.pins.iter().any(|&(cj, pj)| p.components[cj].info().pins[pj].ptype == PinType::PowerOut);
                if supply {
                    out.push(
                        Issue::new(
                            Severity::Warning,
                            "ERC",
                            format!("{}'s base is wired straight to a supply; a base resistor is needed.", c.reference),
                        )
                        .comp(c.id)
                        .at_sch(c.pos),
                    );
                }
            }
        }
    }

    // Different supplies joined together, and outputs fighting.
    for net in &nl.nets {
        let names: BTreeSet<&String> = net.power.iter().collect();
        if names.len() >= 2 {
            let list: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            let all_ground = names.iter().all(|n| is_ground_name(n));
            let mut is = if all_ground {
                Issue::new(
                    Severity::Warning,
                    "ERC",
                    format!("Grounds {} are joined on one net; check this is intended (e.g. a single ground point).", list.join(" and ")),
                )
            } else {
                Issue::new(
                    Severity::Error,
                    "ERC",
                    format!("Short circuit: supplies {} are wired together.", list.join(" and ")),
                )
            };
            if let Some(&(ci, _)) = net.pins.first() {
                is = is.comp(p.components[ci].id).at_sch(p.components[ci].pos);
            }
            out.push(is);
        }
        let drivers: Vec<(usize, usize)> = net
            .pins
            .iter()
            .copied()
            .filter(|&(ci, pi)| {
                let c = &p.components[ci];
                !c.kind.is_power() && matches!(c.info().pins[pi].ptype, PinType::Output | PinType::PowerOut)
            })
            .collect();
        let mut owners: Vec<usize> = drivers.iter().map(|d| d.0).collect();
        owners.dedup();
        if owners.len() >= 2 {
            let names: Vec<String> = drivers.iter().map(|&(ci, pi)| pin_desc(p, ci, pi)).collect();
            let mut is = Issue::new(
                Severity::Error,
                "ERC",
                format!("Outputs fighting on net {}: {}.", net.name, names.join(", ")),
            );
            for &(ci, _) in &drivers {
                is = is.comp(p.components[ci].id);
            }
            out.push(is);
        }
    }
    for (li, l) in p.labels.iter().enumerate() {
        let n = nl.label_net[li];
        if nl.part_pins(p, n).count() == 0 {
            out.push(
                Issue::new(Severity::Warning, "ERC", format!("Label {} isn't attached to any pin.", l.text)).at_sch(l.pos),
            );
        }
    }
    out
}

// ---------------------------------------------------------------- geometry

pub fn dist_pt_seg(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 <= 1e-12 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

fn orient(a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

fn segs_cross(a: (f32, f32), b: (f32, f32), c: (f32, f32), d: (f32, f32)) -> bool {
    let d1 = orient(c, d, a);
    let d2 = orient(c, d, b);
    let d3 = orient(a, b, c);
    let d4 = orient(a, b, d);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

pub fn seg_seg_dist(a: (f32, f32), b: (f32, f32), c: (f32, f32), d: (f32, f32)) -> f32 {
    if segs_cross(a, b, c, d) {
        return 0.0;
    }
    dist_pt_seg(a, c, d).min(dist_pt_seg(b, c, d)).min(dist_pt_seg(c, a, b)).min(dist_pt_seg(d, a, b))
}

fn pt_rect_dist(p: (f32, f32), r: (f32, f32, f32, f32)) -> f32 {
    let dx = (r.0 - p.0).max(0.0).max(p.0 - r.2);
    let dy = (r.1 - p.1).max(0.0).max(p.1 - r.3);
    (dx * dx + dy * dy).sqrt()
}

pub fn seg_rect_dist(a: (f32, f32), b: (f32, f32), r: (f32, f32, f32, f32)) -> f32 {
    let inside = |p: (f32, f32)| p.0 >= r.0 && p.0 <= r.2 && p.1 >= r.1 && p.1 <= r.3;
    if inside(a) || inside(b) {
        return 0.0;
    }
    let cs = [(r.0, r.1), (r.2, r.1), (r.2, r.3), (r.0, r.3)];
    for i in 0..4 {
        if segs_cross(a, b, cs[i], cs[(i + 1) % 4]) {
            return 0.0;
        }
    }
    let mut d = pt_rect_dist(a, r).min(pt_rect_dist(b, r));
    for c in cs {
        d = d.min(dist_pt_seg(c, a, b));
    }
    d
}

#[derive(Clone, Debug)]
pub struct PadItem {
    pub ci: usize,
    pub pi: usize,
    pub pos: (f32, f32),
    pub size: (f32, f32),
    pub round: bool,
    pub tht: bool,
    pub back: bool,
    pub net: Option<usize>,
}

impl PadItem {
    pub fn rect(&self) -> (f32, f32, f32, f32) {
        (self.pos.0 - self.size.0 / 2.0, self.pos.1 - self.size.1 / 2.0, self.pos.0 + self.size.0 / 2.0, self.pos.1 + self.size.1 / 2.0)
    }
    pub fn on(&self, l: Layer) -> bool {
        match l {
            Layer::Front => self.tht || !self.back,
            Layer::Back => self.tht || self.back,
            // Through-hole pads go through every layer; SMD pads stay on the outside.
            Layer::Inner(_) => self.tht,
        }
    }
    pub fn seg_dist(&self, a: (f32, f32), b: (f32, f32)) -> f32 {
        if self.round {
            (dist_pt_seg(self.pos, a, b) - self.size.0.min(self.size.1) / 2.0).max(0.0)
        } else {
            seg_rect_dist(a, b, self.rect())
        }
    }
    pub fn contains(&self, p: (f32, f32)) -> bool {
        if self.round {
            ((p.0 - self.pos.0).powi(2) + (p.1 - self.pos.1).powi(2)).sqrt() <= self.size.0.min(self.size.1) / 2.0
        } else {
            pt_rect_dist(p, self.rect()) <= 0.0
        }
    }
}

pub fn pad_items(p: &Project, nl: &Netlist) -> Vec<PadItem> {
    let mut v = Vec::new();
    for (ci, c) in p.components.iter().enumerate() {
        if !c.has_fp() {
            continue;
        }
        for (pi, pd) in c.info().pins.iter().enumerate() {
            if pd.pad.is_empty() {
                continue;
            }
            v.push(PadItem {
                ci,
                pi,
                pos: c.pad_pos(pd),
                size: c.pad_size(pd),
                round: pd.shape == PadShape::Round,
                tht: pd.tht,
                back: pd.back,
                net: nl.net_of(ci, pi),
            });
        }
    }
    v
}

#[derive(Clone, Debug, Default)]
pub struct PcbConn {
    pub pads: Vec<PadItem>,
    pub pad_island: Vec<usize>,
    pub track_island: Vec<usize>,
    pub via_island: Vec<usize>,
    /// Schematic net of each copper island when it has exactly one.
    pub island_net: Vec<Option<usize>>,
    pub ratsnest: Vec<((f32, f32), (f32, f32))>,
    pub unrouted: usize,
}

impl PcbConn {
    pub fn track_net(&self, ti: usize) -> Option<usize> {
        self.track_island.get(ti).and_then(|&i| self.island_net[i])
    }
    pub fn via_net(&self, vi: usize) -> Option<usize> {
        self.via_island.get(vi).and_then(|&i| self.island_net[i])
    }
}

/// Copper connectivity + design rules check on the board.
pub fn drc(p: &Project, nl: &Netlist) -> (Vec<Issue>, PcbConn) {
    let mut out = Vec::new();
    let pads = pad_items(p, nl);
    let (np, nt, nv) = (pads.len(), p.tracks.len(), p.vias.len());
    let nz = p.zones.len();
    let mut uf = Uf::new(np + nt + nv + nz);
    let t0 = np;
    let v0 = np + nt;
    // track-track
    for i in 0..nt {
        for j in i + 1..nt {
            let (a, b) = (&p.tracks[i], &p.tracks[j]);
            if a.layer == b.layer && seg_seg_dist(a.a, a.b, b.a, b.b) < (a.width + b.width) / 2.0 - 1e-4 {
                uf.union(t0 + i, t0 + j);
            }
        }
    }
    // track-pad, via-pad, pad-pad
    for (k, pad) in pads.iter().enumerate() {
        for (i, t) in p.tracks.iter().enumerate() {
            if pad.on(t.layer) && pad.seg_dist(t.a, t.b) < t.width / 2.0 - 1e-4 {
                uf.union(k, t0 + i);
            }
        }
        for (i, v) in p.vias.iter().enumerate() {
            if pad.seg_dist(v.pos, v.pos) < v.size / 2.0 - 1e-4 {
                uf.union(k, v0 + i);
            }
        }
        for (k2, pad2) in pads.iter().enumerate().skip(k + 1) {
            if pad2.ci == pad.ci {
                continue;
            }
            let (r1, r2) = (pad.rect(), pad2.rect());
            let overlap = r1.0 < r2.2 && r2.0 < r1.2 && r1.1 < r2.3 && r2.1 < r1.3;
            if overlap {
                uf.union(k, k2);
            }
        }
    }
    // via-track
    for (i, v) in p.vias.iter().enumerate() {
        for (j, t) in p.tracks.iter().enumerate() {
            if dist_pt_seg(v.pos, t.a, t.b) < v.size / 2.0 + t.width / 2.0 - 1e-4 {
                uf.union(v0 + i, t0 + j);
            }
        }
    }
    // copper zones
    let z0 = np + nt + nv;
    for (zi, z) in p.zones.iter().enumerate() {
        let inside = |q: (f32, f32)| z.polys.iter().any(|poly| point_in_poly(q, poly));
        for (k, pad) in pads.iter().enumerate() {
            let on = match z.layer {
                Some(l) => pad.on(l),
                None => pad.tht,
            };
            // Thermal spokes touch the pad edge, so test points across the pad, not just its centre.
            let (hx, hy) = (pad.size.0 * 0.45, pad.size.1 * 0.45);
            let probes = [(0.0, 0.0), (hx, 0.0), (-hx, 0.0), (0.0, hy), (0.0, -hy), (hx, hy), (-hx, hy), (hx, -hy), (-hx, -hy)];
            if on && probes.iter().any(|d| inside((pad.pos.0 + d.0, pad.pos.1 + d.1))) {
                uf.union(z0 + zi, k);
            }
        }
        for (i, v) in p.vias.iter().enumerate() {
            let r = v.size * 0.45;
            if [(0.0, 0.0), (r, 0.0), (-r, 0.0), (0.0, r), (0.0, -r)].iter().any(|d| inside((v.pos.0 + d.0, v.pos.1 + d.1))) {
                uf.union(z0 + zi, np + nt + i);
            }
        }
        if let Some(l) = z.layer {
            for (i, t) in p.tracks.iter().enumerate() {
                if t.layer == l && (inside(t.a) || inside(t.b)) {
                    uf.union(z0 + zi, t0 + i);
                }
            }
        }
    }
    // islands
    let mut root_isl: HashMap<usize, usize> = HashMap::new();
    let mut isl = vec![0usize; np + nt + nv + nz];
    for (i, slot) in isl.iter_mut().enumerate() {
        let r = uf.find(i);
        let n = root_isl.len();
        *slot = *root_isl.entry(r).or_insert(n);
    }
    let n_isl = root_isl.len();
    let mut isl_nets: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n_isl];
    for (k, pad) in pads.iter().enumerate() {
        if let Some(n) = pad.net {
            isl_nets[isl[k]].insert(n);
        }
    }
    let island_net: Vec<Option<usize>> =
        isl_nets.iter().map(|s| if s.len() == 1 { s.iter().next().copied() } else { None }).collect();
    let item_pos = |i: usize| -> (f32, f32) {
        if i < np {
            pads[i].pos
        } else if i < v0 {
            let t = &p.tracks[i - t0];
            ((t.a.0 + t.b.0) / 2.0, (t.a.1 + t.b.1) / 2.0)
        } else if i < z0 {
            p.vias[i - v0].pos
        } else {
            p.zones[i - z0].polys.first().and_then(|q| q.first().copied()).unwrap_or_default()
        }
    };
    for (is, nets) in isl_nets.iter().enumerate() {
        if nets.len() >= 2 {
            let names: Vec<&str> = nets.iter().map(|&n| nl.nets[n].name.as_str()).collect();
            let at = (0..isl.len()).find(|&i| isl[i] == is && i >= np).or_else(|| (0..isl.len()).find(|&i| isl[i] == is));
            let mut issue = Issue::new(
                Severity::Error,
                "DRC",
                format!("Short circuit on the board: copper joins nets {}.", names.join(" and ")),
            );
            if let Some(i) = at {
                issue = issue.at_pcb(item_pos(i));
            }
            for (k, pad) in pads.iter().enumerate() {
                if isl[k] == is && !issue.comps.contains(&p.components[pad.ci].id) {
                    issue.comps.push(p.components[pad.ci].id);
                }
            }
            out.push(issue);
        }
    }
    // Ratsnest & unrouted.
    let mut ratsnest = Vec::new();
    let mut unrouted = 0;
    let mut by_net: HashMap<usize, Vec<usize>> = HashMap::new();
    for (k, pad) in pads.iter().enumerate() {
        if let Some(n) = pad.net {
            by_net.entry(n).or_default().push(k);
        }
    }
    let mut net_ids: Vec<usize> = by_net.keys().copied().collect();
    net_ids.sort();
    for n in net_ids {
        let ks = &by_net[&n];
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut gi: HashMap<usize, usize> = HashMap::new();
        for &k in ks {
            let g = *gi.entry(isl[k]).or_insert_with(|| {
                groups.push(vec![]);
                groups.len() - 1
            });
            groups[g].push(k);
        }
        if groups.len() < 2 {
            continue;
        }
        let missing = groups.len() - 1;
        unrouted += missing;
        let mut in_tree = vec![false; groups.len()];
        in_tree[0] = true;
        for _ in 1..groups.len() {
            let mut best: Option<(f32, usize, usize, usize)> = None;
            for (g, gr) in groups.iter().enumerate() {
                if !in_tree[g] {
                    continue;
                }
                for (h, hr) in groups.iter().enumerate() {
                    if in_tree[h] {
                        continue;
                    }
                    for &a in gr {
                        for &b in hr {
                            let d = (pads[a].pos.0 - pads[b].pos.0).hypot(pads[a].pos.1 - pads[b].pos.1);
                            if best.map_or(true, |x| d < x.0) {
                                best = Some((d, a, b, h));
                            }
                        }
                    }
                }
            }
            if let Some((_, a, b, h)) = best {
                in_tree[h] = true;
                ratsnest.push((pads[a].pos, pads[b].pos));
            }
        }
        let first = &pads[groups[0][0]];
        out.push(
            Issue::new(
                Severity::Warning,
                "DRC",
                format!(
                    "Net {}: {} connection{} not routed yet.",
                    nl.nets[n].name,
                    missing,
                    if missing > 1 { "s" } else { "" }
                ),
            )
            .comp(p.components[first.ci].id)
            .at_pcb(first.pos),
        );
    }
    // Clearance.
    let clear: f32 = if p.board.clearance > 0.0 { p.board.clearance } else { 0.2 };
    let same_net = |ia: usize, ib: usize| island_net[ia].is_some() && island_net[ia] == island_net[ib];
    let mut clearance_hits = 0;
    for i in 0..nt {
        let a = &p.tracks[i];
        for j in i + 1..nt {
            let b = &p.tracks[j];
            if a.layer != b.layer || isl[t0 + i] == isl[t0 + j] || same_net(isl[t0 + i], isl[t0 + j]) {
                continue;
            }
            let gap = seg_seg_dist(a.a, a.b, b.a, b.b) - (a.width + b.width) / 2.0;
            if gap < clear && clearance_hits < 20 {
                clearance_hits += 1;
                out.push(
                    Issue::new(
                        Severity::Error,
                        "DRC",
                        format!("Tracks too close: {:.2} mm gap (minimum {:.2} mm).", gap.max(0.0), clear),
                    )
                    .at_pcb(((a.a.0 + a.b.0) / 2.0, (a.a.1 + a.b.1) / 2.0)),
                );
            }
        }
        for (k, pad) in pads.iter().enumerate() {
            if !pad.on(a.layer) || isl[k] == isl[t0 + i] || (pad.net.is_some() && pad.net == island_net[isl[t0 + i]]) {
                continue;
            }
            let gap = pad.seg_dist(a.a, a.b) - a.width / 2.0;
            if gap < clear && clearance_hits < 20 {
                clearance_hits += 1;
                let c = &p.components[pad.ci];
                out.push(
                    Issue::new(
                        Severity::Error,
                        "DRC",
                        format!(
                            "Track passes too close to {} pad {} ({:.2} mm gap).",
                            c.reference,
                            c.info().pins[pad.pi].pad,
                            gap.max(0.0)
                        ),
                    )
                    .comp(c.id)
                    .at_pcb(pad.pos),
                );
            }
        }
    }
    // Placement.
    let b = &p.board;
    let boxes: Vec<(usize, (f32, f32, f32, f32))> =
        p.components.iter().enumerate().filter(|(_, c)| c.has_fp()).map(|(i, c)| (i, c.pcb_bbox())).collect();
    for (i, bb) in &boxes {
        let c = &p.components[*i];
        if bb.0 < b.x0 || bb.1 < b.y0 || bb.2 > b.x1 || bb.3 > b.y1 {
            out.push(
                Issue::new(Severity::Warning, "DRC", format!("{} is outside the board outline.", c.reference))
                    .comp(c.id)
                    .at_pcb(c.pcb_pos),
            );
        }
    }
    for (x, (i, a)) in boxes.iter().enumerate() {
        for (j, bb) in boxes.iter().skip(x + 1) {
            let m = 0.05;
            if a.0 + m < bb.2 && bb.0 + m < a.2 && a.1 + m < bb.3 && bb.1 + m < a.3 {
                let (ci, cj) = (&p.components[*i], &p.components[*j]);
                out.push(
                    Issue::new(Severity::Warning, "DRC", format!("{} and {} overlap on the board.", ci.reference, cj.reference))
                        .comp(ci.id)
                        .comp(cj.id)
                        .at_pcb(ci.pcb_pos),
                );
            }
        }
    }
    let mut orphan = 0;
    for t in &p.tracks {
        if !p.board.has(t.layer) {
            orphan += 1;
        }
    }
    if orphan > 0 {
        out.push(Issue::new(
            Severity::Error,
            "DRC",
            format!("{} track(s) are on a copper layer this board doesn't have ({} layers). Select them and move them to another layer.", orphan, p.board.layers),
        ));
    }
    for t in &p.tracks {
        for q in [t.a, t.b] {
            if q.0 < b.x0 || q.1 < b.y0 || q.0 > b.x1 || q.1 > b.y1 {
                out.push(Issue::new(Severity::Warning, "DRC", "A track leaves the board outline.".into()).at_pcb(q));
                break;
            }
        }
    }
    for (i, _) in p.tracks.iter().enumerate() {
        let is = isl[t0 + i];
        if isl_nets[is].is_empty() {
            let t = &p.tracks[i];
            out.push(
                Issue::new(Severity::Info, "DRC", "A track isn't connected to any pad.".into()).at_pcb(t.a),
            );
        }
    }
    let conn = PcbConn {
        pad_island: isl[..np].to_vec(),
        track_island: isl[t0..v0].to_vec(),
        via_island: isl[v0..z0].to_vec(),
        pads,
        island_net,
        ratsnest,
        unrouted,
    };
    (out, conn)
}
