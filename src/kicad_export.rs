//! Export to a KiCad project (KiCad 8 file format, also opened by KiCad 9):
//! `<name>.kicad_pro`, `<name>.kicad_sch` and `<name>.kicad_pcb`.
//!
//! Every part gets its own symbol in the schematic's embedded library, drawn exactly as in
//! FerrisCad; footprints are written inline in the board with their pads and silkscreen.

use crate::checks::PcbConn;
use crate::model::*;
use crate::sch::{record_builtin, Prim};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Schematic units (grid 10 = 1.27 mm) to millimetres.
const MM: f32 = 0.127;

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn f(v: f32) -> String {
    let v = if v.abs() < 5e-5 { 0.0 } else { v };
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// Unique ids (random enough for KiCad's needs).
struct Ids(u64);

impl Ids {
    fn new() -> Self {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7);
        Ids(t ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next(&mut self) -> String {
        let mut r = || {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        };
        let (a, b) = (r(), r());
        format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            (a >> 32) as u32,
            (a >> 16) as u16,
            (a & 0xfff) as u16,
            ((b >> 48) as u16 & 0x3fff) | 0x8000,
            b & 0xffff_ffff_ffff
        )
    }
}

fn pin_kind(t: PinType) -> &'static str {
    match t {
        PinType::Input => "input",
        PinType::Output => "output",
        PinType::Bidir => "bidirectional",
        PinType::PowerIn => "power_in",
        PinType::PowerOut => "power_out",
        PinType::NotConnected => "no_connect",
        PinType::Passive => "passive",
    }
}

fn font(size: f32) -> String {
    format!("(effects (font (size {} {})))", f(size), f(size))
}

fn font_hidden(size: f32) -> String {
    format!("(effects (font (size {} {})) hide)", f(size), f(size))
}

/// The drawing of one part, in its own coordinates (schematic units, y down).
fn part_drawing(c: &Component) -> Vec<Prim> {
    match &c.custom {
        Some(cu) => {
            let mut v: Vec<Prim> = cu.polys.iter().map(|(pts, fill)| Prim::Line(pts.clone(), *fill)).collect();
            v.extend(cu.circles.iter().map(|&(x, y, r, fill)| Prim::Circle(x, y, r, fill)));
            if !c.kind.is_power() {
                v.extend(cu.texts.iter().map(|(x, y, t)| Prim::Text(*x, *y, t.clone())));
            }
            v
        }
        None => {
            let mut v = record_builtin(c.kind, &c.value);
            if c.kind.is_power() {
                // KiCad shows the power symbol's value itself.
                v.retain(|p| !matches!(p, Prim::Text(..)));
            }
            v
        }
    }
}

