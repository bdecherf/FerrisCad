//! The "Manufacture with JLCPCB" window.

use crate::app::{App, Role};
use crate::checks::Severity;
use crate::jlc::{self, JlcPart};
use eframe::egui::{self, Align, Color32, Layout, RichText, Vec2};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

pub enum JlcMsg {
    Found(Vec<u32>, Result<Vec<JlcPart>, String>),
    Auto(Vec<(Vec<u32>, Option<JlcPart>)>),
}

#[derive(Default)]
pub struct JlcState {
    pub open: bool,
    /// The BOM line being searched (its part ids).
    pub line: Option<Vec<u32>>,
    pub query: String,
    pub results: Vec<JlcPart>,
    pub job: Option<Receiver<JlcMsg>>,
    pub status: String,
    pub out_dir: String,
    pub exported: Vec<PathBuf>,
    pub notes: Vec<String>,
}

fn open_path(p: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(p).spawn();
}

impl App {
    fn default_jlc_dir(&self) -> String {
        let base: String = self.project.name.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
        let parent = self.project_dir_for_output();
        parent.join(format!("{}-jlcpcb", if base.is_empty() { "board".into() } else { base })).to_string_lossy().to_string()
    }

    fn set_lcsc(&mut self, ids: &[u32], code: &str) {
        for id in ids {
            if let Some(ci) = self.project.find(*id) {
                self.project.components[ci].lcsc = code.to_string();
            }
        }
        self.dirty = true;
    }

    fn jlc_search(&mut self, ids: Vec<u32>, query: String) {
        if self.jlc.job.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx_handle();
        std::thread::spawn(move || {
            let r = jlc::search(&query);
            let _ = tx.send(JlcMsg::Found(ids, r));
            ctx.request_repaint();
        });
        self.jlc.job = Some(rx);
        self.jlc.status = "Searching the JLCPCB catalogue…".into();
    }

    /// Pick a part for every BOM line that has none: Basic parts in stock, matching package first.
    fn jlc_auto(&mut self) {
        if self.jlc.job.is_some() {
            return;
        }
        let todo: Vec<(Vec<u32>, String, String)> = jlc::bom(&self.project)
            .into_iter()
            .filter(|l| l.lcsc.is_empty())
            .filter_map(|l| {
                let c = &self.project.components[self.project.find(*l.ids.first()?)?];
                let (q, pkg) = jlc::auto_query(c)?;
                Some((l.ids.clone(), q, pkg))
            })
            .collect();
        if todo.is_empty() {
            self.jlc.status = "Every part already has a JLCPCB part number.".into();
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx_handle();
        let n = todo.len();
        std::thread::spawn(move || {
            let mut out = vec![];
            for (ids, q, pkg) in todo {
                // Only a part in stock and in the same package is picked automatically.
                let pick = jlc::search(&q).ok().and_then(|r| {
                    r.iter()
                        .find(|p| p.stock > 0 && (pkg.is_empty() || p.package.to_uppercase().contains(&pkg.to_uppercase())))
                        .cloned()
                });
                out.push((ids, pick));
            }
            let _ = tx.send(JlcMsg::Auto(out));
            ctx.request_repaint();
        });
        self.jlc.job = Some(rx);
        self.jlc.status = format!("Looking up {} part{} at JLCPCB…", n, if n == 1 { "" } else { "s" });
    }

    pub fn poll_jlc(&mut self) {
        let Some(rx) = &self.jlc.job else { return };
        let msg = match rx.try_recv() {
            Ok(m) => m,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.jlc.job = None;
                return;
            }
        };
        self.jlc.job = None;
        match msg {
            JlcMsg::Found(ids, Ok(r)) => {
                self.jlc.status = format!("{} result{}", r.len(), if r.len() == 1 { "" } else { "s" });
                self.jlc.line = Some(ids);
                self.jlc.results = r;
            }
            JlcMsg::Found(_, Err(e)) => self.jlc.status = format!("Search failed: {}", e),
            JlcMsg::Auto(picks) => {
                self.checkpoint();
                let total = picks.len();
                let mut found = 0;
                for (ids, p) in picks {
                    if let Some(p) = p {
                        self.set_lcsc(&ids, &p.code);
                        found += 1;
                    }
                }
                let skipped = jlc::bom(&self.project).iter().filter(|l| l.lcsc.is_empty()).count();
                self.jlc.status = format!(
                    "Found {} of {} part{}. {} line(s) left to choose by hand with Find (batteries, switches, unusual packages…). Basic parts avoid JLCPCB's extra fee.",
                    found,
                    total,
                    if total == 1 { "" } else { "s" },
                    skipped
                );
            }
        }
    }

