//! Import of KiCad projects (KiCad 6 and later): the schematic (.kicad_sch) and
//! the board (.kicad_pcb). Both are S-expression text files.

use crate::model::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- S-expressions

#[derive(Debug, Clone)]
pub enum Sx {
    L(Vec<Sx>),
    A(String),
}

impl Sx {
    pub fn list(&self) -> &[Sx] {
        match self {
            Sx::L(v) => v,
            Sx::A(_) => &[],
        }
    }
    pub fn atom(&self) -> &str {
        match self {
            Sx::A(s) => s,
            Sx::L(_) => "",
        }
    }
    pub fn head(&self) -> &str {
        self.list().first().map(|x| x.atom()).unwrap_or("")
    }
    pub fn get(&self, name: &str) -> Option<&Sx> {
        self.list().iter().find(|x| x.head() == name)
    }
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Sx> + 'a {
        self.list().iter().filter(move |x| x.head() == name)
    }
    /// Argument i (1 = first after the head).
    pub fn arg(&self, i: usize) -> &str {
        self.list().get(i).map(|x| x.atom()).unwrap_or("")
    }
    pub fn num(&self, i: usize) -> f32 {
        self.arg(i).parse().unwrap_or(0.0)
    }
    pub fn xy(&self, name: &str) -> Option<(f32, f32)> {
        self.get(name).map(|s| (s.num(1), s.num(2)))
    }
    pub fn has_atom(&self, a: &str) -> bool {
        self.list().iter().any(|x| x.atom() == a)
    }
    /// Hidden text: `(effects ... hide)`, `(effects ... (hide yes))` or `(hide yes)`.
    pub fn hidden(&self) -> bool {
        let yes = |s: &Sx| s.get("hide").map_or(false, |h| h.arg(1) != "no");
        yes(self) || self.get("effects").map_or(false, |e| e.has_atom("hide") || yes(e))
    }
}

pub fn parse_sx(text: &str) -> Result<Sx, String> {
    let mut stack: Vec<Vec<Sx>> = vec![vec![]];
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '(' => stack.push(vec![]),
            ')' => {
                let done = stack.pop().ok_or("unbalanced )")?;
                stack.last_mut().ok_or("unbalanced )")?.push(Sx::L(done));
            }
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some(o) => s.push(o),
                            None => {}
                        },
                        '"' => break,
                        o => s.push(o),
                    }
                }
                stack.last_mut().ok_or("bad file")?.push(Sx::A(s));
            }
            c if c.is_whitespace() => {}
            c => {
                let mut s = String::from(c);
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || n == '(' || n == ')' {
                        break;
                    }
                    s.push(n);
                    chars.next();
                }
                stack.last_mut().ok_or("bad file")?.push(Sx::A(s));
            }
        }
    }
    let mut top = stack.pop().ok_or("empty file")?;
    if !stack.is_empty() {
        return Err("the file is cut short (missing closing brackets)".into());
    }
    if top.is_empty() {
        return Err("empty file".into());
    }
    Ok(top.remove(0))
}

// ---------------------------------------------------------------- geometry helpers

/// KiCad schematic grid is 1.27 mm; ours is 10 units.
pub const U: f32 = 10.0 / 1.27;

fn rot_ccw(p: (f32, f32), deg: f32) -> (f32, f32) {
    let r = deg.to_radians();
    let (s, c) = r.sin_cos();
    let q = (p.0 * c + p.1 * s, -p.0 * s + p.1 * c);
    (clean(q.0), clean(q.1))
}

fn clean(v: f32) -> f32 {
    if v.abs() < 1e-5 {
        0.0
    } else {
        v
    }
}

fn arc_pts(a: (f32, f32), m: (f32, f32), b: (f32, f32)) -> Vec<(f32, f32)> {
    let d = 2.0 * (a.0 * (m.1 - b.1) + m.0 * (b.1 - a.1) + b.0 * (a.1 - m.1));
    if d.abs() < 1e-6 {
        return vec![a, m, b];
    }
    let sq = |p: (f32, f32)| p.0 * p.0 + p.1 * p.1;
    let cx = (sq(a) * (m.1 - b.1) + sq(m) * (b.1 - a.1) + sq(b) * (a.1 - m.1)) / d;
    let cy = (sq(a) * (b.0 - m.0) + sq(m) * (a.0 - b.0) + sq(b) * (m.0 - a.0)) / d;
    let r = ((a.0 - cx).powi(2) + (a.1 - cy).powi(2)).sqrt();
    let ang = |p: (f32, f32)| (p.1 - cy).atan2(p.0 - cx);
    let tau = std::f32::consts::TAU;
    let (t0, tm, t1) = (ang(a), ang(m), ang(b));
    let d1 = (t1 - t0).rem_euclid(tau);
    let dm = (tm - t0).rem_euclid(tau);
    let sweep = if dm <= d1 { d1 } else { d1 - tau };
    (0..=12).map(|i| {
        let t = t0 + sweep * i as f32 / 12.0;
        (cx + r * t.cos(), cy + r * t.sin())
    })
    .collect()
}

fn pin_type(s: &str) -> PinType {
    match s {
        "input" => PinType::Input,
        "output" | "open_collector" | "open_emitter" => PinType::Output,
        "bidirectional" | "tri_state" => PinType::Bidir,
        "power_in" => PinType::PowerIn,
        "power_out" => PinType::PowerOut,
        "no_connect" => PinType::NotConnected,
        _ => PinType::Passive,
    }
}

// ---------------------------------------------------------------- loading

pub struct Imported {
    pub project: Project,
    pub notes: Vec<String>,
}

/// Find the schematic and board files that belong to a project, from any of its files or its folder.
fn project_files(path: &Path) -> (Option<PathBuf>, Option<PathBuf>, String) {
    let (dir, stem) = if path.is_dir() {
        let pro = std::fs::read_dir(path)
            .ok()
            .and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.extension().map_or(false, |e| e == "kicad_pro")));
        let stem = pro
            .as_ref()
            .and_then(|p| p.file_stem())
            .or_else(|| path.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        (path.to_path_buf(), stem)
    } else {
        (
            path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        )
    };
    let pick = |ext: &str| -> Option<PathBuf> {
        let p = dir.join(format!("{}.{}", stem, ext));
        if p.exists() {
            return Some(p);
        }
        if path.extension().map_or(false, |e| e == ext) && path.exists() {
            return Some(path.to_path_buf());
        }
        // Single file of that type in the folder.
        let v: Vec<PathBuf> = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |e| e == ext))
            .collect();
        if v.len() == 1 {
            v.into_iter().next()
        } else {
            None
        }
    };
    (pick("kicad_sch"), pick("kicad_pcb"), stem)
}

pub fn is_kicad_path(path: &Path) -> bool {
    if path.is_dir() {
        return std::fs::read_dir(path).map_or(false, |rd| {
            rd.flatten().any(|e| {
                e.path().extension().map_or(false, |x| x == "kicad_pro" || x == "kicad_sch" || x == "kicad_pcb")
            })
        });
    }
    path.extension().map_or(false, |e| e == "kicad_pro" || e == "kicad_sch" || e == "kicad_pcb")
}

