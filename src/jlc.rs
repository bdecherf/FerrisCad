//! JLCPCB: search the parts catalogue (stock, price, Basic/Extended) and generate the
//! manufacturing files they ask for: Gerbers + drill (zipped), BOM and placement (CPL).
//!
//! JLCPCB has no public API to create orders from other software, so FerrisCad prepares the
//! files and opens their order page; the person uploads the ZIP, then the BOM and CPL.

use crate::checks::PcbConn;
use crate::model::*;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const ORDER_URL: &str = "https://cart.jlcpcb.com/quote";
const SEARCH_URL: &str = "https://jlcpcb.com/api/overseas-pcb-order/v1/shoppingCart/smtGood/selectSmtComponentList";

#[derive(Clone, Debug)]
pub struct JlcPart {
    pub code: String,
    pub model: String,
    pub brand: String,
    pub package: String,
    pub description: String,
    pub stock: i64,
    pub basic: bool,
    pub price: f64,
    pub datasheet: String,
}

/// Search the JLCPCB assembly parts catalogue (the same search their website uses).
pub fn search(keyword: &str) -> Result<Vec<JlcPart>, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build();
    let body = serde_json::json!({ "keyword": keyword, "currentPage": 1, "pageSize": 25 });
    let resp = agent
        .post(SEARCH_URL)
        .set("content-type", "application/json")
        .set("user-agent", "Mozilla/5.0 (FerrisCad)")
        .send_string(&body.to_string());
    let text = match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(c, _)) => return Err(format!("JLCPCB answered {}", c)),
        Err(e) => return Err(format!("no connection to JLCPCB ({})", e)),
    };
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| "JLCPCB sent an unexpected answer (their site may have changed)".to_string())?;
    let list = v["data"]["componentPageInfo"]["list"].as_array().cloned().unwrap_or_default();
    let mut out: Vec<JlcPart> = list
        .iter()
        .filter_map(|p| {
            Some(JlcPart {
                code: p["componentCode"].as_str()?.to_string(),
                model: p["componentModelEn"].as_str().unwrap_or("").to_string(),
                brand: p["componentBrandEn"].as_str().unwrap_or("").to_string(),
                package: p["componentSpecificationEn"].as_str().unwrap_or("").to_string(),
                description: p["describe"].as_str().unwrap_or("").to_string(),
                stock: p["stockCount"].as_i64().unwrap_or(0),
                basic: p["componentLibraryType"].as_str() == Some("base"),
                price: p["componentPrices"].as_array().and_then(|a| a.first()).and_then(|x| x["productPrice"].as_f64()).unwrap_or(0.0),
                datasheet: p["dataManualUrl"].as_str().unwrap_or("").to_string(),
            })
        })
        .collect();
    // Basic parts first (no extra loading fee at JLCPCB), then by stock.
    out.sort_by(|a, b| b.basic.cmp(&a.basic).then((b.stock > 0).cmp(&(a.stock > 0))).then(b.stock.cmp(&a.stock)));
    Ok(out)
}

/// Package name found in a footprint name ("R_0805_2012Metric" -> "0805", "SOT-23" -> "SOT-23").
pub fn package_of(footprint: &str) -> String {
    let name = footprint.rsplit(':').next().unwrap_or(footprint);
    for size in ["0201", "0402", "0603", "0805", "1206", "1210", "2512"] {
        if name.contains(size) {
            return size.to_string();
        }
    }
    for pkg in ["SOT-23-6", "SOT-23-5", "SOT-23", "SOT-223", "SOT-89", "SOD-123", "SOD-323", "SOIC-8", "SOIC-14", "SOIC-16", "TSSOP-20", "TO-220", "TO-252", "TO-92", "DIP-8", "QFN", "LQFP", "SMA", "SMB"] {
        if name.to_uppercase().contains(pkg) {
            return pkg.to_string();
        }
    }
    String::new()
}

