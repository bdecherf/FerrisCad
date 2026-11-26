//! Reloading the open project when its file changes on disk (for example when Claude or
//! another program edits it in the background).

use crate::app::{load_any, App, Role, Sel};
use eframe::egui::{self, Color32, RichText, Stroke};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

/// Latest modification time of the project: the file itself, or for a KiCad project every
/// schematic / board / project file in its folder (sub-sheets included).
pub fn project_stamp(path: &str) -> Option<SystemTime> {
    let p = Path::new(path);
    if crate::kicad::is_kicad_path(p) {
        let dir = if p.is_dir() { p.to_path_buf() } else { p.parent()?.to_path_buf() };
        std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .filter(|e| {
                e.path().extension().map_or(false, |x| x == "kicad_sch" || x == "kicad_pcb" || x == "kicad_pro")
                    && !e.file_name().to_string_lossy().starts_with("_autosave")
            })
            .filter_map(|e| e.metadata().ok()?.modified().ok())
            .max()
    } else {
        std::fs::metadata(p).ok()?.modified().ok()
    }
}

impl App {
    /// Remember which file the project came from, to notice later changes.
    pub fn track_source(&mut self, path: &str) {
        self.source = Some(path.to_string());
        self.source_stamp = project_stamp(path);
        self.edited_since_load = false;
        self.external_change = false;
    }

    /// Checked about once a second.
    pub fn watch_source(&mut self) {
        let now = Instant::now();
        if self.watch_at.map_or(false, |t| now.duration_since(t) < Duration::from_secs(1)) {
            return;
        }
        self.watch_at = Some(now);
        let Some(src) = self.source.clone() else { return };
        let Some(stamp) = project_stamp(&src) else { return };
        if self.source_stamp.map_or(true, |old| stamp > old) && !self.external_change {
            if self.edited_since_load {
                // You changed things here too: ask before replacing them.
                self.external_change = true;
            } else {
                self.reload_from_disk(true);
            }
        }
        self.ctx_handle().request_repaint_after(Duration::from_secs(1));
    }

    /// Reload the open project from disk (undo brings the previous version back).
    pub fn reload_from_disk(&mut self, automatic: bool) {
        let Some(src) = self.source.clone() else {
            self.status = "Nothing to reload: this project hasn't been opened from or saved to a file yet.".into();
            return;
        };
        match load_any(&src) {
            Ok((p, _notes)) => {
                self.checkpoint();
                self.project = p;
                self.sel = Sel::None;
                self.group.clear();
                self.drag = None;
                self.wire_draw = None;
                self.note_draft = None;
                self.route = None;
                self.analysis_dirty = true;
                self.track_source(&src);
                let name = Path::new(&src).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(src.clone());
                self.status = if automatic {
                    format!("{} changed on disk and was reloaded (Ctrl+Z goes back).", name)
                } else {
                    format!("Reloaded {} from disk (Ctrl+Z goes back).", name)
                };
                self.chat_note(Role::Note, self.status.clone());
            }
            Err(e) => {
                // A half-written file: try again on the next check.
                self.source_stamp = None;
                if !automatic {
                    self.status = format!("Couldn't reload {}: {}", src, e);
                }
            }
        }
    }

    /// Banner shown when the file changed on disk while you have edits of your own.
    pub fn reload_banner(&mut self, ctx: &egui::Context) {
        if !self.external_change {
            return;
        }
        let name = self.source.as_deref().map(|s| Path::new(s).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).unwrap_or_default();
        let mut reload = false;
        let mut keep = false;
        egui::TopBottomPanel::top("reload_banner")
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(255, 243, 205))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(230, 180, 60)))
                    .inner_margin(egui::Margin::symmetric(10.0, 6.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "⚠ {} was changed outside FerrisCad (by Claude or another program), and you have changes here too.",
                            name
                        ))
                        .color(Color32::from_rgb(120, 80, 0)),
                    );
                    let b = egui::Button::new(RichText::new("Reload from disk").color(Color32::WHITE).strong()).fill(Color32::from_rgb(40, 90, 170));
                    if ui.add(b).on_hover_text("Your changes stay available with Ctrl+Z").clicked() {
                        reload = true;
                    }
                    if ui.button("Keep my version").clicked() {
                        keep = true;
                    }
                });
            });
        if reload {
            self.reload_from_disk(false);
        }
        if keep {
            self.external_change = false;
            self.source_stamp = self.source.as_deref().and_then(project_stamp);
        }
    }
}
