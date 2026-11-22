//! A small DC operating-point simulator (modified nodal analysis + Newton iterations).
//! Good enough to spot burnt LEDs, wrong resistor values, dropout and shorts.

use crate::checks::{ground_net, Issue, Severity};
use crate::model::*;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct SimResult {
    pub ok: bool,
    pub message: String,
    pub net_v: Vec<Option<f64>>,
    pub notes: HashMap<u32, String>,
    pub issues: Vec<Issue>,
    pub assumed: Vec<String>,
}

type N = Option<usize>;

#[derive(Clone, Debug)]
enum Dev {
    Res { a: N, b: N, r: f64 },
    Vsrc { p: N, n: N, v: f64, k: usize },
    Diode { a: N, c: N, is: f64, nvt: f64, j: usize },
    Bjt { b: N, c: N, e: N, pnp: bool, jf: usize, jr: usize },
    Mos { g: N, d: N, s: N, m: usize },
    Opamp { ip: N, im: N, out: N, vp: N, vm: N, k: usize, o: usize },
    Reg { inp: N, out: N, gnd: N, vreg: f64, k: usize, r: usize },
}

const VT: f64 = 0.025_85;
const BJT_IS: f64 = 1e-14;
const BF: f64 = 100.0;
const BR: f64 = 1.0;
const MOS_K: f64 = 0.1;
const MOS_VTH: f64 = 2.0;
const OPAMP_GAIN: f64 = 1e5;
const REG_DROP: f64 = 1.2;

fn led_vf(value: &str) -> f64 {
    let v = value.to_lowercase();
    if let Some(x) = parse_si(&v) {
        if x > 0.5 && x < 5.0 {
            return x;
        }
    }
    if v.contains("blue") || v.contains("white") || v.contains("uv") {
        3.0
    } else if v.contains("green") {
        2.1
    } else if v.contains("yellow") || v.contains("orange") {
        2.0
    } else if v.contains("ir") {
        1.3
    } else {
        1.9
    }
}

fn sg(a: &mut [Vec<f64>], n1: N, n2: N, g: f64) {
    if let Some(i) = n1 {
        a[i][i] += g;
    }
    if let Some(j) = n2 {
        a[j][j] += g;
    }
    if let (Some(i), Some(j)) = (n1, n2) {
        a[i][j] -= g;
        a[j][i] -= g;
    }
}

/// Constant current `i` flowing through an element from `from` to `to`.
fn si(z: &mut [f64], from: N, to: N, i: f64) {
    if let Some(f) = from {
        z[f] -= i;
    }
    if let Some(t) = to {
        z[t] += i;
    }
}

/// Current gm*(V(cp)-V(cn)) flowing through an element from `from` to `to`.
fn svccs(a: &mut [Vec<f64>], from: N, to: N, cp: N, cn: N, gm: f64) {
    for (row, sign) in [(from, 1.0), (to, -1.0)] {
        if let Some(r) = row {
            if let Some(p) = cp {
                a[r][p] += sign * gm;
            }
            if let Some(n) = cn {
                a[r][n] -= sign * gm;
            }
        }
    }
}

fn diode_iv(vd: f64, is: f64, nvt: f64) -> (f64, f64) {
    let e = (vd / nvt).min(80.0).exp();
    (is * (e - 1.0), is / nvt * e + 1e-12)
}

/// Stamp a junction linearised at vd, scaled; returns unscaled (id, gd).
fn junction(a: &mut [Vec<f64>], z: &mut [f64], an: N, cn: N, vd: f64, is: f64, nvt: f64, scale: f64) -> (f64, f64) {
    let (id, gd) = diode_iv(vd, is, nvt);
    sg(a, an, cn, gd * scale);
    si(z, an, cn, (id - gd * vd) * scale);
    (id, gd)
}

fn pnjlim(vnew: f64, vold: f64, nvt: f64, vcrit: f64) -> f64 {
    if vnew > vcrit && (vnew - vold).abs() > 2.0 * nvt {
        if vold > 0.0 {
            let arg = 1.0 + (vnew - vold) / nvt;
            if arg > 0.0 {
                vold + nvt * arg.ln()
            } else {
                vcrit
            }
        } else {
            nvt * (vnew / nvt).ln()
        }
    } else {
        vnew
    }
}