fn trim_num(x: f64) -> String {
    let s = format!("{:.3}", x);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Value written the way the catalogue writes it: 22Ω, 4.7kΩ, 100nF, 10uH.
fn catalogue_value(c: &Component) -> String {
    let v = c.value.trim();
    let Some(x) = parse_si(v) else { return v.to_string() };
    match c.kind {
        Kind::Resistor => format!("{}Ω", fmt_res(x)),
        Kind::Capacitor | Kind::CapPolarized | Kind::Inductor => {
            let unit = if c.kind == Kind::Inductor { "H" } else { "F" };
            if x >= 1e-6 {
                format!("{}u{}", trim_num(x * 1e6), unit)
            } else if x >= 1e-9 {
                format!("{}n{}", trim_num(x * 1e9), unit)
            } else {
                format!("{}p{}", trim_num(x * 1e12), unit)
            }
        }
        _ => v.to_string(),
    }
}

/// A catalogue search for a part (for the manual search box).
pub fn query_for(c: &Component) -> String {
    let pkg = package_of(&c.info().footprint);
    let v = catalogue_value(c);
    match c.kind {
        Kind::Led => format!("LED {} {}", c.value.trim(), pkg),
        _ => format!("{} {}", v, pkg),
    }
    .trim()
    .to_string()
}

/// Search and required package for automatic matching, or None when the part can't be matched
/// safely (unknown package, batteries, switches, connectors, generic chips).
pub fn auto_query(c: &Component) -> Option<(String, String)> {
    let pkg = package_of(&c.info().footprint);
    match c.kind {
        Kind::Battery | Kind::Switch | Kind::Connector2 | Kind::Gnd | Kind::Power | Kind::Mcu8 => None,
        Kind::Resistor | Kind::Capacitor | Kind::CapPolarized | Kind::Inductor | Kind::Led | Kind::Diode if pkg.is_empty() => None,
        // A real part number (2N3904, L7805, NE555P...) is worth a search even without a package.
        _ if pkg.is_empty() && !(c.value.chars().any(|ch| ch.is_ascii_digit()) && c.value.chars().any(|ch| ch.is_ascii_alphabetic()) && c.value.len() >= 4) => None,
        _ => Some((query_for(c), pkg)),
    }
}

/// One BOM line: identical parts grouped together.
#[derive(Clone, Debug)]
pub struct BomLine {
    pub comment: String,
    pub footprint: String,
    pub lcsc: String,
    pub refs: Vec<String>,
    pub ids: Vec<u32>,
}

pub fn bom(p: &Project) -> Vec<BomLine> {
    let mut groups: BTreeMap<(String, String, String), BomLine> = BTreeMap::new();
    let mut comps: Vec<&Component> = p.components.iter().filter(|c| c.has_fp()).collect();
    comps.sort_by(|a, b| natural_key(&a.reference).cmp(&natural_key(&b.reference)));
    for c in comps {
        let fp = c.info().footprint;
        let key = (c.value.clone(), fp.clone(), c.lcsc.trim().to_string());
        let line = groups.entry(key).or_insert_with(|| BomLine {
            comment: c.value.clone(),
            footprint: fp.rsplit(':').next().unwrap_or(&fp).to_string(),
            lcsc: c.lcsc.trim().to_string(),
            refs: vec![],
            ids: vec![],
        });
        line.refs.push(c.reference.clone());
        line.ids.push(c.id);
    }
    groups.into_values().collect()
}

fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------- Gerber writer

struct Gerber {
    body: String,
    apertures: Vec<String>,
    current: Option<usize>,
}

/// Gerber Y goes up; the board's Y goes down.
fn gx(v: f32) -> i64 {
    (v as f64 * 1e6).round() as i64
}
fn gy(v: f32) -> i64 {
    (-(v as f64) * 1e6).round() as i64
}

impl Gerber {
    fn new() -> Self {
        Gerber { body: String::new(), apertures: vec![], current: None }
    }
    fn aperture(&mut self, def: String) {
        let i = match self.apertures.iter().position(|a| *a == def) {
            Some(i) => i,
            None => {
                self.apertures.push(def);
                self.apertures.len() - 1
            }
        };
        if self.current != Some(i) {
            let _ = writeln!(self.body, "D{}*", i + 10);
            self.current = Some(i);
        }
    }
    fn circle(&mut self, d: f32) {
        self.aperture(format!("C,{:.6}", d.max(0.01)));
    }
    fn line(&mut self, a: (f32, f32), b: (f32, f32), w: f32) {
        self.circle(w);
        let _ = writeln!(self.body, "X{}Y{}D02*", gx(a.0), gy(a.1));
        let _ = writeln!(self.body, "X{}Y{}D01*", gx(b.0), gy(b.1));
    }
    fn pad(&mut self, pos: (f32, f32), size: (f32, f32), round: bool, grow: f32) {
        let (w, h) = (size.0 + 2.0 * grow, size.1 + 2.0 * grow);
        if round {
            self.circle(w.min(h));
        } else {
            self.aperture(format!("R,{:.6}X{:.6}", w.max(0.01), h.max(0.01)));
        }
        let _ = writeln!(self.body, "X{}Y{}D03*", gx(pos.0), gy(pos.1));
    }
    /// Dark (copper) or clear (carves what was drawn before) polarity.
    fn polarity(&mut self, dark: bool) {
        let _ = writeln!(self.body, "%LP{}*%", if dark { 'D' } else { 'C' });
    }
    fn region(&mut self, pts: &[(f32, f32)]) {
        if pts.len() < 3 {
            return;
        }
        let _ = writeln!(self.body, "G36*");
        let _ = writeln!(self.body, "X{}Y{}D02*", gx(pts[0].0), gy(pts[0].1));
        for q in pts.iter().skip(1).chain(std::iter::once(&pts[0])) {
            let _ = writeln!(self.body, "X{}Y{}D01*", gx(q.0), gy(q.1));
        }
        let _ = writeln!(self.body, "G37*");
    }
    fn finish(self, function: &str) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "%TF.GenerationSoftware,FerrisCad,,0.1*%");
        let _ = writeln!(s, "%TF.FileFunction,{}*%", function);
        let _ = writeln!(s, "%FSLAX46Y46*%");
        let _ = writeln!(s, "%MOMM*%");
        let _ = writeln!(s, "%LPD*%");
        let _ = writeln!(s, "G01*");
        for (i, a) in self.apertures.iter().enumerate() {
            let _ = writeln!(s, "%ADD{}{}*%", i + 10, a);
        }
        s.push_str(&self.body);
        s.push_str("M02*\n");
        s
    }
}