/// Library symbol for one part. Returns (symbol text, pin numbers).
fn lib_symbol(name: &str, c: &Component) -> (String, Vec<String>) {
    let inf = c.info();
    let power = c.kind.is_power();
    let mut s = String::new();
    let _ = write!(s, "\t\t(symbol {}", q(&format!("FerrisCad:{}", name)));
    if power {
        s.push_str(" (power)");
    }
    let few_pins = inf.pins.len() <= 2;
    if few_pins || power {
        s.push_str(" (pin_numbers hide) (pin_names (offset 0) hide)");
    } else {
        s.push_str(" (pin_names (offset 0.508))");
    }
    s.push_str(" (exclude_from_sim no) (in_bom yes) (on_board yes)\n");
    let prefix = if power { "#PWR".to_string() } else { inf.prefix.to_string() };
    let _ = writeln!(s, "\t\t\t(property \"Reference\" {} (at 0 0 0) {})", q(&prefix), font_hidden(1.27));
    let _ = writeln!(s, "\t\t\t(property \"Value\" {} (at 0 0 0) {})", q(&c.value), if power { font(1.27) } else { font_hidden(1.27) });
    let _ = writeln!(s, "\t\t\t(property \"Footprint\" \"\" (at 0 0 0) {})", font_hidden(1.27));
    let _ = writeln!(s, "\t\t\t(property \"Datasheet\" \"\" (at 0 0 0) {})", font_hidden(1.27));
    // Drawing.
    let _ = writeln!(s, "\t\t\t(symbol {}", q(&format!("{}_0_1", name)));
    let p = |x: f32, y: f32| format!("(xy {} {})", f(x * MM), f(-y * MM));
    let mut bb = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for prim in part_drawing(c) {
        match prim {
            Prim::Line(pts, fill) => {
                if pts.len() < 2 {
                    continue;
                }
                for &(x, y) in &pts {
                    bb = (bb.0.min(x), bb.1.min(y), bb.2.max(x), bb.3.max(y));
                }
                let list: Vec<String> = pts.iter().map(|&(x, y)| p(x, y)).collect();
                let _ = writeln!(
                    s,
                    "\t\t\t\t(polyline (pts {}) (stroke (width 0.254) (type default)) (fill (type {})))",
                    list.join(" "),
                    if fill { "background" } else { "none" }
                );
            }
            Prim::Circle(x, y, r, fill) => {
                bb = (bb.0.min(x - r), bb.1.min(y - r), bb.2.max(x + r), bb.3.max(y + r));
                let _ = writeln!(
                    s,
                    "\t\t\t\t(circle (center {} {}) (radius {}) (stroke (width 0.254) (type default)) (fill (type {})))",
                    f(x * MM),
                    f(-y * MM),
                    f(r * MM),
                    if fill { "background" } else { "none" }
                );
            }
            Prim::Text(x, y, t) => {
                if t.trim().is_empty() {
                    continue;
                }
                let _ = writeln!(s, "\t\t\t\t(text {} (at {} {} 0) {})", q(&t), f(x * MM), f(-y * MM), font(1.0));
            }
        }
    }
    s.push_str("\t\t\t)\n");
    // Pins (length 0: their leads are part of the drawing).
    let _ = writeln!(s, "\t\t\t(symbol {}", q(&format!("{}_1_1", name)));
    let center = if bb.0 <= bb.2 { ((bb.0 + bb.2) / 2.0, (bb.1 + bb.3) / 2.0) } else { (0.0, 0.0) };
    let mut numbers = vec![];
    for (i, pd) in inf.pins.iter().enumerate() {
        let number = match &c.custom {
            Some(cu) => cu.pins.get(i).map(|cp| cp.number.clone()).unwrap_or_else(|| (i + 1).to_string()),
            None if !pd.pad.is_empty() => pd.pad.clone(),
            None => (i + 1).to_string(),
        };
        let number = if number.is_empty() { (i + 1).to_string() } else { number };
        let (x, y) = (pd.pos.0 as f32, pd.pos.1 as f32);
        // Point the pin towards the body (KiCad angles are counter-clockwise, y up).
        let (dx, dy) = (center.0 - x, center.1 - y);
        let angle = if dx.abs() >= dy.abs() {
            if dx >= 0.0 {
                0
            } else {
                180
            }
        } else if dy >= 0.0 {
            270
        } else {
            90
        };
        let (kind, hide) = if power { ("power_in", " hide") } else { (pin_kind(pd.ptype), "") };
        let name_txt = if power { c.value.clone() } else { pd.name.clone() };
        let _ = writeln!(
            s,
            "\t\t\t\t(pin {} line (at {} {} {}) (length 0){} (name {} {}) (number {} {}))",
            kind,
            f(x * MM),
            f(-y * MM),
            angle,
            hide,
            q(&name_txt),
            font(1.27),
            q(&number),
            font(1.27)
        );
        numbers.push(number);
    }
    s.push_str("\t\t\t)\n\t\t)\n");
    (s, numbers)
}