    pub fn jlc_window(&mut self, ctx: &egui::Context) {
        if !self.jlc.open {
            return;
        }
        if self.jlc.out_dir.is_empty() {
            self.jlc.out_dir = self.default_jlc_dir();
        }
        let mut open = true;
        let accent = Color32::from_rgb(40, 90, 170);
        let mut search: Option<(Vec<u32>, String)> = None;
        let mut use_part: Option<(Vec<u32>, String)> = None;
        let mut edits: Vec<(Vec<u32>, String)> = vec![];
        let mut auto = false;
        let mut export = false;
        let lines = jlc::bom(&self.project);
        egui::Window::new("Manufacture with JLCPCB")
            .collapsible(false)
            .resizable(true)
            .default_size([900.0, 620.0])
            .open(&mut open)
            .show(ctx, |ui| {
                let b = &self.project.board;
                let parts: usize = lines.iter().map(|l| l.refs.len()).sum();
                let missing: usize = lines.iter().filter(|l| l.lcsc.is_empty()).map(|l| l.refs.len()).sum();
                let drc_errors = self.drc.iter().filter(|i| i.severity == Severity::Error).count();
                ui.label(format!(
                    "Board {:.1} × {:.1} mm, {} copper layers · {} parts in {} BOM lines · {} without a JLCPCB part number · {} connection(s) not routed",
                    b.x1 - b.x0,
                    b.y1 - b.y0,
                    b.layers,
                    parts,
                    lines.len(),
                    missing,
                    self.conn.unrouted
                ));
                if drc_errors > 0 || self.conn.unrouted > 0 {
                    ui.colored_label(
                        Color32::from_rgb(190, 110, 0),
                        format!("⚠ The board isn't finished: {} DRC error(s), {} unrouted connection(s). You can still prepare the files.", drc_errors, self.conn.unrouted),
                    );
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("1. Parts").strong().size(15.0));
                    if ui.add_enabled(self.jlc.job.is_none() && missing > 0, egui::Button::new("Find JLCPCB parts for all missing")).clicked() {
                        auto = true;
                    }
                    if self.jlc.job.is_some() {
                        ui.spinner();
                    }
                    ui.label(RichText::new(&self.jlc.status).small().color(Color32::DARK_GRAY));
                });
                egui::ScrollArea::vertical().id_salt("bom").max_height(230.0).show(ui, |ui| {
                    egui::Grid::new("bomgrid").striped(true).num_columns(5).spacing([10.0, 4.0]).show(ui, |ui| {
                        for h in ["Parts", "Value", "Footprint", "JLCPCB part #", ""] {
                            ui.label(RichText::new(h).small().strong());
                        }
                        ui.end_row();
                        for l in &lines {
                            let refs = if l.refs.len() > 6 { format!("{} … ({})", l.refs[..5].join(", "), l.refs.len()) } else { l.refs.join(", ") };
                            ui.add_sized([150.0, 18.0], egui::Label::new(refs).truncate());
                            ui.add_sized([90.0, 18.0], egui::Label::new(&l.comment).truncate());
                            ui.add_sized([200.0, 18.0], egui::Label::new(&l.footprint).truncate());
                            let mut code = l.lcsc.clone();
                            let r = ui.add(egui::TextEdit::singleline(&mut code).desired_width(90.0).hint_text("C…"));
                            if r.changed() {
                                edits.push((l.ids.clone(), code.trim().to_string()));
                            }
                            let label = if l.lcsc.is_empty() { "Find" } else { "Change" };
                            if ui.add_enabled(self.jlc.job.is_none(), egui::Button::new(label).small()).clicked() {
                                let c = self.project.find(l.ids[0]).map(|i| jlc::query_for(&self.project.components[i])).unwrap_or_default();
                                search = Some((l.ids.clone(), c));
                            }
                            ui.end_row();
                        }
                    });
                });
                if let Some(ids) = self.jlc.line.clone() {
                    ui.add_space(4.0);
                    let refs: Vec<String> = ids.iter().filter_map(|id| self.project.find(*id)).map(|i| self.project.components[i].reference.clone()).collect();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Catalogue for {}:", refs.join(", "))).strong());
                        let r = ui.add(egui::TextEdit::singleline(&mut self.jlc.query).desired_width(260.0));
                        if (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || ui.button("🔍 Search").clicked() {
                            search = Some((ids.clone(), self.jlc.query.clone()));
                        }
                        if ui.small_button("Close").clicked() {
                            self.jlc.line = None;
                            self.jlc.results.clear();
                        }
                    });
                    egui::ScrollArea::vertical().id_salt("jlcres").max_height(170.0).show(ui, |ui| {
                        egui::Grid::new("jlcgrid").striped(true).num_columns(8).spacing([10.0, 3.0]).show(ui, |ui| {
                            for h in ["", "Part #", "Type", "Model", "Package", "Stock", "Price", "Description"] {
                                ui.label(RichText::new(h).small().strong());
                            }
                            ui.end_row();
                            for p in &self.jlc.results {
                                if ui.small_button("Use").clicked() {
                                    use_part = Some((ids.clone(), p.code.clone()));
                                }
                                if p.datasheet.is_empty() {
                                    ui.label(&p.code);
                                } else {
                                    ui.hyperlink_to(&p.code, &p.datasheet).on_hover_text("Open the datasheet");
                                }
                                ui.label(if p.basic {
                                    RichText::new("Basic").color(Color32::from_rgb(30, 130, 60))
                                } else {
                                    RichText::new("Extended").color(Color32::from_rgb(170, 100, 0))
                                });
                                ui.add(egui::Label::new(format!("{} {}", p.brand, p.model)).truncate());
                                ui.label(&p.package);
                                ui.label(if p.stock > 0 { p.stock.to_string() } else { "out of stock".into() });
                                ui.label(format!("${:.4}", p.price));
                                ui.add(egui::Label::new(RichText::new(&p.description).small()).truncate());
                                ui.end_row();
                            }
                        });
                    });
                }
                ui.separator();
                ui.label(RichText::new("2. Manufacturing files").strong().size(15.0));
                ui.horizontal(|ui| {
                    ui.label("Folder");
                    ui.add(egui::TextEdit::singleline(&mut self.jlc.out_dir).desired_width(480.0));
                    let b = egui::Button::new(RichText::new("Generate files").color(Color32::WHITE).strong()).fill(accent);
                    if ui.add(b).clicked() {
                        export = true;
                    }
                });
                ui.label(
                    RichText::new("Gerbers + drill (one ZIP), BOM and placement (CPL) in JLCPCB's format.")
                        .small()
                        .color(Color32::GRAY),
                );
                if !self.jlc.exported.is_empty() {
                    for f in &self.jlc.exported {
                        ui.label(format!("✔ {}", f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
                    }
                    for n in &self.jlc.notes {
                        ui.label(RichText::new(format!("• {}", n)).small().color(Color32::from_rgb(150, 100, 0)));
                    }
                    ui.add_space(4.0);
                    ui.label(RichText::new("3. Order").strong().size(15.0));
                    ui.label(
                        "JLCPCB doesn't let other apps place orders. On their page: upload the ZIP, choose \"PCB Assembly\", \
                         then add the BOM and CPL files when asked.",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("📂 Open the folder").clicked() {
                            open_path(&PathBuf::from(&self.jlc.out_dir));
                        }
                        let b = egui::Button::new(RichText::new("Open the JLCPCB order page").color(Color32::WHITE).strong())
                            .fill(Color32::from_rgb(30, 110, 60));
                        if ui.add(b).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(jlc::ORDER_URL));
                        }
                    });
                }
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.label(
                        RichText::new("Part search uses JLCPCB's public website catalogue; it may change without notice.")
                            .small()
                            .color(Color32::GRAY),
                    );
                    ui.add_space(0.0);
                    let _ = Vec2::ZERO;
                });
            });
        if !edits.is_empty() {
            self.checkpoint();
            for (ids, code) in edits {
                self.set_lcsc(&ids, &code);
            }
        }
        if let Some((ids, code)) = use_part {
            self.checkpoint();
            self.set_lcsc(&ids, &code);
            self.jlc.status = format!("Using {}.", code);
            self.jlc.line = None;
            self.jlc.results.clear();
        }
        if let Some((ids, q)) = search {
            self.jlc.query = q.clone();
            self.jlc.line = Some(ids.clone());
            self.jlc_search(ids, q);
        }
        if auto {
            self.jlc_auto();
        }
        if export {
            let dir = PathBuf::from(self.jlc.out_dir.trim());
            match jlc::export(&self.project, &self.conn, &dir) {
                Ok(e) => {
                    self.jlc.exported = e.files.clone();
                    self.jlc.notes = e.notes.clone();
                    self.status = format!("JLCPCB files written to {}", e.dir.display());
                    self.push_note(format!("JLCPCB files ready in {}.", e.dir.display()));
                }
                Err(e) => {
                    self.jlc.exported.clear();
                    self.jlc.notes = vec![format!("Couldn't write the files: {}", e)];
                }
            }
        }
        if !open {
            self.jlc.open = false;
        }
    }

    /// Search for the BOM line already chosen in `self.jlc.line`.
    pub fn jlc_open_search(&mut self, query: String) {
        if let Some(ids) = self.jlc.line.clone() {
            self.jlc_search(ids, query);
        }
    }

    pub fn jlc_auto_public(&mut self) {
        self.jlc_auto();
    }

    pub fn push_note(&mut self, text: String) {
        self.chat_note(Role::Note, text);
    }
}