fn drill_of(pad: &crate::checks::PadItem, p: &Project) -> f32 {
    let c = &p.components[pad.ci];
    if pad.drill > 0.0 {
        return pad.drill;
    }
    let given = c.fp_geom().and_then(|(pads, _, _)| {
        let num = &c.info().pins[pad.pi].pad;
        pads.iter().find(|x| &x.number == num).map(|x| x.drill)
    });
    match given {
        Some(d) if d > 0.0 => d,
        _ => ((pad.size.0.min(pad.size.1) * 0.55 / 0.05).round() * 0.05).clamp(0.6, 2.0),
    }
}

/// Silkscreen outline of every footprint, in board coordinates.
fn silk_lines(p: &Project) -> Vec<((f32, f32), (f32, f32))> {
    let mut v = vec![];
    for c in p.components.iter().filter(|c| c.has_fp()) {
        let xf = |q: (f32, f32)| {
            let r = rot_f(q, c.pcb_rot);
            (c.pcb_pos.0 + r.0, c.pcb_pos.1 + r.1)
        };
        let circle = |cx: f32, cy: f32, r: f32, v: &mut Vec<((f32, f32), (f32, f32))>| {
            let n = 32;
            for i in 0..n {
                let a0 = i as f32 / n as f32 * std::f32::consts::TAU;
                let a1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
                v.push((xf((cx + r * a0.cos(), cy + r * a0.sin())), xf((cx + r * a1.cos(), cy + r * a1.sin()))));
            }
        };
        match c.fp_geom() {
            Some((_, silk, circles)) => {
                for (a, b) in silk {
                    v.push((xf(*a), xf(*b)));
                }
                for &(x, y, r) in circles {
                    circle(x, y, r, &mut v);
                }
            }
            None => {
                let inf = c.info();
                let (x0, y0, x1, y1) = inf.fp_body;
                if inf.fp_round {
                    circle(0.0, 0.0, (x1 - x0) / 2.0, &mut v);
                } else {
                    let k = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
                    for i in 0..4 {
                        v.push((xf(k[i]), xf(k[(i + 1) % 4])));
                    }
                }
            }
        }
    }
    v
}