/// Silkscreen and pads of one footprint, in its own coordinates. `nets` gives the netlist, the
/// part index and the net numbering when writing a board, and is `None` for a library footprint.
fn fp_body(c: &Component, angle: i32, nets: Option<(&Netlist, usize, &dyn Fn(Option<usize>) -> (usize, String))>, ids: &mut Ids) -> String {
    let inf = c.info();
    let mut out = String::new();
    // Silkscreen, in the footprint's own coordinates.
    let line = |a: (f32, f32), b2: (f32, f32), ids: &mut Ids| {
        format!(
            "\t\t(fp_line (start {} {}) (end {} {}) (stroke (width 0.12) (type solid)) (layer \"F.SilkS\") (uuid {}))\n",
            f(a.0),
            f(a.1),
            f(b2.0),
            f(b2.1),
            q(&ids.next())
        )
    };
    match c.fp_geom() {
        Some((_, silk, circles)) => {
            for (a, b2) in silk {
                out.push_str(&line(*a, *b2, ids));
            }
            for &(x, y, r) in circles {
                let _ = writeln!(
                    out,
                    "\t\t(fp_circle (center {} {}) (end {} {}) (stroke (width 0.12) (type solid)) (fill none) (layer \"F.SilkS\") (uuid {}))",
                    f(x),
                    f(y),
                    f(x + r),
                    f(y),
                    q(&ids.next())
                );
            }
        }
        None => {
            let (x0, y0, x1, y1) = inf.fp_body;
            if inf.fp_round {
                let _ = writeln!(
                    out,
                    "\t\t(fp_circle (center 0 0) (end {} 0) (stroke (width 0.12) (type solid)) (fill none) (layer \"F.SilkS\") (uuid {}))",
                    f((x1 - x0) / 2.0),
                    q(&ids.next())
                );
            } else {
                let k = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
                for i in 0..4 {
                    out.push_str(&line(k[i], k[(i + 1) % 4], ids));
                }
            }
        }
    }
    // Pads.
    // One entry per pad: a pin may own several pads with the same number.
    let mut pad_list: Vec<(usize, PinDef, Option<f32>)> = vec![];
    for (pi, pd) in inf.pins.iter().enumerate() {
        pad_list.push((pi, pd.clone(), None));
        for e in pd.extra.iter().filter(|_| !pd.pad.is_empty()) {
            let mut v = pd.clone();
            v.pad_pos = e.pos;
            v.pad_size = e.size;
            v.shape = e.shape;
            v.tht = e.tht;
            v.back = e.back;
            v.extra = vec![];
            pad_list.push((pi, v, Some(e.drill)));
        }
    }
    for (pi, pd, drill_given) in &pad_list {
        let pi = *pi;
        if pd.pad.is_empty() {
            continue;
        }
        let (num, name) = match nets { Some((nl, ci, net_of)) => net_of(nl.net_of(ci, pi)), None => (0, String::new()) };
        let round = pd.shape == PadShape::Round;
        let shape = if round {
            if (pd.pad_size.0 - pd.pad_size.1).abs() < 1e-3 {
                "circle"
            } else {
                "oval"
            }
        } else {
            "rect"
        };
        let net = if num > 0 { format!(" (net {} {})", num, q(&name)) } else { String::new() };
        if pd.tht {
            let drill = drill_given
                .filter(|d| *d > 0.0)
                .or_else(|| c.fp_geom().and_then(|(pads, _, _)| pads.iter().find(|x| x.number == pd.pad).map(|x| x.drill)).filter(|d| *d > 0.0))
                .unwrap_or(((pd.pad_size.0.min(pd.pad_size.1) * 0.55 / 0.05).round() * 0.05).clamp(0.6, 2.0));
            let _ = writeln!(
                out,
                "\t\t(pad {} thru_hole {} (at {} {} {}) (size {} {}) (drill {}) (layers \"*.Cu\" \"*.Mask\"){} (uuid {}))",
                q(&pd.pad),
                shape,
                f(pd.pad_pos.0),
                f(pd.pad_pos.1),
                angle,
                f(pd.pad_size.0),
                f(pd.pad_size.1),
                f(drill),
                net,
                q(&ids.next())
            );
        } else {
            let side = if pd.back { "B" } else { "F" };
            let _ = writeln!(
                out,
                "\t\t(pad {} smd {} (at {} {} {}) (size {} {}) (layers \"{s}.Cu\" \"{s}.Paste\" \"{s}.Mask\"){} (uuid {}))",
                q(&pd.pad),
                shape,
                f(pd.pad_pos.0),
                f(pd.pad_pos.1),
                angle,
                f(pd.pad_size.0),
                f(pd.pad_size.1),
                net,
                q(&ids.next()),
                s = side
            );
        }
    }
    let _ = writeln!(out, "\t\t(attr {})", if inf.pins.iter().any(|pd| pd.tht) { "through_hole" } else { "smd" });
    out
}

