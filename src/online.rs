//! Online parts: search the official KiCad libraries (gitlab.com/kicad/libraries, CC-BY-SA 4.0
//! with the design exception) and download a part's symbol and footprint. Everything
//! downloaded is cached in ~/.cache/ferriscad/kicad-lib, so a part is fetched only once.
//!
//! Layout of the libraries: `kicad-symbols/<Lib>.kicad_symdir/<Symbol>.kicad_sym` (one file per
//! symbol) and `kicad-footprints/<Lib>.pretty/<Footprint>.kicad_mod`.

use crate::kicad::{self, Sx};
use crate::model::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

const API: &str = "https://gitlab.com/api/v4/projects/";
const SYM_PROJECT: &str = "kicad%2Flibraries%2Fkicad-symbols";
const FP_PROJECT: &str = "kicad%2Flibraries%2Fkicad-footprints";
const SYM_RAW: &str = "https://gitlab.com/kicad/libraries/kicad-symbols/-/raw/master/";
const FP_RAW: &str = "https://gitlab.com/kicad/libraries/kicad-footprints/-/raw/master/";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hit {
    pub lib: String,
    pub name: String,
    #[serde(default)]
    pub score: i32,
}

impl Hit {
    pub fn id(&self) -> String {
        format!("{}:{}", self.lib, self.name)
    }
}

/// A downloaded part, ready to be added to the project.
pub struct Part {
    pub kind: Kind,
    pub custom: Custom,
    pub value: String,
    pub prefix: String,
    pub description: String,
    pub notes: Vec<String>,
}

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("ferriscad")
        .join("kicad-lib")
}

fn get(url: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(60)).build();
    match agent.get(url).set("user-agent", "FerrisCad").call() {
        Ok(r) => r.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(404, _)) => Err("not found in the KiCad library".into()),
        Err(ureq::Error::Status(c, _)) => Err(format!("the library server answered {}", c)),
        Err(e) => Err(format!("no connection to the KiCad library ({})", e)),
    }
}

fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => o.push(b as char),
            _ => o.push_str(&format!("%{:02X}", b)),
        }
    }
    o
}

fn cache_path(rel: &str) -> PathBuf {
    cache_dir().join(rel.replace(':', "_"))
}

/// Download once, then read from the cache.
fn cached(rel: &str, url: &str) -> Result<String, String> {
    let path = cache_path(rel);
    if let Ok(t) = std::fs::read_to_string(&path) {
        return Ok(t);
    }
    let t = get(url)?;
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&path, &t);
    Ok(t)
}

/// Names in a repository folder (paginated GitLab tree), cached for a week.
fn tree(project: &str, path: &str, cache_rel: &str) -> Result<Vec<(String, bool)>, String> {
    let cp = cache_path(cache_rel);
    let fresh = std::fs::metadata(&cp)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().map_or(false, |e| e.as_secs() < 7 * 86400))
        .unwrap_or(false);
    if fresh {
        if let Some(v) = std::fs::read_to_string(&cp).ok().and_then(|t| serde_json::from_str(&t).ok()) {
            return Ok(v);
        }
    }
    let mut out: Vec<(String, bool)> = vec![];
    for page in 1..=30 {
        let mut url = format!("{}{}/repository/tree?per_page=100&page={}", API, project, page);
        if !path.is_empty() {
            url.push_str(&format!("&path={}", enc(path)));
        }
        let t = get(&url)?;
        let v: serde_json::Value = serde_json::from_str(&t).map_err(|e| e.to_string())?;
        let items = v.as_array().cloned().unwrap_or_default();
        for it in &items {
            if let Some(n) = it["name"].as_str() {
                out.push((n.to_string(), it["type"] == "tree"));
            }
        }
        if items.len() < 100 {
            break;
        }
    }
    if out.is_empty() {
        return Err("the KiCad library list came back empty".into());
    }
    if let Some(d) = cp.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&cp, serde_json::to_string(&out).unwrap_or_default());
    Ok(out)
}

/// Names of all symbol libraries.
pub fn libraries() -> Result<Vec<String>, String> {
    Ok(tree(SYM_PROJECT, "", "symbols/_libraries.json")?
        .into_iter()
        .filter_map(|(n, _)| n.strip_suffix(".kicad_symdir").map(String::from))
        .collect())
}

