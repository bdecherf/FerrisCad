#![allow(dead_code)]
//! FerrisCad: schematic capture + PCB layout with an AI assistant for circuit debugging.

mod annot;
mod ai;
mod aiedit;
mod blocks;
mod fpick;
mod app;
mod checks;
mod filedlg;
mod font;
mod jlc;
mod jlcui;
mod kicad;
mod kicad_export;
mod model;
mod multisel;
mod online;
mod pcb;
mod reload;
mod sch;
mod sim;
mod view3d;

use eframe::egui;

/// Portable float maths. Recent glibc versions give `atan2f`, `acosf` and `hypotf` new
/// symbol versions; defining them here (computed in double precision) keeps the
/// AppImage running on older Linux systems (glibc 2.34 and later).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod compat_math {
    use std::hint::black_box;
    #[no_mangle]
    pub extern "C" fn atan2f(y: f32, x: f32) -> f32 {
        black_box(y as f64).atan2(black_box(x as f64)) as f32
    }
    #[no_mangle]
    pub extern "C" fn acosf(x: f32) -> f32 {
        black_box(x as f64).acos() as f32
    }
    #[no_mangle]
    pub extern "C" fn hypotf(x: f32, y: f32) -> f32 {
        let (a, b) = (black_box(x as f64), black_box(y as f64));
        (a * a + b * b).sqrt() as f32
    }

    // Rust's standard library references these (glibc 2.39) only for the optional
    // "pidfd" process API, which this app never uses. Stubs keep older systems working.
    extern "C" {
        fn __errno_location() -> *mut i32;
    }
    const ENOSYS: i32 = 38;
    #[no_mangle]
    pub extern "C" fn pidfd_spawnp(
        _pidfd: *mut i32,
        _file: *const u8,
        _actions: *const u8,
        _attr: *const u8,
        _argv: *const *const u8,
        _envp: *const *const u8,
    ) -> i32 {
        ENOSYS
    }
    #[no_mangle]
    pub extern "C" fn pidfd_getpid(_fd: i32) -> i32 {
        unsafe { *__errno_location() = ENOSYS };
        -1
    }
}
use std::path::PathBuf;

fn selftest() {
    report(&model::demo_project());
}

