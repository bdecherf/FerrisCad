#![allow(dead_code)]
//! FerrisCad: schematic capture + PCB layout with an AI assistant for circuit debugging.

mod ai;
mod aiedit;
mod app;
mod checks;
mod filedlg;
mod jlc;
mod jlcui;
mod kicad;
mod model;
mod multisel;
mod online;
mod pcb;
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
