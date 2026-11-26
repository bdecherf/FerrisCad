//! Reusable circuit blocks: a part of the schematic and board saved once (an nRF52840 with its
//! decoupling and antenna, say) and inserted into any project. An inserted block is a plain,
//! independent copy: parts, wires, tracks, zones... can all be edited afterwards.

use crate::app::{App, Sel};
use crate::model::*;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Block {
    pub name: String,
    /// Copper layers of the board the block was made on.
    pub layers: u8,
    /// Board clearance rule (mm) the block was routed with.
    #[serde(default)]
    pub clearance: Option<f32>,
    pub components: Vec<Component>,
    pub wires: Vec<Wire>,
    pub labels: Vec<Label>,
    #[serde(default)]
    pub no_connects: Vec<(i32, i32)>,
    pub tracks: Vec<Track>,
    pub vias: Vec<Via>,
    #[serde(default)]
    pub zones: Vec<Zone>,
    #[serde(default)]
    pub texts: Vec<BoardText>,
    #[serde(default)]
    pub notes: Vec<Note>,
}

/// A block opened in the editor: the project it replaced is kept here until it is closed.
pub struct BlockEdit {
    pub name: String,
    pub project: Project,
    pub undo: Vec<Project>,
    pub redo: Vec<Project>,
    pub path: String,
    pub dirty: bool,
    pub source: Option<String>,
    pub source_stamp: Option<std::time::SystemTime>,
    pub project_dir: Option<PathBuf>,
    pub edited_since_load: bool,
    pub mode: crate::app::Mode,
}

pub struct BlockInfo {
    pub name: String,
    pub path: PathBuf,
    pub parts: usize,
    pub tracks: usize,
    pub layers: u8,
}

pub struct BlocksState {
    pub open: bool,
    pub name: String,
    /// Labels of an inserted block join the project's labels of the same name.
    pub merge: bool,
    pub list: Vec<BlockInfo>,
    pub loaded: bool,
    /// Set while a block is being edited.
    pub editing: Option<BlockEdit>,
    pub msg: String,
}

impl Default for BlocksState {
    fn default() -> Self {
        BlocksState { open: false, name: String::new(), merge: true, list: vec![], loaded: false, editing: None, msg: String::new() }
    }
}

pub fn dir() -> Option<PathBuf> {
    crate::ai::config_dir().map(|d| d.join("blocks"))
}

fn file_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || "-_.+ ".contains(c) { c } else { '_' }).collect();
    format!("{}.fblock", s.trim())
}

pub fn list() -> Vec<BlockInfo> {
    let mut v = vec![];
    if let Some(d) = dir() {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let path = e.path();
                if path.extension().map_or(true, |x| x != "fblock") {
                    continue;
                }
                if let Some(b) = load(&path) {
                    let parts = b.components.iter().filter(|c| !c.kind.is_power()).count();
                    v.push(BlockInfo { name: b.name.clone(), path, parts, tracks: b.tracks.len(), layers: b.layers });
                }
            }
        }
    }
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

pub fn load(path: &std::path::Path) -> Option<Block> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn save(b: &Block) -> Result<PathBuf, String> {
    let d = dir().ok_or("No settings folder to store blocks in.")?;
    std::fs::create_dir_all(&d).map_err(|e| format!("can't create {}: {}", d.display(), e))?;
    let path = d.join(file_name(&b.name));
    let text = serde_json::to_string_pretty(b).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("can't write {}: {}", path.display(), e))?;
    Ok(path)
}

impl Block {
    /// The block as a small project (its own board, sized around the parts).
    pub fn to_project(&self) -> Project {
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for c in self.components.iter().filter(|c| c.has_fp()) {
            let bb = c.pcb_bbox();
            b = (b.0.min(bb.0), b.1.min(bb.1), b.2.max(bb.2), b.3.max(bb.3));
        }
        for t in &self.tracks {
            for q in [t.a, t.b] {
                b = (b.0.min(q.0), b.1.min(q.1), b.2.max(q.0), b.3.max(q.1));
            }
        }
        let mut board = Board { layers: self.layers.max(2), clearance: self.clearance.unwrap_or(0.2), ..Board::default() };
        if b.0 <= b.2 {
            board = Board { x0: b.0 - 5.0, y0: b.1 - 5.0, x1: b.2 + 5.0, y1: b.3 + 5.0, ..board };
        }
        Project {
            name: self.name.clone(),
            components: self.components.clone(),
            wires: self.wires.clone(),
            labels: self.labels.clone(),
            tracks: self.tracks.clone(),
            vias: self.vias.clone(),
            board,
            next_id: self.components.iter().map(|c| c.id).max().unwrap_or(0),
            no_connects: self.no_connects.clone(),
            zones: self.zones.clone(),
            texts: self.texts.clone(),
            notes: self.notes.clone(),
        }
    }