// ---------------------------------------------------------------- zip (stored)

fn zip(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let n = name.as_bytes();
        let local = |buf: &mut Vec<u8>, central: bool| {
            if central {
                buf.extend_from_slice(&0x02014b50u32.to_le_bytes());
                buf.extend_from_slice(&20u16.to_le_bytes());
            } else {
                buf.extend_from_slice(&0x04034b50u32.to_le_bytes());
            }
            buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
            buf.extend_from_slice(&0u16.to_le_bytes()); // flags
            buf.extend_from_slice(&0u16.to_le_bytes()); // stored
            buf.extend_from_slice(&0u16.to_le_bytes()); // time
            buf.extend_from_slice(&0x5a21u16.to_le_bytes()); // date
            buf.extend_from_slice(&crc.to_le_bytes());
            buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&(n.len() as u16).to_le_bytes());
            buf.extend_from_slice(&0u16.to_le_bytes()); // extra
            if central {
                buf.extend_from_slice(&0u16.to_le_bytes()); // comment
                buf.extend_from_slice(&0u16.to_le_bytes()); // disk
                buf.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
                buf.extend_from_slice(&0u32.to_le_bytes()); // external attrs
                buf.extend_from_slice(&offset.to_le_bytes());
            }
            buf.extend_from_slice(n);
        };
        local(&mut out, false);
        out.extend_from_slice(data);
        local(&mut central, true);
    }
    let cd_offset = out.len() as u32;
    let cd_len = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

// ---------------------------------------------------------------- export

pub struct Export {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub notes: Vec<String>,
}