/// Names of the symbols in one library.
fn symbols_in(lib: &str) -> Result<Vec<String>, String> {
    Ok(tree(SYM_PROJECT, &format!("{}.kicad_symdir", lib), &format!("symbols/{}/_index.json", lib))?
        .into_iter()
        .filter_map(|(n, _)| n.strip_suffix(".kicad_sym").map(String::from))
        .collect())
}

fn symbol_text(lib: &str, name: &str) -> Result<String, String> {
    cached(
        &format!("symbols/{}/{}.kicad_sym", lib, name),
        &format!("{}{}.kicad_symdir/{}.kicad_sym", SYM_RAW, enc(lib), enc(name)),
    )
}

/// Common part families and the libraries they live in.
const HINTS: &[(&str, &str)] = &[
    ("78", "Regulator_Linear"),
    ("79", "Regulator_Linear"),
    ("ldo", "Regulator_Linear"),
    ("1117", "Regulator_Linear"),
    ("lm317", "Regulator_Linear"),
    ("regulator", "Regulator_Linear"),
    ("buck", "Regulator_Switching"),
    ("boost", "Regulator_Switching"),
    ("lm259", "Regulator_Switching"),
    ("mp15", "Regulator_Switching"),
    ("opamp", "Amplifier_Operational"),
    ("lm358", "Amplifier_Operational"),
    ("lm324", "Amplifier_Operational"),
    ("tl07", "Amplifier_Operational"),
    ("ne5532", "Amplifier_Operational"),
    ("mcp60", "Amplifier_Operational"),
    ("555", "Timer"),
    ("atmega", "MCU_Microchip_ATmega"),
    ("attiny", "MCU_Microchip_ATtiny"),
    ("pic", "MCU_Microchip_PIC"),
    ("stm32", "MCU_ST_STM32"),
    ("esp32", "RF_Module"),
    ("esp8266", "RF_Module"),
    ("nrf5", "MCU_Nordic"),
    ("rp2040", "MCU_RaspberryPi"),
    ("ch340", "Interface_USB"),
    ("cp210", "Interface_USB"),
    ("ft232", "Interface_USB"),
    ("max232", "Interface_UART"),
    ("max485", "Interface_UART"),
    ("usb", "Connector"),
    ("led", "LED"),
    ("ws2812", "LED"),
    ("2n3904", "Transistor_BJT"),
    ("2n3906", "Transistor_BJT"),
    ("2n2222", "Transistor_BJT"),
    ("bc54", "Transistor_BJT"),
    ("bc55", "Transistor_BJT"),
    ("bc33", "Transistor_BJT"),
    ("tip1", "Transistor_BJT"),
    ("irf", "Transistor_FET"),
    ("2n7002", "Transistor_FET"),
    ("bss1", "Transistor_FET"),
    ("ao34", "Transistor_FET"),
    ("mosfet", "Transistor_FET"),
    ("1n4", "Diode"),
    ("1n5", "Diode"),
    ("schottky", "Diode"),
    ("zener", "Diode"),
    ("74hc", "74xx"),
    ("74ls", "74xx"),
    ("cd40", "4xxx"),
    ("pc817", "Isolator"),
    ("4n3", "Isolator"),
    ("opto", "Isolator"),
    ("relay", "Relay"),
    ("bme", "Sensor"),
    ("bmp", "Sensor_Pressure"),
    ("dht", "Sensor"),
    ("lm35", "Sensor_Temperature"),
    ("ds18b20", "Sensor_Temperature"),
    ("24c", "Memory_EEPROM"),
    ("w25q", "Memory_Flash"),
    ("max7219", "Driver_LED"),
    ("l298", "Driver_Motor"),
    ("drv8", "Driver_Motor"),
    ("a4988", "Driver_Motor"),
    ("uln200", "Transistor_Array"),
    ("crystal", "Device"),
    ("resistor", "Device"),
    ("capacitor", "Device"),
    ("inductor", "Device"),
    ("fuse", "Device"),
    ("buzzer", "Device"),
    ("header", "Connector_Generic"),
    ("conn", "Connector_Generic"),
    ("terminal", "Connector"),
    ("battery", "Device"),
    ("switch", "Switch"),
    ("button", "Switch"),
];

/// Longest run of characters of `t` found in `s`.
fn common_len(t: &str, s: &str) -> usize {
    let mut best = 0;
    for i in 0..t.len() {
        for j in (i + best + 1)..=t.len() {
            if s.contains(&t[i..j]) {
                best = j - i;
            } else {
                break;
            }
        }
    }
    best
}