/// Write the three KiCad files next to `pro_path` (any of .kicad_pro/.kicad_sch/.kicad_pcb).
pub fn export(p: &Project, nl: &Netlist, conn: &PcbConn, pro_path: &Path) -> Result<Vec<PathBuf>, String> {
    let dir = pro_path.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let stem = pro_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "board".into());
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {}", dir.display(), e))?;
    let mut ids = Ids::new();
    let root = ids.next();

    // ---------------------------------------------------------------- schematic
    // Place the drawing on a sheet large enough, with a margin, on the 1.27 mm grid.
    let mut bb = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    let mut grow = |x: i32, y: i32| bb = (bb.0.min(x), bb.1.min(y), bb.2.max(x), bb.3.max(y));
    for c in &p.components {
        for b in c.sch_boxes() {
            grow(b.0, b.1);
            grow(b.2, b.3);
        }
    }
    for w in &p.wires {
        grow(w.a.0, w.a.1);
        grow(w.b.0, w.b.1);
    }
    for l in &p.labels {
        grow(l.pos.0, l.pos.1);
    }
    for n in &p.notes {
        let b = n.bbox();
        grow(b.0, b.1);
        grow(b.2, b.3);
    }
    if bb.0 > bb.2 {
        bb = (0, 0, 100, 100);
    }
    let margin = 200; // 25.4 mm
    // Keep the origin on KiCad's 1.27 mm grid (10 units).
    let off = (margin - bb.0.div_euclid(10) * 10, margin - bb.1.div_euclid(10) * 10);
    let (wmm, hmm) = (((bb.2 - bb.0) + 2 * margin) as f32 * MM, ((bb.3 - bb.1) + 2 * margin) as f32 * MM);
    let paper = [("A4", 297.0, 210.0), ("A3", 420.0, 297.0), ("A2", 594.0, 420.0), ("A1", 841.0, 594.0), ("A0", 1189.0, 841.0)]
        .iter()
        .find(|(_, w, h)| wmm <= *w && hmm <= *h)
        .map(|(n, _, _)| format!("(paper {})", q(n)))
        .unwrap_or_else(|| format!("(paper \"User\" {} {})", f(wmm.ceil()), f(hmm.ceil())));
    let xy = |pt: (i32, i32)| format!("{} {}", f((pt.0 + off.0) as f32 * MM), f((pt.1 + off.1) as f32 * MM));

    let mut sch = String::new();
    let _ = writeln!(sch, "(kicad_sch\n\t(version 20231120)\n\t(generator \"ferriscad\")\n\t(generator_version \"1.0\")\n\t(uuid {})\n\t{}", q(&root), paper);
    let _ = writeln!(sch, "\t(title_block (title {}) (comment 1 \"Exported from FerrisCad\"))", q(&p.name));
    sch.push_str("\t(lib_symbols\n");
    let mut names: Vec<String> = vec![];
    let mut sym_texts: Vec<String> = vec![];
    let mut pin_numbers: Vec<Vec<String>> = vec![];
    for (ci, c) in p.components.iter().enumerate() {
        let base: String = c.reference.chars().filter(|ch| ch.is_ascii_alphanumeric()).collect();
        let name = format!("{}_{}", if base.is_empty() { "P".to_string() } else { base }, ci);
        let (text, nums) = lib_symbol(&name, c);
        sch.push_str(&text);
        sym_texts.push(text);
        names.push(name);
        pin_numbers.push(nums);
    }
    sch.push_str("\t)\n");
    for &j in &nl.junctions {
        let _ = writeln!(sch, "\t(junction (at {}) (diameter 0) (color 0 0 0 0) (uuid {}))", xy(j), q(&ids.next()));
    }
    for nc in &p.no_connects {
        let _ = writeln!(sch, "\t(no_connect (at {}) (uuid {}))", xy(*nc), q(&ids.next()));
    }
    // KiCad only joins wires at their ends: a pin, a label, a junction or another wire's end that
    // sits in the middle of a wire would stay disconnected (and a junction there even cuts the
    // wire's far end off). FerrisCad connects them, so split every wire at those points, as KiCad
    // itself does when it saves.
    let mut touch: Vec<(i32, i32)> = nl.junctions.clone();
    for c in &p.components {
        touch.extend(c.info().pins.iter().map(|pd| c.pin_pos(pd)));
    }
    touch.extend(p.labels.iter().map(|l| l.pos));
    touch.extend(p.no_connects.iter().copied());
    for w in &p.wires {
        touch.push(w.a);
        touch.push(w.b);
    }
    for w in &p.wires {
        let mut cuts: Vec<(i32, i32)> = touch.iter().copied().filter(|&t| t != w.a && t != w.b && on_segment(t, w.a, w.b)).collect();
        cuts.sort_by_key(|t| (t.0 - w.a.0).abs() + (t.1 - w.a.1).abs());
        cuts.dedup();
        let mut from = w.a;
        for to in cuts.into_iter().chain(std::iter::once(w.b)) {
            let _ = writeln!(
                sch,
                "\t(wire (pts (xy {}) (xy {})) (stroke (width 0) (type default)) (uuid {}))",
                xy(from),
                xy(to),
                q(&ids.next())
            );
            from = to;
        }
    }
    for l in &p.labels {
        let _ = writeln!(
            sch,
            "\t(label {} (at {} 0) (fields_autoplaced yes) (effects (font (size 1.27 1.27)) (justify left bottom)) (uuid {}))",
            q(&l.text),
            xy(l.pos),
            q(&ids.next())
        );
    }
    for n in &p.notes {
        let (r, g, b) = n.color.rgb();
        let stroke = format!("(stroke (width 0.15) (type {}) (color {} {} {} 1))", if n.dashed { "dash" } else { "solid" }, r, g, b);
        let fill = if n.filled {
            let (r, g, b) = n.color.fill();
            format!("(fill (type color) (color {} {} {} 1))", r, g, b)
        } else {
            "(fill (type none))".to_string()
        };
        let h = f(n.size as f32 * MM);
        let effects = format!("(effects (font (size {h} {h}) (color {} {} {} 1)) (justify left top))", r, g, b);
        let text = q(&n.text.replace('\r', "")).replace('\n', "\\n");
        let size = (f((n.b.0 - n.a.0) as f32 * MM), f((n.b.1 - n.a.1) as f32 * MM));
        let m = f(crate::annot::PAD * MM);
        let id = q(&ids.next());
        let _ = match n.kind {
            NoteKind::Line => writeln!(sch, "\t(polyline (pts (xy {}) (xy {})) {} (uuid {}))", xy(n.a), xy(n.b), stroke, id),
            NoteKind::Box if n.text.trim().is_empty() => {
                writeln!(sch, "\t(rectangle (start {}) (end {}) {} {} (uuid {}))", xy(n.a), xy(n.b), stroke, fill, id)
            }
            NoteKind::Box => writeln!(
                sch,
                "\t(text_box {} (exclude_from_sim no) (at {} 0) (size {} {}) (margins {m} {m} {m} {m}) {} {} {} (uuid {}))",
                text, xy(n.a), size.0, size.1, stroke, fill, effects, id
            ),
            NoteKind::Text if n.text.trim().is_empty() => Ok(()),
            NoteKind::Text => writeln!(sch, "\t(text {} (exclude_from_sim no) (at {} 0) {} (uuid {}))", text, xy(n.a), effects, id),
        };
    }
    for (ci, c) in p.components.iter().enumerate() {
        let angle = (360 - c.rot as i32 * 90).rem_euclid(360);
        let b = c.sch_bbox();
        let ref_at = (b.2 + 10, (b.1 + b.3) / 2 - 10);
        let val_at = (b.2 + 10, (b.1 + b.3) / 2 + 10);
        let power = c.kind.is_power();
        let fp = c.info().footprint;
        let fp_id = if fp.is_empty() || power {
            String::new()
        } else if fp.contains(':') {
            fp.clone()
        } else {
            format!("FerrisCad:{}", fp)
        };
        let _ = writeln!(
            sch,
            "\t(symbol (lib_id {}) (at {} {}) (unit 1) (exclude_from_sim no) (in_bom {}) (on_board {}) (dnp no) (uuid {})",
            q(&format!("FerrisCad:{}", names[ci])),
            xy(c.pos),
            angle,
            if power { "yes" } else { "yes" },
            if power { "yes" } else { "yes" },
            q(&ids.next())
        );
        let reference = if power && !c.reference.starts_with('#') { format!("#PWR{:02}", ci + 1) } else { c.reference.clone() };
        let _ = writeln!(
            sch,
            "\t\t(property \"Reference\" {} (at {} 0) {})",
            q(&reference),
            xy(ref_at),
            if power { "(effects (font (size 1.27 1.27)) hide)".to_string() } else { "(effects (font (size 1.27 1.27)) (justify left))".to_string() }
        );
        let val_pos = if power { xy((c.pos.0, c.pos.1 + if c.kind == Kind::Gnd { 30 } else { -20 })) } else { xy(val_at) };
        let _ = writeln!(
            sch,
            "\t\t(property \"Value\" {} (at {} 0) {})",
            q(&c.value),
            val_pos,
            if power { "(effects (font (size 1.27 1.27)))".to_string() } else { "(effects (font (size 1.27 1.27)) (justify left))".to_string() }
        );
        let _ = writeln!(sch, "\t\t(property \"Footprint\" {} (at {} 0) {})", q(&fp_id), xy(c.pos), font_hidden(1.27));
        let _ = writeln!(sch, "\t\t(property \"Datasheet\" \"\" (at {} 0) {})", xy(c.pos), font_hidden(1.27));
        if !c.lcsc.trim().is_empty() {
            let _ = writeln!(sch, "\t\t(property \"LCSC\" {} (at {} 0) {})", q(c.lcsc.trim()), xy(c.pos), font_hidden(1.27));
        }
        // Lets FerrisCad recognise the part again when the project is re-opened.
        let kind = serde_json::to_string(&c.kind).unwrap_or_default().trim_matches('"').to_string();
        let _ = writeln!(sch, "\t\t(property \"FerrisCad_Kind\" {} (at {} 0) {})", q(&kind), xy(c.pos), font_hidden(1.27));
        for n in &pin_numbers[ci] {
            let _ = writeln!(sch, "\t\t(pin {} (uuid {}))", q(n), q(&ids.next()));
        }
        let _ = writeln!(
            sch,
            "\t\t(instances (project {} (path {} (reference {}) (unit 1))))\n\t)",
            q(&stem),
            q(&format!("/{}", root)),
            q(&reference)
        );
    }
    sch.push_str("\t(sheet_instances (path \"/\" (page \"1\")))\n)\n");

    // ---------------------------------------------------------------- board
    // Keep the board inside the page: shift it if it starts too close to the corner.
    let b = &p.board;
    let shift = (if b.x0 < 20.0 { 50.0 - b.x0 } else { 0.0 }, if b.y0 < 20.0 { 50.0 - b.y0 } else { 0.0 });
    let pp = |pt: (f32, f32)| format!("{} {}", f(pt.0 + shift.0), f(pt.1 + shift.1));
    // Nets: every net that reaches a pad.
    let mut net_no: HashMap<usize, usize> = HashMap::new();
    let mut net_list: Vec<String> = vec![String::new()];
    for pad in &conn.pads {
        if let Some(n) = pad.net {
            net_no.entry(n).or_insert_with(|| {
                net_list.push(nl.nets[n].name.clone());
                net_list.len() - 1
            });
        }
    }
    let net_of = |n: Option<usize>| -> (usize, String) {
        match n.and_then(|n| net_no.get(&n).copied()) {
            Some(k) => (k, net_list[k].clone()),
            None => (0, String::new()),
        }
    };
    let layers = b.copper();
    let layer_name = |l: Layer| l.short();
    let mut pcb = String::new();
    let _ = writeln!(pcb, "(kicad_pcb\n\t(version 20240108)\n\t(generator \"ferriscad\")\n\t(generator_version \"1.0\")");
    let _ = writeln!(pcb, "\t(general (thickness 1.6) (legacy_teardrops no))\n\t(paper \"A3\")");
    pcb.push_str("\t(layers\n");
    let _ = writeln!(pcb, "\t\t(0 \"F.Cu\" signal)");
    for l in &layers {
        if let Layer::Inner(n) = l {
            let _ = writeln!(pcb, "\t\t({} {} signal)", n, q(&l.short()));
        }
    }
    let _ = writeln!(pcb, "\t\t(31 \"B.Cu\" signal)");
    for (i, n, u) in [
        (34, "B.Paste", "user"),
        (35, "F.Paste", "user"),
        (36, "B.SilkS", "user \"B.Silkscreen\""),
        (37, "F.SilkS", "user \"F.Silkscreen\""),
        (38, "B.Mask", "user"),
        (39, "F.Mask", "user"),
        (44, "Edge.Cuts", "user"),
        (46, "B.CrtYd", "user \"B.Courtyard\""),
        (47, "F.CrtYd", "user \"F.Courtyard\""),
        (48, "B.Fab", "user"),
        (49, "F.Fab", "user"),
    ] {
        let _ = writeln!(pcb, "\t\t({} {} {})", i, q(n), u);
    }
    pcb.push_str("\t)\n\t(setup (pad_to_mask_clearance 0))\n");
    for (i, n) in net_list.iter().enumerate() {
        let _ = writeln!(pcb, "\t(net {} {})", i, q(n));
    }
    let mut fp_lib: std::collections::BTreeMap<String, String> = Default::default();
    for (ci, c) in p.components.iter().enumerate() {
        if !c.has_fp() {
            continue;
        }
        let inf = c.info();
        let angle = (360 - c.pcb_rot as i32 * 90).rem_euclid(360);
        let fp = if inf.footprint.contains(':') { inf.footprint.clone() } else { format!("FerrisCad:{}", inf.footprint) };
        let _ = writeln!(pcb, "\t(footprint {} (layer \"F.Cu\") (uuid {}) (at {} {})", q(&fp), q(&ids.next()), pp(c.pcb_pos), angle);
        let bb = c.pcb_bbox();
        let top = (bb.1 - c.pcb_pos.1).min(-0.5) - 1.0;
        let bottom = (bb.3 - c.pcb_pos.1).max(0.5) + 1.0;
        let _ = writeln!(
            pcb,
            "\t\t(property \"Reference\" {} (at 0 {} {}) (layer \"F.SilkS\") (uuid {}) (effects (font (size 1 1) (thickness 0.15))))",
            q(&c.reference),
            f(top),
            angle,
            q(&ids.next())
        );
        let _ = writeln!(
            pcb,
            "\t\t(property \"Value\" {} (at 0 {} {}) (layer \"F.Fab\") (uuid {}) (effects (font (size 1 1) (thickness 0.15))))",
            q(&c.value),
            f(bottom),
            angle,
            q(&ids.next())
        );
        if !c.lcsc.trim().is_empty() {
            let _ = writeln!(
                pcb,
                "\t\t(property \"LCSC\" {} (at 0 0 0) (layer \"F.Fab\") (hide yes) (uuid {}) (effects (font (size 1 1) (thickness 0.15))))",
                q(c.lcsc.trim()),
                q(&ids.next())
            );
        }
        let body = fp_body(c, angle, Some((nl, ci, &net_of)), &mut ids);
        pcb.push_str(&body);
        if !inf.footprint.contains(':') && !fp_lib.contains_key(&inf.footprint) {
            let lib_body = fp_body(c, 0, None, &mut ids);
            let (top, bottom) = ((c.pcb_bbox().1 - c.pcb_pos.1).min(-0.5) - 1.0, (c.pcb_bbox().3 - c.pcb_pos.1).max(0.5) + 1.0);
            let text = format!(
                "(footprint {} (version 20240108) (generator \"ferriscad\") (generator_version \"1.0\") (layer \"F.Cu\")\n\t(property \"Reference\" \"REF**\" (at 0 {} 0) (layer \"F.SilkS\") (uuid {}) (effects (font (size 1 1) (thickness 0.15))))\n\t(property \"Value\" {} (at 0 {} 0) (layer \"F.Fab\") (uuid {}) (effects (font (size 1 1) (thickness 0.15))))\n{})\n",
                q(&inf.footprint),
                f(top),
                q(&ids.next()),
                q(&inf.footprint),
                f(bottom),
                q(&ids.next()),
                lib_body
            );
            fp_lib.insert(inf.footprint.clone(), text);
        }
        pcb.push_str("\t)\n");
    }
    for (i, t) in p.tracks.iter().enumerate() {
        if !b.has(t.layer) {
            continue;
        }
        let (num, _) = net_of(conn.track_net(i));
        let _ = writeln!(
            pcb,
            "\t(segment (start {}) (end {}) (width {}) (layer {}) (net {}) (uuid {}))",
            pp(t.a),
            pp(t.b),
            f(t.width),
            q(&layer_name(t.layer)),
            num,
            q(&ids.next())
        );
    }
    for (i, v) in p.vias.iter().enumerate() {
        let (num, _) = net_of(conn.via_net(i));
        let _ = writeln!(
            pcb,
            "\t(via (at {}) (size {}) (drill {}) (layers \"F.Cu\" \"B.Cu\") (net {}) (uuid {}))",
            pp(v.pos),
            f(v.size),
            f((v.size * 0.5).max(0.3)),
            num,
            q(&ids.next())
        );
    }
    for z in &p.zones {
        let Some(l) = z.layer.filter(|l| b.has(*l)) else { continue };
        let k = net_list.iter().position(|n| *n == z.net).unwrap_or(0);
        // One KiCad zone per outline (the board outline when the original is unknown), holding
        // all the fill islands inside it, instead of one zone per island.
        let outlines: Vec<Vec<(f32, f32)>> = if z.outlines.is_empty() {
            vec![vec![(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)]]
        } else {
            z.outlines.clone()
        };
        let mut fills: Vec<Vec<&Vec<(f32, f32)>>> = vec![vec![]; outlines.len()];
        for poly in &z.polys {
            let i = poly.first().and_then(|&pt| outlines.iter().position(|o| point_in_poly(pt, o))).unwrap_or(0);
            fills[i].push(poly);
        }
        for (oi, outline) in outlines.iter().enumerate() {
            let pts_of = |poly: &[(f32, f32)]| poly.iter().map(|&q2| format!("(xy {})", pp(q2))).collect::<Vec<_>>().join(" ");
            // A zone drawn in FerrisCad has no real fill yet: KiCad computes it (press B).
            let filled: String = fills[oi]
                .iter()
                .filter(|_| !z.drawn)
                .map(|poly| format!(" (filled_polygon (layer {}) (pts {}))", q(&layer_name(l)), pts_of(poly)))
                .collect();
            let _ = writeln!(
                pcb,
                "\t(zone (net {}) (net_name {}) (layer {}) (uuid {}) (hatch edge 0.5) (connect_pads (clearance {})) (min_thickness 0.25) (filled_areas_thickness no) (fill yes (thermal_gap 0.5) (thermal_bridge_width 0.5)) (polygon (pts {})){})",
                k,
                q(&z.net),
                q(&layer_name(l)),
                q(&ids.next()),
                f(b.clearance),
                pts_of(outline),
                filled
            );
        }
    }
    let _ = writeln!(
        pcb,
        "\t(gr_rect (start {}) (end {}) (stroke (width 0.1) (type default)) (fill none) (layer \"Edge.Cuts\") (uuid {}))",
        pp((b.x0, b.y0)),
        pp((b.x1, b.y1)),
        q(&ids.next())
    );
    for t in &p.texts {
        if let TextLayer::Copper(l) = t.layer {
            if !b.has(l) {
                continue;
            }
        }
        let angle = (360 - t.rot as i32 * 90).rem_euclid(360);
        let mirror = if t.layer.is_back() { " (justify mirror)" } else { "" };
        let _ = writeln!(
            pcb,
            "\t(gr_text {} (at {} {}) (layer {}) (uuid {}) (effects (font (size {} {}) (thickness {})){}))",
            q(&t.text),
            pp(t.pos),
            angle,
            q(&t.layer.kicad()),
            q(&ids.next()),
            f(t.size),
            f(t.size),
            f(crate::font::stroke_width(t.size)),
            mirror
        );
    }
    pcb.push_str(")\n");

    // ---------------------------------------------------------------- project
    let pro = serde_json::json!({
        "meta": { "filename": format!("{}.kicad_pro", stem), "version": 1 },
        "board": { "design_settings": { "defaults": {}, "rules": { "min_clearance": b.clearance } } },
        "net_settings": {
            "classes": [{
                "name": "Default",
                "clearance": b.clearance,
                "track_width": 0.25,
                "via_diameter": 0.8,
                "via_drill": 0.4
            }],
            "meta": { "version": 3 }
        },
        "sheets": [[root, "Root"]]
    });
    let files = [
        (dir.join(format!("{}.kicad_pro", stem)), serde_json::to_string_pretty(&pro).unwrap()),
        (dir.join(format!("{}.kicad_sch", stem)), sch),
        (dir.join(format!("{}.kicad_pcb", stem)), pcb),
    ];
    // The "FerrisCad" libraries the files refer to, so KiCad finds every symbol and footprint.
    let mut sym_lib = String::from("(kicad_symbol_lib\n\t(version 20231120)\n\t(generator \"ferriscad\")\n\t(generator_version \"1.0\")\n");
    for t in &sym_texts {
        sym_lib.push_str(&t.replacen("\"FerrisCad:", "\"", 1));
    }
    sym_lib.push_str(")\n");
    let table = |kind: &str, ext: &str| {
        format!(
            "({}_lib_table\n\t(version 7)\n\t(lib (name \"FerrisCad\") (type \"KiCad\") (uri \"${{KIPRJMOD}}/FerrisCad.{}\") (options \"\") (descr \"Parts exported from FerrisCad\"))\n)\n",
            kind, ext
        )
    };
    let pretty = dir.join("FerrisCad.pretty");
    std::fs::create_dir_all(&pretty).map_err(|e| format!("can't create {}: {}", pretty.display(), e))?;
    let mut files = Vec::from(files);
    files.push((dir.join("FerrisCad.kicad_sym"), sym_lib));
    files.push((dir.join("sym-lib-table"), table("sym", "kicad_sym")));
    files.push((dir.join("fp-lib-table"), table("fp", "pretty")));
    for (name, text) in &fp_lib {
        let file: String = name.chars().map(|ch| if ch.is_ascii_alphanumeric() || "-_.+".contains(ch) { ch } else { '_' }).collect();
        files.push((pretty.join(format!("{}.kicad_mod", file)), text.clone()));
    }
    for (path, text) in &files {
        std::fs::write(path, text).map_err(|e| format!("can't write {}: {}", path.display(), e))?;
    }
    let _ = layers;
    Ok(files.into_iter().map(|(p, _)| p).collect())
}