/// Write the Gerber ZIP, BOM and CPL into `dir`.
pub fn export(p: &Project, conn: &PcbConn, dir: &Path) -> Result<Export, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {}", dir.display(), e))?;
    let base: String = p.name.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let base = if base.is_empty() { "board".to_string() } else { base };
    let mut notes = vec![];

    // Copper (one Gerber per layer), mask and paste.
    let layers = p.board.copper();
    let mut copper: Vec<Gerber> = layers.iter().map(|_| Gerber::new()).collect();
    let idx = |l: Layer| layers.iter().position(|x| *x == l);
    let mut f_mask = Gerber::new();
    let mut b_mask = Gerber::new();
    let mut f_paste = Gerber::new();
    let mut lost = 0;
    let nl = build_netlist(p);
    for z in &p.zones {
        match z.layer.and_then(idx) {
            Some(i) if z.drawn => {
                // Zone drawn in FerrisCad: pour it, then carve the clearance around every other net.
                let (zn, l, clr) = (nl.find(&z.net), z.layer.unwrap(), p.board.clearance);
                z.outlines.iter().for_each(|poly| copper[i].region(poly));
                copper[i].polarity(false);
                for pad in conn.pads.iter().filter(|pd| pd.on(l) && (zn.is_none() || pd.net != zn)) {
                    copper[i].pad(pad.pos, pad.size, pad.round, clr);
                }
                for (ti, t) in p.tracks.iter().enumerate().filter(|(ti, t)| t.layer == l && (zn.is_none() || conn.track_net(*ti) != zn)) {
                    let _ = ti;
                    copper[i].line(t.a, t.b, t.width + 2.0 * clr);
                }
                for (vi, v) in p.vias.iter().enumerate().filter(|(vi, _)| zn.is_none() || conn.via_net(*vi) != zn) {
                    let _ = vi;
                    copper[i].pad(v.pos, (v.size, v.size), true, clr);
                }
                copper[i].polarity(true);
                notes.push(format!("Zone on {} ({}) is poured with {} mm clearance around other nets; thermal reliefs aren't generated.", l.name(), z.net, clr));
            }
            Some(i) => z.polys.iter().for_each(|poly| copper[i].region(poly)),
            None => lost += 1,
        }
    }
    for t in &p.tracks {
        match idx(t.layer) {
            Some(i) => copper[i].line(t.a, t.b, t.width),
            None => lost += 1,
        }
    }
    if lost > 0 {
        notes.push(format!("{} track(s)/zone(s) sit on a layer this board doesn't have and were left out.", lost));
    }
    for pad in &conn.pads {
        let front = pad.tht || !pad.back;
        let back = pad.tht || pad.back;
        if pad.tht {
            // Through-hole pads have copper on the inner layers too.
            for (i, l) in layers.iter().enumerate() {
                if let Layer::Inner(_) = l {
                    copper[i].pad(pad.pos, pad.size, pad.round, 0.0);
                }
            }
        }
        if front {
            copper[0].pad(pad.pos, pad.size, pad.round, 0.0);
            f_mask.pad(pad.pos, pad.size, pad.round, 0.05);
            if !pad.tht {
                f_paste.pad(pad.pos, pad.size, pad.round, 0.0);
            }
        }
        if back {
            copper[layers.len() - 1].pad(pad.pos, pad.size, pad.round, 0.0);
            b_mask.pad(pad.pos, pad.size, pad.round, 0.05);
        }
    }
    for v in &p.vias {
        for g in copper.iter_mut() {
            g.pad(v.pos, (v.size, v.size), true, 0.0);
        }
    }
    // Silkscreen and outline.
    let mut f_silk = Gerber::new();
    for (a, b) in silk_lines(p) {
        f_silk.line(a, b, 0.15);
    }
    let mut b_silk = Gerber::new();
    // Part references above each footprint, as in the board view.
    for c in p.components.iter().filter(|c| c.has_fp()) {
        let bb = c.pcb_bbox();
        let t = BoardText { text: c.reference.clone(), pos: ((bb.0 + bb.2) / 2.0, bb.1 - 0.8), size: 1.0, rot: 0, layer: TextLayer::SilkFront };
        for s in crate::font::board_strokes(&t) {
            for w in s.windows(2) {
                f_silk.line(w[0], w[1], crate::font::stroke_width(t.size));
            }
        }
    }
    // Board texts on their layer.
    for t in &p.texts {
        let width = crate::font::stroke_width(t.size);
        let target: Option<&mut Gerber> = match t.layer {
            TextLayer::SilkFront => Some(&mut f_silk),
            TextLayer::SilkBack => Some(&mut b_silk),
            TextLayer::Copper(l) => idx(l).map(|i| &mut copper[i]),
        };
        if let Some(g) = target {
            for s in crate::font::board_strokes(t) {
                for w in s.windows(2) {
                    g.line(w[0], w[1], width);
                }
            }
        }
    }
    let mut edge = Gerber::new();
    let bd = &p.board;
    let k = [(bd.x0, bd.y0), (bd.x1, bd.y0), (bd.x1, bd.y1), (bd.x0, bd.y1)];
    for i in 0..4 {
        edge.line(k[i], k[(i + 1) % 4], 0.1);
    }
    // Drill (Excellon, plated holes).
    let mut holes: BTreeMap<String, Vec<(f32, f32)>> = BTreeMap::new();
    for pad in conn.pads.iter().filter(|x| x.tht) {
        holes.entry(format!("{:.3}", drill_of(pad, p))).or_default().push(pad.pos);
    }
    for v in &p.vias {
        holes.entry(format!("{:.3}", (v.size * 0.5).max(0.3))).or_default().push(v.pos);
    }
    let mut drl = String::from("M48\n; DRILL file from FerrisCad\nFMAT,2\nMETRIC\n");
    for (i, d) in holes.keys().enumerate() {
        let _ = writeln!(drl, "T{}C{}", i + 1, d);
    }
    drl.push_str("%\nG90\nG05\n");
    for (i, pts) in holes.values().enumerate() {
        let _ = writeln!(drl, "T{}", i + 1);
        for q in pts {
            let _ = writeln!(drl, "X{:.3}Y{:.3}", q.0, -q.1);
        }
    }
    drl.push_str("M30\n");

    let n = layers.len();
    let mut gerbers: Vec<(String, Vec<u8>)> = vec![];
    for (i, (l, g)) in layers.iter().zip(copper).enumerate() {
        // File names and extensions as KiCad writes them (JLCPCB recognises these).
        let (name, func) = match l {
            Layer::Front => (format!("{}-F_Cu.gtl", base), "Copper,L1,Top".to_string()),
            Layer::Back => (format!("{}-B_Cu.gbl", base), format!("Copper,L{},Bot", n)),
            Layer::Inner(k) => (format!("{}-In{}_Cu.g{}", base, k, k + 1), format!("Copper,L{},Inr", i + 1)),
        };
        gerbers.push((name, g.finish(&func).into_bytes()));
    }
    gerbers.extend(vec![
        (format!("{}-F_Mask.gts", base), f_mask.finish("Soldermask,Top").into_bytes()),
        (format!("{}-B_Mask.gbs", base), b_mask.finish("Soldermask,Bot").into_bytes()),
        (format!("{}-F_Paste.gtp", base), f_paste.finish("Paste,Top").into_bytes()),
        (format!("{}-F_Silkscreen.gto", base), f_silk.finish("Legend,Top").into_bytes()),
        (format!("{}-B_Silkscreen.gbo", base), b_silk.finish("Legend,Bot").into_bytes()),
        (format!("{}-Edge_Cuts.gm1", base), edge.finish("Profile,NP").into_bytes()),
        (format!("{}.drl", base), drl.into_bytes()),
    ]);
    if n > 2 {
        notes.push(format!("{}-layer board: choose {} layers on JLCPCB's order page; the layer order is F.Cu, In1.Cu … B.Cu.", n, n));
    }
    let zip_path = dir.join(format!("{}-gerbers.zip", base));
    std::fs::write(&zip_path, zip(&gerbers)).map_err(|e| e.to_string())?;

    // BOM (JLCPCB columns).
    let lines = bom(p);
    let mut b = String::from("Comment,Designator,Footprint,JLCPCB Part #\n");
    for l in &lines {
        let _ = writeln!(b, "{},{},{},{}", csv(&l.comment), csv(&l.refs.join(",")), csv(&l.footprint), csv(&l.lcsc));
    }
    let bom_path = dir.join(format!("{}-BOM.csv", base));
    std::fs::write(&bom_path, b).map_err(|e| e.to_string())?;
    let missing: usize = lines.iter().filter(|l| l.lcsc.is_empty()).map(|l| l.refs.len()).sum();
    if missing > 0 {
        notes.push(format!("{} part(s) have no JLCPCB part number: JLCPCB won't assemble them unless you pick one.", missing));
    }

    // Placement (CPL). Rotation counter-clockwise, Y up like the Gerbers.
    let mut cpl = String::from("Designator,Mid X,Mid Y,Layer,Rotation\n");
    let mut comps: Vec<&Component> = p.components.iter().filter(|c| c.has_fp()).collect();
    comps.sort_by(|a, b| natural_key(&a.reference).cmp(&natural_key(&b.reference)));
    for c in comps {
        let rot = (360 - c.pcb_rot as i32 * 90).rem_euclid(360);
        let _ = writeln!(cpl, "{},{:.4}mm,{:.4}mm,Top,{}", csv(&c.reference), c.pcb_pos.0, -c.pcb_pos.1, rot);
    }
    let cpl_path = dir.join(format!("{}-CPL.csv", base));
    std::fs::write(&cpl_path, cpl).map_err(|e| e.to_string())?;
    notes.push("Check part rotations in JLCPCB's assembly preview; some packages need a correction there.".into());

    Ok(Export { dir: dir.to_path_buf(), files: vec![zip_path, bom_path, cpl_path], notes })
}