fn candidates(query: &str, libs: &[String]) -> Vec<String> {
    let q = query.to_lowercase();
    let toks: Vec<&str> = q.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| t.len() >= 2).collect();
    let mut score: HashMap<&str, i32> = HashMap::new();
    for l in libs {
        let ll = l.to_lowercase();
        for t in &toks {
            if t.len() >= 3 && ll.contains(t) {
                *score.entry(l).or_default() += 10;
            }
        }
        for (key, prefix) in HINTS {
            if toks.iter().any(|t| t.contains(key)) && ll.starts_with(&prefix.to_lowercase()) {
                // Prefer the most specific library, e.g. MCU_ST_STM32F1 for "stm32f103".
                let common = toks.iter().map(|t| common_len(t, &ll)).max().unwrap_or(0) as i32;
                *score.entry(l).or_default() += 8 + common;
            }
        }
    }
    let mut v: Vec<(&str, i32)> = score.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let mut out: Vec<String> = v.into_iter().take(5).map(|(l, _)| l.to_string()).collect();
    if !out.iter().any(|l| l == "Device") {
        out.push("Device".into());
    }
    // Libraries already listed cost nothing to search again.
    if let Ok(rd) = std::fs::read_dir(cache_dir().join("symbols")) {
        for e in rd.flatten() {
            if e.path().join("_index.json").is_file() {
                let n = e.file_name().to_string_lossy().to_string();
                if !out.contains(&n) {
                    out.push(n);
                }
            }
        }
    }
    out
}

/// Search the official KiCad symbol libraries by part name.
/// "Library:Name" and plain library names work too.
pub fn search(query: &str) -> Result<Vec<Hit>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let libs = libraries()?;
    if let Some((lib, name)) = q.split_once(':') {
        let n = name.to_lowercase();
        return Ok(symbols_in(lib)?
            .into_iter()
            .filter(|s| s.to_lowercase().contains(&n))
            .take(60)
            .map(|name| Hit { lib: lib.to_string(), name, score: 0 })
            .collect());
    }
    if let Some(lib) = libs.iter().find(|l| l.eq_ignore_ascii_case(q)) {
        return Ok(symbols_in(lib)?.into_iter().take(80).map(|name| Hit { lib: lib.clone(), name, score: 0 }).collect());
    }
    let ql: String = q.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let mut hits = vec![];
    let mut errors = vec![];
    for lib in candidates(q, &libs) {
        let names = match symbols_in(&lib) {
            Ok(n) => n,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        for name in names {
            let nl: String = name.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            let s = if nl == ql {
                100
            } else if nl.starts_with(&ql) {
                70
            } else if nl.contains(&ql) {
                50
            } else if ql.len() >= 4 && ql.contains(&nl) && nl.len() >= 4 {
                30
            } else {
                0
            };
            if s > 0 {
                hits.push(Hit { lib: lib.clone(), name, score: s });
            }
        }
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.name.len().cmp(&b.name.len())).then(a.name.cmp(&b.name)));
    hits.dedup_by(|a, b| a.lib == b.lib && a.name == b.name);
    hits.truncate(40);
    if hits.is_empty() && !errors.is_empty() {
        return Err(errors.remove(0));
    }
    Ok(hits)
}

fn prop_text(s: &Sx, name: &str) -> String {
    kicad::prop(s, name).map(|p| p.arg(2).to_string()).unwrap_or_default()
}

/// Footprint libraries for footprint-filter patterns such as "TO?220*" or "SOIC*3.9x4.9mm*".
const FP_HINTS: &[(&str, &[&str])] = &[
    ("TO?220", &["Package_TO_SOT_THT"]),
    ("TO?92", &["Package_TO_SOT_THT"]),
    ("TO?247", &["Package_TO_SOT_THT"]),
    ("TO?252", &["Package_TO_SOT_SMD"]),
    ("TO?263", &["Package_TO_SOT_SMD"]),
    ("SOT", &["Package_TO_SOT_SMD"]),
    ("SOIC", &["Package_SO"]),
    ("SO*", &["Package_SO"]),
    ("SOP", &["Package_SO"]),
    ("SSOP", &["Package_SO"]),
    ("MSOP", &["Package_SO"]),
    ("DIP", &["Package_DIP"]),
    ("QFN", &["Package_DFN_QFN"]),
    ("DFN", &["Package_DFN_QFN"]),
    ("QFP", &["Package_QFP"]),
    ("R_", &["Resistor_SMD", "Resistor_THT"]),
    ("CP_", &["Capacitor_THT", "Capacitor_SMD"]),
    ("C_", &["Capacitor_SMD", "Capacitor_THT"]),
    ("L_", &["Inductor_SMD", "Inductor_THT"]),
    ("LED", &["LED_SMD", "LED_THT"]),
    ("D_", &["Diode_SMD", "Diode_THT"]),
    ("PinHeader", &["Connector_PinHeader_2.54mm"]),
    ("Crystal", &["Crystal"]),
    ("SW", &["Button_Switch_THT", "Button_Switch_SMD"]),
    ("ESP", &["RF_Module"]),
];