pub fn import(path: &Path) -> Result<Imported, String> {
    let (sch, pcb, stem) = project_files(path);
    if sch.is_none() && pcb.is_none() {
        return Err("no KiCad schematic (.kicad_sch) or board (.kicad_pcb) found there".into());
    }
    let mut p = Project { name: stem, ..Default::default() };
    let mut notes = Vec::new();
    if let Some(s) = &sch {
        let text = std::fs::read_to_string(s).map_err(|e| format!("couldn't read {}: {}", s.display(), e))?;
        if text.trim_start().starts_with("EESchema") {
            return Err("this schematic uses the old KiCad 5 format; open and save it once in KiCad 6 or later".into());
        }
        let root = parse_sx(&text).map_err(|e| format!("{}: {}", s.display(), e))?;
        let dir = s.parent().map(|d| d.to_path_buf()).unwrap_or_default();
        import_schematic(&root, &mut p, &mut notes, (0.0, 0.0), "", &dir, 0);
    }
    if let Some(b) = &pcb {
        let text = std::fs::read_to_string(b).map_err(|e| format!("couldn't read {}: {}", b.display(), e))?;
        let root = parse_sx(&text).map_err(|e| format!("{}: {}", b.display(), e))?;
        import_board(&root, &mut p, &mut notes, sch.is_none());
    } else {
        notes.push("No board file was found, so footprints are standard ones parked beside the board.".into());
    }
    // Clearance rule from the project settings (default net class).
    let pro_dir = sch.as_ref().or(pcb.as_ref()).and_then(|f| f.parent()).map(|d| d.to_path_buf()).unwrap_or_default();
    if let Some(v) = std::fs::read_to_string(pro_dir.join(format!("{}.kicad_pro", p.name)))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    {
        let cl = v["net_settings"]["classes"]
            .as_array()
            .and_then(|a| a.iter().find(|c| c["name"] == "Default").or(a.first()))
            .and_then(|c| c["clearance"].as_f64());
        if let Some(cl) = cl {
            p.board.clearance = cl as f32;
            notes.push(format!("Using the project's clearance rule: {:.2} mm.", cl));
        }
    }
    // Park parts that have a footprint but no board position.
    let placed: Vec<bool> = p.components.iter().map(|c| c.custom.as_ref().map_or(false, |cu| !cu.pads.is_empty())).collect();
    let mut k = 0;
    for (i, c) in p.components.iter_mut().enumerate() {
        if !placed[i] && c.kind.has_footprint() && c.kind != Kind::Generic {
            c.pcb_pos = (p.board.x1 + 8.0, p.board.y0 + 4.0 + k as f32 * 7.0);
            k += 1;
        }
    }
    p.next_id = p.components.iter().map(|c| c.id).max().unwrap_or(0);
    let parts = p.components.iter().filter(|c| !c.kind.is_power()).count();
    notes.insert(
        0,
        format!(
            "Imported from KiCad: {} parts, {} wires, {} labels, {} tracks, {} vias.",
            parts,
            p.wires.len(),
            p.labels.len(),
            p.tracks.len(),
            p.vias.len()
        ),
    );
    Ok(Imported { project: p, notes })
}

// ---------------------------------------------------------------- schematic

struct Inst<'a> {
    /// Part type saved by a FerrisCad export.
    fc_kind: Option<Kind>,
    lcsc: String,
    lib_id: String,
    at: (f32, f32, f32),
    mirror: String,
    unit: i32,
    reference: String,
    value: String,
    footprint: String,
    ref_at: Option<(f32, f32)>,
    val_at: Option<(f32, f32)>,
    lib: Option<&'a Sx>,
}

pub fn prop<'a>(s: &'a Sx, name: &str) -> Option<&'a Sx> {
    s.all("property").find(|p| p.arg(1) == name)
}

fn sch_xf(i: &Inst, l: (f32, f32)) -> (f32, f32) {
    let q = rot_ccw((l.0, -l.1), i.at.2);
    let q = match i.mirror.as_str() {
        "x" => (q.0, -q.1),
        "y" => (-q.0, q.1),
        _ => q,
    };
    (i.at.0 + q.0, i.at.1 + q.1)
}

pub fn sub_unit(name: &str) -> (i32, i32) {
    let mut it = name.rsplit('_');
    let style = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let unit = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (unit, style)
}

/// Bounding box (mm) of a sheet's content.
fn content_bbox(root: &Sx) -> Option<(f32, f32, f32, f32)> {
    let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut add = |q: (f32, f32)| b = (b.0.min(q.0), b.1.min(q.1), b.2.max(q.0), b.3.max(q.1));
    for item in root.list() {
        match item.head() {
            "symbol" | "label" | "global_label" | "hierarchical_label" | "no_connect" | "junction" => {
                if let Some(a) = item.xy("at") {
                    add(a);
                }
            }
            "text" => {
                if let Some(a) = item.xy("at") {
                    add(a);
                }
            }
            "text_box" => {
                if let (Some(a), Some(sz)) = (item.xy("at"), item.xy("size")) {
                    add(a);
                    add((a.0 + sz.0, a.1 + sz.1));
                }
            }
            "rectangle" => {
                for a in [item.xy("start"), item.xy("end")].into_iter().flatten() {
                    add(a);
                }
            }
            "wire" | "polyline" => {
                if let Some(pts) = item.get("pts") {
                    pts.all("xy").for_each(|xy| add((xy.num(1), xy.num(2))));
                }
            }
            "sheet" => {
                if let (Some(a), Some(sz)) = (item.xy("at"), item.xy("size")) {
                    add(a);
                    add((a.0 + sz.0, a.1 + sz.1));
                }
            }
            _ => {}
        }
    }
    if b.0 <= b.2 {
        Some(b)
    } else {
        None
    }
}

/// A non-electrical drawing (sheet box or sheet frame) kept as a part with no pins.
fn add_drawing(p: &mut Project, reference: String, value: String, cu: Custom, pos: (i32, i32)) {
    p.next_id += 1;
    p.components.push(Component {
        id: p.next_id,
        kind: Kind::Generic,
        reference,
        value,
        pos,
        rot: 0,
        pcb_pos: (0.0, 0.0),
        pcb_rot: 0,
        custom: Some(Box::new(cu)),
        lcsc: String::new(),
        fp_over: None,
    });
}