    pub fn from_project(name: &str, p: &Project) -> Block {
        Block {
            name: name.to_string(),
            layers: p.board.layers,
            clearance: Some(p.board.clearance),
            components: p.components.clone(),
            wires: p.wires.clone(),
            labels: p.labels.clone(),
            no_connects: p.no_connects.clone(),
            tracks: p.tracks.clone(),
            vias: p.vias.clone(),
            zones: p.zones.clone(),
            texts: p.texts.clone(),
            notes: p.notes.clone(),
        }
    }
}

impl App {
    /// Bar shown while a block is being edited.
    pub fn block_banner(&mut self, ctx: &egui::Context) {
        let dirty = self.dirty;
        let Some(ed) = self.blocks.editing.as_mut() else { return };
        let mut act = 0; // 1 save, 2 save and close, 3 close
        egui::TopBottomPanel::top("block_edit").frame(egui::Frame::none().fill(egui::Color32::from_rgb(255, 243, 205)).inner_margin(egui::Margin::symmetric(8.0, 6.0))).show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("✏ Editing a block").strong().color(egui::Color32::from_rgb(120, 80, 0)));
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut ed.name).desired_width(160.0));
                if ui.button("Save block").on_hover_text("Ctrl+S. A new name saves a new block.").clicked() {
                    act = 1;
                }
                if ui.button("Save and close").clicked() {
                    act = 2;
                }
                if ui.button(if dirty { "Close without saving" } else { "Close" }).clicked() {
                    act = 3;
                }
                ui.label(egui::RichText::new("Your project is untouched until you close this.").small().color(egui::Color32::GRAY));
            });
        });
        match act {
            1 => self.save_block_edit(),
            2 => {
                self.save_block_edit();
                if !self.dirty {
                    self.end_block_edit();
                }
            }
            3 => self.end_block_edit(),
            _ => {}
        }
    }

    /// Open a saved block in the editor, in place of the project (which is put aside).
    pub fn begin_block_edit(&mut self, b: &Block) {
        if self.blocks.editing.is_some() {
            self.status = "Already editing a block: close it first.".into();
            return;
        }
        let stash = BlockEdit {
            name: b.name.clone(),
            project: std::mem::take(&mut self.project),
            undo: std::mem::take(&mut self.undo),
            redo: std::mem::take(&mut self.redo),
            path: std::mem::take(&mut self.path),
            dirty: self.dirty,
            source: self.source.take(),
            source_stamp: self.source_stamp.take(),
            project_dir: self.take_project_dir(),
            edited_since_load: self.edited_since_load,
            mode: self.mode,
        };
        self.blocks.editing = Some(stash);
        self.project = b.to_project();
        let mut guessed = false;
        if b.clearance.is_none() {
            // Older block: it doesn't say which clearance rule it was routed with. Use the tightest
            // gap actually routed in it, so the checks don't flag the original routing.
            self.analyze();
            let gap = self
                .drc
                .iter()
                .filter(|i| i.severity == crate::checks::Severity::Error && i.message.contains("mm gap"))
                .filter_map(|i| i.message.split_whitespace().zip(i.message.split_whitespace().skip(1)).find(|(_, n)| n.starts_with("mm")).and_then(|(v, _)| v.parse::<f32>().ok()))
                .fold(f32::MAX, f32::min);
            if gap < 0.2 {
                self.project.board.clearance = ((gap / 0.05).floor() * 0.05).max(0.05);
                guessed = true;
            }
        }
        self.dirty = false;
        self.edited_since_load = false;
        self.reset_editor_state();
        self.status = format!("Editing block \"{}\". Save block when you are done, Close to go back to your project.", b.name);
        if guessed {
            self.status.push_str(&format!(" Clearance set to {:.2} mm to match its routing (Clearance in the board panel).", self.project.board.clearance));
        }
    }

    /// Write the block being edited to the blocks folder.
    pub fn save_block_edit(&mut self) {
        let Some(ed) = self.blocks.editing.as_ref() else { return };
        let name = ed.name.trim().to_string();
        if name.is_empty() {
            self.status = "Give the block a name first.".into();
            return;
        }
        match save(&Block::from_project(&name, &self.project)) {
            Ok(p) => {
                self.dirty = false;
                self.blocks.loaded = false;
                self.status = format!("Block \"{}\" saved to {}.", name, p.display());
            }
            Err(e) => self.status = e,
        }
    }

    /// Close the block editor and bring the project back.
    pub fn end_block_edit(&mut self) {
        let Some(ed) = self.blocks.editing.take() else { return };
        self.project = ed.project;
        self.undo = ed.undo;
        self.redo = ed.redo;
        self.path = ed.path;
        self.dirty = ed.dirty;
        self.source = ed.source;
        self.source_stamp = ed.source_stamp;
        self.restore_project_dir(ed.project_dir);
        self.edited_since_load = ed.edited_since_load;
        self.mode = ed.mode;
        self.reset_editor_state();
        self.status = "Back to your project.".into();
    }

    /// Build a block from the selected parts: their wires, labels, power symbols, tracks, vias,
    /// zones and texts around them (the nets of the selected parts only).
    pub fn make_block(&self, name: &str) -> Result<Block, String> {
        let p = &self.project;
        let sel = self.selection();
        let parts: Vec<usize> = sel
            .iter()
            .filter_map(|s| if let Sel::Comp(id) = s { p.find(*id) } else { None })
            .filter(|&i| !p.components[i].kind.is_power())
            .collect();
        if parts.is_empty() {
            return Err("Select the parts of the block first (drag a box around them), then save.".into());
        }
        let mut nets: HashSet<usize> = HashSet::new();
        for &ci in &parts {
            for pi in 0..p.components[ci].info().pins.len() {
                if let Some(n) = self.nl.net_of(ci, pi) {
                    nets.insert(n);
                }
            }
        }
        let net_names: HashSet<&str> = nets.iter().map(|&n| self.nl.nets[n].name.as_str()).collect();
        // Schematic area around the parts.
        let mut sb = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for &ci in &parts {
            let b = p.components[ci].sch_bbox();
            sb = (sb.0.min(b.0), sb.1.min(b.1), sb.2.max(b.2), sb.3.max(b.3));
        }
        let m = 100;
        let in_sch = |q: (i32, i32)| q.0 >= sb.0 - m && q.0 <= sb.2 + m && q.1 >= sb.1 - m && q.1 <= sb.3 + m;
        // Board area around the footprints.
        let mut pb = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &ci in parts.iter().filter(|&&ci| p.components[ci].has_fp()) {
            let b = p.components[ci].pcb_bbox();
            pb = (pb.0.min(b.0), pb.1.min(b.1), pb.2.max(b.2), pb.3.max(b.3));
        }
        let has_pcb = pb.0 <= pb.2;
        let in_pcb = |q: (f32, f32), m: f32| has_pcb && q.0 >= pb.0 - m && q.0 <= pb.2 + m && q.1 >= pb.1 - m && q.1 <= pb.3 + m;

        let mut blk = Block { name: name.to_string(), layers: p.board.layers, clearance: Some(p.board.clearance), ..Default::default() };
        let explicit = |f: &dyn Fn(&Sel) -> bool| sel.iter().any(|s| f(s));
        let mut comp_idx: Vec<usize> = parts.clone();
        for (i, c) in p.components.iter().enumerate() {
            if c.kind.is_power() && !comp_idx.contains(&i) {
                let pin_nets = (0..c.info().pins.len()).filter_map(|pi| self.nl.net_of(i, pi));
                if in_sch(c.pos) && pin_nets.into_iter().any(|n| nets.contains(&n)) || explicit(&|s| *s == Sel::Comp(c.id)) {
                    comp_idx.push(i);
                }
            }
        }
        comp_idx.sort();
        blk.components = comp_idx.iter().map(|&i| p.components[i].clone()).collect();
        for (i, w) in p.wires.iter().enumerate() {
            let n = self.nl.wire_net.get(i).copied();
            if (n.map_or(false, |n| nets.contains(&n)) && in_sch(w.a) && in_sch(w.b)) || explicit(&|s| *s == Sel::Wire(i)) {
                blk.wires.push(w.clone());
            }
        }
        for (i, l) in p.labels.iter().enumerate() {
            let n = self.nl.label_net.get(i).copied();
            if (n.map_or(false, |n| nets.contains(&n)) && in_sch(l.pos)) || explicit(&|s| *s == Sel::Label(i)) {
                blk.labels.push(l.clone());
            }
        }
        // Annotations that are selected, or lie entirely within the parts' area.
        for (i, n) in p.notes.iter().enumerate() {
            let b = n.bbox();
            if (in_sch((b.0, b.1)) && in_sch((b.2, b.3))) || explicit(&|s| *s == Sel::Note(i)) {
                blk.notes.push(n.clone());
            }
        }
        blk.no_connects = p.no_connects.iter().copied().filter(|&q| q.0 >= sb.0 - 20 && q.0 <= sb.2 + 20 && q.1 >= sb.1 - 20 && q.1 <= sb.3 + 20).collect();
        for (i, t) in p.tracks.iter().enumerate() {
            let n = self.conn.track_net(i);
            if (n.map_or(false, |n| nets.contains(&n)) && in_pcb(t.a, 6.0) && in_pcb(t.b, 6.0)) || explicit(&|s| *s == Sel::Track(i)) {
                blk.tracks.push(t.clone());
            }
        }
        for (i, v) in p.vias.iter().enumerate() {
            let n = self.conn.via_net(i);
            if (n.map_or(false, |n| nets.contains(&n)) && in_pcb(v.pos, 6.0)) || explicit(&|s| *s == Sel::Via(i)) {
                blk.vias.push(v.clone());
            }
        }
        for (i, t) in p.texts.iter().enumerate() {
            if in_pcb(t.pos, 2.0) || explicit(&|s| *s == Sel::Text(i)) {
                blk.texts.push(t.clone());
            }
        }
        for (i, z) in p.zones.iter().enumerate() {
            let pts: Vec<(f32, f32)> = z.outlines.iter().chain(z.polys.iter()).flatten().copied().collect();
            if (net_names.contains(z.net.as_str()) && !pts.is_empty() && pts.iter().all(|&q| in_pcb(q, 6.0))) || explicit(&|s| *s == Sel::Zone(i)) {
                blk.zones.push(z.clone());
            }
        }
        Ok(blk)
    }

    /// Insert a copy of the block into the project, centred in the current views, and select it
    /// so it can be dragged into place.
    pub fn insert_block(&mut self, b: &Block, merge: bool) -> String {
        self.checkpoint();
        // Offsets: schematic (grid steps) and board (mm).
        let mut sb = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for c in &b.components {
            let bb = c.sch_bbox();
            sb = (sb.0.min(bb.0), sb.1.min(bb.1), sb.2.max(bb.2), sb.3.max(bb.3));
        }
        for w in &b.wires {
            for q in [w.a, w.b] {
                sb = (sb.0.min(q.0), sb.1.min(q.1), sb.2.max(q.0), sb.3.max(q.1));
            }
        }
        for n in &b.notes {
            let q = n.bbox();
            sb = (sb.0.min(q.0), sb.1.min(q.1), sb.2.max(q.2), sb.3.max(q.3));
        }
        let rect = self.canvas_rect;
        let sc = self.sch_view.w(rect.min, rect.center());
        let so = if sb.0 <= sb.2 {
            let g = GRID as f32;
            ((((sc.0 - (sb.0 + sb.2) as f32 / 2.0) / g).round() as i32) * GRID, (((sc.1 - (sb.1 + sb.3) as f32 / 2.0) / g).round() as i32) * GRID)
        } else {
            (0, 0)
        };
        let mut pb = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for c in b.components.iter().filter(|c| c.has_fp()) {
            let bb = c.pcb_bbox();
            pb = (pb.0.min(bb.0), pb.1.min(bb.1), pb.2.max(bb.2), pb.3.max(bb.3));
        }
        let pc = self.pcb_view.w(rect.min, rect.center());
        let po = if pb.0 <= pb.2 { ((pc.0 - (pb.0 + pb.2) / 2.0).round(), (pc.1 - (pb.1 + pb.3) / 2.0).round()) } else { (0.0, 0.0) };
        let mv = |q: (f32, f32)| (q.0 + po.0, q.1 + po.1);

        // Net names a label may keep or must change.
        let mut taken: HashSet<String> = self.project.labels.iter().map(|l| l.text.clone()).collect();
        taken.extend(self.project.components.iter().filter(|c| c.kind.is_power()).map(|c| if c.kind == Kind::Gnd { "GND".to_string() } else { c.value.clone() }));
        let mut renamed: std::collections::HashMap<String, String> = Default::default();
        if !merge {
            for l in &b.labels {
                if taken.contains(&l.text) && !renamed.contains_key(&l.text) {
                    let mut k = 2;
                    while taken.contains(&format!("{}_{}", l.text, k)) || renamed.values().any(|v| *v == format!("{}_{}", l.text, k)) {
                        k += 1;
                    }
                    renamed.insert(l.text.clone(), format!("{}_{}", l.text, k));
                }
            }
        }

        let mut items: Vec<Sel> = vec![];
        for c in &b.components {
            let mut c = c.clone();
            self.project.next_id = self.project.next_id.max(self.project.components.iter().map(|c| c.id).max().unwrap_or(0)) + 1;
            c.id = self.project.next_id;
            if !c.kind.is_power() {
                let r: String = c.reference.chars().take_while(|ch| !ch.is_ascii_digit()).collect();
                let prefix = if r.is_empty() { c.info().prefix.to_string() } else { r };
                c.reference = self.project.next_ref(&prefix);
            }
            c.pos = (c.pos.0 + so.0, c.pos.1 + so.1);
            c.pcb_pos = mv(c.pcb_pos);
            items.push(Sel::Comp(c.id));
            self.project.components.push(c);
        }
        for w in &b.wires {
            items.push(Sel::Wire(self.project.wires.len()));
            self.project.wires.push(Wire { a: (w.a.0 + so.0, w.a.1 + so.1), b: (w.b.0 + so.0, w.b.1 + so.1) });
        }
        for l in &b.labels {
            items.push(Sel::Label(self.project.labels.len()));
            self.project.labels.push(Label { pos: (l.pos.0 + so.0, l.pos.1 + so.1), text: renamed.get(&l.text).cloned().unwrap_or_else(|| l.text.clone()) });
        }
        for q in &b.no_connects {
            self.project.no_connects.push((q.0 + so.0, q.1 + so.1));
        }
        for t in &b.tracks {
            items.push(Sel::Track(self.project.tracks.len()));
            self.project.tracks.push(Track { a: mv(t.a), b: mv(t.b), ..t.clone() });
        }
        for v in &b.vias {
            items.push(Sel::Via(self.project.vias.len()));
            self.project.vias.push(Via { pos: mv(v.pos), ..v.clone() });
        }
        for t in &b.texts {
            items.push(Sel::Text(self.project.texts.len()));
            self.project.texts.push(BoardText { pos: mv(t.pos), ..t.clone() });
        }
        for n in &b.notes {
            let mut n = n.clone();
            n.shift(so);
            items.push(Sel::Note(self.project.notes.len()));
            self.project.notes.push(n);
        }
        let nz = b.zones.len();
        for z in &b.zones {
            let mut z = z.clone();
            z.outlines = z.outlines.iter().map(|o| o.iter().map(|&q| mv(q)).collect()).collect();
            z.polys = z.polys.iter().map(|o| o.iter().map(|&q| mv(q)).collect()).collect();
            self.project.zones.push(z);
        }
        self.set_selection(items);
        self.analysis_dirty = true;
        let mut msg = format!(
            "Block \"{}\" inserted ({} parts{}). It's selected: drag it into place, then edit anything you like.",
            b.name,
            b.components.iter().filter(|c| !c.kind.is_power()).count(),
            if nz > 0 { format!(", {} zone(s)", nz) } else { String::new() }
        );
        if b.layers > self.project.board.layers {
            msg.push_str(&format!(" The block was made for {} copper layers, this board has {}: tracks on missing layers are flagged.", b.layers, self.project.board.layers));
        }
        msg
    }

    pub fn blocks_window(&mut self, ctx: &egui::Context) {
        if !self.blocks.open {
            return;
        }
        if !self.blocks.loaded {
            self.blocks.list = list();
            self.blocks.loaded = true;
        }
        let n_sel = self.selection().iter().filter(|s| matches!(s, Sel::Comp(_))).count();
        let mut open = true;
        let mut save_it = false;
        let mut insert: Option<PathBuf> = None;
        let mut delete: Option<PathBuf> = None;
        let mut edit: Option<PathBuf> = None;
        egui::Window::new("Blocks").open(&mut open).default_width(380.0).show(ctx, |ui| {
            ui.label(egui::RichText::new("SAVE THE SELECTION AS A BLOCK").small().strong().color(egui::Color32::GRAY));
            ui.label(format!("{} part(s) selected. The block takes their wires, labels, tracks, vias, zones and texts too.", n_sel));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.blocks.name).hint_text("Block name (e.g. nRF52840)").desired_width(190.0));
                if ui.add_enabled(n_sel > 0 && !self.blocks.name.trim().is_empty(), egui::Button::new("Save block")).clicked() {
                    save_it = true;
                }
            });
            ui.separator();
            ui.label(egui::RichText::new("YOUR BLOCKS").small().strong().color(egui::Color32::GRAY));
            ui.checkbox(&mut self.blocks.merge, "Labels join the labels of the same name already in the project");
            if self.blocks.list.is_empty() {
                ui.label(egui::RichText::new("No block yet.").color(egui::Color32::GRAY));
            }
            for b in &self.blocks.list {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&b.name).strong());
                    ui.label(egui::RichText::new(format!("{} parts, {} tracks, {} layers", b.parts, b.tracks, b.layers)).small().color(egui::Color32::GRAY));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("🗑").on_hover_text("Delete this block (projects that used it are not affected)").clicked() {
                            delete = Some(b.path.clone());
                        }
                        if ui.button("Insert").clicked() {
                            insert = Some(b.path.clone());
                        }
                        if ui.button("Edit").on_hover_text("Open this block in the editor").clicked() {
                            edit = Some(b.path.clone());
                        }
                    });
                });
            }
            if !self.blocks.msg.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new(&self.blocks.msg).small());
            }
            if let Some(d) = dir() {
                ui.label(egui::RichText::new(format!("Stored in {}", d.display())).small().color(egui::Color32::GRAY));
            }
        });
        if !open {
            self.blocks.open = false;
        }
        if save_it {
            let name = self.blocks.name.trim().to_string();
            self.blocks.msg = match self.make_block(&name).and_then(|b| save(&b).map(|p| (b, p))) {
                Ok((b, p)) => {
                    self.blocks.loaded = false;
                    format!("Saved \"{}\" ({} parts, {} wires, {} tracks) to {}.", b.name, b.components.iter().filter(|c| !c.kind.is_power()).count(), b.wires.len(), b.tracks.len(), p.display())
                }
                Err(e) => e,
            };
        }
        if let Some(path) = insert {
            self.blocks.msg = match load(&path) {
                Some(b) => {
                    let merge = self.blocks.merge;
                    let m = self.insert_block(&b, merge);
                    self.status = m.clone();
                    m
                }
                None => "Couldn't read that block.".into(),
            };
        }
        if let Some(path) = edit {
            match load(&path) {
                Some(b) => {
                    self.begin_block_edit(&b);
                    self.blocks.open = false;
                }
                None => self.blocks.msg = "Couldn't read that block.".into(),
            }
        }
        if let Some(path) = delete {
            let _ = std::fs::remove_file(path);
            self.blocks.loaded = false;
        }
    }
}