/// `?` = one character, `*` = anything; case-insensitive.
fn glob(pat: &str, s: &str) -> bool {
    let (p, s): (Vec<char>, Vec<char>) = (pat.to_lowercase().chars().collect(), s.to_lowercase().chars().collect());
    fn m(p: &[char], s: &[char]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some('*') => (0..=s.len()).any(|i| m(&p[1..], &s[i..])),
            Some('?') => !s.is_empty() && m(&p[1..], &s[1..]),
            Some(c) => s.first() == Some(c) && m(&p[1..], &s[1..]),
        }
    }
    m(&p, &s)
}

/// Pick a footprint matching the symbol's footprint filters.
fn footprint_from_filters(filters: &str) -> Option<String> {
    for pat in filters.split_whitespace() {
        let upper = pat.to_uppercase();
        let libs: Vec<&str> = FP_HINTS
            .iter()
            .filter(|(k, _)| upper.starts_with(&k.to_uppercase().replace('*', "")) || glob(&format!("{}*", k), pat))
            .flat_map(|(_, l)| l.iter().copied())
            .collect();
        for lib in libs {
            let Ok(items) = tree(FP_PROJECT, &format!("{}.pretty", lib), &format!("footprints/{}.pretty/_index.json", lib)) else { continue };
            let mut names: Vec<String> = items.into_iter().filter_map(|(n, _)| n.strip_suffix(".kicad_mod").map(String::from)).collect();
            names.sort();
            if let Some(n) = names.iter().filter(|n| glob(pat, n)).min_by_key(|n| n.len()) {
                return Some(format!("{}:{}", lib, n));
            }
        }
    }
    None
}