fn report(p: &model::Project) {
    let p = p.clone();
    let nl = model::build_netlist(&p);
    println!("{} nets", nl.nets.len());
    for n in &nl.nets {
        let pins: Vec<String> =
            n.pins.iter().map(|&(ci, pi)| format!("{}.{}", p.components[ci].reference, p.components[ci].info().pins[pi].name)).collect();
        println!("  {}: {}", n.name, pins.join(" "));
    }
    let erc = checks::erc(&p, &nl);
    let (drc, conn) = checks::drc(&p, &nl);
    let sim = sim::simulate(&p, &nl);
    println!("sim: {}", sim.message);
    for (i, v) in sim.net_v.iter().enumerate() {
        println!("  V({}) = {:?}", nl.nets[i].name, v);
    }
    for c in &p.components {
        if let Some(n) = sim.notes.get(&c.id) {
            println!("  {}: {}", c.reference, n);
        }
    }
    for i in erc.iter().chain(sim.issues.iter()).chain(drc.iter()) {
        println!("[{:?}][{}] {} fix={:?}", i.severity, i.source, i.message, i.fix);
    }
    println!("unrouted {}", conn.unrouted);
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut screenshot: Option<PathBuf> = None;
    let mut open: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--screenshot" if i + 1 < args.len() => {
                screenshot = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--online-search" if i + 1 < args.len() => {
                match online::search(&args[i + 1]) {
                    Ok(h) => {
                        for x in h.iter().take(12) {
                            println!("{}  [{}]", x.id(), x.score);
                        }
                    }
                    Err(e) => println!("error: {}", e),
                }
                return Ok(());
            }
            "--online-part" if i + 1 < args.len() => {
                let (lib, name) = args[i + 1].split_once(':').unwrap_or(("Device", args[i + 1].as_str()));
                match online::fetch_part(lib, name) {
                    Ok(p) => {
                        let pins: Vec<String> = p.custom.pins.iter().map(|x| format!("{}({})@{:?}", x.name, x.number, x.pos)).collect();
                        println!("kind={:?} value={} prefix={} footprint={} pads={} units={} | {}", p.kind, p.value, p.prefix, p.custom.footprint, p.custom.pads.len(), p.custom.boxes.len(), p.description);
                        println!("pins: {}", pins.join(" "));
                        for n in p.notes {
                            println!("note: {}", n);
                        }
                    }
                    Err(e) => println!("error: {}", e),
                }
                return Ok(());
            }
            "--multisel-test" => {
                let mut app = app::App::with_ctx(egui::Context::default(), Some(PathBuf::from("/dev/null")), None);
                let nets = |a: &app::App| -> Vec<String> {
                    let mut v: Vec<String> = a
                        .nl
                        .pin_net
                        .iter()
                        .map(|(&(ci, pi), &n)| format!("{}.{}={}", a.project.components[ci].reference, pi, a.nl.nets[n].name))
                        .collect();
                    v.sort();
                    v
                };
                app.analyze();
                let before = nets(&app);
                // Box around R1, D1 and their supply symbols (schematic x 420..470, y 120..280).
                let items = app.band_items((410.0, 120.0), (480.0, 285.0));
                println!("(box misses the +5V symbol on purpose)");
                println!("box selected {} items: {:?}", items.len(), items);
                app.set_selection(items);
                let r1 = app.project.find_ref("R1").unwrap();
                let p0 = app.project.components[r1].pos;
                app.start_sch_group_drag((440, 170));
                let d = app.drag.clone().unwrap();
                app.drag_sch_group(&d, (540, 170));
                app.drag = None;
                app.analyze();
                println!("R1 moved {:?} -> {:?}", p0, app.project.components[r1].pos);
                let after = nets(&app);
                let changed: Vec<_> = before.iter().zip(after.iter()).filter(|(a, b)| a != b).collect();
                println!("connections changed: {}", changed.len());
                for c in changed.iter().take(5) {
                    println!("  {:?}", c);
                }
                // Select R1 only with a box that touches it (drag right-to-left).
                let only = app.band_items((450.0, 175.0), (436.0, 165.0));
                println!("touch box selected {:?}", only);
                let n = app.selection().len();
                app.delete_group();
                app.analyze();
                println!("deleted {} items, parts left {}", n, app.project.components.len());
                return Ok(());
            }
            "--jlc-test" if i + 1 < args.len() => {
                let mut p = model::demo_project();
                for l in jlc::bom(&p) {
                    let ci = p.find(l.ids[0]).unwrap();
                    let Some((q, pkg)) = jlc::auto_query(&p.components[ci]) else {
                        println!("{:<14} (left for manual choice)", l.refs.join(","));
                        continue;
                    };
                    match jlc::search(&q) {
                        Ok(r) => {
                            let pick = r.iter().find(|x| x.stock > 0 && (pkg.is_empty() || x.package.to_uppercase().contains(&pkg.to_uppercase())));
                            if let Some(x) = pick {
                                println!("{:<14} {:<22} -> {} {} {} {} stock {}", l.refs.join(","), q, x.code, if x.basic { "Basic" } else { "Ext" }, x.model, x.package, x.stock);
                                for id in &l.ids {
                                    let ci = p.find(*id).unwrap();
                                    p.components[ci].lcsc = x.code.clone();
                                }
                            } else {
                                println!("{:<14} {:<22} -> nothing", l.refs.join(","), q);
                            }
                        }
                        Err(e) => println!("search error: {}", e),
                    }
                }
                let nl = model::build_netlist(&p);
                let (_, conn) = checks::drc(&p, &nl);
                match jlc::export(&p, &conn, std::path::Path::new(&args[i + 1])) {
                    Ok(e) => {
                        for f in e.files {
                            println!("wrote {} ({} bytes)", f.display(), std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0));
                        }
                        for n in e.notes {
                            println!("note: {}", n);
                        }
                    }
                    Err(e) => println!("export error: {}", e),
                }
                return Ok(());
            }
            "--layers-test" => {
                let mut p = model::demo_project();
                p.board.layers = 4;
                let nl = model::build_netlist(&p);
                let (_, before) = checks::drc(&p, &nl);
                // BT1 "-" (through-hole) to C1 "-" (through-hole), both GND, on inner layer 1.
                let pad = |p: &model::Project, r: &str, pin: &str| {
                    let c = &p.components[p.find_ref(r).unwrap()];
                    let inf = c.info();
                    c.pad_pos(inf.pins.iter().find(|x| x.name == pin).unwrap())
                };
                let (a, b) = (pad(&p, "BT1", "-"), pad(&p, "C1", "-"));
                p.tracks.push(model::Track { a, b, width: 0.4, layer: model::Layer::Inner(1) });
                let nl = model::build_netlist(&p);
                let (issues, after) = checks::drc(&p, &nl);
                println!("layers: {:?}", p.board.copper().iter().map(|l| l.short()).collect::<Vec<_>>());
                println!("unrouted before {} after {}", before.unrouted, after.unrouted);
                for i in issues.iter().filter(|i| i.severity == checks::Severity::Error) {
                    println!("  [Error] {}", i.message);
                }
                let dir = std::env::temp_dir().join("vp-layers");
                match jlc::export(&p, &after, &dir) {
                    Ok(e) => {
                        for n in e.notes.iter().filter(|n| n.contains("layer")) {
                            println!("note: {}", n);
                        }
                        println!("zip: {}", e.files[0].display());
                    }
                    Err(e) => println!("export error {}", e),
                }
                p.board.layers = 2;
                let nl = model::build_netlist(&p);
                let (issues, _) = checks::drc(&p, &nl);
                for i in issues.iter().filter(|i| i.message.contains("layer")) {
                    println!("after going back to 2 layers: {}", i.message);
                }
                return Ok(());
            }
            "--reload-test" => {
                let path = std::env::temp_dir().join("ferriscad-reload-test.fcad");
                let path_s = path.to_string_lossy().to_string();
                std::fs::write(&path, serde_json::to_string_pretty(&model::demo_project()).unwrap()).unwrap();
                let mut app = app::App::with_ctx(egui::Context::default(), Some(PathBuf::from("/dev/null")), Some(path_s.clone()));
                let r1 = |a: &app::App| a.project.components[a.project.find_ref("R1").unwrap()].value.clone();
                println!("opened: R1 = {}", r1(&app));
                // Someone else (e.g. Claude) edits the file.
                std::thread::sleep(std::time::Duration::from_millis(20));
                let text = std::fs::read_to_string(&path).unwrap().replacen("\"value\": \"22\"", "\"value\": \"220\"", 1);
                std::fs::write(&path, text).unwrap();
                app.watch_at = None;
                app.watch_source();
                println!("after outside edit, no local edits: R1 = {} | {}", r1(&app), app.status);
                // Now edit here, then the file changes again: the app must ask.
                app.checkpoint();
                std::thread::sleep(std::time::Duration::from_millis(20));
                let text = std::fs::read_to_string(&path).unwrap().replacen("\"value\": \"220\"", "\"value\": \"330\"", 1);
                std::fs::write(&path, text).unwrap();
                app.watch_at = None;
                app.watch_source();
                println!("with local edits: R1 = {} | banner shown = {}", r1(&app), app.external_change);
                app.reload_from_disk(false);
                println!("after Reload: R1 = {} | {}", r1(&app), app.status);
                return Ok(());
            }
            "--block-test" => {
                // Save the LED-driver part of the demo as a block, insert it twice in an empty project.
                let mut app = app::App::with_ctx(egui::Context::default(), Some(PathBuf::from("/dev/null")), None);
                let ids: Vec<u32> = ["U2", "R2", "Q1", "R3", "D2"].iter().filter_map(|r| app.project.find_ref(r).map(|i| app.project.components[i].id)).collect();
                app.set_selection(ids.iter().map(|&id| app::Sel::Comp(id)).collect());
                app.analyze();
                let b = app.make_block("LED driver").unwrap_or_else(|e| panic!("{}", e));
                println!("block: {} comps ({} power), {} wires, {} labels, {} tracks, {} vias", b.components.len(), b.components.iter().filter(|c| c.kind.is_power()).count(), b.wires.len(), b.labels.len(), b.tracks.len(), b.vias.len());
                let path = blocks::save(&b).unwrap_or_else(|e| panic!("{}", e));
                let b = blocks::load(&path).expect("reload");
                println!("saved and reloaded {} ({} blocks listed)", path.display(), blocks::list().len());
                let mut app2 = app::App::with_ctx(egui::Context::default(), Some(PathBuf::from("/dev/null")), None);
                app2.project = model::Project { name: "empty".into(), ..app2.project.clone() };
                app2.project.components.clear();
                app2.project.wires.clear();
                app2.project.labels.clear();
                app2.project.tracks.clear();
                app2.project.vias.clear();
                for k in 0..2 {
                    let m = app2.insert_block(&b, true);
                    if k == 0 {
                        println!("{}", m);
                    }
                }
                app2.analyze();
                // Edit the saved block: change a value, save, check the project comes back intact.
                let n_before = app2.project.components.len();
                app2.begin_block_edit(&b);
                let r2 = app2.project.find_ref("R2").unwrap();
                app2.project.components[r2].value = "4k7".into();
                app2.dirty = true;
                app2.blocks.editing.as_mut().unwrap().name = "LED driver v2".into();
                app2.save_block_edit();
                println!("editing: {} comps in editor, saved dirty={}", app2.project.components.len(), app2.dirty);
                app2.end_block_edit();
                println!("back: {} comps (was {}), name {:?}", app2.project.components.len(), n_before, app2.project.name);
                let again = blocks::list().into_iter().find(|i| i.name == "LED driver v2").and_then(|i| blocks::load(&i.path)).expect("v2");
                println!("v2 R2 value: {}", again.components.iter().find(|c| c.reference == "R2").map(|c| c.value.clone()).unwrap_or_default());
                let refs: Vec<String> = app2.project.components.iter().filter(|c| !c.kind.is_power()).map(|c| c.reference.clone()).collect();
                println!("refs after 2 inserts: {:?}", refs);
                let uniq: std::collections::BTreeSet<_> = refs.iter().collect();
                println!("unique refs: {} | wires {} | selected group {}", uniq.len() == refs.len(), app2.project.wires.len(), app2.group.len());
                return Ok(());
            }
            "--block-info" if i + 1 < args.len() => {
                let b = blocks::load(std::path::Path::new(&args[i + 1])).expect("block");
                let mut app = app::App::with_ctx(egui::Context::default(), Some(PathBuf::from("/dev/null")), None);
                app.begin_block_edit(&b);
                app.analyze();
                let board = &app.project.board;
                println!("board {:.1},{:.1} -> {:.1},{:.1}, {} layers", board.x0, board.y0, board.x1, board.y1, board.layers);
                let errs: Vec<_> = app.drc.iter().filter(|i| i.severity == checks::Severity::Error).collect();
                println!("DRC errors with a marker: {} / {} issues", errs.iter().filter(|i| i.pcb_pos.is_some()).count(), app.drc.len());
                let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
                for i in &app.drc {
                    *kinds.entry(format!("{:?}: {}", i.severity, i.message.chars().take(70).collect::<String>())).or_default() += 1;
                }
                for (k, n) in kinds { println!("  {}x {}", n, k); }
                println!("vias {}, unrouted {}", app.project.vias.len(), app.conn.unrouted);
                for c in app.project.components.iter().enumerate().filter(|(_, c)| c.reference == "A1" || c.reference == "IC1") {
                    let pads: Vec<_> = app.conn.pads.iter().filter(|p| p.ci == c.0).collect();
                    let nets: std::collections::BTreeSet<String> = pads.iter().filter_map(|p| p.net).map(|n| app.nl.nets[n].name.clone()).collect();
                    println!("{}: {} pads drawn, {} nets", c.1.reference, pads.len(), nets.len());
                    if c.1.reference == "A1" {
                        for p in &pads { println!("   pad at ({:.2},{:.2}) size {:?} net {:?}", p.pos.0, p.pos.1, p.size, p.net.map(|n| app.nl.nets[n].name.clone())); }
                    }
                }
                return Ok(());
            }
            "--nc-test" => {
                let mut app = app::App::with_ctx(egui::Context::default(), Some(PathBuf::from("/dev/null")), None);
                let id = app.project.add_component(model::Kind::Resistor, (600, 600), 0);
                app.analyze();
                let count = |a: &app::App| a.erc.iter().filter(|i| i.message.contains("not connected to anything")).count();
                println!("open-pin warnings with a lone resistor: {}", count(&app));
                let ci = app.project.find(id).unwrap();
                let c = app.project.components[ci].clone();
                for pd in c.info().pins.iter() {
                    app.project.no_connects.push(c.pin_pos(pd));
                }
                app.analyze();
                println!("after marking both pins as no-connect: {} (marks {})", count(&app), app.project.no_connects.len());
                app.project.components[ci].pos.0 += 50;
                app.analyze();
                println!("after moving the resistor: warnings {} , marks kept {}", count(&app), app.project.no_connects.len());
                return Ok(());
            }
            "--fp-test" if i + 1 < args.len() => {
                let mut app = app::App::with_ctx(egui::Context::default(), Some(PathBuf::from("/dev/null")), None);
                let id = app.project.add_component(model::Kind::Resistor, (600, 600), 0);
                let ci = app.project.find(id).unwrap();
                let show = |c: &model::Component, t: &str| {
                    let inf = c.info();
                    println!("{}: {} pads={:?}", t, inf.footprint, inf.pins.iter().map(|p| (p.pad.clone(), p.pad_pos, p.pad_size)).collect::<Vec<_>>());
                };
                show(&app.project.components[ci], "before");
                match online::fetch_footprint(&args[i + 1]) {
                    Ok(o) => {
                        println!("loaded {} pads, {} silk", o.pads.len(), o.silk.len());
                        app.project.components[ci].fp_over = Some(Box::new(o));
                    }
                    Err(e) => println!("error: {}", e),
                }
                show(&app.project.components[ci], "after");
                let s = serde_json::to_string(&app.project).unwrap();
                let back: model::Project = serde_json::from_str(&s).unwrap();
                println!("saved+reloaded keeps override: {}", back.components.iter().any(|c| c.id == id && c.fp_over.is_some()));
                return Ok(());
            }
            "--zone-test" => {
                // Draw a GND zone over the demo board, check connectivity and the exports.
                let mut p = model::demo_project();
                let b = p.board.clone();
                p.zones.push(model::Zone::new(model::Layer::Back, "GND", vec![(b.x0 + 1.0, b.y0 + 1.0), (b.x1 - 1.0, b.y0 + 1.0), (b.x1 - 1.0, b.y1 - 1.0), (b.x0 + 1.0, b.y1 - 1.0)]));
                let nl = model::build_netlist(&p);
                let (_, conn) = checks::drc(&p, &nl);
                let base = model::demo_project();
                let nl0 = model::build_netlist(&base);
                let (_, conn0) = checks::drc(&base, &nl0);
                println!("unrouted without zone {} -> with GND zone on back {}", conn0.unrouted, conn.unrouted);
                let dir = std::env::temp_dir().join("ferriscad-zone-test");
                let out = dir.join("z.kicad_pro");
                kicad_export::export(&p, &nl, &conn, &out).map(|_| ()).unwrap_or_else(|e| println!("export error {}", e));
                let back = kicad::import(&out).map(|x| x.project).unwrap_or_else(|e| panic!("{}", e));
                println!("re-import: zones {} (drawn {}), outlines {}", back.zones.len(), back.zones.iter().filter(|z| z.drawn).count(), back.zones.iter().map(|z| z.outlines.len()).sum::<usize>());
                match jlc::export(&p, &conn, &dir.join("jlc")) {
                    Ok(e) => e.notes.iter().for_each(|n| println!("note: {}", n)),
                    Err(e) => println!("jlc error {}", e),
                }
                return Ok(());
            }
            "--kicad-roundtrip" if i + 1 < args.len() => {
                // Export a project to KiCad, import it back, compare every connection.
                let src = &args[i + 1];
                let mut p = if src == "demo" { model::demo_project() } else { kicad::import(std::path::Path::new(src)).map(|x| x.project).unwrap_or_else(|e| panic!("{}", e)) };
                if src == "demo" {
                    p.texts.push(model::BoardText { text: "Test v1".into(), pos: (10.0, 5.0), size: 1.5, rot: 1, layer: model::TextLayer::SilkBack });
                    use model::{Note, NoteColor, NoteKind};
                    p.notes.push(Note { a: (60, 40), b: (60, 420), dashed: true, color: NoteColor::Gray, filled: false, ..Default::default() });
                    p.notes.push(Note { kind: NoteKind::Text, a: (90, 30), b: (90, 30), text: "POWER INPUT".into(), size: 20, filled: false, ..Default::default() });
                    p.notes.push(Note { kind: NoteKind::Box, a: (90, 440), b: (330, 520), text: "Note: R1 limits the LED current.\nCheck \"D1\" polarity.".into(), color: NoteColor::Orange, ..Default::default() });
                    p.notes.push(Note { kind: NoteKind::Box, a: (360, 60), b: (760, 400), color: NoteColor::Green, dashed: true, filled: false, ..Default::default() });
                    p.texts.push(model::BoardText { text: "GND".into(), pos: (5.0, 8.0), size: 1.0, rot: 0, layer: model::TextLayer::Copper(model::Layer::Front) });
                }
                let nl = model::build_netlist(&p);
                let (_, conn) = checks::drc(&p, &nl);
                let dir = std::env::temp_dir().join("ferriscad-kicad-export");
                let out = dir.join(format!("{}.kicad_pro", if src == "demo" { "demo" } else { "roundtrip" }));
                match kicad_export::export(&p, &nl, &conn, &out) {
                    Ok(files) => files.iter().for_each(|f| println!("wrote {} ({} bytes)", f.display(), std::fs::metadata(f).map(|m| m.len()).unwrap_or(0))),
                    Err(e) => {
                        println!("export error: {}", e);
                        return Ok(());
                    }
                }
                let back = kicad::import(&out).map(|x| x.project).unwrap_or_else(|e| panic!("re-import: {}", e));
                let conns = |p: &model::Project| -> std::collections::BTreeSet<String> {
                    let nl = model::build_netlist(p);
                    let mut groups: Vec<Vec<String>> = nl
                        .nets
                        .iter()
                        .enumerate()
                        .map(|(n, _)| {
                            let mut v: Vec<String> = nl
                                .part_pins(p, n)
                                .map(|(ci, pi)| format!("{}.{}", p.components[ci].reference, p.components[ci].info().pins[pi].name))
                                .collect();
                            v.sort();
                            v
                        })
                        .filter(|v| v.len() > 1)
                        .collect();
                    groups.sort();
                    groups.into_iter().map(|g| g.join(" ")).collect()
                };
                let (a, b) = (conns(&p), conns(&back));
                println!("nets with 2+ pins: original {}, re-imported {}", a.len(), b.len());
                for d in a.symmetric_difference(&b).take(8) {
                    println!("  DIFF {}{}", if a.contains(d) { "only original: " } else { "only re-import: " }, d);
                }
                let kinds_same = p.components.iter().filter(|c| !c.kind.is_power()).all(|c| back.find_ref(&c.reference).map_or(false, |i| back.components[i].kind == c.kind));
                println!("part types kept: {}", kinds_same);
                let tsig = |p: &model::Project| -> Vec<String> {
                    let mut v: Vec<String> = p.texts.iter().map(|t| format!("{}|{}|{}|{:.2}", t.text, t.layer.name(), t.rot, t.size)).collect();
                    v.sort();
                    v
                };
                let shape = |p: &model::Project| { let mut v: Vec<String> = p.notes.iter().map(|n| { let o = p.components.first().map(|c| c.pos).unwrap_or_default(); format!("{:?} {:?} {:?} {:?} {} {:?} {} {}", n.kind, (n.a.0 - o.0, n.a.1 - o.1), (n.b.0 - o.0, n.b.1 - o.1), n.text, if n.kind == model::NoteKind::Line { 0 } else { n.size }, n.color, n.dashed, n.filled) }).collect(); v.sort(); v };
                println!("schematic notes: {} -> {}, same: {}", p.notes.len(), back.notes.len(), shape(&p) == shape(&back));
                println!("board texts: {} -> {}, same: {}", p.texts.len(), back.texts.len(), tsig(&p) == tsig(&back));
                if tsig(&p) != tsig(&back) {
                    println!("  original {:?}\n  re-import {:?}", tsig(&p), tsig(&back));
                }
                let nl2 = model::build_netlist(&back);
                let (_, conn2) = checks::drc(&back, &nl2);
                println!("board: tracks {} -> {}, vias {} -> {}, unrouted {} -> {}, layers {} -> {}", p.tracks.len(), back.tracks.len(), p.vias.len(), back.vias.len(), conn.unrouted, conn2.unrouted, p.board.layers, back.board.layers);
                return Ok(());
            }
            "--export-kicad" if i + 1 < args.len() => {
                // Export the example project to a KiCad project at the given path (as the menu does).
                let mut app = app::App::with_ctx(egui::Context::default(), Some(PathBuf::from("/dev/null")), None);
                println!("real path: {}", filedlg::host_path(std::path::Path::new(&args[i + 1])).display());
                app.export_kicad(&args[i + 1]);
                println!("{}", app.status);
                return Ok(());
            }
            "--board" => {
                std::env::set_var("FERRISCAD_START_BOARD", "1");
                i += 1;
            }
            "--3d" => {
                std::env::set_var("FERRISCAD_START_3D", "1");
                i += 1;
            }
            "--ai-edit" if i + 1 < args.len() => {
                // Ask the configured AI for a change on the example circuit, apply every
                // proposed action, and print what happened (no window).
                let mut app = app::App::with_ctx(egui::Context::default(), None, None);
                let before = app.project.components.len();
                app.ask_ai(args[i + 1].clone());
                while app.waiting_for_ai() {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    app.poll_ai();
                }
                let mi = app.chat.len() - 1;
                println!("--- AI answer ---\n{}", app.chat[mi].raw);
                println!("--- {} change(s) proposed ---", app.chat[mi].actions.len());
                app.apply_ai_changes(mi, None);
                app.analyze();
                for m in &app.chat[mi + 1..] {
                    println!("[{:?}] {}", m.role, m.text);
                }
                println!("parts: {} -> {}", before, app.project.components.len());
                for is in app.all_issues().iter().filter(|i| i.source != "DRC") {
                    println!("  [{:?}][{}] {}", is.severity, is.source, is.message);
                }
                return Ok(());
            }
            "--import-report" if i + 1 < args.len() => {
                match kicad::import(std::path::Path::new(&args[i + 1])) {
                    Ok(im) => {
                        for n in &im.notes {
                            println!("note: {}", n);
                        }
                        for c in &im.project.components {
                            let pins: Vec<String> = c.info().pins.iter().map(|p| format!("{}@{:?}", p.name, c.pin_pos(p))).collect();
                            println!("PART {} {:?} {} lib={} pos={:?} pins={}", c.reference, c.kind, c.value,
                                c.custom.as_ref().map(|x| x.lib.clone()).unwrap_or_default(), c.pos, pins.join(" "));
                        }
                        report(&im.project);
                    }
                    Err(e) => println!("import failed: {}", e),
                }
                return Ok(());
            }
            "--ask" if i + 1 < args.len() => {
                // Ask the configured AI about the example circuit (connection test).
                let p = model::demo_project();
                let nl = model::build_netlist(&p);
                let mut issues = checks::erc(&p, &nl);
                let (drc, conn) = checks::drc(&p, &nl);
                let sim = sim::simulate(&p, &nl);
                issues.extend(sim.issues.iter().cloned());
                issues.extend(drc);
                let mut settings = ai::AiSettings::load();
                if let Ok(d) = std::env::var("FERRISCAD_TEST_DOCS") {
                    settings.provider = ai::Provider::ClaudeCode;
                    settings.allow_files = true;
                    settings.doc_dirs = vec![d];
                }
                println!("provider: {}", settings.describe());
                let system = ai::system_prompt(&ai::circuit_report(&p, &nl, &issues, &sim, &conn)) + &settings.files_note();
                match ai::ask(&settings, &system, &vec![("user".to_string(), args[i + 1].clone())]) {
                    Ok(a) => println!("{}", a),
                    Err(e) => println!("error: {}", e),
                }
                return Ok(());
            }
            "--kicad-selftest" => {
                let dir = std::env::temp_dir().join("ferriscad_kicad_test");
                let _ = std::fs::create_dir_all(&dir);
                std::fs::write(dir.join("demo.kicad_sch"), kicad::SAMPLE_SCH).unwrap();
                std::fs::write(dir.join("demo.kicad_pcb"), kicad::SAMPLE_PCB).unwrap();
                match kicad::import(&dir.join("demo.kicad_pro")) {
                    Ok(im) => {
                        for n in &im.notes {
                            println!("note: {}", n);
                        }
                        report(&im.project);
                    }
                    Err(e) => println!("import failed: {}", e),
                }
                return Ok(());
            }
            "--selftest" => {
                selftest();
                return Ok(());
            }
            a => {
                open = Some(a.to_string());
                i += 1;
            }
        }
    }
    let mut vp = egui::ViewportBuilder::default()
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([960.0, 600.0])
        .with_title("FerrisCad — schematic, board layout & AI debugging");
    if screenshot.is_some() {
        vp = vp.with_active(false);
        #[cfg(target_os = "windows")]
        {
            vp = vp.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions { viewport: vp, ..Default::default() };
    if screenshot.is_some() {
        // A hidden window gets no frame callbacks on Wayland; don't wait for them.
        options.vsync = false;
    }
    #[cfg(target_os = "macos")]
    if screenshot.is_some() {
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            b.with_activate_ignoring_other_apps(false);
        }));
    }
    eframe::run_native("FerrisCad", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, screenshot, open)))))
}
