//! Circuit data: parts library, schematic items, board items and the netlist.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Schematic grid step, in schematic units.
pub const GRID: i32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Resistor,
    Capacitor,
    CapPolarized,
    Inductor,
    Diode,
    Led,
    Npn,
    Pnp,
    Nmos,
    OpAmp,
    Mcu8,
    Regulator,
    Battery,
    Connector2,
    Switch,
    Gnd,
    Power,
    /// A part imported from another tool, drawn with its own symbol.
    Generic,
}

impl Kind {
    pub fn is_power(self) -> bool {
        matches!(self, Kind::Gnd | Kind::Power)
    }
    pub fn has_footprint(self) -> bool {
        !self.is_power()
    }
}

pub const LIBRARY: &[(&str, &[Kind])] = &[
    ("Passives", &[Kind::Resistor, Kind::Capacitor, Kind::CapPolarized, Kind::Inductor]),
    ("Semiconductors", &[Kind::Diode, Kind::Led, Kind::Npn, Kind::Pnp, Kind::Nmos]),
    ("Chips", &[Kind::OpAmp, Kind::Mcu8, Kind::Regulator]),
    ("Power & connectors", &[Kind::Battery, Kind::Switch, Kind::Connector2, Kind::Gnd, Kind::Power]),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum PinType {
    NotConnected,
    Passive,
    Input,
    Output,
    Bidir,
    PowerIn,
    PowerOut,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PadShape {
    Rect,
    Round,
}

#[derive(Clone, Debug)]
pub struct PinDef {
    pub name: String,
    pub pos: (i32, i32),
    pub ptype: PinType,
    pub pad: String,
    pub pad_pos: (f32, f32),
    pub pad_size: (f32, f32),
    pub shape: PadShape,
    pub tht: bool,
    /// SMD pad on the back side.
    pub back: bool,
}

pub struct KindInfo {
    pub label: &'static str,
    pub prefix: &'static str,
    pub default_value: &'static str,
    pub footprint: String,
    pub pins: Vec<PinDef>,
    /// Local schematic body box (x0, y0, x1, y1).
    pub body: (i32, i32, i32, i32),
    /// Local silkscreen outline box in mm.
    pub fp_body: (f32, f32, f32, f32),
    pub fp_round: bool,
}

#[allow(clippy::too_many_arguments)]
fn pin(
    name: &'static str,
    pos: (i32, i32),
    ptype: PinType,
    pad: &'static str,
    pad_pos: (f32, f32),
    pad_size: (f32, f32),
    shape: PadShape,
    tht: bool,
) -> PinDef {
    PinDef { name: name.into(), pos, ptype, pad: pad.into(), pad_pos, pad_size, shape, tht, back: false }
}

fn ppin(name: &'static str) -> PinDef {
    pin(name, (0, 0), PinType::PowerOut, "", (0.0, 0.0), (0.0, 0.0), PadShape::Round, false)
}

use PadShape::{Rect as PR, Round as PO};
use PinType::*;

pub fn info(k: Kind) -> KindInfo {
    let smd2 = |a: &'static str, b: &'static str, dx: f32, w: f32, h: f32, ta: PinType, tb: PinType| {
        vec![
            pin(a, (0, -30), ta, "1", (-dx, 0.0), (w, h), PR, false),
            pin(b, (0, 30), tb, "2", (dx, 0.0), (w, h), PR, false),
        ]
    };
    match k {
        Kind::Resistor => KindInfo {
            label: "Resistor",
            prefix: "R",
            default_value: "10k",
            footprint: "R_0805".into(),
            pins: smd2("1", "2", 0.95, 1.0, 1.3, Passive, Passive),
            body: (-8, -20, 8, 20),
            fp_body: (-1.9, -1.0, 1.9, 1.0),
            fp_round: false,
        },
        Kind::Capacitor => KindInfo {
            label: "Capacitor",
            prefix: "C",
            default_value: "100n",
            footprint: "C_0805".into(),
            pins: smd2("1", "2", 0.95, 1.0, 1.3, Passive, Passive),
            body: (-14, -8, 14, 8),
            fp_body: (-1.9, -1.0, 1.9, 1.0),
            fp_round: false,
        },
        Kind::CapPolarized => KindInfo {
            label: "Polarized capacitor",
            prefix: "C",
            default_value: "10u",
            footprint: "CP_Radial_D5mm".into(),
            pins: vec![
                pin("+", (0, -30), Passive, "1", (-1.25, 0.0), (1.6, 1.6), PR, true),
                pin("-", (0, 30), Passive, "2", (1.25, 0.0), (1.6, 1.6), PO, true),
            ],
            body: (-14, -16, 14, 10),
            fp_body: (-2.6, -2.6, 2.6, 2.6),
            fp_round: true,
        },
        Kind::Inductor => KindInfo {
            label: "Inductor",
            prefix: "L",
            default_value: "10u",
            footprint: "L_1210".into(),
            pins: smd2("1", "2", 1.5, 1.2, 2.6, Passive, Passive),
            body: (-4, -20, 8, 20),
            fp_body: (-2.4, -1.6, 2.4, 1.6),
            fp_round: false,
        },
        Kind::Diode => KindInfo {
            label: "Diode",
            prefix: "D",
            default_value: "1N4148",
            footprint: "D_SOD-123".into(),
            pins: vec![
                pin("A", (0, -30), Passive, "2", (1.6, 0.0), (0.9, 1.2), PR, false),
                pin("K", (0, 30), Passive, "1", (-1.6, 0.0), (0.9, 1.2), PR, false),
            ],
            body: (-11, -9, 11, 9),
            fp_body: (-2.4, -0.9, 2.4, 0.9),
            fp_round: false,
        },
        Kind::Led => KindInfo {
            label: "LED",
            prefix: "D",
            default_value: "red",
            footprint: "LED_D5.0mm".into(),
            pins: vec![
                pin("A", (0, -30), Passive, "2", (1.27, 0.0), (1.8, 1.8), PO, true),
                pin("K", (0, 30), Passive, "1", (-1.27, 0.0), (1.8, 1.8), PR, true),
            ],
            body: (-11, -12, 22, 9),
            fp_body: (-2.6, -2.6, 2.6, 2.6),
            fp_round: true,
        },
        Kind::Npn | Kind::Pnp => {
            let pnp = k == Kind::Pnp;
            let (top, bot) = if pnp { ("E", "C") } else { ("C", "E") };
            let (tp, bp) = if pnp { ("2", "3") } else { ("3", "2") };
            let pos = |p: &str| if p == "3" { (0.0, -1.0) } else if p == "2" { (0.95, 1.0) } else { (-0.95, 1.0) };
            KindInfo {
                label: if pnp { "PNP transistor" } else { "NPN transistor" },
                prefix: "Q",
                default_value: if pnp { "2N3906" } else { "2N3904" },
                footprint: "SOT-23".into(),
                pins: vec![
                    pin("B", (-30, 0), Input, "1", pos("1"), (0.9, 0.8), PR, false),
                    pin(top, (10, -30), Passive, tp, pos(tp), (0.9, 0.8), PR, false),
                    pin(bot, (10, 30), Passive, bp, pos(bp), (0.9, 0.8), PR, false),
                ],
                body: (-20, -20, 20, 20),
                fp_body: (-1.5, -0.7, 1.5, 0.7),
                fp_round: false,
            }
        }
        Kind::Nmos => KindInfo {
            label: "N-MOSFET",
            prefix: "Q",
            default_value: "2N7002",
            footprint: "SOT-23".into(),
            pins: vec![
                pin("G", (-30, 0), Input, "1", (-0.95, 1.0), (0.9, 0.8), PR, false),
                pin("D", (10, -30), Passive, "3", (0.0, -1.0), (0.9, 0.8), PR, false),
                pin("S", (10, 30), Passive, "2", (0.95, 1.0), (0.9, 0.8), PR, false),
            ],
            body: (-20, -20, 20, 20),
            fp_body: (-1.5, -0.7, 1.5, 0.7),
            fp_round: false,
        },
        Kind::OpAmp => {
            let s = (1.55, 0.6);
            KindInfo {
                label: "Op-amp",
                prefix: "U",
                default_value: "LM358",
                footprint: "SOIC-8".into(),
                pins: vec![
                    pin("IN-", (-40, -10), Input, "2", (-2.7, -0.635), s, PR, false),
                    pin("IN+", (-40, 10), Input, "3", (-2.7, 0.635), s, PR, false),
                    pin("OUT", (40, 0), Output, "1", (-2.7, -1.905), s, PR, false),
                    pin("V+", (0, -30), PowerIn, "8", (2.7, -1.905), s, PR, false),
                    pin("V-", (0, 30), PowerIn, "4", (-2.7, 1.905), s, PR, false),
                ],
                body: (-30, -30, 30, 30),
                fp_body: (-1.95, -2.45, 1.95, 2.45),
                fp_round: false,
            }
        }
        Kind::Mcu8 => {
            let s = (1.55, 0.6);
            KindInfo {
                label: "Microcontroller (8 pins)",
                prefix: "U",
                default_value: "MCU-8",
                footprint: "SOIC-8".into(),
                pins: vec![
                    pin("IO5", (-60, -30), Bidir, "1", (-2.7, -1.905), s, PR, false),
                    pin("IO3", (-60, -10), Bidir, "2", (-2.7, -0.635), s, PR, false),
                    pin("IO4", (-60, 10), Bidir, "3", (-2.7, 0.635), s, PR, false),
                    pin("GND", (-60, 30), PowerIn, "4", (-2.7, 1.905), s, PR, false),
                    pin("IO0", (60, 30), Bidir, "5", (2.7, 1.905), s, PR, false),
                    pin("IO1", (60, 10), Bidir, "6", (2.7, 0.635), s, PR, false),
                    pin("IO2", (60, -10), Bidir, "7", (2.7, -0.635), s, PR, false),
                    pin("VCC", (60, -30), PowerIn, "8", (2.7, -1.905), s, PR, false),
                ],
                body: (-40, -40, 40, 40),
                fp_body: (-1.95, -2.45, 1.95, 2.45),
                fp_round: false,
            }
        }
        Kind::Regulator => KindInfo {
            label: "Voltage regulator",
            prefix: "U",
            default_value: "5V",
            footprint: "TO-220".into(),
            pins: vec![
                pin("IN", (-50, 0), PowerIn, "1", (-2.54, 0.0), (1.9, 1.9), PR, true),
                pin("OUT", (50, 0), PowerOut, "3", (2.54, 0.0), (1.9, 1.9), PO, true),
                pin("GND", (0, 40), PowerIn, "2", (0.0, 0.0), (1.9, 1.9), PO, true),
            ],
            body: (-30, -20, 30, 20),
            fp_body: (-5.2, -3.0, 5.2, 1.6),
            fp_round: false,
        },
        Kind::Battery => KindInfo {
            label: "Battery",
            prefix: "BT",
            default_value: "9V",
            footprint: "Battery_Holder".into(),
            pins: vec![
                pin("+", (0, -30), PowerOut, "1", (-6.0, 0.0), (2.2, 2.2), PR, true),
                pin("-", (0, 30), Passive, "2", (6.0, 0.0), (2.2, 2.2), PO, true),
            ],
            body: (-15, -10, 15, 9),
            fp_body: (-12.0, -5.0, 12.0, 5.0),
            fp_round: false,
        },
        Kind::Connector2 => KindInfo {
            label: "Connector (2 pins)",
            prefix: "J",
            default_value: "Conn_2",
            footprint: "PinHeader_1x02".into(),
            pins: vec![
                pin("1", (-30, -10), Passive, "1", (0.0, 0.0), (1.7, 1.7), PR, true),
                pin("2", (-30, 10), Passive, "2", (0.0, 2.54), (1.7, 1.7), PO, true),
            ],
            body: (-10, -20, 10, 20),
            fp_body: (-1.27, -1.27, 1.27, 3.81),
            fp_round: false,
        },
        Kind::Switch => KindInfo {
            label: "Switch",
            prefix: "SW",
            default_value: "closed",
            footprint: "SW_Push_6mm".into(),
            pins: vec![
                pin("1", (0, -30), Passive, "1", (-3.25, 0.0), (1.8, 1.8), PO, true),
                pin("2", (0, 30), Passive, "2", (3.25, 0.0), (1.8, 1.8), PO, true),
            ],
            body: (-13, -14, 6, 14),
            fp_body: (-3.0, -3.0, 3.0, 3.0),
            fp_round: false,
        },
        Kind::Gnd => KindInfo {
            label: "Ground (GND)",
            prefix: "#PWR",
            default_value: "GND",
            footprint: "".into(),
            pins: vec![ppin("GND")],
            body: (-10, 0, 10, 18),
            fp_body: (0.0, 0.0, 0.0, 0.0),
            fp_round: false,
        },
        Kind::Generic => KindInfo {
            label: "Imported part",
            prefix: "U",
            default_value: "",
            footprint: String::new(),
            pins: vec![],
            body: (-10, -10, 10, 10),
            fp_body: (-1.0, -1.0, 1.0, 1.0),
            fp_round: false,
        },
        Kind::Power => KindInfo {
            label: "Power rail (+5V…)",
            prefix: "#PWR",
            default_value: "+5V",
            footprint: "".into(),
            pins: vec![ppin("PWR")],
            body: (-10, -22, 10, 0),
            fp_body: (0.0, 0.0, 0.0, 0.0),
            fp_round: false,
        },
    }
}

pub fn rot_i(p: (i32, i32), r: u8) -> (i32, i32) {
    let (mut x, mut y) = p;
    for _ in 0..(r % 4) {
        let nx = -y;
        y = x;
        x = nx;
    }
    (x, y)
}

pub fn rot_f(p: (f32, f32), r: u8) -> (f32, f32) {
    let (mut x, mut y) = p;
    for _ in 0..(r % 4) {
        let nx = -y;
        y = x;
        x = nx;
    }
    (x, y)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Component {
    pub id: u32,
    pub kind: Kind,
    pub reference: String,
    pub value: String,
    pub pos: (i32, i32),
    pub rot: u8,
    pub pcb_pos: (f32, f32),
    pub pcb_rot: u8,
    /// Symbol and footprint geometry for imported parts.
    #[serde(default)]
    pub custom: Option<Box<Custom>>,
    /// JLCPCB / LCSC part number (e.g. C17414), used for the assembly BOM.
    #[serde(default)]
    pub lcsc: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CPin {
    pub name: String,
    pub number: String,
    pub pos: (i32, i32),
    pub ptype: PinType,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CPad {
    /// Drill diameter for through-hole pads (mm); 0 = estimate from the pad size.
    #[serde(default)]
    pub drill: f32,
    pub number: String,
    pub pos: (f32, f32),
    pub size: (f32, f32),
    pub round: bool,
    pub tht: bool,
    pub back: bool,
}

/// Geometry of an imported part (schematic units for the symbol, mm for the footprint),
/// relative to the part's position.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Custom {
    pub lib: String,
    pub pins: Vec<CPin>,
    /// Polylines, with a flag for "filled".
    pub polys: Vec<(Vec<(f32, f32)>, bool)>,
    pub circles: Vec<(f32, f32, f32, bool)>,
    pub texts: Vec<(f32, f32, String)>,
    pub boxes: Vec<(i32, i32, i32, i32)>,
    pub ref_pos: Option<(f32, f32)>,
    pub val_pos: Option<(f32, f32)>,
    pub footprint: String,
    pub pads: Vec<CPad>,
    pub silk: Vec<((f32, f32), (f32, f32))>,
    pub silk_circles: Vec<(f32, f32, f32)>,
}

pub fn switch_closed(value: &str) -> bool {
    let v = value.to_lowercase();
    if v.contains("closed") {
        return true;
    }
    !(v.contains("open") || v.contains("push"))
}

/// Output voltage of a regulator from its value: "5V", "3.3", "L7805", "AMS1117-3.3", "LM1117-3V3".
pub fn reg_voltage(value: &str) -> Option<f64> {
    if let Some(v) = parse_si(value) {
        if v > 0.0 && v < 60.0 {
            return Some(v);
        }
    }
    let up = value.to_uppercase();
    if let Some(i) = up.find("78") {
        let d: String = up[i + 2..].chars().filter(|c| c.is_ascii_alphanumeric()).skip_while(|c| c.is_ascii_alphabetic()).take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = d.parse::<f64>() {
            if n > 0.0 && n < 30.0 {
                return Some(n);
            }
        }
    }
    let tail = up.rsplit(['-', '_']).next().unwrap_or("");
    parse_si(tail).filter(|v| *v > 0.0 && *v < 60.0)
}

impl Component {
    pub fn info(&self) -> KindInfo {
        let base = info(self.kind);
        let Some(cu) = &self.custom else { return base };
        let pins: Vec<PinDef> = cu
            .pins
            .iter()
            .enumerate()
            .map(|(i, cp)| {
                let mut pd = PinDef {
                    name: cp.name.clone(),
                    pos: cp.pos,
                    ptype: cp.ptype,
                    pad: String::new(),
                    pad_pos: (0.0, 0.0),
                    pad_size: (0.0, 0.0),
                    shape: PadShape::Rect,
                    tht: false,
                    back: false,
                };
                if let Some(pad) = cu.pads.iter().find(|p| p.number == cp.number) {
                    pd.pad = pad.number.clone();
                    pd.pad_pos = pad.pos;
                    pd.pad_size = pad.size;
                    pd.shape = if pad.round { PadShape::Round } else { PadShape::Rect };
                    pd.tht = pad.tht;
                    pd.back = pad.back;
                } else if cu.pads.is_empty() {
                    if let Some(b) = base.pins.get(i) {
                        pd.pad = b.pad.clone();
                        pd.pad_pos = b.pad_pos;
                        pd.pad_size = b.pad_size;
                        pd.shape = b.shape;
                        pd.tht = b.tht;
                    }
                }
                pd
            })
            .collect();
        let mut body = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for b in &cu.boxes {
            body = (body.0.min(b.0), body.1.min(b.1), body.2.max(b.2), body.3.max(b.3));
        }
        if body.0 > body.2 {
            body = base.body;
        }
        let (fp_body, fp_round) = if cu.pads.is_empty() {
            (base.fp_body, base.fp_round)
        } else {
            let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            let mut add = |q: (f32, f32)| {
                b = (b.0.min(q.0), b.1.min(q.1), b.2.max(q.0), b.3.max(q.1));
            };
            for p in &cu.pads {
                add((p.pos.0 - p.size.0 / 2.0, p.pos.1 - p.size.1 / 2.0));
                add((p.pos.0 + p.size.0 / 2.0, p.pos.1 + p.size.1 / 2.0));
            }
            for (a, c) in &cu.silk {
                add(*a);
                add(*c);
            }
            for &(x, y, r) in &cu.silk_circles {
                add((x - r, y - r));
                add((x + r, y + r));
            }
            (b, false)
        };
        KindInfo {
            label: base.label,
            prefix: base.prefix,
            default_value: base.default_value,
            footprint: if cu.footprint.is_empty() { base.footprint } else { cu.footprint.clone() },
            pins,
            body,
            fp_body,
            fp_round,
        }
    }
    /// Whether the part has a footprint with pads on the board.
    pub fn has_fp(&self) -> bool {
        self.kind.has_footprint() && self.info().pins.iter().any(|p| !p.pad.is_empty())
    }
    /// Schematic boxes used for clicking (one per drawn unit), in sheet coordinates.
    pub fn sch_boxes(&self) -> Vec<(i32, i32, i32, i32)> {
        match &self.custom {
            Some(cu) if !cu.boxes.is_empty() => cu
                .boxes
                .iter()
                .map(|&(x0, y0, x1, y1)| {
                    let a = rot_i((x0, y0), self.rot);
                    let b = rot_i((x1, y1), self.rot);
                    (self.pos.0 + a.0.min(b.0), self.pos.1 + a.1.min(b.1), self.pos.0 + a.0.max(b.0), self.pos.1 + a.1.max(b.1))
                })
                .collect(),
            _ => vec![self.sch_bbox()],
        }
    }
    pub fn pin_pos(&self, p: &PinDef) -> (i32, i32) {
        let r = rot_i(p.pos, self.rot);
        (self.pos.0 + r.0, self.pos.1 + r.1)
    }
    pub fn pad_pos(&self, p: &PinDef) -> (f32, f32) {
        let r = rot_f(p.pad_pos, self.pcb_rot);
        (self.pcb_pos.0 + r.0, self.pcb_pos.1 + r.1)
    }
    pub fn pad_size(&self, p: &PinDef) -> (f32, f32) {
        if self.pcb_rot % 2 == 1 {
            (p.pad_size.1, p.pad_size.0)
        } else {
            p.pad_size
        }
    }
    pub fn sch_bbox(&self) -> (i32, i32, i32, i32) {
        let inf = self.info();
        let (x0, y0, x1, y1) = inf.body;
        let mut pts = vec![(x0, y0), (x1, y0), (x0, y1), (x1, y1)];
        pts.extend(inf.pins.iter().map(|p| p.pos));
        let mut b = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for p in pts {
            let r = rot_i(p, self.rot);
            let q = (self.pos.0 + r.0, self.pos.1 + r.1);
            b.0 = b.0.min(q.0);
            b.1 = b.1.min(q.1);
            b.2 = b.2.max(q.0);
            b.3 = b.3.max(q.1);
        }
        b
    }
    pub fn pcb_bbox(&self) -> (f32, f32, f32, f32) {
        let inf = self.info();
        let (x0, y0, x1, y1) = inf.fp_body;
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let mut add = |q: (f32, f32)| {
            b.0 = b.0.min(q.0);
            b.1 = b.1.min(q.1);
            b.2 = b.2.max(q.0);
            b.3 = b.3.max(q.1);
        };
        for p in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            let r = rot_f(p, self.pcb_rot);
            add((self.pcb_pos.0 + r.0, self.pcb_pos.1 + r.1));
        }
        for p in &inf.pins {
            let c = self.pad_pos(p);
            let s = self.pad_size(p);
            add((c.0 - s.0 / 2.0, c.1 - s.1 / 2.0));
            add((c.0 + s.0 / 2.0, c.1 + s.1 / 2.0));
        }
        b
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Wire {
    pub a: (i32, i32),
    pub b: (i32, i32),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Label {
    pub pos: (i32, i32),
    pub text: String,
}

/// A copper layer: front, back, or inner layer n (1 = just below the front).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Layer {
    Front,
    Back,
    Inner(u8),
}

impl Layer {
    pub fn name(self) -> String {
        match self {
            Layer::Front => "Front copper".into(),
            Layer::Back => "Back copper".into(),
            Layer::Inner(n) => format!("Inner {} copper", n),
        }
    }
    /// Short name as in KiCad: F.Cu, In1.Cu, B.Cu.
    pub fn short(self) -> String {
        match self {
            Layer::Front => "F.Cu".into(),
            Layer::Back => "B.Cu".into(),
            Layer::Inner(n) => format!("In{}.Cu", n),
        }
    }
    /// Front <-> back; an inner layer jumps to the front.
    pub fn other(self) -> Layer {
        match self {
            Layer::Front => Layer::Back,
            Layer::Back => Layer::Front,
            Layer::Inner(_) => Layer::Front,
        }
    }
    /// Parse "F", "B", "In2", "In2.Cu", "inner2", "2"...
    pub fn parse(s: &str) -> Option<Layer> {
        let u = s.trim().to_uppercase().replace(".CU", "");
        match u.as_str() {
            "F" | "FRONT" | "TOP" => Some(Layer::Front),
            "B" | "BACK" | "BOTTOM" => Some(Layer::Back),
            _ => {
                let n = u.trim_start_matches("INNER").trim_start_matches("IN").trim();
                n.parse::<u8>().ok().filter(|n| *n >= 1 && *n <= 30).map(Layer::Inner)
            }
        }
    }
    /// Stacking position: 0 = front, then inner layers, then back.
    pub fn order(self) -> u8 {
        match self {
            Layer::Front => 0,
            Layer::Inner(n) => n,
            Layer::Back => 255,
        }
    }
}

/// The copper layers of a board with `count` layers, from front to back.
pub fn copper_layers(count: u8) -> Vec<Layer> {
    let count = count.clamp(2, 32);
    let mut v = vec![Layer::Front];
    v.extend((1..=count - 2).map(Layer::Inner));
    v.push(Layer::Back);
    v
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub a: (f32, f32),
    pub b: (f32, f32),
    pub width: f32,
    pub layer: Layer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Via {
    pub pos: (f32, f32),
    pub size: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Board {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// Minimum copper-to-copper gap (mm).
    #[serde(default = "default_clearance")]
    pub clearance: f32,
    /// Number of copper layers (2, 4, 6...).
    #[serde(default = "default_layers")]
    pub layers: u8,
}

fn default_layers() -> u8 {
    2
}

impl Board {
    pub fn copper(&self) -> Vec<Layer> {
        copper_layers(self.layers)
    }
    pub fn has(&self, l: Layer) -> bool {
        match l {
            Layer::Inner(n) => (n as u16) + 2 <= self.layers as u16,
            _ => true,
        }
    }
}

fn default_clearance() -> f32 {
    0.2
}

impl Default for Board {
    fn default() -> Self {
        Board { x0: 0.0, y0: 0.0, x1: 60.0, y1: 40.0, clearance: 0.2, layers: 2 }
    }
}

/// A filled copper area (from an imported board). `layer` None = an inner layer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Zone {
    pub layer: Option<Layer>,
    pub net: String,
    pub polys: Vec<Vec<(f32, f32)>>,
}

pub fn point_in_poly(p: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Ground-like supply names (GND, GNDPWR, GNDA, VSS…).
pub fn is_ground_name(n: &str) -> bool {
    let u = n.to_uppercase();
    u.starts_with("GND") || u == "VSS" || u == "0V"
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub components: Vec<Component>,
    pub wires: Vec<Wire>,
    pub labels: Vec<Label>,
    pub tracks: Vec<Track>,
    pub vias: Vec<Via>,
    pub board: Board,
    pub next_id: u32,
    /// "Not connected" markers: pins here are deliberately left open.
    #[serde(default)]
    pub no_connects: Vec<(i32, i32)>,
    #[serde(default)]
    pub zones: Vec<Zone>,
}

impl Project {
    pub fn find(&self, id: u32) -> Option<usize> {
        self.components.iter().position(|c| c.id == id)
    }
    pub fn find_ref(&self, r: &str) -> Option<usize> {
        self.components.iter().position(|c| c.reference.eq_ignore_ascii_case(r) && !c.kind.is_power())
    }
    pub fn next_ref(&self, prefix: &str) -> String {
        let mut n = 0;
        for c in &self.components {
            if let Some(rest) = c.reference.strip_prefix(prefix) {
                if let Ok(v) = rest.parse::<u32>() {
                    n = n.max(v);
                }
            }
        }
        if prefix == "#PWR" {
            format!("{}{:02}", prefix, n + 1)
        } else {
            format!("{}{}", prefix, n + 1)
        }
    }
    pub fn add_component(&mut self, kind: Kind, pos: (i32, i32), rot: u8) -> u32 {
        let inf = info(kind);
        self.next_id += 1;
        let id = self.next_id;
        // Park new footprints in a column to the right of the board.
        let parked = self
            .components
            .iter()
            .filter(|c| c.has_fp() && c.pcb_pos.0 > self.board.x1)
            .count();
        let pcb_pos = (self.board.x1 + 8.0, self.board.y0 + 4.0 + parked as f32 * 7.0);
        self.components.push(Component {
            id,
            kind,
            reference: self.next_ref(inf.prefix),
            value: inf.default_value.to_string(),
            pos,
            rot,
            pcb_pos,
            pcb_rot: 0,
            custom: None,
            lcsc: String::new(),
        });
        id
    }

    /// Arrange all footprints on the board in tidy rows.
    pub fn auto_place(&mut self) {
        let mut order: Vec<usize> = (0..self.components.len()).filter(|&i| self.components[i].has_fp()).collect();
        order.sort_by(|&a, &b| natural_key(&self.components[a].reference).cmp(&natural_key(&self.components[b].reference)));
        let (bx0, by0, bx1) = (self.board.x0, self.board.y0, self.board.x1);
        let mut x = bx0 + 3.0;
        let mut y = by0 + 3.0;
        let mut row_h: f32 = 0.0;
        for i in order {
            let c = &mut self.components[i];
            c.pcb_pos = (0.0, 0.0);
            let b = c.pcb_bbox();
            let (w, h) = (b.2 - b.0, b.3 - b.1);
            if x + w > bx1 - 3.0 && x > bx0 + 3.0 {
                x = bx0 + 3.0;
                y += row_h + 3.0;
                row_h = 0.0;
            }
            c.pcb_pos = (snap_f(x - b.0, 0.25), snap_f(y - b.1, 0.25));
            x += w + 3.0;
            row_h = row_h.max(h);
        }
    }
}

pub fn snap_f(v: f32, g: f32) -> f32 {
    (v / g).round() * g
}

pub fn natural_key(s: &str) -> (String, u32) {
    let p: String = s.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let n: u32 = s[p.len()..].parse().unwrap_or(0);
    (p, n)
}

// ---------------------------------------------------------------- netlist

pub struct Uf {
    p: Vec<usize>,
}

impl Uf {
    pub fn new(n: usize) -> Self {
        Uf { p: (0..n).collect() }
    }
    pub fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.p[r] != r {
            r = self.p[r];
        }
        let mut c = x;
        while self.p[c] != r {
            let n = self.p[c];
            self.p[c] = r;
            c = n;
        }
        r
    }
    pub fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.p[ra] = rb;
        }
    }
}

pub fn on_segment(p: (i32, i32), a: (i32, i32), b: (i32, i32)) -> bool {
    let cross = (b.0 - a.0) as i64 * (p.1 - a.1) as i64 - (b.1 - a.1) as i64 * (p.0 - a.0) as i64;
    if cross != 0 {
        return false;
    }
    p.0 >= a.0.min(b.0) && p.0 <= a.0.max(b.0) && p.1 >= a.1.min(b.1) && p.1 <= a.1.max(b.1)
}

#[derive(Clone, Debug, Default)]
pub struct Net {
    pub name: String,
    /// (component index, pin index)
    pub pins: Vec<(usize, usize)>,
    pub has_wire: bool,
    pub power: Vec<String>,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Netlist {
    pub nets: Vec<Net>,
    pub pin_net: HashMap<(usize, usize), usize>,
    pub wire_net: Vec<usize>,
    pub label_net: Vec<usize>,
    /// Points where three or more things meet (junction dots).
    pub junctions: Vec<(i32, i32)>,
}

impl Netlist {
    pub fn net_of(&self, ci: usize, pi: usize) -> Option<usize> {
        self.pin_net.get(&(ci, pi)).copied()
    }
    pub fn find(&self, name: &str) -> Option<usize> {
        self.nets.iter().position(|n| n.name == name)
    }
    /// Pins of real parts (not power symbols) on the net.
    pub fn part_pins<'a>(&'a self, p: &'a Project, n: usize) -> impl Iterator<Item = (usize, usize)> + 'a {
        self.nets[n].pins.iter().copied().filter(move |&(ci, _)| !p.components[ci].kind.is_power())
    }
}

pub fn build_netlist(p: &Project) -> Netlist {
    let mut pts: Vec<(i32, i32)> = Vec::new();
    let mut index: HashMap<(i32, i32), usize> = HashMap::new();
    let mut degree: HashMap<(i32, i32), usize> = HashMap::new();
    let mut reg = |q: (i32, i32), pts: &mut Vec<(i32, i32)>| -> usize {
        *index.entry(q).or_insert_with(|| {
            pts.push(q);
            pts.len() - 1
        })
    };
    let mut pin_pt: Vec<((usize, usize), usize)> = Vec::new();
    for (ci, c) in p.components.iter().enumerate() {
        for (pi, pd) in c.info().pins.iter().enumerate() {
            let q = c.pin_pos(pd);
            let id = reg(q, &mut pts);
            *degree.entry(q).or_default() += 1;
            pin_pt.push(((ci, pi), id));
        }
    }
    let wire_pts: Vec<(usize, usize)> = p
        .wires
        .iter()
        .map(|w| {
            *degree.entry(w.a).or_default() += 1;
            *degree.entry(w.b).or_default() += 1;
            (reg(w.a, &mut pts), reg(w.b, &mut pts))
        })
        .collect();
    let label_pts: Vec<usize> = p.labels.iter().map(|l| reg(l.pos, &mut pts)).collect();

    let mut uf = Uf::new(pts.len());
    for &(a, b) in &wire_pts {
        uf.union(a, b);
    }
    for (i, &q) in pts.iter().enumerate() {
        for (wi, w) in p.wires.iter().enumerate() {
            if q != w.a && q != w.b && on_segment(q, w.a, w.b) {
                uf.union(i, wire_pts[wi].0);
                *degree.entry(q).or_default() += 2;
            }
        }
    }
    // Same name => same net (power symbols and labels share one namespace).
    let mut first: HashMap<String, usize> = HashMap::new();
    let mut named: Vec<(usize, String, bool)> = Vec::new();
    for &((ci, _), id) in &pin_pt {
        let c = &p.components[ci];
        match c.kind {
            Kind::Gnd => named.push((id, "GND".into(), true)),
            Kind::Power => named.push((id, c.value.trim().to_string(), true)),
            _ => {}
        }
    }
    for (li, l) in p.labels.iter().enumerate() {
        if !l.text.trim().is_empty() {
            named.push((label_pts[li], l.text.trim().to_string(), false));
        }
    }
    for (id, name, _) in &named {
        if let Some(&f) = first.get(name) {
            uf.union(f, *id);
        } else {
            first.insert(name.clone(), *id);
        }
    }

    let mut pt_net = vec![0usize; pts.len()];
    let mut root_net: HashMap<usize, usize> = HashMap::new();
    let mut count = 0;
    for (i, slot) in pt_net.iter_mut().enumerate() {
        let r = uf.find(i);
        *slot = *root_net.entry(r).or_insert_with(|| {
            count += 1;
            count - 1
        });
    }
    let mut nets: Vec<Net> = (0..count).map(|_| Net::default()).collect();
    let mut pin_net = HashMap::new();
    for &((ci, pi), id) in &pin_pt {
        let n = pt_net[id];
        nets[n].pins.push((ci, pi));
        pin_net.insert((ci, pi), n);
    }
    let wire_net: Vec<usize> = wire_pts.iter().map(|&(a, _)| pt_net[a]).collect();
    for &n in &wire_net {
        nets[n].has_wire = true;
    }
    let label_net: Vec<usize> = label_pts.iter().map(|&a| pt_net[a]).collect();
    for (id, name, power) in named {
        let n = &mut nets[pt_net[id]];
        let list = if power { &mut n.power } else { &mut n.labels };
        if !list.contains(&name) {
            list.push(name);
        }
    }
    for net in nets.iter_mut() {
        net.pins.sort_by(|a, b| {
            let ra = natural_key(&p.components[a.0].reference);
            let rb = natural_key(&p.components[b.0].reference);
            ra.cmp(&rb).then(a.1.cmp(&b.1))
        });
        net.power.sort_by_key(|n| if n == "GND" { 0 } else { 1 });
        net.labels.sort();
        net.name = if let Some(n) = net.power.first() {
            n.clone()
        } else if let Some(n) = net.labels.first() {
            n.clone()
        } else if let Some(&(ci, pi)) = net.pins.iter().find(|(ci, _)| !p.components[*ci].kind.is_power()) {
            let c = &p.components[ci];
            format!("Net-({}-{})", c.reference, c.info().pins[pi].name)
        } else {
            "unconnected".into()
        };
    }
    let junctions = degree.into_iter().filter(|(_, d)| *d >= 3).map(|(q, _)| q).collect();
    Netlist { nets, pin_net, wire_net, label_net, junctions }
}

// ---------------------------------------------------------------- values

/// Parse "4.7k", "4k7", "220R", "100n", "10u", "1M", "9V" into a number.
pub fn parse_si(s: &str) -> Option<f64> {
    let t: String = s
        .trim()
        .replace('µ', "u")
        .replace("Ω", "")
        .replace("ohm", "")
        .replace("Ohm", "")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if t.is_empty() {
        return None;
    }
    let mult = |c: char| -> Option<f64> {
        Some(match c {
            'p' => 1e-12,
            'n' => 1e-9,
            'u' => 1e-6,
            'm' => 1e-3,
            'k' | 'K' => 1e3,
            'M' => 1e6,
            'G' => 1e9,
            'R' | 'r' | 'V' | 'v' | 'F' | 'H' | 'A' => 1.0,
            _ => return None,
        })
    };
    // Find first non-numeric char.
    let idx = t.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'));
    match idx {
        None => t.parse().ok(),
        Some(i) => {
            let (num, rest) = t.split_at(i);
            let c = rest.chars().next()?;
            let m = mult(c)?;
            let after: String = rest.chars().skip(1).take_while(|c| c.is_ascii_digit()).collect();
            let base: f64 = if num.is_empty() || num == "+" || num == "-" {
                return None;
            } else {
                num.parse().ok()?
            };
            if !after.is_empty() && !num.contains('.') {
                // 4k7 style
                let frac: f64 = format!("0.{}", after).parse().ok()?;
                Some((base.abs() + frac) * m * base.signum())
            } else {
                Some(base * m)
            }
        }
    }
}

pub fn fmt_si(v: f64, unit: &str) -> String {
    let a = v.abs();
    let (d, p) = if a >= 1e6 {
        (1e6, "M")
    } else if a >= 1e3 {
        (1e3, "k")
    } else if a >= 1.0 || a == 0.0 {
        (1.0, "")
    } else if a >= 1e-3 {
        (1e-3, "m")
    } else if a >= 1e-6 {
        (1e-6, "µ")
    } else {
        (1e-9, "n")
    };
    let x = v / d;
    let s = if x.abs() >= 100.0 {
        format!("{:.0}", x)
    } else if x.abs() >= 10.0 {
        format!("{:.1}", x)
    } else {
        format!("{:.2}", x)
    };
    format!("{} {}{}", s, p, unit)
}

/// Format a resistor value the way it's written on a schematic (220, 4.7k, 1M).
pub fn fmt_res(r: f64) -> String {
    if r >= 1e6 {
        trim_num(r / 1e6) + "M"
    } else if r >= 1e3 {
        trim_num(r / 1e3) + "k"
    } else {
        trim_num(r)
    }
}

fn trim_num(x: f64) -> String {
    let s = format!("{:.2}", x);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// Next standard (E12) resistor value at or above r.
pub fn e12_up(r: f64) -> f64 {
    const E12: [f64; 12] = [1.0, 1.2, 1.5, 1.8, 2.2, 2.7, 3.3, 3.9, 4.7, 5.6, 6.8, 8.2];
    if r <= 0.0 {
        return 1.0;
    }
    let mut dec = 10f64.powf(r.log10().floor());
    loop {
        for e in E12 {
            if e * dec >= r * 0.999 {
                return (e * dec * 100.0).round() / 100.0;
            }
        }
        dec *= 10.0;
    }
}

// ---------------------------------------------------------------- demo

fn add(p: &mut Project, kind: Kind, r: &str, v: &str, pos: (i32, i32), rot: u8, pcb: (f32, f32), prot: u8) {
    let id = p.add_component(kind, pos, rot);
    let c = p.components.iter_mut().find(|c| c.id == id).unwrap();
    if !r.is_empty() {
        c.reference = r.into();
    }
    c.value = v.into();
    c.pcb_pos = pcb;
    c.pcb_rot = prot;
}

fn wire(p: &mut Project, pts: &[(i32, i32)]) {
    for w in pts.windows(2) {
        p.wires.push(Wire { a: w[0], b: w[1] });
    }
}

fn route(p: &mut Project, a: (&str, &str), b: (&str, &str), vertical_first: bool, layer: Layer) {
    let pad = |p: &Project, (r, pin): (&str, &str)| -> (f32, f32) {
        let c = &p.components[p.find_ref(r).unwrap()];
        let inf = c.info();
        let pd = inf.pins.iter().find(|x| x.name == pin).unwrap();
        c.pad_pos(pd)
    };
    let pa = pad(p, a);
    let pb = pad(p, b);
    let mid = if vertical_first { (pa.0, pb.1) } else { (pb.0, pa.1) };
    for (s, e) in [(pa, mid), (mid, pb)] {
        if s != e {
            p.tracks.push(Track { a: s, b: e, width: 0.5, layer });
        }
    }
}

/// A small example board with a few deliberate mistakes for the assistant to find.
pub fn demo_project() -> Project {
    let mut p = Project { name: "LED driver demo".into(), ..Default::default() };
    use Kind::*;
    add(&mut p, Battery, "BT1", "9V", (100, 220), 0, (14.0, 33.0), 0);
    add(&mut p, Gnd, "", "GND", (100, 250), 0, (0.0, 0.0), 0);
    add(&mut p, Switch, "SW1", "closed", (150, 160), 1, (9.0, 18.0), 0);
    wire(&mut p, &[(100, 190), (100, 160), (120, 160)]);
    add(&mut p, Regulator, "U1", "5V", (260, 160), 0, (24.0, 10.0), 0);
    wire(&mut p, &[(180, 160), (210, 160)]);
    add(&mut p, Gnd, "", "GND", (260, 200), 0, (0.0, 0.0), 0);
    add(&mut p, CapPolarized, "C1", "10u", (200, 210), 0, (16.0, 24.0), 0);
    wire(&mut p, &[(200, 180), (200, 160)]);
    add(&mut p, Gnd, "", "GND", (200, 240), 0, (0.0, 0.0), 0);
    add(&mut p, Capacitor, "C2", "100n", (330, 210), 0, (31.0, 18.0), 0);
    wire(&mut p, &[(310, 160), (370, 160)]);
    wire(&mut p, &[(330, 180), (330, 160)]);
    add(&mut p, Gnd, "", "GND", (330, 240), 0, (0.0, 0.0), 0);
    add(&mut p, Power, "", "+5V", (370, 160), 0, (0.0, 0.0), 0);
    p.labels.push(Label { pos: (190, 160), text: "VIN".into() });

    // LED 1: resistor value far too small.
    add(&mut p, Power, "", "+5V", (440, 140), 0, (0.0, 0.0), 0);
    add(&mut p, Resistor, "R1", "22", (440, 170), 0, (38.0, 10.0), 0);
    add(&mut p, Led, "D1", "red", (440, 230), 0, (46.0, 10.0), 2);
    wire(&mut p, &[(440, 200), (440, 200)]);
    add(&mut p, Gnd, "", "GND", (440, 260), 0, (0.0, 0.0), 0);

    // Microcontroller driving a second LED through a transistor (no decoupling capacitor).
    add(&mut p, Mcu8, "U2", "MCU-8", (600, 220), 0, (33.0, 32.0), 0);
    wire(&mut p, &[(660, 190), (680, 190), (680, 170)]);
    add(&mut p, Power, "", "+5V", (680, 170), 0, (0.0, 0.0), 0);
    wire(&mut p, &[(540, 250), (520, 250)]);
    add(&mut p, Gnd, "", "GND", (520, 250), 0, (0.0, 0.0), 0);
    add(&mut p, Resistor, "R2", "1k", (710, 250), 1, (41.0, 32.0), 0);
    wire(&mut p, &[(660, 250), (680, 250)]);
    p.labels.push(Label { pos: (670, 250), text: "LED_CTRL".into() });
    add(&mut p, Npn, "Q1", "2N3904", (780, 250), 0, (47.0, 32.0), 0);
    wire(&mut p, &[(740, 250), (750, 250)]);
    add(&mut p, Gnd, "", "GND", (790, 280), 0, (0.0, 0.0), 0);
    add(&mut p, Led, "D2", "green", (790, 190), 0, (51.0, 21.0), 0);
    add(&mut p, Resistor, "R3", "330", (790, 100), 0, (44.0, 21.0), 0);
    wire(&mut p, &[(790, 130), (790, 160)]);
    add(&mut p, Power, "", "+5V", (790, 70), 0, (0.0, 0.0), 0);
    p.wires.retain(|w| w.a != w.b);

    route(&mut p, ("SW1", "1"), ("U1", "IN"), true, Layer::Front);
    route(&mut p, ("U1", "OUT"), ("R1", "1"), false, Layer::Front);
    route(&mut p, ("R1", "2"), ("D1", "A"), false, Layer::Front);
    route(&mut p, ("BT1", "+"), ("SW1", "2"), false, Layer::Back);
    p
}