fn vcrit(is: f64, nvt: f64) -> f64 {
    nvt * (nvt / (std::f64::consts::SQRT_2 * is)).ln()
}

fn solve(mut a: Vec<Vec<f64>>, mut z: Vec<f64>) -> Option<Vec<f64>> {
    let n = z.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().partial_cmp(&a[j][col].abs()).unwrap())?;
        if a[piv][col].abs() < 1e-13 {
            return None;
        }
        a.swap(col, piv);
        z.swap(col, piv);
        for r in col + 1..n {
            let f = a[r][col] / a[col][col];
            if f == 0.0 {
                continue;
            }
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
            z[r] -= f * z[col];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let mut s = z[r];
        for c in r + 1..n {
            s -= a[r][c] * x[c];
        }
        x[r] = s / a[r][r];
    }
    if x.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some(x)
}

pub fn simulate(p: &Project, nl: &Netlist) -> SimResult {
    let mut res = SimResult { net_v: vec![None; nl.nets.len()], ..Default::default() };
    if !p.components.iter().any(|c| !c.kind.is_power()) {
        res.message = "Add some parts to simulate.".into();
        return res;
    }
    let gnd = ground_net(nl).or_else(|| {
        p.components
            .iter()
            .enumerate()
            .find(|(_, c)| c.kind == Kind::Battery)
            .and_then(|(ci, _)| nl.net_of(ci, 1))
    });
    let Some(gnd) = gnd else {
        res.message = "No ground: add a GND symbol so voltages have a reference.".into();
        return res;
    };
    let mut node_of: Vec<N> = vec![None; nl.nets.len()];
    let mut nn = 0;
    for (i, slot) in node_of.iter_mut().enumerate() {
        if i != gnd {
            *slot = Some(nn);
            nn += 1;
        }
    }
    let pn = |ci: usize, pi: usize| -> N { nl.net_of(ci, pi).and_then(|n| node_of[n]) };

    let mut devs: Vec<(u32, Dev)> = Vec::new();
    let (mut nk, mut nj, mut nm, mut no, mut nr) = (0, 0, 0, 0, 0);
    let mut jparams: Vec<(f64, f64)> = Vec::new();
    for (ci, c) in p.components.iter().enumerate() {
        let id = c.id;
        let pin = |i: usize| pn(ci, i);
        match c.kind {
            Kind::Resistor => {
                let r = match parse_si(&c.value) {
                    Some(r) if r > 0.0 => r,
                    _ => {
                        res.issues.push(
                            Issue::new(Severity::Warning, "SIM", format!("Can't read the value of {} (\"{}\"); using 1k.", c.reference, c.value))
                                .comp(id),
                        );
                        1000.0
                    }
                };
                devs.push((id, Dev::Res { a: pin(0), b: pin(1), r }));
            }
            Kind::Inductor => devs.push((id, Dev::Res { a: pin(0), b: pin(1), r: 0.05 })),
            Kind::Switch => {
                if switch_closed(&c.value) {
                    devs.push((id, Dev::Res { a: pin(0), b: pin(1), r: 0.02 }));
                }
            }
            Kind::Diode | Kind::Led => {
                let (vf, n) = if c.kind == Kind::Led { (led_vf(&c.value), 2.0) } else { (0.7, 1.8) };
                let nvt = n * VT;
                let is = 0.01 / (vf / nvt).exp();
                jparams.push((is, nvt));
                devs.push((id, Dev::Diode { a: pin(0), c: pin(1), is, nvt, j: nj }));
                nj += 1;
            }
            Kind::Npn | Kind::Pnp => {
                let pnp = c.kind == Kind::Pnp;
                // pins: B, top, bottom. NPN top=C, PNP top=E.
                let (cc, ee) = if pnp { (pin(2), pin(1)) } else { (pin(1), pin(2)) };
                jparams.push((BJT_IS, VT));
                jparams.push((BJT_IS, VT));
                devs.push((id, Dev::Bjt { b: pin(0), c: cc, e: ee, pnp, jf: nj, jr: nj + 1 }));
                nj += 2;
            }
            Kind::Nmos => {
                devs.push((id, Dev::Mos { g: pin(0), d: pin(1), s: pin(2), m: nm }));
                nm += 1;
            }
            Kind::OpAmp => {
                let out = pin(2);
                if out.is_some() {
                    devs.push((id, Dev::Opamp { ip: pin(1), im: pin(0), out, vp: pin(3), vm: pin(4), k: nk, o: no }));
                    nk += 1;
                    no += 1;
                }
            }
            Kind::Mcu8 => {
                if nl.net_of(ci, 7) != nl.net_of(ci, 3) {
                    devs.push((id, Dev::Res { a: pin(7), b: pin(3), r: 1000.0 }));
                }
            }
            Kind::Regulator => {
                let vreg = reg_voltage(&c.value).unwrap_or(5.0);
                let out = pin(1);
                if out.is_some() && nl.net_of(ci, 1) != nl.net_of(ci, 2) {
                    devs.push((id, Dev::Reg { inp: pin(0), out, gnd: pin(2), vreg, k: nk, r: nr }));
                    nk += 1;
                    nr += 1;
                }
            }
            Kind::Battery => {
                let v = parse_si(&c.value).unwrap_or(9.0);
                if nl.net_of(ci, 0) != nl.net_of(ci, 1) {
                    devs.push((id, Dev::Vsrc { p: pin(0), n: pin(1), v, k: nk }));
                    nk += 1;
                }
            }
            Kind::Capacitor | Kind::CapPolarized | Kind::Connector2 | Kind::Gnd | Kind::Power | Kind::Generic => {}
        }
    }
    // Power rails that nothing in the schematic produces become ideal supplies.
    for (ni, net) in nl.nets.iter().enumerate() {
        if ni == gnd {
            continue;
        }
        let Some(name) = net.power.iter().find(|n| *n != "GND") else { continue };
        let driven = net.pins.iter().any(|&(ci, pi)| {
            let c = &p.components[ci];
            !c.kind.is_power() && c.info().pins[pi].ptype == PinType::PowerOut
        });
        if driven {
            continue;
        }
        if let Some(v) = parse_si(name) {
            devs.push((u32::MAX, Dev::Vsrc { p: node_of[ni], n: None, v, k: nk }));
            nk += 1;
            res.assumed.push(format!("{} has no source in the schematic, so an ideal {} supply was assumed.", name, name));
        }
    }

    let size = nn + nk;
    let mut x = vec![0.0f64; size];
    let mut jv = vec![0.0f64; nj];
    let mut mv = vec![0.0f64; nm];
    let mut op_mode = vec![0u8; no];
    let mut reg_mode = vec![0u8; nr];
    let mut reg_iin = vec![0.0f64; nr];
    let v = |n: N, x: &[f64]| n.map(|i| x[i]).unwrap_or(0.0);
    let mut converged_all = true;

    for _outer in 0..25 {
        let mut converged = false;
        for it in 0..400 {
            let mut a = vec![vec![0.0; size]; size];
            let mut z = vec![0.0; size];
            let mut jdelta: f64 = 0.0;
            for (i, row) in a.iter_mut().enumerate().take(nn) {
                row[i] += 1e-9;
            }
            for (_, d) in &devs {
                match *d {
                    Dev::Res { a: na, b: nb, r } => sg(&mut a, na, nb, 1.0 / r),
                    Dev::Vsrc { p: np, n: nnn, v: vv, k } => {
                        let row = nn + k;
                        if let Some(i) = np {
                            a[i][row] += 1.0;
                            a[row][i] += 1.0;
                        }
                        if let Some(i) = nnn {
                            a[i][row] -= 1.0;
                            a[row][i] -= 1.0;
                        }
                        z[row] = vv;
                    }
                    Dev::Diode { a: an, c: cn, is, nvt, j } => {
                        let raw = v(an, &x) - v(cn, &x);
                        let vd = pnjlim(raw, jv[j], nvt, vcrit(is, nvt));
                        jdelta = jdelta.max((vd - jv[j]).abs()).max((vd - raw).abs());
                        jv[j] = vd;
                        junction(&mut a, &mut z, an, cn, vd, is, nvt, 1.0);
                    }
                    Dev::Bjt { b, c, e, pnp, jf, jr } => {
                        let (t1, t2) = if pnp { (e, c) } else { (c, e) };
                        let (fa, fk) = if pnp { (e, b) } else { (b, e) };
                        let (ra, rk) = if pnp { (c, b) } else { (b, c) };
                        let vc = vcrit(BJT_IS, VT);
                        let vf = pnjlim(v(fa, &x) - v(fk, &x), jv[jf], VT, vc);
                        let vr = pnjlim(v(ra, &x) - v(rk, &x), jv[jr], VT, vc);
                        jdelta = jdelta.max((vf - jv[jf]).abs()).max((vr - jv[jr]).abs());
                        jv[jf] = vf;
                        jv[jr] = vr;
                        let (idf, gf) = junction(&mut a, &mut z, fa, fk, vf, BJT_IS, VT, 1.0 / BF);
                        let (idr, gr) = junction(&mut a, &mut z, ra, rk, vr, BJT_IS, VT, 1.0 / BR);
                        svccs(&mut a, t1, t2, fa, fk, gf);
                        si(&mut z, t1, t2, idf - gf * vf);
                        svccs(&mut a, t2, t1, ra, rk, gr);
                        si(&mut z, t2, t1, idr - gr * vr);
                    }
                    Dev::Mos { g, d, s, m } => {
                        let raw = v(g, &x) - v(s, &x);
                        let vgs = mv[m] + (raw - mv[m]).clamp(-0.5, 0.5);
                        jdelta = jdelta.max((vgs - raw).abs());
                        mv[m] = vgs;
                        let vds = v(d, &x) - v(s, &x);
                        let vov = vgs - MOS_VTH;
                        let (id, gm, mut gds) = if vov <= 0.0 {
                            (0.0, 0.0, 0.0)
                        } else if vds < vov {
                            (MOS_K * (2.0 * vov * vds - vds * vds), 2.0 * MOS_K * vds, 2.0 * MOS_K * (vov - vds))
                        } else {
                            (MOS_K * vov * vov * (1.0 + 0.01 * vds), 2.0 * MOS_K * vov, MOS_K * vov * vov * 0.01)
                        };
                        gds += 1e-9;
                        svccs(&mut a, d, s, g, s, gm);
                        sg(&mut a, d, s, gds);
                        si(&mut z, d, s, id - gm * vgs - gds * vds);
                    }
                    Dev::Opamp { ip, im, out, vp, vm, k, o } => {
                        let row = nn + k;
                        let oi = out.unwrap();
                        a[oi][row] += 1.0;
                        a[row][oi] += 1.0;
                        match op_mode[o] {
                            0 => {
                                if let Some(i) = ip {
                                    a[row][i] -= OPAMP_GAIN;
                                }
                                if let Some(i) = im {
                                    a[row][i] += OPAMP_GAIN;
                                }
                            }
                            1 => {
                                if let Some(i) = vp {
                                    a[row][i] -= 1.0;
                                }
                                z[row] = -0.05;
                            }
                            _ => {
                                if let Some(i) = vm {
                                    a[row][i] -= 1.0;
                                }
                                z[row] = 0.05;
                            }
                        }
                    }
                    Dev::Reg { inp, out, gnd: g, vreg, k, r } => {
                        let row = nn + k;
                        let oi = out.unwrap();
                        a[oi][row] += 1.0;
                        a[row][oi] += 1.0;
                        match reg_mode[r] {
                            0 => {
                                if let Some(i) = g {
                                    a[row][i] -= 1.0;
                                }
                                z[row] = vreg;
                            }
                            1 => {
                                if let Some(i) = inp {
                                    a[row][i] -= 1.0;
                                }
                                z[row] = -REG_DROP;
                            }
                            _ => {
                                if let Some(i) = g {
                                    a[row][i] -= 1.0;
                                }
                                z[row] = 0.0;
                            }
                        }
                        si(&mut z, inp, g, reg_iin[r]);
                    }
                }
            }
            let Some(xn) = solve(a, z) else {
                res.message = "The circuit can't be solved: two voltage sources are fighting or a supply is shorted.".into();
                res.issues.push(Issue::new(
                    Severity::Error,
                    "SIM",
                    "Simulation failed: a supply seems to be shorted directly, or two supplies are connected together.".into(),
                ));
                return res;
            };
            let delta = (0..nn).map(|i| (xn[i] - x[i]).abs()).fold(0.0, f64::max);
            x = xn;
            if it > 0 && delta < 1e-7 && jdelta < 1e-6 {
                converged = true;
                break;
            }
        }
        if !converged {
            converged_all = false;
        }
        // Update op-amp and regulator modes.
        let mut changed = false;
        for (_, d) in &devs {
            match *d {
                Dev::Opamp { ip, im, vp, vm, o, .. } => {
                    let target = OPAMP_GAIN * (v(ip, &x) - v(im, &x));
                    let hi = v(vp, &x) - 0.05;
                    let lo = v(vm, &x) + 0.05;
                    let m = if target > hi { 1 } else if target < lo { 2 } else { 0 };
                    if m != op_mode[o] {
                        op_mode[o] = m;
                        changed = true;
                    }
                }
                Dev::Reg { inp, gnd: g, vreg, k, r, .. } => {
                    let vin = v(inp, &x) - v(g, &x);
                    let m = if vin - REG_DROP >= vreg {
                        0
                    } else if vin - REG_DROP > 0.0 {
                        1
                    } else {
                        2
                    };
                    let iout = -x[nn + k];
                    let iin = if vin > 1.0 { iout.max(0.0) + 0.005 } else { 0.0 };
                    if m != reg_mode[r] || (iin - reg_iin[r]).abs() > 1e-7 {
                        reg_mode[r] = m;
                        reg_iin[r] = iin;
                        changed = true;
                    }
                }
                _ => {}
            }
        }
        if !changed {
            break;
        }
    }

    res.ok = true;
    res.message = if converged_all { "DC operating point solved.".into() } else { "Solved approximately (did not fully converge).".into() };
    for (i, slot) in res.net_v.iter_mut().enumerate() {
        *slot = Some(v(node_of[i], &x));
    }

    // Results per part.
    let comp = |id: u32| p.components.iter().find(|c| c.id == id).unwrap();
    let mut led_currents: Vec<(u32, f64, N, N)> = Vec::new();
    let mut res_info: HashMap<u32, (f64, f64)> = HashMap::new(); // (voltage, current)
    for (id, d) in &devs {
        if *id == u32::MAX {
            continue;
        }
        let c = comp(*id);
        let name = &c.reference;
        match *d {
            Dev::Res { a, b, r } => {
                let vr = v(a, &x) - v(b, &x);
                let i = vr / r;
                let pw = vr * i;
                res_info.insert(*id, (vr, i));
                if c.kind == Kind::Resistor {
                    res.notes.insert(*id, format!("{} across, {}, {}", fmt_si(vr.abs(), "V"), fmt_si(i.abs(), "A"), fmt_si(pw, "W")));
                    if pw > 0.25 {
                        res.issues.push(
                            Issue::new(
                                Severity::Warning,
                                "SIM",
                                format!("{} burns {} — more than a standard 0.25 W resistor can take; it will overheat.", name, fmt_si(pw, "W")),
                            )
                            .comp(*id)
                            .at_sch(c.pos),
                        );
                    }
                } else if c.kind == Kind::Mcu8 {
                    let vcc = vr;
                    res.notes.insert(*id, format!("supply {}", fmt_si(vcc, "V")));
                    if vcc < 1.8 {
                        res.issues.push(
                            Issue::new(Severity::Error, "SIM", format!("{} only gets {} — it needs 1.8 V to 5.5 V to run.", name, fmt_si(vcc, "V")))
                                .comp(*id)
                                .at_sch(c.pos),
                        );
                    } else if vcc > 5.6 {
                        res.issues.push(
                            Issue::new(Severity::Error, "SIM", format!("{} gets {} — above its 5.5 V maximum; it can be damaged.", name, fmt_si(vcc, "V")))
                                .comp(*id)
                                .at_sch(c.pos),
                        );
                    }
                } else {
                    res.notes.insert(*id, format!("{}", fmt_si(i.abs(), "A")));
                }
            }
            Dev::Vsrc { k, v: vv, .. } => {
                let i = -x[nn + k];
                res.notes.insert(*id, format!("{} supplying {}, {}", fmt_si(vv, "V"), fmt_si(i, "A"), fmt_si(vv * i, "W")));
                if i > 2.0 {
                    res.issues.push(
                        Issue::new(Severity::Error, "SIM", format!("{} delivers {} — that looks like a short circuit.", name, fmt_si(i, "A")))
                            .comp(*id)
                            .at_sch(c.pos),
                    );
                } else if i < -0.001 {
                    res.issues.push(
                        Issue::new(Severity::Warning, "SIM", format!("Current flows backwards into {} ({}).", name, fmt_si(-i, "A")))
                            .comp(*id)
                            .at_sch(c.pos),
                    );
                }
            }
            Dev::Diode { a, c: cn, is, nvt, .. } => {
                let vd = v(a, &x) - v(cn, &x);
                let (i, _) = diode_iv(vd, is, nvt);
                res.notes.insert(*id, format!("{} through, {} across", fmt_si(i.max(0.0), "A"), fmt_si(vd, "V")));
                if c.kind == Kind::Led {
                    led_currents.push((*id, i, a, cn));
                    if i > 0.03 {
                        res.issues.push(
                            Issue::new(
                                Severity::Error,
                                "SIM",
                                format!(
                                    "LED {} carries {} — far above the ~20 mA a standard LED is rated for; it will burn out.",
                                    name,
                                    fmt_si(i, "A")
                                ),
                            )
                            .comp(*id)
                            .at_sch(c.pos),
                        );
                    } else if vd < -0.3 {
                        res.issues.push(
                            Issue::new(
                                Severity::Warning,
                                "SIM",
                                format!("LED {} is reverse-biased ({}): it is probably the wrong way round.", name, fmt_si(vd, "V")),
                            )
                            .comp(*id)
                            .at_sch(c.pos),
                        );
                    } else if i < 0.0005 {
                        res.issues.push(
                            Issue::new(
                                Severity::Info,
                                "SIM",
                                format!("LED {} is off in this DC state (only {} flows).", name, fmt_si(i.max(0.0), "A")),
                            )
                            .comp(*id)
                            .at_sch(c.pos),
                        );
                    }
                }
            }
            Dev::Bjt { b, c: cc, e, pnp, jf, jr } => {
                let s = if pnp { -1.0 } else { 1.0 };
                let vbe = s * (v(b, &x) - v(e, &x));
                let vbc = s * (v(b, &x) - v(cc, &x));
                let (idf, _) = diode_iv(vbe, BJT_IS, VT);
                let (idr, _) = diode_iv(vbc, BJT_IS, VT);
                let _ = (jf, jr);
                let ic = idf - idr - idr / BR;
                let ib = idf / BF + idr / BR;
                let vce = s * (v(cc, &x) - v(e, &x));
                let state = if ib < 1e-6 {
                    "off"
                } else if vce < 0.3 {
                    "fully on (saturated)"
                } else {
                    "partly on (active)"
                };
                res.notes.insert(*id, format!("{}: Ib {}, Ic {}, Vce {}", state, fmt_si(ib, "A"), fmt_si(ic, "A"), fmt_si(vce, "V")));
            }
            Dev::Mos { g, d, s, .. } => {
                let vgs = v(g, &x) - v(s, &x);
                let vds = v(d, &x) - v(s, &x);
                let vov = vgs - MOS_VTH;
                let id_ = if vov <= 0.0 {
                    0.0
                } else if vds < vov {
                    MOS_K * (2.0 * vov * vds - vds * vds)
                } else {
                    MOS_K * vov * vov
                };
                let state = if vov <= 0.0 { "off" } else { "on" };
                res.notes.insert(*id, format!("{}: Vgs {}, Id {}", state, fmt_si(vgs, "V"), fmt_si(id_, "A")));
                if g.is_some() && nl.net_of(p.find(*id).unwrap(), 0).map(|n| nl.nets[n].pins.len() <= 1).unwrap_or(false) {
                    res.issues.push(
                        Issue::new(Severity::Warning, "SIM", format!("{}'s gate is floating; add a pull-down resistor.", name))
                            .comp(*id)
                            .at_sch(c.pos),
                    );
                }
            }
            Dev::Opamp { out, vp, vm, o, .. } => {
                let vo = v(out, &x);
                let st = match op_mode[o] {
                    0 => "linear",
                    1 => "stuck at the + rail",
                    _ => "stuck at the - rail",
                };
                res.notes.insert(*id, format!("output {} ({})", fmt_si(vo, "V"), st));
                if v(vp, &x) - v(vm, &x) < 1.0 {
                    res.issues.push(
                        Issue::new(Severity::Warning, "SIM", format!("Op-amp {} has no supply voltage across V+ and V-.", name))
                            .comp(*id)
                            .at_sch(c.pos),
                    );
                }
            }
            Dev::Reg { inp, out, gnd: g, vreg, k, r } => {
                let vin = v(inp, &x) - v(g, &x);
                let vo = v(out, &x) - v(g, &x);
                let iout = -x[nn + k];
                let pw = (vin - vo) * iout.max(0.0);
                res.notes.insert(*id, format!("in {}, out {}, {}, heat {}", fmt_si(vin, "V"), fmt_si(vo, "V"), fmt_si(iout, "A"), fmt_si(pw, "W")));
                match reg_mode[r] {
                    1 => res.issues.push(
                        Issue::new(
                            Severity::Warning,
                            "SIM",
                            format!(
                                "{} is in dropout: its input ({}) is too low to make {} (it needs about {}).",
                                name,
                                fmt_si(vin, "V"),
                                fmt_si(vreg, "V"),
                                fmt_si(vreg + REG_DROP, "V")
                            ),
                        )
                        .comp(*id)
                        .at_sch(c.pos),
                    ),
                    2 => res.issues.push(
                        Issue::new(Severity::Error, "SIM", format!("{} gets no input power.", name)).comp(*id).at_sch(c.pos),
                    ),
                    _ => {}
                }
                if pw > 1.0 {
                    res.issues.push(
                        Issue::new(Severity::Warning, "SIM", format!("{} turns {} into heat; it needs a heatsink.", name, fmt_si(pw, "W")))
                            .comp(*id)
                            .at_sch(c.pos),
                    );
                }
            }
        }
    }
    // Capacitors.
    for (ci, c) in p.components.iter().enumerate() {
        if matches!(c.kind, Kind::Capacitor | Kind::CapPolarized) {
            let vc = v(pn(ci, 0), &x) - v(pn(ci, 1), &x);
            res.notes.insert(c.id, format!("{} across", fmt_si(vc, "V")));
            if c.kind == Kind::CapPolarized && vc < -0.5 {
                res.issues.push(
                    Issue::new(
                        Severity::Error,
                        "SIM",
                        format!("{} is charged backwards ({}); electrolytic capacitors can fail when reversed.", c.reference, fmt_si(vc, "V")),
                    )
                    .comp(c.id)
                    .at_sch(c.pos),
                );
            }
        }
    }
    // Suggest a resistor value for over-driven LEDs.
    for (lid, i, a, cn) in led_currents {
        if i <= 0.03 {
            continue;
        }
        for (rid, (vr, ir)) in &res_info {
            let rc = comp(*rid);
            if rc.kind != Kind::Resistor {
                continue;
            }
            let ri = p.find(*rid).unwrap();
            let shares = [pn(ri, 0), pn(ri, 1)].iter().any(|n| n.is_some() && (*n == a || *n == cn));
            if shares && (ir.abs() - i).abs() < 0.3 * i {
                let target = e12_up(vr.abs() / 0.015);
                if let Some(is) = res.issues.iter_mut().find(|is| is.comps.first() == Some(&lid) && is.severity == Severity::Error) {
                    is.message.push_str(&format!(" Use about {} Ω for {} (≈15 mA).", fmt_res(target), rc.reference));
                    is.fix = Some((*rid, fmt_res(target)));
                }
                break;
            }
        }
    }
    let has_mcu = p.components.iter().any(|c| c.kind == Kind::Mcu8);
    if has_mcu {
        res.assumed.push("Microcontroller pins are treated as unconnected inputs: the DC check doesn't know what the firmware drives.".into());
    }
    res
}