/// Graphic lines, rectangles, text and text boxes of one sheet.
fn import_notes(root: &Sx, p: &mut Project, off: (f32, f32)) {
    let pt = |q: (f32, f32)| (((q.0 + off.0) * U).round() as i32, ((q.1 + off.1) * U).round() as i32);
    // A colour with zero opacity means "KiCad's default".
    let colour = |s: Option<&Sx>| {
        s.and_then(|s| s.get("color"))
            .filter(|c| c.num(4) > 0.0)
            .map(|c| NoteColor::nearest((c.num(1), c.num(2), c.num(3))))
            .unwrap_or_default()
    };
    let dashed = |s: &Sx| s.get("stroke").and_then(|s| s.get("type")).map_or(false, |t| !matches!(t.arg(1), "solid" | "default"));
    let filled = |s: &Sx| s.get("fill").and_then(|s| s.get("type")).map_or(false, |t| t.arg(1) != "none");
    for item in root.list() {
        let mut n = Note { dashed: dashed(item), filled: filled(item), color: colour(item.get("stroke")), ..Note::default() };
        match item.head() {
            "polyline" => {
                let Some(pts) = item.get("pts") else { continue };
                let v: Vec<(i32, i32)> = pts.all("xy").map(|xy| pt((xy.num(1), xy.num(2)))).collect();
                for s in v.windows(2).filter(|s| s[0] != s[1]) {
                    p.notes.push(Note { a: s[0], b: s[1], filled: false, ..n.clone() });
                }
                continue;
            }
            "rectangle" => {
                let (Some(a), Some(b)) = (item.xy("start"), item.xy("end")) else { continue };
                n.kind = NoteKind::Box;
                (n.a, n.b) = (pt(a), pt(b));
            }
            "text" | "text_box" => {
                let Some(a) = item.xy("at") else { continue };
                let effects = item.get("effects");
                let font = effects.and_then(|e| e.get("font"));
                n.text = item.arg(1).to_string();
                n.size = font.and_then(|f| f.get("size")).map(|s| (s.num(1) * U).round() as i32).unwrap_or(10).max(4);
                n.color = colour(font);
                if item.head() == "text_box" {
                    let Some(sz) = item.xy("size") else { continue };
                    n.kind = NoteKind::Box;
                    (n.a, n.b) = (pt(a), pt((a.0 + sz.0, a.1 + sz.1)));
                } else {
                    n.kind = NoteKind::Text;
                    n.a = pt(a);
                    // Notes are anchored at their top-left corner; KiCad's anchor depends on
                    // the justification (centred when none is given).
                    let j = effects.and_then(|e| e.get("justify"));
                    let has = |w: &str| j.map_or(false, |j| j.has_atom(w));
                    let (w, h) = n.text_extent();
                    let dx = if has("left") { 0.0 } else if has("right") { w } else { w / 2.0 };
                    let dy = if has("top") { 0.0 } else if has("bottom") { h } else { h / 2.0 };
                    n.a = (n.a.0 - dx.round() as i32, n.a.1 - dy.round() as i32);
                }
            }
            _ => continue,
        }
        n.normalize();
        if !n.is_degenerate() {
            p.notes.push(n);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn import_schematic(root: &Sx, p: &mut Project, notes: &mut Vec<String>, off: (f32, f32), prefix: &str, dir: &Path, depth: u32) {
    let mut libs: HashMap<String, &Sx> = HashMap::new();
    if let Some(ls) = root.get("lib_symbols") {
        for s in ls.all("symbol") {
            libs.insert(s.arg(1).to_string(), s);
        }
    }
    let to_i = |v: f32| (v * U).round() as i32;
    // Instances, grouped by reference so units of one part become one part.
    let mut groups: Vec<Vec<Inst>> = Vec::new();
    let mut by_ref: HashMap<String, usize> = HashMap::new();
    for s in root.all("symbol") {
        let lib_id = s.get("lib_id").map(|x| x.arg(1).to_string()).unwrap_or_default();
        if lib_id.is_empty() {
            continue;
        }
        let at = s.get("at").map(|a| (a.num(1) + off.0, a.num(2) + off.1, a.num(3))).unwrap_or_default();
        let mut reference = prop(s, "Reference").map(|x| x.arg(2).to_string()).unwrap_or_default();
        if reference.ends_with('?') || reference.is_empty() {
            // KiCad 6 keeps the annotated reference in the instances block.
            if let Some(r) = s
                .get("instances")
                .and_then(|i| i.all("project").next())
                .and_then(|pr| pr.get("path"))
                .and_then(|pa| pa.get("reference"))
            {
                reference = r.arg(1).to_string();
            }
        }
        let value = prop(s, "Value").map(|x| x.arg(2).to_string()).unwrap_or_default();
        if value == "PWR_FLAG" || lib_id.ends_with(":PWR_FLAG") {
            continue;
        }
        let pos_of = |name: &str| prop(s, name).filter(|x| !x.hidden()).and_then(|x| x.get("at")).map(|a| (a.num(1) + off.0, a.num(2) + off.1));
        let inst = Inst {
            lib: libs.get(&lib_id).copied(),
            lib_id: lib_id.clone(),
            at,
            mirror: s.get("mirror").map(|m| m.arg(1).to_string()).unwrap_or_default(),
            unit: s.get("unit").map(|u| u.num(1) as i32).unwrap_or(1),
            footprint: prop(s, "Footprint").map(|x| x.arg(2).to_string()).unwrap_or_default(),
            fc_kind: prop(s, "FerrisCad_Kind").and_then(|x| serde_json::from_str::<Kind>(&format!("\"{}\"", x.arg(2))).ok()),
            lcsc: ["LCSC", "LCSC Part", "LCSC Part #", "LCSC#", "JLCPCB Part #", "JLC", "JLCPCB"]
                .iter()
                .find_map(|n| prop(s, n).map(|x| x.arg(2).trim().to_string()).filter(|v| !v.is_empty()))
                .unwrap_or_default(),
            ref_at: pos_of("Reference"),
            val_at: pos_of("Value"),
            reference: reference.clone(),
            value,
        };
        let is_power = inst.lib.map_or(false, |l| l.get("power").is_some()) || lib_id.starts_with("power:") || reference.starts_with('#');
        if !is_power && !reference.is_empty() {
            if let Some(&g) = by_ref.get(&reference) {
                groups[g].push(inst);
                continue;
            }
            by_ref.insert(reference, groups.len());
        }
        groups.push(vec![inst]);
    }
    let mut missing_lib = 0;
    for g in groups {
        let first = &g[0];
        let pos = (to_i(first.at.0), to_i(first.at.1));
        let posf = (pos.0 as f32, pos.1 as f32);
        let mut cu = Custom { lib: first.lib_id.clone(), footprint: first.footprint.clone(), ..Default::default() };
        let local = |w: (f32, f32)| (w.0 * U - posf.0, w.1 * U - posf.1);
        let mut is_power = false;
        for inst in &g {
            let Some(lib) = inst.lib else {
                missing_lib += 1;
                continue;
            };
            is_power |= lib.get("power").is_some() || inst.lib_id.starts_with("power:");
            let show_names = lib.get("pin_names").map_or(true, |pn| !pn.has_atom("hide") && pn.get("hide").map_or(true, |h| h.arg(1) == "no"));
            let mut parts: Vec<&Sx> = vec![lib];
            parts.extend(lib.all("symbol").filter(|s| {
                let (u, st) = sub_unit(s.arg(1));
                (u == 0 || u == inst.unit) && st <= 1
            }));
            let bb = symbol_items(&mut cu, &parts, is_power, show_names, &|q| local(sch_xf(inst, q)));
            if bb.0 <= bb.2 {
                cu.boxes.push((bb.0.floor() as i32, bb.1.floor() as i32, bb.2.ceil() as i32, bb.3.ceil() as i32));
            }
        }
        cu.ref_pos = first.ref_at.map(local);
        cu.val_pos = first.val_at.map(local);
        let lname = first.lib_id.rsplit(':').next().unwrap_or("").to_lowercase();
        // A FerrisCad export says what the part is, with its pins already in order.
        let (kind, order) = match first.fc_kind {
            Some(k) if !k.is_power() || is_power => (k, None),
            _ => map_kind(&first.lib_id, &lname, &first.value, is_power, &cu.pins),
        };
        if let Some(order) = order {
            let base = info(kind);
            let pins = std::mem::take(&mut cu.pins);
            cu.pins = order
                .iter()
                .enumerate()
                .map(|(i, &k)| {
                    let mut pn = pins[k].clone();
                    if let Some(b) = base.pins.get(i) {
                        pn.name = b.name.clone();
                    }
                    pn
                })
                .collect();
        }
        p.next_id += 1;
        let value = if kind == Kind::Gnd { "GND".to_string() } else { first.value.clone() };
        p.components.push(Component {
            id: p.next_id,
            kind,
            reference: first.reference.clone(),
            value,
            pos,
            rot: 0,
            pcb_pos: (0.0, 0.0),
            pcb_rot: 0,
            custom: Some(Box::new(cu)),
            lcsc: first.lcsc.clone(),
            fp_over: None,
        });
    }
    for w in root.all("wire") {
        if let Some(pts) = w.get("pts") {
            let v: Vec<(i32, i32)> = pts.all("xy").map(|xy| (to_i(xy.num(1) + off.0), to_i(xy.num(2) + off.1))).collect();
            for s in v.windows(2) {
                if s[0] != s[1] {
                    p.wires.push(Wire { a: s[0], b: s[1] });
                }
            }
        }
    }
    for kind in ["label", "global_label", "hierarchical_label"] {
        for l in root.all(kind) {
            if let Some(a) = l.xy("at") {
                // Local and hierarchical labels only connect inside their own sheet.
                let text = if kind == "global_label" { l.arg(1).to_string() } else { format!("{}{}", prefix, l.arg(1)) };
                p.labels.push(Label { pos: (to_i(a.0 + off.0), to_i(a.1 + off.1)), text });
            }
        }
    }
    for nc in root.all("no_connect") {
        if let Some(a) = nc.xy("at") {
            p.no_connects.push((to_i(a.0 + off.0), to_i(a.1 + off.1)));
        }
    }
    import_notes(root, p, off);
    for sh in root.all("sheet") {
        let name = prop(sh, "Sheetname").or_else(|| prop(sh, "Sheet name")).map(|x| x.arg(2).to_string()).unwrap_or_else(|| "Sheet".into());
        let file = prop(sh, "Sheetfile").or_else(|| prop(sh, "Sheet file")).map(|x| x.arg(2).to_string()).unwrap_or_default();
        let child_prefix = format!("{}{}/", prefix, name);
        let at = sh.xy("at").unwrap_or_default();
        let size = sh.xy("size").unwrap_or((20.0, 20.0));
        // The sheet box on this page, with its pins joined to the sheet's hierarchical labels.
        let origin = (to_i(at.0 + off.0), to_i(at.1 + off.1));
        let o = (origin.0 as f32, origin.1 as f32);
        let mut cu = Custom { lib: "sheet".into(), ..Default::default() };
        let (w, h) = (size.0 * U, size.1 * U);
        cu.polys.push((vec![(0.0, 0.0), (w, 0.0), (w, h), (0.0, h), (0.0, 0.0)], false));
        cu.texts.push((w / 2.0, -6.0, format!("{} ({})", name, file)));
        cu.boxes.push((0, 0, w.ceil() as i32, h.ceil() as i32));
        for pin in sh.all("pin") {
            if let Some(pa) = pin.xy("at") {
                let q = (to_i(pa.0 + off.0), to_i(pa.1 + off.1));
                p.labels.push(Label { pos: q, text: format!("{}{}", child_prefix, pin.arg(1)) });
                let lx = q.0 as f32 - o.0;
                let inward = if lx < w / 2.0 { 1.0 } else { -1.0 };
                cu.texts.push((lx + inward * (pin.arg(1).len() as f32 * 3.0 + 4.0), q.1 as f32 - o.1, pin.arg(1).to_string()));
            }
        }
        add_drawing(p, format!("Sheet {}", child_prefix.trim_end_matches('/')), file.clone(), cu, origin);
        // The sheet's own content goes to the right of what is already loaded.
        let path = dir.join(&file);
        if depth >= 8 {
            notes.push(format!("Sheet {} is nested too deep and was skipped.", name));
            continue;
        }
        let child = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| parse_sx(&t));
        let child = match child {
            Ok(c) => c,
            Err(e) => {
                notes.push(format!("Couldn't load sheet {} ({}): {}", name, file, e));
                continue;
            }
        };
        let Some(cb) = content_bbox(&child) else { continue };
        let mut right = f32::MIN;
        let mut top = f32::MAX;
        for c in &p.components {
            for b in c.sch_boxes() {
                right = right.max(b.2 as f32);
                top = top.min(b.1 as f32);
            }
        }
        for wr in &p.wires {
            right = right.max(wr.a.0.max(wr.b.0) as f32);
        }
        for n in &p.notes {
            right = right.max(n.bbox().2 as f32);
        }
        let (right_mm, top_mm) = (right / U, if top == f32::MAX { cb.1 } else { top / U });
        let snap = |v: f32| (v / 1.27).round() * 1.27;
        let coff = (snap(right_mm + 40.0 - cb.0), snap(top_mm - cb.1));
        // Frame and title around the sheet's content.
        let fo = (to_i(cb.0 + coff.0 - 8.0), to_i(cb.1 + coff.1 - 12.0));
        let (fw, fh) = ((cb.2 - cb.0 + 16.0) * U, (cb.3 - cb.1 + 20.0) * U);
        let mut frame = Custom { lib: "sheet frame".into(), ..Default::default() };
        frame.polys.push((vec![(0.0, 0.0), (fw, 0.0), (fw, fh), (0.0, fh), (0.0, 0.0)], false));
        frame.texts.push((60.0, 10.0, format!("Sheet: {}", child_prefix.trim_end_matches('/'))));
        frame.boxes.push((0, 0, 120, 20));
        add_drawing(p, format!("Page {}", child_prefix.trim_end_matches('/')), file.clone(), frame, fo);
        import_schematic(&child, p, notes, coff, &child_prefix, dir, depth + 1);
        notes.push(format!("Sheet \"{}\" ({}) was loaded to the right of the main sheet.", name, file));
    }
    if missing_lib > 0 {
        notes.push(format!("{} symbol(s) had no drawing in the file and were skipped.", missing_lib));
    }
    if root.all("bus").count() > 0 {
        notes.push("Buses were not imported (single wires and labels were).".into());
    }
}

fn by_names(pins: &[CPin], names: &[&str]) -> Option<Vec<usize>> {
    names.iter().map(|n| pins.iter().position(|p| p.name.eq_ignore_ascii_case(n))).collect()
}

fn by_numbers(pins: &[CPin], nums: &[&str]) -> Option<Vec<usize>> {
    if pins.len() != nums.len() {
        return None;
    }
    nums.iter().map(|n| pins.iter().position(|p| p.number == *n)).collect()
}

/// Recognise common KiCad library parts so the checks and the simulator understand them.
pub fn map_kind(lib_id: &str, name: &str, value: &str, is_power: bool, pins: &[CPin]) -> (Kind, Option<Vec<usize>>) {
    if is_power {
        let k = if value.eq_ignore_ascii_case("GND") { Kind::Gnd } else { Kind::Power };
        return (k, None);
    }
    let lib = lib_id.split(':').next().unwrap_or("").to_lowercase();
    let v = value.to_lowercase();
    let two = |k: Kind| -> (Kind, Option<Vec<usize>>) {
        match by_numbers(pins, &["1", "2"]) {
            Some(o) => (k, Some(o)),
            None => (Kind::Generic, None),
        }
    };
    let first = name.split('_').next().unwrap_or("");
    if lib == "device" || lib == "led" || lib == "diode" || lib == "switch" || lib == "battery" {
        match first {
            "r" if !name.contains("pack") && !name.contains("network") => return two(Kind::Resistor),
            "c" if name.contains("polarized") => return two(Kind::CapPolarized),
            "c" => return two(Kind::Capacitor),
            "cp" => return two(Kind::CapPolarized),
            "l" => return two(Kind::Inductor),
            "battery" => return two(Kind::Battery),
            _ => {}
        }
        if name.starts_with("led") || lib == "led" {
            if let Some(o) = by_names(pins, &["A", "K"]) {
                if pins.len() == 2 {
                    return (Kind::Led, Some(o));
                }
            }
        }
        if (first == "d" || lib == "diode") && pins.len() == 2 {
            if let Some(o) = by_names(pins, &["A", "K"]) {
                return (Kind::Diode, Some(o));
            }
        }
        if (name.starts_with("sw_") || lib == "switch") && pins.len() == 2 {
            return two(Kind::Switch);
        }
    }
    if pins.len() == 3 {
        if let Some(o) = by_names(pins, &["B", "C", "E"]) {
            const PNP: [&str; 13] = ["pnp", "3906", "557", "558", "559", "327", "328", "bd136", "bd138", "bd140", "tip42", "tip32", "2907"];
            let pnp = PNP.iter().any(|k| name.contains(k) || v.contains(k));
            return if pnp { (Kind::Pnp, by_names(pins, &["B", "E", "C"])) } else { (Kind::Npn, Some(o)) };
        }
        if let Some(o) = by_names(pins, &["G", "D", "S"]) {
            let pch = name.contains("pmos") || name.contains("p-ch") || v.contains("pmos");
            if !pch {
                return (Kind::Nmos, Some(o));
            }
        }
        let reg = by_names(pins, &["IN", "OUT", "GND"]).or_else(|| by_names(pins, &["VI", "VO", "GND"]));
        if let Some(o) = reg {
            return (Kind::Regulator, Some(o));
        }
    }
    if pins.len() == 5 {
        if let Some(o) = by_names(pins, &["-", "+", "~", "V+", "V-"]).or_else(|| by_names(pins, &["-", "+", "OUT", "V+", "V-"])) {
            return (Kind::OpAmp, Some(o));
        }
    }
    (Kind::Generic, None)
}

// ---------------------------------------------------------------- board

/// Add a library symbol's drawing and pins to `cu`; `xf` maps library millimetres to local
/// schematic units. Returns the bounding box of what was added.
pub fn symbol_items(cu: &mut Custom, parts: &[&Sx], is_power: bool, show_names: bool, xf: &dyn Fn((f32, f32)) -> (f32, f32)) -> (f32, f32, f32, f32) {
    let mut bb = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut grow = |q: (f32, f32)| bb = (bb.0.min(q.0), bb.1.min(q.1), bb.2.max(q.0), bb.3.max(q.1));
    for part in parts {
        for item in part.list() {
            let fill = item.get("fill").map_or(false, |f| f.get("type").map_or(false, |t| t.arg(1) == "background" || t.arg(1) == "outline"));
            match item.head() {
                "rectangle" => {
                    let (a, b) = (item.xy("start").unwrap_or_default(), item.xy("end").unwrap_or_default());
                    let pts: Vec<(f32, f32)> = [(a.0, a.1), (b.0, a.1), (b.0, b.1), (a.0, b.1), (a.0, a.1)].iter().map(|&q| xf(q)).collect();
                    pts.iter().for_each(|&q| grow(q));
                    cu.polys.push((pts, fill));
                }
                "polyline" => {
                    let pts: Vec<(f32, f32)> =
                        item.get("pts").map(|ps| ps.all("xy").map(|xy| xf((xy.num(1), xy.num(2)))).collect()).unwrap_or_default();
                    pts.iter().for_each(|&q| grow(q));
                    if pts.len() >= 2 {
                        cu.polys.push((pts, fill));
                    }
                }
                "circle" => {
                    let c = xf(item.xy("center").unwrap_or_default());
                    let r = item.get("radius").map_or(1.0, |r| r.num(1)) * U;
                    grow((c.0 - r, c.1 - r));
                    grow((c.0 + r, c.1 + r));
                    cu.circles.push((c.0, c.1, r, fill));
                }
                "arc" => {
                    let pa = item.xy("start").unwrap_or_default();
                    let pm = item.xy("mid").unwrap_or(pa);
                    let pb = item.xy("end").unwrap_or_default();
                    let pts = arc_pts(xf(pa), xf(pm), xf(pb));
                    pts.iter().for_each(|&q| grow(q));
                    cu.polys.push((pts, false));
                }
                "pin" => {
                    let at = item.get("at").map(|a| (a.num(1), a.num(2), a.num(3))).unwrap_or_default();
                    let len = item.get("length").map_or(2.54, |l| l.num(1));
                    let r = at.2.to_radians();
                    let tip = xf((at.0, at.1));
                    let end = xf((at.0 + len * r.cos(), at.1 + len * r.sin()));
                    grow(tip);
                    grow(end);
                    // KiCad writes inverted signals as ~{RST}; show them as /RST.
                    let name = item.get("name").map(|n| n.arg(1).replace("~{", "/").replace('}', "")).unwrap_or_default();
                    let number = item.get("number").map(|n| n.arg(1).to_string()).unwrap_or_default();
                    if !item.has_atom("hide") || !is_power {
                        cu.polys.push((vec![tip, end], false));
                    }
                    if show_names && !is_power && name != "~" && !name.is_empty() {
                        let (dx, dy) = (end.0 - tip.0, end.1 - tip.1);
                        let l = (dx * dx + dy * dy).sqrt().max(0.01);
                        cu.texts.push((end.0 + dx / l * 7.0, end.1 + dy / l * 7.0, name.clone()));
                    }
                    cu.pins.push(CPin {
                        name: if name == "~" || name.is_empty() { number.clone() } else { name },
                        number,
                        pos: (tip.0.round() as i32, tip.1.round() as i32),
                        ptype: pin_type(item.arg(1)),
                    });
                }
                _ => {}
            }
        }
    }
    bb
}

pub type FpGeometry = (Vec<CPad>, Vec<(String, String)>, Vec<((f32, f32), (f32, f32))>, Vec<(f32, f32, f32)>);

/// Pads (rotated by the footprint angle), pad nets and silkscreen of a footprint.
pub fn footprint_geometry(fp: &Sx, angle: f32, pad_net: &dyn Fn(&Sx) -> String) -> FpGeometry {
    let mut pads = Vec::new();
    let mut nets: Vec<(String, String)> = Vec::new();
    for pad in fp.all("pad") {
        let kind = pad.arg(2);
        if kind == "np_thru_hole" {
            continue;
        }
        let shape = pad.arg(3);
        let pa = pad.get("at").map(|a| (a.num(1), a.num(2), a.list().get(3).map(|x| x.atom().parse::<f32>().unwrap_or(0.0)))).unwrap_or((0.0, 0.0, None));
        let mut size = pad.get("size").map(|s| (s.num(1), s.num(2))).unwrap_or((1.0, 1.0));
        let total = pa.2.unwrap_or(angle);
        if ((total / 90.0).round() as i64).rem_euclid(2) == 1 {
            size = (size.1, size.0);
        }
        let layers: Vec<String> = pad.get("layers").map(|l| l.list().iter().skip(1).map(|x| x.atom().to_string()).collect()).unwrap_or_default();
        let tht = kind == "thru_hole";
        let back = !tht && layers.iter().any(|l| l == "B.Cu") && !layers.iter().any(|l| l == "F.Cu");
        let number = pad.arg(1).to_string();
        if number.is_empty() {
            continue;
        }
        pads.push(CPad {
            drill: pad.get("drill").map(|d| d.list().iter().skip(1).find_map(|x| x.atom().parse::<f32>().ok()).unwrap_or(0.0)).unwrap_or(0.0),
            number: number.clone(),
            pos: rot_ccw((pa.0, pa.1), angle),
            size,
            round: shape == "circle" || (shape == "oval" && (size.0 - size.1).abs() < 1e-3),
            tht,
            back,
        });
        nets.push((number, pad_net(pad)));
    }
    let mut silk = Vec::new();
    let mut silk_circles = Vec::new();
    let on_silk = |g: &Sx| g.get("layer").map_or(false, |l| l.arg(1).ends_with("SilkS") || l.arg(1).ends_with("Silkscreen"));
    let xf = |q: (f32, f32)| rot_ccw(q, angle);
    for g in fp.list() {
        if !on_silk(g) {
            continue;
        }
        match g.head() {
            "fp_line" => {
                if let (Some(a), Some(b)) = (g.xy("start"), g.xy("end")) {
                    silk.push((xf(a), xf(b)));
                }
            }
            "fp_rect" => {
                if let (Some(a), Some(b)) = (g.xy("start"), g.xy("end")) {
                    let c = [(a.0, a.1), (b.0, a.1), (b.0, b.1), (a.0, b.1)];
                    for i in 0..4 {
                        silk.push((xf(c[i]), xf(c[(i + 1) % 4])));
                    }
                }
            }
            "fp_circle" => {
                if let (Some(c), Some(e)) = (g.xy("center"), g.xy("end")) {
                    let r = ((e.0 - c.0).powi(2) + (e.1 - c.1).powi(2)).sqrt();
                    let cc = xf(c);
                    silk_circles.push((cc.0, cc.1, r));
                }
            }
            "fp_arc" => {
                if let (Some(a), Some(m), Some(b)) = (g.xy("start"), g.xy("mid"), g.xy("end")) {
                    let pts = arc_pts(xf(a), xf(m), xf(b));
                    for w in pts.windows(2) {
                        silk.push((w[0], w[1]));
                    }
                }
            }
            _ => {}
        }
    }
    (pads, nets, silk, silk_circles)
}

fn import_board(root: &Sx, p: &mut Project, notes: &mut Vec<String>, pcb_only: bool) {
    let mut net_names: HashMap<String, String> = HashMap::new();
    for n in root.all("net") {
        net_names.insert(n.arg(1).to_string(), n.arg(2).to_string());
    }
    let pad_net = |pad: &Sx| -> String {
        pad.get("net")
            .map(|n| {
                if !n.arg(2).is_empty() {
                    n.arg(2).to_string()
                } else {
                    net_names.get(n.arg(1)).cloned().unwrap_or_else(|| n.arg(1).to_string())
                }
            })
            .unwrap_or_default()
    };
    let mut row = 0;
    let mut not_in_sch = 0;
    let mut fps = 0;
    for fp in root.all("footprint").chain(root.all("module")) {
        fps += 1;
        let at = fp.get("at").map(|a| (a.num(1), a.num(2), a.num(3))).unwrap_or_default();
        let reference = prop(fp, "Reference")
            .map(|x| x.arg(2).to_string())
            .or_else(|| fp.all("fp_text").find(|t| t.arg(1) == "reference").map(|t| t.arg(2).to_string()))
            .unwrap_or_default();
        let value = prop(fp, "Value")
            .map(|x| x.arg(2).to_string())
            .or_else(|| fp.all("fp_text").find(|t| t.arg(1) == "value").map(|t| t.arg(2).to_string()))
            .unwrap_or_default();
        let (pads, nets, silk, silk_circles) = footprint_geometry(fp, at.2, &pad_net);
        let fp_name = fp.arg(1).to_string();
        let target = p.components.iter_mut().find(|c| !c.kind.is_power() && c.reference == reference);
        match target {
            Some(c) => {
                c.pcb_pos = (at.0, at.1);
                c.pcb_rot = 0;
                let cu = c.custom.get_or_insert_with(Default::default);
                cu.pads = pads;
                cu.silk = silk;
                cu.silk_circles = silk_circles;
                if cu.footprint.is_empty() {
                    cu.footprint = fp_name;
                }
            }
            None => {
                if pads.is_empty() {
                    continue;
                }
                not_in_sch += 1;
                // Board-only part: draw a simple box symbol, label each pin with its net.
                let n = pads.len() as i32;
                let pos = (200 + (row % 6) * 200, 200 + (row / 6) * (n * 10 + 80).max(120));
                row += 1;
                let mut cu = Custom { lib: "board only".into(), footprint: fp_name, pads, silk, silk_circles, ..Default::default() };
                cu.polys.push((vec![(-20.0, -10.0), (20.0, -10.0), (20.0, (n * 10) as f32), (-20.0, (n * 10) as f32)], true));
                cu.boxes.push((-40, -10, 20, n * 10));
                for (i, (num, net)) in nets.iter().enumerate() {
                    let y = i as i32 * 10;
                    cu.polys.push((vec![(-40.0, y as f32), (-20.0, y as f32)], false));
                    cu.texts.push((-12.0, y as f32, num.clone()));
                    cu.pins.push(CPin { name: num.clone(), number: num.clone(), pos: (-40, y), ptype: PinType::Passive });
                    if !net.is_empty() && !net.starts_with("unconnected-") {
                        p.labels.push(Label { pos: (pos.0 - 40, pos.1 + y), text: net.clone() });
                    }
                }
                cu.ref_pos = Some((-20.0, -16.0));
                cu.val_pos = Some((-20.0, (n * 10 + 8) as f32));
                p.next_id += 1;
                p.components.push(Component {
                    id: p.next_id,
                    kind: Kind::Generic,
                    reference: if reference.is_empty() { format!("FP{}", row) } else { reference },
                    value,
                    pos,
                    rot: 0,
                    pcb_pos: (at.0, at.1),
                    pcb_rot: 0,
                    custom: Some(Box::new(cu)),
                    lcsc: String::new(),
                    fp_over: None,
                });
            }
        }
    }
    // Copper layer count from the board's layer table (F.Cu, In1.Cu … B.Cu).
    if let Some(ls) = root.get("layers") {
        let inner = ls.list().iter().skip(1).filter(|l| {
            let n = l.arg(1);
            n.starts_with("In") && n.ends_with(".Cu")
        }).count();
        p.board.layers = (2 + inner as u8).max(2);
    }
    let layer_of = |s: &Sx| -> Option<Layer> {
        let name = s.get("layer").map(|l| l.arg(1)).unwrap_or("");
        if name.ends_with(".Cu") {
            Layer::parse(name)
        } else {
            None
        }
    };
    for s in root.all("segment") {
        let (Some(a), Some(b), Some(l)) = (s.xy("start"), s.xy("end"), layer_of(s)) else { continue };
        p.tracks.push(Track { a, b, width: s.get("width").map_or(0.25, |w| w.num(1)), layer: l });
    }
    for s in root.all("arc") {
        let (Some(a), Some(m), Some(b), Some(l)) = (s.xy("start"), s.xy("mid"), s.xy("end"), layer_of(s)) else { continue };
        let w = s.get("width").map_or(0.25, |w| w.num(1));
        for q in arc_pts(a, m, b).windows(2) {
            p.tracks.push(Track { a: q[0], b: q[1], width: w, layer: l });
        }
    }
    for v in root.all("via") {
        if let Some(a) = v.xy("at") {
            p.vias.push(Via { pos: a, size: v.get("size").map_or(0.8, |s| s.num(1)) });
        }
    }
    // Free text on the board (silkscreen or copper).
    for g in root.all("gr_text") {
        let layer_name = g.get("layer").map(|l| l.arg(1)).unwrap_or("");
        let layer = match layer_name {
            "F.SilkS" | "F.Silkscreen" => Some(TextLayer::SilkFront),
            "B.SilkS" | "B.Silkscreen" => Some(TextLayer::SilkBack),
            n if n.ends_with(".Cu") => Layer::parse(n).map(TextLayer::Copper),
            _ => None,
        };
        let (Some(layer), Some(at)) = (layer, g.get("at")) else { continue };
        let size = g
            .get("effects")
            .and_then(|e| e.get("font"))
            .and_then(|f| f.get("size"))
            .map(|s| s.num(1))
            .filter(|s| *s > 0.0)
            .unwrap_or(1.0);
        let angle = at.num(3);
        let rot = (((360.0 - angle) / 90.0).round() as i32).rem_euclid(4) as u8;
        let text = g.arg(1).replace("\\n", " ");
        if !text.trim().is_empty() {
            p.texts.push(BoardText { text, pos: (at.num(1), at.num(2)), size, rot, layer });
        }
    }
    // Board outline from the Edge.Cuts drawings.
    let mut bb = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut grow = |q: (f32, f32)| bb = (bb.0.min(q.0), bb.1.min(q.1), bb.2.max(q.0), bb.3.max(q.1));
    for g in root.list() {
        if g.get("layer").map_or(true, |l| l.arg(1) != "Edge.Cuts") {
            continue;
        }
        match g.head() {
            "gr_circle" => {
                if let (Some(c), Some(e)) = (g.xy("center"), g.xy("end")) {
                    let r = ((e.0 - c.0).powi(2) + (e.1 - c.1).powi(2)).sqrt();
                    grow((c.0 - r, c.1 - r));
                    grow((c.0 + r, c.1 + r));
                }
            }
            "gr_poly" => {
                if let Some(pts) = g.get("pts") {
                    for xy in pts.all("xy") {
                        grow((xy.num(1), xy.num(2)));
                    }
                }
            }
            _ => {
                for k in ["start", "mid", "end"] {
                    if let Some(q) = g.xy(k) {
                        grow(q);
                    }
                }
            }
        }
    }
    if bb.0 < bb.2 {
        p.board = Board { x0: bb.0, y0: bb.1, x1: bb.2, y1: bb.3, ..p.board.clone() };
    } else if let Some(fb) = p
        .components
        .iter()
        .filter(|c| c.custom.as_ref().map_or(false, |cu| !cu.pads.is_empty()))
        .map(|c| c.pcb_bbox())
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
    {
        p.board = Board { x0: fb.0 - 3.0, y0: fb.1 - 3.0, x1: fb.2 + 3.0, y1: fb.3 + 3.0, ..p.board.clone() };
        notes.push("The board has no outline (Edge.Cuts), so one was drawn around the parts.".into());
    }
    let mut filled = 0;
    let mut unfilled = 0;
    for z in root.all("zone") {
        let net = z.get("net_name").map(|n| n.arg(1).to_string()).unwrap_or_default();
        let mut any = false;
        let outline: Vec<(f32, f32)> = z.get("polygon").and_then(|pg| pg.get("pts")).map(|ps| ps.all("xy").map(|xy| (xy.num(1), xy.num(2))).collect()).unwrap_or_default();
        for fp in z.all("filled_polygon") {
            let layer = Layer::parse(fp.get("layer").map(|l| l.arg(1)).unwrap_or(""));
            let pts: Vec<(f32, f32)> = fp.get("pts").map(|ps| ps.all("xy").map(|xy| (xy.num(1), xy.num(2))).collect()).unwrap_or_default();
            if pts.len() >= 3 {
                any = true;
                match p.zones.iter_mut().find(|q| q.layer == layer && q.net == net) {
                    Some(q) => {
                        q.polys.push(pts);
                        if outline.len() >= 3 && !q.outlines.contains(&outline) {
                            q.outlines.push(outline.clone());
                        }
                    }
                    None => p.zones.push(Zone { layer, net: net.clone(), polys: vec![pts], outlines: if outline.len() >= 3 { vec![outline.clone()] } else { vec![] }, drawn: false }),
                }
            }
        }
        let is_keepout = z.get("keepout").is_some();
        let layer = z.get("layer").and_then(|l| Layer::parse(l.arg(1)));
        if any {
            filled += 1;
        } else if !is_keepout && !net.is_empty() && outline.len() >= 3 && layer.is_some() {
            // Zone never filled in KiCad: keep it so it can be seen, edited and exported again.
            let mut nz = Zone::new(layer.unwrap(), &net, outline.clone());
            nz.drawn = true;
            p.zones.push(nz);
            filled += 1;
        } else if !net.is_empty() {
            unfilled += 1;
        }
    }
    if filled > 0 {
        notes.push(format!("{} copper zone(s) imported: pads and vias inside them count as connected.", filled));
    }
    if unfilled > 0 {
        notes.push(format!("{} copper zone(s) had never been filled in KiCad, so they connect nothing here (refill them in KiCad and save).", unfilled));
    }
    if p.board.layers > 2 {
        notes.push(format!("{}-layer board: inner layers In1.Cu to In{}.Cu were imported.", p.board.layers, p.board.layers - 2));
    }
    // Make sure every imported track fits the layer count.
    let deepest = p.tracks.iter().filter_map(|t| if let Layer::Inner(n) = t.layer { Some(n) } else { None }).max().unwrap_or(0);
    if deepest as u16 + 2 > p.board.layers as u16 {
        p.board.layers = deepest + 2;
    }
    if pcb_only && not_in_sch > 0 {
        notes.push("No schematic was found: each part got a simple box symbol, with its pins labelled by the board's net names.".into());
    } else if not_in_sch > 0 {
        notes.push(format!("{} footprint(s) on the board weren't in the schematic; they were added as simple parts.", not_in_sch));
    }
    let _ = fps;
}

/// A tiny KiCad project used by the self-test.
pub const SAMPLE_SCH: &str = r##"(kicad_sch (version 20231120) (generator "eeschema")
  (lib_symbols
    (symbol "Device:R" (pin_numbers hide) (pin_names (offset 0)) (in_bom yes) (on_board yes)
      (property "Reference" "R" (at 2.032 0 90))
      (symbol "R_0_1" (rectangle (start -1.016 -2.54) (end 1.016 2.54) (stroke (width 0.254) (type default)) (fill (type none))))
      (symbol "R_1_1"
        (pin passive line (at 0 3.81 270) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
        (pin passive line (at 0 -3.81 90) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))))
    (symbol "Device:LED" (pin_numbers hide) (pin_names (offset 1.016) hide) (in_bom yes) (on_board yes)
      (symbol "LED_0_1" (polyline (pts (xy -1.27 -1.27) (xy -1.27 1.27)) (stroke (width 0.254) (type default)) (fill (type none)))
        (polyline (pts (xy 1.27 -1.27) (xy 1.27 1.27) (xy -1.27 0) (xy 1.27 -1.27)) (stroke (width 0.254) (type default)) (fill (type none))))
      (symbol "LED_1_1"
        (pin passive line (at -3.81 0 0) (length 2.54) (name "K" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
        (pin passive line (at 3.81 0 180) (length 2.54) (name "A" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))))
    (symbol "power:GND" (power) (pin_names (offset 0)) (in_bom yes) (on_board yes)
      (property "Reference" "#PWR" (at 0 -6.35 0) (effects (font (size 1.27 1.27)) hide))
      (property "Value" "GND" (at 0 -3.81 0))
      (symbol "GND_0_1" (polyline (pts (xy 0 0) (xy 0 -1.27) (xy 1.27 -1.27) (xy 0 -2.54) (xy -1.27 -1.27) (xy 0 -1.27)) (stroke (width 0) (type default)) (fill (type none))))
      (symbol "GND_1_1" (pin power_in line (at 0 0 270) (length 0) hide (name "GND" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))))
    (symbol "power:+5V" (power) (pin_names (offset 0)) (in_bom yes) (on_board yes)
      (property "Value" "+5V" (at 0 3.556 0))
      (symbol "+5V_0_1" (polyline (pts (xy -0.762 1.27) (xy 0 2.54)) (stroke (width 0) (type default)) (fill (type none))) (polyline (pts (xy 0 0) (xy 0 2.54)) (stroke (width 0) (type default)) (fill (type none))))
      (symbol "+5V_1_1" (pin power_in line (at 0 0 90) (length 0) hide (name "+5V" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))))
  (wire (pts (xy 100.33 87.63) (xy 100.33 91.44)))
  (wire (pts (xy 100.33 99.06) (xy 100.33 102.87)))
  (label "LED_A" (at 100.33 88.9 0))
  (symbol (lib_id "power:+5V") (at 100.33 80.01 0) (unit 1) (property "Reference" "#PWR01" (at 0 0 0) (effects hide)) (property "Value" "+5V" (at 100.33 76.2 0)))
  (symbol (lib_id "Device:R") (at 100.33 83.82 0) (unit 1) (property "Reference" "R1" (at 102.87 82.55 0)) (property "Value" "47" (at 102.87 85.09 0)) (property "Footprint" "Resistor_SMD:R_0805_2012Metric" (at 0 0 0) (effects hide)))
  (symbol (lib_id "Device:LED") (at 100.33 95.25 90) (unit 1) (property "Reference" "D1" (at 104.14 95.25 0)) (property "Value" "red" (at 104.14 97.79 0)))
  (symbol (lib_id "power:GND") (at 100.33 102.87 0) (unit 1) (property "Reference" "#PWR02" (at 0 0 0) (effects hide)) (property "Value" "GND" (at 100.33 106.68 0)))
)"##;

pub const SAMPLE_PCB: &str = r##"(kicad_pcb (version 20221018) (generator pcbnew)
  (net 0 "") (net 1 "+5V") (net 2 "LED_A") (net 3 "GND")
  (footprint "Resistor_SMD:R_0805_2012Metric" (layer "F.Cu") (at 110 100 90)
    (fp_text reference "R1" (at 0 -1.65 90) (layer "F.SilkS"))
    (fp_line (start -0.2 -0.7) (end 0.2 -0.7) (layer "F.SilkS") (stroke (width 0.12) (type solid)))
    (pad "1" smd roundrect (at -0.9125 0 90) (size 1.025 1.4) (layers "F.Cu" "F.Paste" "F.Mask") (net 1 "+5V"))
    (pad "2" smd roundrect (at 0.9125 0 90) (size 1.025 1.4) (layers "F.Cu" "F.Paste" "F.Mask") (net 2 "LED_A")))
  (footprint "LED_THT:LED_D5.0mm" (layer "F.Cu") (at 118 100)
    (property "Reference" "D1" (at 1.27 -3.96 0) (layer "F.SilkS"))
    (fp_circle (center 1.27 0) (end 3.77 0) (layer "F.SilkS") (stroke (width 0.12) (type solid)))
    (pad "1" thru_hole rect (at 0 0) (size 1.8 1.8) (drill 0.9) (layers "*.Cu" "*.Mask") (net 3 "GND"))
    (pad "2" thru_hole circle (at 2.54 0) (size 1.8 1.8) (drill 0.9) (layers "*.Cu" "*.Mask") (net 2 "LED_A")))
  (gr_rect (start 100 90) (end 130 110) (layer "Edge.Cuts") (stroke (width 0.1) (type default)))
  (segment (start 110 99.0875) (end 110 97) (width 0.25) (layer "F.Cu") (net 2))
  (segment (start 110 97) (end 120.54 97) (width 0.25) (layer "F.Cu") (net 2))
  (segment (start 120.54 97) (end 120.54 100) (width 0.25) (layer "F.Cu") (net 2))
)"##;
