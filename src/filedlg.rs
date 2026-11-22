//! File dialogs: the system's native dialog (desktop portal on Linux, system dialog on
//! Windows/macOS), with a built-in file browser as a fallback.

use eframe::egui::{self, Align, Align2, Color32, Key, Layout, RichText, Vec2};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Instant, SystemTime};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    Open,
    OpenKicad,
    Save,
}

const KICAD: [&str; 3] = ["kicad_pro", "kicad_sch", "kicad_pcb"];

/// A native dialog running on another thread.
pub struct Native {
    pub purpose: Purpose,
    pub started: Instant,
    pub rx: Receiver<Option<PathBuf>>,
}

pub fn start_native(purpose: Purpose, dir: PathBuf, file_name: String, ctx: egui::Context) -> Native {
    let (tx, rx) = std::sync::mpsc::channel();
    let job = move || {
        let d = rfd::FileDialog::new().set_directory(&dir);
        let r = match purpose {
            Purpose::Save => d.set_title("Save project").add_filter("FerrisCad project", &["fcad", "vpcb"]).set_file_name(&file_name).save_file(),
            Purpose::OpenKicad => d
                .set_title("Open a KiCad project")
                .add_filter("KiCad project", &KICAD)
                .add_filter("FerrisCad project", &["fcad", "vpcb"])
                .pick_file(),
            Purpose::Open => d
                .set_title("Open a project")
                .add_filter("All supported projects", &["fcad", "vpcb", "kicad_pro", "kicad_sch", "kicad_pcb"])
                .add_filter("FerrisCad project", &["fcad", "vpcb"])
                .add_filter("KiCad project", &KICAD)
                .pick_file(),
        };
        let _ = tx.send(r);
        ctx.request_repaint();
    };
    #[cfg(target_os = "macos")]
    job();
    #[cfg(not(target_os = "macos"))]
    std::thread::spawn(job);
    Native { purpose, started: Instant::now(), rx }
}

struct Entry {
    path: PathBuf,
    name: String,
    is_dir: bool,
    kind: &'static str,
    modified: Option<SystemTime>,
    kicad_folder: bool,
}

pub enum Outcome {
    Pending,
    Cancel,
    Pick(PathBuf),
}

/// The built-in browser, used when no native dialog is available.
pub struct Browser {
    pub purpose: Purpose,
    dir: PathBuf,
    dir_text: String,
    name: String,
    selected: Option<PathBuf>,
    entries: Vec<Entry>,
    error: Option<String>,
}

fn kind_of(p: &Path) -> Option<&'static str> {
    match p.extension().and_then(|e| e.to_str()) {
        Some("fcad") | Some("vpcb") => Some("FerrisCad project"),
        Some("kicad_pro") => Some("KiCad project"),
        Some("kicad_sch") => Some("KiCad schematic"),
        Some("kicad_pcb") => Some("KiCad board"),
        _ => None,
    }
}

fn has_kicad_project(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().any(|e| e.path().extension().map_or(false, |x| x == "kicad_pro")))
        .unwrap_or(false)
}

fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn age(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return String::new() };
    let Ok(d) = SystemTime::now().duration_since(t) else { return String::new() };
    let s = d.as_secs();
    if s < 3600 {
        format!("{} min ago", (s / 60).max(1))
    } else if s < 86400 {
        format!("{} h ago", s / 3600)
    } else if s < 86400 * 60 {
        format!("{} days ago", s / 86400)
    } else {
        format!("{} months ago", s / (86400 * 30))
    }
}

impl Browser {
    pub fn new(purpose: Purpose, dir: PathBuf, name: String) -> Self {
        let dir = if dir.is_dir() { dir } else { home() };
        let mut b = Browser { purpose, dir_text: String::new(), dir: PathBuf::new(), name, selected: None, entries: vec![], error: None };
        b.go(dir);
        b
    }

