//! Applying the design changes the AI proposes (`[[ADD ...]]`, `[[CONNECT ...]]`, ...).
//! Each change is applied by the person with a button; the caller makes the undo checkpoint.

use crate::ai::AiAction;
use crate::app::App;
use crate::model::*;

fn kind_from_name(s: &str) -> Option<Kind> {
    let k = s.to_lowercase().replace(['-', ' '], "_");
    Some(match k.as_str() {
        "resistor" | "r" | "res" => Kind::Resistor,
        "capacitor" | "c" | "cap" => Kind::Capacitor,
        "polarized_capacitor" | "polarized_cap" | "cap_polarized" | "electrolytic" | "cp" => Kind::CapPolarized,
        "inductor" | "l" | "coil" => Kind::Inductor,
        "diode" | "d" => Kind::Diode,
        "led" => Kind::Led,
        "npn" | "transistor" => Kind::Npn,
        "pnp" => Kind::Pnp,
        "nmos" | "n_mosfet" | "mosfet" => Kind::Nmos,
        "opamp" | "op_amp" | "amplifier" => Kind::OpAmp,
        "mcu" | "microcontroller" => Kind::Mcu8,
        "regulator" | "ldo" | "voltage_regulator" => Kind::Regulator,
        "battery" | "cell" => Kind::Battery,
        "switch" | "button" | "push_button" => Kind::Switch,
        "connector" | "header" | "conn" => Kind::Connector2,
        "gnd" | "ground" => Kind::Gnd,
        "power" | "supply" | "rail" => Kind::Power,
        _ => return None,
    })
}

fn parse_xy(s: &str) -> Option<(f32, f32)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// "90", "180°", "1" (quarter turns) -> quarter turns.
fn parse_rot(s: Option<&str>) -> u8 {
    let Some(r) = s.and_then(|v| v.trim_end_matches('°').trim().parse::<i32>().ok()) else { return 0 };
    if r % 90 == 0 {
        (r / 90).rem_euclid(4) as u8
    } else {
        r.rem_euclid(4) as u8
    }
}

fn snap(v: f32) -> i32 {
    ((v / GRID as f32).round() as i32) * GRID
}

fn is_supply_name(n: &str) -> bool {
    let u = n.to_uppercase();
    n.starts_with('+') || n.starts_with('-') || is_ground_name(n) || ["VCC", "VDD", "VBAT", "VBUS", "VEE", "VSS"].contains(&u.as_str())
}