/// Download a part's symbol (and footprint) and turn it into a FerrisCad part.
pub fn fetch_part(lib: &str, name: &str) -> Result<Part, String> {
    let text = symbol_text(lib, name).map_err(|e| {
        let close: Vec<String> = search(&format!("{}:{}", lib, name)).unwrap_or_default().into_iter().take(5).map(|h| h.name).collect();
        if close.is_empty() {
            format!("{}:{} — {}", lib, name, e)
        } else {
            format!("{}:{} — {} (close matches: {})", lib, name, e, close.join(", "))
        }
    })?;
    let root = kicad::parse_sx(&text)?;
    let sym = root.all("symbol").find(|s| s.arg(1) == name).or_else(|| root.all("symbol").next()).ok_or("empty symbol file")?;
    // Derived symbols (extends) take their drawing from another symbol of the same library.
    let base_root = match sym.get("extends") {
        Some(e) => Some(kicad::parse_sx(&symbol_text(lib, e.arg(1))?)?),
        None => None,
    };
    let base = base_root.as_ref().and_then(|r| r.all("symbol").next()).unwrap_or(sym);
    let pick = |n: &str| {
        let t = prop_text(sym, n);
        if t.is_empty() {
            prop_text(base, n)
        } else {
            t
        }
    };
    let is_power = base.get("power").is_some() || sym.get("power").is_some();
    let show_names = base.get("pin_names").map_or(true, |pn| !pn.has_atom("hide") && pn.get("hide").map_or(true, |h| h.arg(1) == "no"));
    let mut units: Vec<i32> = base.all("symbol").map(|s| kicad::sub_unit(s.arg(1)).0).filter(|u| *u >= 1).collect();
    units.sort();
    units.dedup();
    if units.is_empty() {
        units.push(1);
    }
    let mut cu = Custom { lib: format!("{}:{}", lib, name), footprint: pick("Footprint"), ..Default::default() };
    let flip = |q: (f32, f32)| (q.0 * kicad::U, -q.1 * kicad::U);
    let mut x_next = 0.0f32;
    for (i, &u) in units.iter().enumerate() {
        let mut parts: Vec<&Sx> = vec![base];
        parts.extend(base.all("symbol").filter(|s| {
            let (su, st) = kicad::sub_unit(s.arg(1));
            ((su == 0 && i == 0) || su == u) && st <= 1
        }));
        // Draw the unit once to measure it, then again shifted right of the previous unit.
        let mut tmp = Custom::default();
        let bb = kicad::symbol_items(&mut tmp, &parts, is_power, show_names, &flip);
        if bb.0 > bb.2 {
            continue;
        }
        let dx = if i == 0 { 0.0 } else { ((x_next - bb.0) / GRID as f32).ceil() * GRID as f32 };
        let mut moved = Custom::default();
        kicad::symbol_items(&mut moved, &parts, is_power, show_names, &|q| {
            let f = flip(q);
            (f.0 + dx, f.1)
        });
        cu.pins.extend(moved.pins);
        cu.polys.extend(moved.polys);
        cu.circles.extend(moved.circles);
        cu.texts.extend(moved.texts);
        cu.boxes.push(((bb.0 + dx).floor() as i32, bb.1.floor() as i32, (bb.2 + dx).ceil() as i32, bb.3.ceil() as i32));
        x_next = bb.2 + dx + 40.0;
    }
    if cu.pins.is_empty() && !is_power {
        return Err(format!("{} has no pins in the library", name));
    }
    let at = |n: &str| {
        kicad::prop(sym, n)
            .or_else(|| kicad::prop(base, n))
            .filter(|p| !p.hidden())
            .and_then(|p| p.get("at"))
            .map(|a| flip((a.num(1), a.num(2))))
    };
    cu.ref_pos = at("Reference");
    cu.val_pos = at("Value");
    let value = {
        let v = pick("Value");
        if v.is_empty() {
            name.to_string()
        } else {
            v
        }
    };
    let prefix = {
        let r = pick("Reference");
        let r = r.trim_end_matches('?').to_string();
        if r.is_empty() {
            "U".to_string()
        } else {
            r
        }
    };
    let description = pick("Description");
    let (kind, order) = kicad::map_kind(&cu.lib, &name.to_lowercase(), &value, is_power, &cu.pins);
    if let Some(order) = order {
        let base_info = info(kind);
        let pins = std::mem::take(&mut cu.pins);
        cu.pins = order
            .iter()
            .enumerate()
            .map(|(i, &k)| {
                let mut pn = pins[k].clone();
                if let Some(b) = base_info.pins.get(i) {
                    pn.name = b.name.clone();
                }
                pn
            })
            .collect();
    }
    let mut notes = vec![];
    // Simple parts recognised by FerrisCad already have a standard footprint.
    if cu.footprint.is_empty() && !is_power && (kind == Kind::Generic || kind == Kind::Regulator) {
        if let Some(f) = footprint_from_filters(&pick("ki_fp_filters")) {
            cu.footprint = f;
        }
    }
    if let Some((flib, fname)) = cu.footprint.clone().split_once(':') {
        let url = format!("{}{}.pretty/{}.kicad_mod", FP_RAW, enc(flib), enc(fname));
        match cached(&format!("footprints/{}.pretty/{}.kicad_mod", flib, fname), &url).and_then(|t| kicad::parse_sx(&t)) {
            Ok(fp) => {
                let (pads, _, silk, circles) = kicad::footprint_geometry(&fp, 0.0, &|_| String::new());
                cu.pads = pads;
                cu.silk = silk;
                cu.silk_circles = circles;
            }
            Err(e) => notes.push(format!("Footprint {} couldn't be downloaded ({}).", cu.footprint, e)),
        }
    } else if !is_power && kind == Kind::Generic {
        notes.push("No footprint was found for this part; it won't appear on the board until one is set.".into());
    }
    Ok(Part { kind, custom: cu, value, prefix, description, notes })
}

impl Project {
    /// Add a downloaded part to the schematic (its footprint is parked beside the board).
    pub fn add_part(&mut self, part: Part, pos: (i32, i32)) -> u32 {
        let id = self.add_component(part.kind, pos, 0);
        let reference = self.next_ref(&part.prefix);
        let ci = self.find(id).unwrap();
        let c = &mut self.components[ci];
        if !part.kind.is_power() {
            c.reference = reference;
        }
        c.value = if part.kind == Kind::Gnd { "GND".into() } else { part.value };
        c.custom = Some(Box::new(part.custom));
        id
    }
}