    fn go(&mut self, dir: PathBuf) {
        self.selected = None;
        self.error = None;
        self.entries.clear();
        match std::fs::read_dir(&dir) {
            Ok(rd) => {
                for e in rd.flatten() {
                    let path = e.path();
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') || name.starts_with('_') && name.contains("autosave") {
                        continue;
                    }
                    let is_dir = path.is_dir();
                    let kind = if is_dir {
                        "Folder"
                    } else {
                        match kind_of(&path) {
                            Some(k) if self.purpose != Purpose::Save || k == "FerrisCad project" => k,
                            _ => continue,
                        }
                    };
                    let modified = e.metadata().ok().and_then(|m| m.modified().ok());
                    let kicad_folder = is_dir && has_kicad_project(&path);
                    self.entries.push(Entry { path, name, is_dir, kind, modified, kicad_folder });
                }
                self.entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.dir_text = dir.to_string_lossy().to_string();
        self.dir = dir;
    }

    pub fn ui(&mut self, ctx: &egui::Context, recent: &[String]) -> Outcome {
        let mut out = Outcome::Pending;
        let mut open = true;
        let mut nav: Option<PathBuf> = None;
        let title = match self.purpose {
            Purpose::Open => "Open a project",
            Purpose::OpenKicad => "Open a KiCad project",
            Purpose::Save => "Save project",
        };
        let accent = Color32::from_rgb(40, 90, 170);
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .fixed_size([760.0, 440.0])
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                // Path bar.
                ui.horizontal(|ui| {
                    if ui.button("⬆ Up").on_hover_text("Parent folder").clicked() {
                        if let Some(p) = self.dir.parent() {
                            nav = Some(p.to_path_buf());
                        }
                    }
                    let r = ui.add(egui::TextEdit::singleline(&mut self.dir_text).desired_width(ui.available_width()));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        let p = PathBuf::from(self.dir_text.trim());
                        if p.is_dir() {
                            nav = Some(p);
                        } else if p.is_file() {
                            out = Outcome::Pick(p);
                        }
                    }
                });
                ui.separator();
                let list_h = 360.0;
                ui.horizontal_top(|ui| {
                    // Places and recent projects.
                    ui.allocate_ui_with_layout(Vec2::new(180.0, list_h), Layout::top_down(Align::Min), |ui| {
                        ui.set_width(180.0);
                        ui.label(RichText::new("PLACES").small().strong().color(Color32::GRAY));
                        let h = home();
                        for (label, p) in [
                            ("Home", h.clone()),
                            ("Documents", h.join("Documents")),
                            ("Desktop", h.join("Desktop")),
                            ("FerrisCad projects", h.join("Documents").join("FerrisCad")),
                        ] {
                            if p.is_dir() && ui.selectable_label(self.dir == p, label).clicked() {
                                nav = Some(p);
                            }
                        }
                        if self.purpose != Purpose::Save && !recent.is_empty() {
                            ui.add_space(8.0);
                            ui.label(RichText::new("RECENT").small().strong().color(Color32::GRAY));
                            egui::ScrollArea::vertical().id_salt("recent").show(ui, |ui| {
                                for r in recent.iter().take(10) {
                                    let p = PathBuf::from(r);
                                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| r.clone());
                                    if ui.selectable_label(false, name).on_hover_text(r).clicked() {
                                        out = Outcome::Pick(p);
                                    }
                                }
                            });
                        }
                    });
                    ui.separator();
                    // Folder contents.
                    ui.vertical(|ui| {
                        if let Some(e) = &self.error {
                            ui.colored_label(Color32::from_rgb(190, 40, 40), format!("Can't read this folder: {}", e));
                        }
                        egui::ScrollArea::vertical().id_salt("files").max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                            if self.entries.is_empty() && self.error.is_none() {
                                ui.label(RichText::new("No folders or projects here.").color(Color32::GRAY));
                            }
                            ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                            for e in &self.entries {
                                let sel = self.selected.as_ref() == Some(&e.path);
                                let icon = if e.is_dir { "📁" } else { "📄" };
                                let mut txt = egui::text::LayoutJob::default();
                                let normal = egui::TextFormat { color: ui.visuals().text_color(), ..Default::default() };
                                let small = egui::TextFormat {
                                    color: Color32::GRAY,
                                    font_id: egui::FontId::proportional(11.5),
                                    ..Default::default()
                                };
                                txt.append(&format!("{}  {}", icon, e.name), 0.0, normal);
                                let kind = if e.kicad_folder { "Folder · KiCad project inside" } else { e.kind };
                                txt.append(&format!("    {}  {}", kind, age(e.modified)), 0.0, small);
                                let r = ui.add(egui::SelectableLabel::new(sel, txt));
                                if r.clicked() {
                                    self.selected = Some(e.path.clone());
                                    if !e.is_dir && self.purpose == Purpose::Save {
                                        self.name = e.name.clone();
                                    }
                                }
                                if r.double_clicked() {
                                    if e.is_dir {
                                        nav = Some(e.path.clone());
                                    } else {
                                        out = Outcome::Pick(e.path.clone());
                                    }
                                }
                            }
                            });
                        });
                    });
                });
                ui.separator();
                // Bottom bar.
                ui.horizontal(|ui| {
                    let can;
                    if self.purpose == Purpose::Save {
                        ui.label("File name");
                        ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(280.0));
                        can = !self.name.trim().is_empty();
                    } else {
                        let sel_dir_kicad = self.selected.as_ref().map_or(false, |p| p.is_dir() && has_kicad_project(p));
                        let here_kicad = self.selected.is_none() && has_kicad_project(&self.dir);
                        let msg = match &self.selected {
                            Some(p) if sel_dir_kicad => format!("Folder {} contains a KiCad project", p.file_name().unwrap_or_default().to_string_lossy()),
                            Some(p) if p.is_dir() => "Double-click a folder to open it".into(),
                            Some(p) => p.file_name().unwrap_or_default().to_string_lossy().to_string(),
                            None if here_kicad => "This folder contains a KiCad project".into(),
                            None => "Pick a FerrisCad or KiCad project".into(),
                        };
                        ui.label(RichText::new(msg).color(Color32::DARK_GRAY));
                        can = self.selected.as_ref().map_or(here_kicad, |p| p.is_file() || sel_dir_kicad);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if self.purpose == Purpose::Save { "Save" } else { "Open" };
                        let b = egui::Button::new(RichText::new(label).color(Color32::WHITE).strong()).fill(accent).min_size(Vec2::new(90.0, 26.0));
                        if ui.add_enabled(can, b).clicked() {
                            out = match self.purpose {
                                Purpose::Save => Outcome::Pick(self.dir.join(self.name.trim())),
                                _ => Outcome::Pick(self.selected.clone().unwrap_or_else(|| self.dir.clone())),
                            };
                        }
                        if ui.button("Cancel").clicked() {
                            out = Outcome::Cancel;
                        }
                    });
                });
            });
        if let Some(p) = nav {
            self.go(p);
        }
        if !open {
            return Outcome::Cancel;
        }
        out
    }
}

/// Ask the system for a folder, on another thread.
pub fn start_pick_folder(title: &str, ctx: egui::Context) -> Receiver<Option<PathBuf>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let title = title.to_string();
    let job = move || {
        let r = rfd::FileDialog::new().set_title(&title).set_directory(home()).pick_folder();
        let _ = tx.send(r);
        ctx.request_repaint();
    };
    #[cfg(target_os = "macos")]
    job();
    #[cfg(not(target_os = "macos"))]
    std::thread::spawn(job);
    rx
}