impl App {
    fn pin_of(&self, spec: &str) -> Result<(usize, usize), String> {
        let (r, pin) = spec.rsplit_once('.').ok_or(format!("\"{}\" isn't a pin (write REF.PIN, e.g. R1.2).", spec))?;
        let ci = self.project.find_ref(r).ok_or(format!("There is no part called {}.", r))?;
        let inf = self.project.components[ci].info();
        let pi = inf
            .pins
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(pin))
            .or_else(|| inf.pins.iter().position(|p| !p.pad.is_empty() && p.pad.eq_ignore_ascii_case(pin)))
            .or_else(|| {
                // Library parts keep their KiCad pin numbers.
                let c = &self.project.components[ci];
                c.custom.as_ref().and_then(|cu| cu.pins.iter().position(|p| p.number.eq_ignore_ascii_case(pin)))
            })
            .or_else(|| pin.parse::<usize>().ok().filter(|n| *n >= 1 && *n <= inf.pins.len()).map(|n| n - 1))
            .or_else(|| {
                // Tolerate "RST" for "/RST" and a unique start like "R" for "RESET".
                let clean = |s: &str| s.trim_start_matches(['/', '~', '!']).to_lowercase();
                let want = clean(pin);
                inf.pins.iter().position(|p| clean(&p.name) == want).or_else(|| {
                    let m: Vec<usize> = inf.pins.iter().enumerate().filter(|(_, p)| clean(&p.name).starts_with(&want)).map(|(i, _)| i).collect();
                    (m.len() == 1).then(|| m[0])
                })
            })
            .ok_or_else(|| {
                let names: Vec<&str> = inf.pins.iter().map(|p| p.name.as_str()).collect();
                format!("{} has no pin {} (its pins are {}).", r, pin, names.join(", "))
            })?;
        Ok((ci, pi))
    }

    fn pin_point(&self, ci: usize, pi: usize) -> (i32, i32) {
        let c = &self.project.components[ci];
        c.pin_pos(&c.info().pins[pi])
    }

    /// All pin points except those of the given pins.
    fn other_pin_points(&self, skip: &[(usize, usize)]) -> Vec<(i32, i32)> {
        let mut v = vec![];
        for (ci, c) in self.project.components.iter().enumerate() {
            for (pi, pd) in c.info().pins.iter().enumerate() {
                if !skip.contains(&(ci, pi)) {
                    v.push(c.pin_pos(pd));
                }
            }
        }
        v
    }

    /// A free spot for a new part, starting at `start` and moving right.
    pub(crate) fn free_spot(&self, start: (i32, i32), kind: Kind, rot: u8) -> (i32, i32) {
        self.free_spot_for(start, kind, rot, None)
    }

    /// Like `free_spot`, using the real outline of a downloaded part.
    pub(crate) fn free_spot_for(&self, start: (i32, i32), kind: Kind, rot: u8, custom: Option<&Custom>) -> (i32, i32) {
        let probe = |pos: (i32, i32)| Component {
            id: 0,
            kind,
            reference: String::new(),
            value: String::new(),
            pos,
            rot,
            pcb_pos: (0.0, 0.0),
            pcb_rot: 0,
            custom: custom.map(|c| Box::new(c.clone())),
            lcsc: String::new(),
            fp_over: None,
        };
        let mut pos = start;
        for _ in 0..60 {
            let b = probe(pos).sch_bbox();
            let clash = self.project.components.iter().any(|c| {
                c.sch_boxes().iter().any(|o| b.0 - 10 < o.2 && o.0 < b.2 + 10 && b.1 - 10 < o.3 && o.1 < b.3 + 10)
            });
            if !clash {
                return pos;
            }
            pos.0 += 40;
        }
        pos
    }

    pub(crate) fn content_right(&self) -> (i32, i32) {
        let mut right = 0;
        let mut top = i32::MAX;
        for c in &self.project.components {
            let b = c.sch_bbox();
            right = right.max(b.2);
            top = top.min(b.1);
        }
        (snap(right as f32 + 80.0), if top == i32::MAX { 100 } else { snap(top as f32 + 40.0) })
    }

    /// The end of a short wire leaving a pin away from its part's body.
    fn stub_end(&self, ci: usize, pi: usize) -> (i32, i32) {
        let c = &self.project.components[ci];
        let q = self.pin_point(ci, pi);
        let d = (q.0 - c.pos.0, q.1 - c.pos.1);
        let dir = if d.0 == 0 && d.1 == 0 {
            (0, 1)
        } else if d.0.abs() >= d.1.abs() {
            (d.0.signum(), 0)
        } else {
            (0, d.1.signum())
        };
        (q.0 + dir.0 * 2 * GRID, q.1 + dir.1 * 2 * GRID)
    }

    /// Put a supply symbol or a net label on a pin.
    fn attach_net(&mut self, ci: usize, pi: usize, name: &str) -> String {
        let q = self.pin_point(ci, pi);
        let end = self.stub_end(ci, pi);
        self.project.wires.push(Wire { a: q, b: end });
        let pin_name = format!("{}.{}", self.project.components[ci].reference, self.project.components[ci].info().pins[pi].name);
        if name.eq_ignore_ascii_case("GND") {
            self.project.add_component(Kind::Gnd, end, 0);
            format!("{} goes to GND", pin_name)
        } else if is_supply_name(name) || self.nl.nets.iter().any(|n| n.power.iter().any(|p| p == name)) {
            let id = self.project.add_component(Kind::Power, end, 0);
            if let Some(i) = self.project.find(id) {
                self.project.components[i].value = name.to_string();
            }
            format!("{} goes to {}", pin_name, name)
        } else {
            self.project.labels.push(Label { pos: end, text: name.to_string() });
            format!("{} is on net {}", pin_name, name)
        }
    }

    pub fn apply_ai_action(&mut self, a: &AiAction) -> Result<String, String> {
        match a.verb.as_str() {
            "SET" => {
                let ci = self.project.find_ref(a.arg(0)).ok_or(format!("There is no part called {}.", a.arg(0)))?;
                let mut done = vec![];
                let value = a.get("value").map(|s| s.to_string()).or_else(|| (!a.arg(1).is_empty()).then(|| a.arg(1).to_string()));
                if let Some(v) = value {
                    self.project.components[ci].value = v.clone();
                    done.push(format!("{} = {}", a.arg(0), v));
                }
                if let Some(code) = a.get("lcsc") {
                    self.project.components[ci].lcsc = code.to_string();
                    done.push(format!("{} JLCPCB part {}", a.arg(0), code));
                }
                if let Some(r) = a.get("ref") {
                    if self.project.find_ref(r).is_some() {
                        return Err(format!("{} already exists.", r));
                    }
                    self.project.components[ci].reference = r.to_string();
                    done.push(format!("renamed to {}", r));
                }
                if done.is_empty() {
                    return Err("Nothing to change (expected value=...).".into());
                }
                Ok(done.join(", "))
            }
            "ADD" if a.get("kicad").is_some() || a.arg(0).contains(':') => {
                let id = a.get("kicad").unwrap_or(a.arg(0)).to_string();
                let (lib, name) = id.split_once(':').ok_or("kicad= needs Library:Symbol")?;
                let part = crate::online::fetch_part(lib, name)?;
                let notes = part.notes.clone();
                let start = if let Some(xy) = a.get("at").and_then(parse_xy) {
                    (snap(xy.0), snap(xy.1))
                } else if let Some(ci) = a.get("near").and_then(|r| self.project.find_ref(r)) {
                    let b = self.project.components[ci].sch_bbox();
                    (snap(b.2 as f32 + 60.0), snap(self.project.components[ci].pos.1 as f32))
                } else {
                    self.content_right()
                };
                let pos = self.free_spot_for(start, part.kind, 0, Some(&part.custom));
                let id = self.project.add_part(part, pos);
                let ci = self.project.find(id).unwrap();
                if let Some(v) = a.get("value") {
                    self.project.components[ci].value = v.to_string();
                }
                if let Some(r) = a.get("ref") {
                    if self.project.find_ref(r).is_none() {
                        self.project.components[ci].reference = r.to_string();
                    }
                }
                let c = &self.project.components[ci];
                let mut msg = format!("added {} ({} from the KiCad library)", c.reference, name);
                if !notes.is_empty() {
                    msg.push_str(&format!(" — {}", notes.join(" ")));
                }
                Ok(msg)
            }
            "ADD" => {
                let kind = kind_from_name(a.arg(0)).ok_or(format!("Unknown part kind \"{}\".", a.arg(0)))?;
                let rot = parse_rot(a.get("rot"));
                let pos = if let Some(xy) = a.get("at").and_then(parse_xy).or_else(|| parse_xy(a.arg(1))) {
                    self.free_spot((snap(xy.0), snap(xy.1)), kind, rot)
                } else if let Some(ci) = a.get("near").and_then(|r| self.project.find_ref(r)) {
                    let c = &self.project.components[ci];
                    let b = c.sch_bbox();
                    self.free_spot((snap(b.2 as f32 + 50.0), snap(c.pos.1 as f32)), kind, rot)
                } else {
                    let p = self.content_right();
                    self.free_spot(p, kind, rot)
                };
                let id = self.project.add_component(kind, pos, rot);
                let ci = self.project.find(id).unwrap();
                if let Some(v) = a.get("value") {
                    self.project.components[ci].value = v.to_string();
                }
                if let Some(r) = a.get("ref") {
                    if self.project.find_ref(r).is_none() {
                        self.project.components[ci].reference = r.to_string();
                    }
                }
                let c = &self.project.components[ci];
                Ok(format!("added {} ({} {})", c.reference, c.info().label.to_lowercase(), c.value))
            }
            "CONNECT" => {
                let (ca, pa) = self.pin_of(a.arg(0))?;
                let target = a.arg(1);
                if target.is_empty() {
                    return Err("CONNECT needs two pins, or a pin and a net.".into());
                }
                if let Ok((cb, pb)) = self.pin_of(target) {
                    if self.nl.net_of(ca, pa).is_some() && self.nl.net_of(ca, pa) == self.nl.net_of(cb, pb) {
                        return Ok(format!("{} and {} were already connected", a.arg(0), target));
                    }
                    let (p, q) = (self.pin_point(ca, pa), self.pin_point(cb, pb));
                    let others = self.other_pin_points(&[(ca, pa), (cb, pb)]);
                    let crosses = |pts: &[(i32, i32)]| {
                        pts.windows(2).any(|s| others.iter().any(|o| on_segment(*o, s[0], s[1])))
                    };
                    let routes = [vec![p, (q.0, p.1), q], vec![p, (p.0, q.1), q]];
                    if let Some(r) = routes.iter().find(|r| !crosses(r)) {
                        for s in r.windows(2) {
                            if s[0] != s[1] {
                                self.project.wires.push(Wire { a: s[0], b: s[1] });
                            }
                        }
                        return Ok(format!("wired {} to {}", a.arg(0), target));
                    }
                    // No clean path: join them with the same net label instead.
                    let named = |ci: usize, pi: usize| {
                        self.nl.net_of(ci, pi).map(|n| self.nl.nets[n].name.clone()).filter(|n| !n.starts_with("Net-("))
                    };
                    let name = a
                        .get("net")
                        .map(|s| s.to_string())
                        .or_else(|| named(ca, pa))
                        .or_else(|| named(cb, pb))
                        .unwrap_or_else(|| format!("N_{}", a.arg(0).replace('.', "_")));
                    self.attach_net(ca, pa, &name);
                    self.attach_net(cb, pb, &name);
                    return Ok(format!("connected {} to {} with net label {}", a.arg(0), target, name));
                }
                Ok(self.attach_net(ca, pa, target))
            }
            "DELETE" => {
                let ci = self.project.find_ref(a.arg(0)).ok_or(format!("There is no part called {}.", a.arg(0)))?;
                let c = self.project.components.remove(ci);
                Ok(format!("removed {}", c.reference))
            }
            "MOVE" => {
                let ci = self.project.find_ref(a.arg(0)).ok_or(format!("There is no part called {}.", a.arg(0)))?;
                let xy = parse_xy(a.arg(1)).or_else(|| a.get("at").and_then(parse_xy)).ok_or("MOVE needs x,y.")?;
                let old: Vec<(i32, i32)> = {
                    let c = &self.project.components[ci];
                    c.info().pins.iter().map(|p| c.pin_pos(p)).collect()
                };
                let marks = self.nc_marks(&[self.project.components[ci].id].into());
                self.project.components[ci].pos = (snap(xy.0), snap(xy.1));
                self.place_nc_marks(&marks);
                let c = self.project.components[ci].clone();
                let new: Vec<(i32, i32)> = c.info().pins.iter().map(|p| c.pin_pos(p)).collect();
                for w in self.project.wires.iter_mut() {
                    for (o, n) in old.iter().zip(new.iter()) {
                        if w.a == *o {
                            w.a = *n;
                        }
                        if w.b == *o {
                            w.b = *n;
                        }
                    }
                }
                Ok(format!("moved {} on the schematic", c.reference))
            }
            "ROTATE" => {
                let ci = self.project.find_ref(a.arg(0)).ok_or(format!("There is no part called {}.", a.arg(0)))?;
                let marks = self.nc_marks(&[self.project.components[ci].id].into());
                self.project.components[ci].rot = (self.project.components[ci].rot + 1) % 4;
                self.place_nc_marks(&marks);
                let c = &self.project.components[ci];
                Ok(format!("rotated {}", c.reference))
            }
            "PLACE" => {
                let ci = self.project.find_ref(a.arg(0)).ok_or(format!("There is no part called {}.", a.arg(0)))?;
                if !self.project.components[ci].has_fp() {
                    return Err(format!("{} has no footprint to place.", a.arg(0)));
                }
                let xy = parse_xy(a.arg(1)).or_else(|| a.get("at").and_then(parse_xy)).ok_or("PLACE needs x,y in mm.")?;
                let c = &mut self.project.components[ci];
                c.pcb_pos = (snap_f(xy.0, 0.05), snap_f(xy.1, 0.05));
                if a.get("rot").is_some() {
                    c.pcb_rot = parse_rot(a.get("rot"));
                }
                Ok(format!("placed {} on the board at {:.2}, {:.2} mm", c.reference, c.pcb_pos.0, c.pcb_pos.1))
            }
            "ROUTE" => {
                let (ca, pa) = self.pin_of(a.arg(0))?;
                let (cb, pb) = self.pin_of(a.arg(1))?;
                let pad = |ci: usize, pi: usize| -> Option<(f32, f32)> {
                    let c = &self.project.components[ci];
                    let inf = c.info();
                    let pd = &inf.pins[pi];
                    (!pd.pad.is_empty()).then(|| c.pad_pos(pd))
                };
                let p = pad(ca, pa).ok_or(format!("{} has no pad on the board.", a.arg(0)))?;
                let q = pad(cb, pb).ok_or(format!("{} has no pad on the board.", a.arg(1)))?;
                let layer = Layer::parse(a.get("layer").unwrap_or("F")).ok_or(format!("Unknown layer {}.", a.get("layer").unwrap_or("")))?;
                if !self.project.board.has(layer) {
                    return Err(format!("This board has only {} copper layers.", self.project.board.layers));
                }
                let width = a.get("width").and_then(|w| w.trim_end_matches("mm").parse::<f32>().ok()).unwrap_or(self.track_width).clamp(0.1, 5.0);
                let mut prev = p;
                for pt in crate::pcb::route45(p, q) {
                    self.project.tracks.push(Track { a: prev, b: pt, width, layer });
                    prev = pt;
                }
                Ok(format!("routed {} to {} on {}", a.arg(0), a.arg(1), layer.name().to_lowercase()))
            }
            "LAYERS" => {
                let n: u8 = a.arg(0).parse().map_err(|_| "LAYERS needs a number (2, 4, 6...).".to_string())?;
                if !(2..=16).contains(&n) || n % 2 != 0 {
                    return Err("Use an even number of layers from 2 to 16.".into());
                }
                self.project.board.layers = n;
                Ok(format!("board set to {} copper layers", n))
            }
            "AUTOPLACE" => {
                self.project.auto_place();
                self.fit_pcb = true;
                Ok("arranged the parts on the board".into())
            }
            v => Err(format!("Unknown change {}.", v)),
        }
    }
}
