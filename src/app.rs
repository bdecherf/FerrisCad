//! Main window: menus, tool panels, problems list, AI assistant and the two editors.

use crate::ai::{self, AiSettings, History, Provider};
use crate::checks::{self, Issue, PcbConn, Severity};
use crate::model::*;
use crate::filedlg::Purpose;
use crate::sim::{self, SimResult};
use eframe::egui::{self, Align, Align2, Color32, Key, Layout, Pos2, Rect, RichText, Stroke, Vec2};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Schematic,
    Pcb,
    View3D,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sel {
    None,
    Comp(u32),
    Wire(usize),
    Label(usize),
    Track(usize),
    Via(usize),
    Text(usize),
    Zone(usize),
    /// A schematic annotation.
    Note(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SchTool {
    Select,
    Wire,
    Label,
    /// Click a pin to mark it as deliberately unconnected (or click the mark again to remove it).
    NoConnect,
    Place(Kind),
    /// Draw an annotation.
    Note(NoteKind),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PcbTool {
    Select,
    Route,
    Via,
    Text,
    Zone,
}

#[derive(Clone, Copy, Debug)]
pub struct View {
    pub offset: Vec2,
    pub zoom: f32,
}

impl View {
    pub fn s(&self, min: Pos2, w: (f32, f32)) -> Pos2 {
        min + self.offset + Vec2::new(w.0, w.1) * self.zoom
    }
    pub fn w(&self, min: Pos2, s: Pos2) -> (f32, f32) {
        let v = (s - min - self.offset) / self.zoom;
        (v.x, v.y)
    }
}

#[derive(Clone, Debug)]
pub enum Drag {
    SchComp { id: u32, start: (i32, i32), orig: (i32, i32), attached: Vec<(usize, bool, usize)>, marks: Vec<(usize, u32, usize)> },
    SchLabel { idx: usize, start: (i32, i32), orig: (i32, i32) },
    SchWireEnd { start: (i32, i32), ends: Vec<(usize, bool)> },
    PcbComp { id: u32, start: (f32, f32), orig: (f32, f32) },
    /// Dragging an end of an annotation line or a corner of a text box.
    NoteResize { idx: usize, fixed: (i32, i32), is_a: bool },
    /// Moving one corner of a copper zone: zone, outline, vertex.
    ZoneVertex { zi: usize, oi: usize, vi: usize },
    /// Moving a whole copper zone.
    ZoneMove { zi: usize, start: (f32, f32), orig: Vec<Vec<(f32, f32)>> },
    /// Selection box being dragged (world coordinates); `add` keeps the current selection.
    Band { start: (f32, f32), cur: (f32, f32), add: bool },
    /// Moving several schematic items: parts (id, original position), wire ends
    /// (index, a, b, move a, move b) and labels (index, original position).
    SchGroup {
        start: (i32, i32),
        comps: Vec<(u32, (i32, i32))>,
        wires: Vec<(usize, (i32, i32), (i32, i32), bool, bool)>,
        labels: Vec<(usize, (i32, i32))>,
        notes: Vec<(usize, Note)>,
        /// No-connect marks that follow their pin: (mark, part, pin).
        marks: Vec<(usize, u32, usize)>,
    },
    /// Moving several board items.
    PcbGroup {
        start: (f32, f32),
        comps: Vec<(u32, (f32, f32))>,
        tracks: Vec<(usize, (f32, f32), (f32, f32), bool, bool)>,
        vias: Vec<(usize, (f32, f32))>,
        texts: Vec<(usize, (f32, f32))>,
    },
}

#[derive(Clone, Debug)]
pub struct RouteState {
    pub pts: Vec<(f32, f32)>,
    pub layers: Vec<Layer>,
    pub vias: Vec<(f32, f32)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    User,
    Assistant,
    Note,
    Error,
}

#[derive(Clone, Debug)]
pub struct ChatMsg {
    pub role: Role,
    pub raw: String,
    pub text: String,
    pub actions: Vec<ai::AiAction>,
    pub applied: Vec<bool>,
    pub refs: Vec<String>,
}

pub enum OnlineMsg {
    Hits(String, Result<Vec<crate::online::Hit>, String>),
    Part(String, Result<crate::online::Part, String>),
}

enum Act {
    ShowIssue(usize),
    Ask(String),
    Fix(u32, String),
    /// (message, action) — apply one AI change.
    Apply(usize, usize),
    /// Apply all remaining changes of a message.
    ApplyAll(usize),
    Focus(String),
}

pub struct App {
    pub project: Project,
    pub path: String,
    pub dirty: bool,
    pub undo: Vec<Project>,
    pub redo: Vec<Project>,
    pub mode: Mode,
    pub sch_view: View,
    pub pcb_view: View,
    pub fit_sch: bool,
    pub fit_pcb: bool,
    pub canvas_rect: Rect,
    pub sch_tool: SchTool,
    pub pcb_tool: PcbTool,
    pub sel: Sel,
    /// Multiple selection (empty when 0 or 1 item is selected; then `sel` is used).
    pub group: Vec<Sel>,
    pub drag: Option<Drag>,
    pub wire_draw: Option<Vec<(i32, i32)>>,
    pub route: Option<RouteState>,
    pub ghost_rot: u8,
    pub label_text: String,
    /// Annotation being drawn: its kind and first point.
    pub note_draft: Option<(NoteKind, (i32, i32))>,
    /// Colour, size, dashes and fill of the next annotation.
    pub note_style: Note,
    pub route_layer: Layer,
    /// Copper zone being drawn (corners so far) and the net a new zone gets.
    pub zone_draft: Vec<(f32, f32)>,
    pub zone_net: String,
    /// Copper layers hidden in the board view.
    pub hidden_layers: Vec<Layer>,
    pub cam3d: crate::view3d::Cam3D,
    /// New board text: content, size (mm) and layer.
    pub text_input: String,
    pub text_size: f32,
    pub text_layer: TextLayer,
    /// Filled copper zones as triangles (key = fingerprint of the zones).
    pub zone_cache: Option<(u64, std::sync::Arc<crate::pcb::ZoneTris>)>,
    pub track_width: f32,
    pub nl: Netlist,
    pub erc: Vec<Issue>,
    pub drc: Vec<Issue>,
    pub sim: SimResult,
    pub conn: PcbConn,
    pub analysis_dirty: bool,
    pub show_voltages: bool,
    pub highlight_net: Option<usize>,
    pub status: String,
    pub hover_info: String,
    pub cursor: Option<(f32, f32)>,
    pub ai: AiSettings,
    pub chat: Vec<ChatMsg>,
    pub input: String,
    pending: Option<Receiver<Result<String, String>>>,
    pub show_ai_settings: bool,
    pub blocks: crate::blocks::BlocksState,
    pub fp_pick: crate::fpick::FpPicker,
    /// Put the keyboard in the Value box on the next frame (after a double-click).
    pub focus_value: bool,
    ai_found: Option<(bool, Vec<String>)>,
    native: Option<crate::filedlg::Native>,
    browser: Option<crate::filedlg::Browser>,
    recent: Vec<String>,
    /// File (or KiCad project) the project was loaded from / saved to, and its date then.
    pub source: Option<String>,
    pub source_stamp: Option<std::time::SystemTime>,
    pub watch_at: Option<std::time::Instant>,
    /// The file changed on disk while there were edits here.
    pub external_change: bool,
    pub edited_since_load: bool,
    /// Folder of the open project (the AI may read it if allowed).
    project_dir: Option<PathBuf>,
    folder_pick: Option<Receiver<Option<PathBuf>>>,
    new_doc_dir: String,
    online_query: String,
    online_hits: Vec<crate::online::Hit>,
    online_job: Option<Receiver<OnlineMsg>>,
    online_status: String,
    pub jlc: crate::jlcui::JlcState,
    show_filter: [bool; 3],
    pub ctx: egui::Context,
    screenshot: Option<PathBuf>,
    frame: u64,
    shot_requested: bool,
    shot_started: Option<std::time::Instant>,
}

fn default_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Documents")
        .join("FerrisCad")
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, screenshot: Option<PathBuf>, open: Option<String>) -> Self {
        Self::with_ctx(cc.egui_ctx.clone(), screenshot, open)
    }

    /// Build the app around an egui context (a window, or a headless one for tests).
    pub fn with_ctx(ctx: egui::Context, screenshot: Option<PathBuf>, open: Option<String>) -> Self {
        let mut vis = egui::Visuals::light();
        vis.panel_fill = Color32::from_rgb(242, 243, 247);
        vis.window_fill = Color32::from_rgb(250, 250, 252);
        vis.extreme_bg_color = Color32::WHITE;
        vis.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(190, 196, 210));
        vis.selection.bg_fill = Color32::from_rgb(196, 220, 255);
        vis.selection.stroke = Stroke::new(1.0_f32, Color32::from_rgb(30, 80, 170));
        ctx.set_visuals(vis);
        ctx.style_mut(|s| {
            s.spacing.item_spacing = Vec2::new(6.0, 5.0);
            s.spacing.button_padding = Vec2::new(7.0, 3.0);
        });
        let mut status = String::new();
        let mut notes: Vec<String> = vec![];
        let (project, path) = match open.as_ref().map(|p| (p.clone(), load_any(p))) {
            Some((p, Ok((pr, n)))) => {
                notes = n;
                let keep = if crate::kicad::is_kicad_path(std::path::Path::new(&p)) { "kicad".to_string() } else { p };
                (pr, keep)
            }
            Some((p, Err(e))) => {
                status = format!("Couldn't open {}: {}", p, e);
                (demo_project(), String::new())
            }
            None => (demo_project(), String::new()),
        };
        let is_demo = path.is_empty();
        let opened_ok = open.is_some() && status.is_empty();
        let mut ai = AiSettings::load();
        if screenshot.is_some() {
            ai = AiSettings::default();
            ai.api_key.clear();
            ai.provider = Provider::Offline;
        }
        let mut app = App {
            project,
            path,
            dirty: false,
            undo: vec![],
            redo: vec![],
            mode: if std::env::var_os("FERRISCAD_START_3D").is_some() {
                Mode::View3D
            } else if std::env::var_os("FERRISCAD_START_BOARD").is_some() {
                Mode::Pcb
            } else {
                Mode::Schematic
            },
            sch_view: View { offset: Vec2::new(40.0, 40.0), zoom: 1.6 },
            pcb_view: View { offset: Vec2::new(40.0, 40.0), zoom: 12.0 },
            fit_sch: true,
            fit_pcb: true,
            canvas_rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)),
            sch_tool: SchTool::Select,
            pcb_tool: PcbTool::Select,
            sel: Sel::None,
            group: vec![],
            drag: None,
            wire_draw: None,
            route: None,
            ghost_rot: 0,
            label_text: "NET1".into(),
            note_draft: None,
            note_style: Note::default(),
            route_layer: Layer::Front,
            zone_draft: vec![],
            zone_net: String::new(),
            hidden_layers: vec![],
            cam3d: Default::default(),
            text_input: "FerrisCad".into(),
            text_size: 1.5,
            text_layer: TextLayer::SilkFront,
            zone_cache: None,
            track_width: 0.5,
            nl: Netlist::default(),
            erc: vec![],
            drc: vec![],
            sim: SimResult::default(),
            conn: PcbConn::default(),
            analysis_dirty: true,
            show_voltages: true,
            highlight_net: None,
            status: if status.is_empty() { "Ready. Pick a part on the left, or ask the assistant on the right.".into() } else { status },
            hover_info: String::new(),
            cursor: None,
            ai,
            chat: vec![],
            input: String::new(),
            pending: None,
            show_ai_settings: false,
            blocks: Default::default(),
            fp_pick: Default::default(),
            focus_value: false,
            ai_found: None,
            native: None,
            browser: None,
            recent: load_recent(),
            source: None,
            source_stamp: None,
            watch_at: None,
            external_change: false,
            edited_since_load: false,
            project_dir: None,
            folder_pick: None,
            new_doc_dir: String::new(),
            online_query: String::new(),
            online_hits: vec![],
            online_job: None,
            online_status: String::new(),
            jlc: Default::default(),
            show_filter: [true, true, true],
            ctx: ctx.clone(),
            screenshot,
            frame: 0,
            shot_requested: false,
            shot_started: None,
        };
        if let (Some(o), true) = (&open, opened_ok) {
            app.project_dir = project_folder(o);
            app.track_source(o);
            if screenshot_free_of_side_effects(&app) {
                app.remember(o);
            }
        }
        if app.path == "kicad" {
            app.path.clear();
            app.dirty = true;
        }
        app.analyze();
        app.analysis_dirty = false;
        if let Ok(r) = std::env::var("FERRISCAD_SELECT") {
            if let Some(ci) = app.project.find_ref(&r) {
                app.sel = Sel::Comp(app.project.components[ci].id);
            }
        }
        app.welcome();
        for n in notes {
            app.push_msg(Role::Note, n);
        }
        if is_demo && !app.ai.online() {
            app.ask_ai("Review my circuit: what's wrong with it?".into());
        }
        app
    }

    fn welcome(&mut self) {
        let issues = self.all_issues();
        let e = issues.iter().filter(|i| i.severity == Severity::Error).count();
        let w = issues.iter().filter(|i| i.severity == Severity::Warning).count();
        let mut t = format!(
            "Hi! I'm your circuit assistant. I can read the whole schematic, the board and the simulated voltages.\n\
             Right now the checks report {} error{} and {} warning{}. Ask me anything about the circuit, or use the buttons above.",
            e,
            if e == 1 { "" } else { "s" },
            w,
            if w == 1 { "" } else { "s" }
        );
        if !self.ai.online() {
            t.push_str("\n\nNo AI is connected yet: click \"Connect an AI…\" above. Until then I answer with the built-in checker.");
        }
        self.push_msg(Role::Assistant, t);
    }

    // ------------------------------------------------------------ analysis & history

    pub fn analyze(&mut self) {
        // A no-connect mark follows its pin: drop the ones whose pin moved away.
        if !self.project.no_connects.is_empty() {
            let pins: std::collections::HashSet<(i32, i32)> =
                self.project.components.iter().flat_map(|c| c.info().pins.iter().map(|pd| c.pin_pos(pd)).collect::<Vec<_>>()).collect();
            self.project.no_connects.retain(|q| pins.contains(q));
        }
        self.nl = build_netlist(&self.project);
        self.erc = checks::erc(&self.project, &self.nl);
        let (d, c) = checks::drc(&self.project, &self.nl);
        self.drc = d;
        self.conn = c;
        self.sim = sim::simulate(&self.project, &self.nl);
        let valid = match self.sel {
            Sel::Comp(id) => self.project.find(id).is_some(),
            Sel::Text(i) => i < self.project.texts.len(),
            Sel::Wire(i) => i < self.project.wires.len(),
            Sel::Label(i) => i < self.project.labels.len(),
            Sel::Track(i) => i < self.project.tracks.len(),
            Sel::Via(i) => i < self.project.vias.len(),
            Sel::Zone(i) => i < self.project.zones.len(),
            Sel::Note(i) => i < self.project.notes.len(),
            Sel::None => true,
        };
        if !valid {
            self.sel = Sel::None;
        }
        self.prune_selection();
        if self.highlight_net.map_or(false, |n| n >= self.nl.nets.len()) {
            self.highlight_net = None;
        }
    }

    pub fn all_issues(&self) -> Vec<Issue> {
        let mut v: Vec<Issue> = self.erc.iter().chain(self.sim.issues.iter()).chain(self.drc.iter()).cloned().collect();
        v.sort_by_key(|i| i.severity);
        v
    }

    pub fn checkpoint(&mut self) {
        self.edited_since_load = true;
        self.undo.push(self.project.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
        self.analysis_dirty = true;
    }

    fn undo(&mut self) {
        if let Some(p) = self.undo.pop() {
            self.group.clear();
            self.redo.push(std::mem::replace(&mut self.project, p));
            self.analysis_dirty = true;
            self.status = "Undone.".into();
        }
    }

    fn redo(&mut self) {
        if let Some(p) = self.redo.pop() {
            self.group.clear();
            self.undo.push(std::mem::replace(&mut self.project, p));
            self.analysis_dirty = true;
            self.status = "Redone.".into();
        }
    }

    pub fn rotate_selection(&mut self) {
        if let SchTool::Place(_) = self.sch_tool {
            if self.mode == Mode::Schematic {
                self.ghost_rot = (self.ghost_rot + 1) % 4;
                return;
            }
        }
        if !self.group.is_empty() {
            self.rotate_group();
            return;
        }
        if let Sel::Text(i) = self.sel {
            self.checkpoint();
            if let Some(t) = self.project.texts.get_mut(i) {
                t.rot = (t.rot + 1) % 4;
            }
            return;
        }
        if let Sel::Note(_) = self.sel {
            self.rotate_group();
            return;
        }
        if let Sel::Comp(id) = self.sel {
            self.checkpoint();
            let mode = self.mode;
            let marks = self.nc_marks(&[id].into());
            if let Some(ci) = self.project.find(id) {
                let c = &mut self.project.components[ci];
                if mode == Mode::Schematic {
                    c.rot = (c.rot + 1) % 4;
                } else {
                    c.pcb_rot = (c.pcb_rot + 1) % 4;
                }
            }
            self.place_nc_marks(&marks);
        }
    }

    pub fn delete_selection(&mut self) {
        if !self.group.is_empty() {
            self.delete_group();
            return;
        }
        match self.sel {
            Sel::None => return,
            Sel::Text(i) => {
                self.checkpoint();
                if i < self.project.texts.len() {
                    self.project.texts.remove(i);
                }
            }
            Sel::Comp(id) => {
                self.checkpoint();
                if let Some(ci) = self.project.find(id) {
                    let r = self.project.components.remove(ci).reference;
                    self.status = format!("Deleted {}.", r);
                }
            }
            Sel::Wire(i) => {
                self.checkpoint();
                self.project.wires.remove(i);
            }
            Sel::Label(i) => {
                self.checkpoint();
                self.project.labels.remove(i);
            }
            Sel::Track(i) => {
                self.checkpoint();
                self.project.tracks.remove(i);
            }
            Sel::Via(i) => {
                self.checkpoint();
                self.project.vias.remove(i);
            }
            Sel::Note(i) => {
                self.checkpoint();
                if i < self.project.notes.len() {
                    self.project.notes.remove(i);
                }
            }
            Sel::Zone(i) => {
                self.checkpoint();
                if i < self.project.zones.len() {
                    self.project.zones.remove(i);
                }
                self.status = "Zone deleted.".into();
            }
        }
        self.sel = Sel::None;
    }

    /// Route on another layer; while routing, drop a via at the current point first.
    pub fn switch_route_layer(&mut self, l: Layer) {
        if l == self.route_layer {
            return;
        }
        if let Some(r) = &mut self.route {
            let last = *r.pts.last().unwrap();
            if !r.vias.contains(&last) {
                r.vias.push(last);
            }
        }
        self.route_layer = l;
        self.hidden_layers.retain(|x| *x != l);
        self.status = format!("Routing on {}.", l.name());
    }

    /// Change the number of copper layers (tracks on removed layers are kept and flagged).
    pub fn set_layer_count(&mut self, n: u8) {
        self.checkpoint();
        self.project.board.layers = n;
        if !self.project.board.has(self.route_layer) {
            self.route_layer = Layer::Front;
        }
        let orphans = self.project.tracks.iter().filter(|t| !self.project.board.has(t.layer)).count();
        self.status = if orphans > 0 {
            format!("{} copper layers. {} track(s) are on a layer that no longer exists: move them (see Problems).", n, orphans)
        } else {
            format!("Board now has {} copper layers.", n)
        };
    }

    pub fn duplicate_selection(&mut self) {
        if let Sel::Note(i) = self.sel {
            self.duplicate_note(i);
            return;
        }
        let Sel::Comp(id) = self.sel else { return };
        let Some(ci) = self.project.find(id) else { return };
        self.checkpoint();
        let mut c = self.project.components[ci].clone();
        let prefix = c.info().prefix;
        self.project.next_id = self.project.next_id.max(self.project.components.iter().map(|c| c.id).max().unwrap_or(0)) + 1;
        c.id = self.project.next_id;
        if !c.kind.is_power() {
            let r: String = c.reference.chars().take_while(|ch| !ch.is_ascii_digit()).collect();
            c.reference = self.project.next_ref(if r.is_empty() { prefix } else { &r });
        }
        c.pos = (c.pos.0 + 2 * GRID, c.pos.1 + 2 * GRID);
        let parked = self.project.components.iter().filter(|c| c.has_fp() && c.pcb_pos.0 > self.project.board.x1).count();
        c.pcb_pos = (self.project.board.x1 + 8.0, self.project.board.y0 + 4.0 + parked as f32 * 7.0);
        c.pcb_rot = 0;
        self.status = format!("Added {} (a copy of {}).", c.reference, self.project.components[ci].reference);
        self.sel = Sel::Comp(c.id);
        self.project.components.push(c);
    }

    pub fn cancel_tool(&mut self) {
        if self.wire_draw.is_some() {
            self.finish_wire();
            return;
        }
        if self.route.is_some() {
            self.finish_route();
            return;
        }
        if self.note_draft.take().is_some() {
            return;
        }
        if !self.zone_draft.is_empty() {
            self.zone_draft.clear();
            return;
        }
        self.sch_tool = SchTool::Select;
        self.pcb_tool = PcbTool::Select;
        self.drag = None;
        self.highlight_net = None;
        if !self.group.is_empty() {
            self.group.clear();
            self.sel = Sel::None;
        }
    }

    pub fn fit_view(&mut self, mode: Mode, rect: Rect) {
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let mut add = |x: f32, y: f32| {
            b.0 = b.0.min(x);
            b.1 = b.1.min(y);
            b.2 = b.2.max(x);
            b.3 = b.3.max(y);
        };
        let (minz, maxz, margin) = match mode {
            Mode::Schematic => {
                for c in &self.project.components {
                    let bb = c.sch_bbox();
                    add(bb.0 as f32 - 20.0, bb.1 as f32 - 20.0);
                    add(bb.2 as f32 + 30.0, bb.3 as f32 + 20.0);
                }
                for w in &self.project.wires {
                    add(w.a.0 as f32, w.a.1 as f32);
                    add(w.b.0 as f32, w.b.1 as f32);
                }
                for n in &self.project.notes {
                    let bb = n.bbox();
                    add(bb.0 as f32 - 10.0, bb.1 as f32 - 10.0);
                    add(bb.2 as f32 + 10.0, bb.3 as f32 + 10.0);
                }
                (0.2, 8.0, 40.0)
            }
            Mode::Pcb | Mode::View3D => {
                let bd = &self.project.board;
                add(bd.x0 - 2.0, bd.y0 - 2.0);
                add(bd.x1 + 2.0, bd.y1 + 2.0);
                for c in self.project.components.iter().filter(|c| c.has_fp()) {
                    let bb = c.pcb_bbox();
                    add(bb.0 - 1.0, bb.1 - 1.0);
                    add(bb.2 + 1.0, bb.3 + 1.0);
                }
                (1.0, 150.0, 30.0)
            }
        };
        if b.0 > b.2 {
            b = (0.0, 0.0, 400.0, 300.0);
        }
        let (w, h) = ((b.2 - b.0).max(1.0), (b.3 - b.1).max(1.0));
        let z = ((rect.width() - margin) / w).min((rect.height() - margin) / h).clamp(minz, maxz);
        let center = Vec2::new((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0);
        let view = View { zoom: z, offset: rect.size() / 2.0 - center * z };
        match mode {
            Mode::Schematic => self.sch_view = view,
            Mode::Pcb | Mode::View3D => self.pcb_view = view,
        }
    }

    fn center_on(&mut self, mode: Mode, w: (f32, f32)) {
        let rect = self.canvas_rect;
        let v = match mode {
            Mode::Schematic => &mut self.sch_view,
            Mode::Pcb | Mode::View3D => &mut self.pcb_view,
        };
        v.offset = rect.size() / 2.0 - Vec2::new(w.0, w.1) * v.zoom;
    }

    pub fn focus_comp(&mut self, id: u32) {
        let Some(ci) = self.project.find(id) else { return };
        self.sel = Sel::Comp(id);
        let c = &self.project.components[ci];
        let (sp, pp) = ((c.pos.0 as f32, c.pos.1 as f32), c.pcb_pos);
        if self.mode == Mode::Pcb && !c.has_fp() {
            self.mode = Mode::Schematic;
        }
        match self.mode {
            Mode::Schematic => self.center_on(Mode::Schematic, sp),
            Mode::Pcb | Mode::View3D => self.center_on(Mode::Pcb, pp),
        }
    }

    fn show_issue(&mut self, is: &Issue) {
        if is.source == "DRC" {
            self.mode = Mode::Pcb;
            if let Some(p) = is.pcb_pos {
                self.center_on(Mode::Pcb, p);
            }
        } else {
            self.mode = Mode::Schematic;
            if let Some(p) = is.sch_pos {
                self.center_on(Mode::Schematic, (p.0 as f32, p.1 as f32));
            }
        }
        self.sel = is.comps.first().map(|&id| Sel::Comp(id)).unwrap_or(Sel::None);
    }

    fn set_value(&mut self, id: u32, val: &str) {
        if let Some(ci) = self.project.find(id) {
            self.checkpoint();
            let c = &mut self.project.components[ci];
            let old = std::mem::replace(&mut c.value, val.to_string());
            let r = c.reference.clone();
            self.push_msg(Role::Note, format!("Changed {} from {} to {}.", r, old, val));
            self.status = format!("{} is now {}.", r, val);
        }
    }

    // ------------------------------------------------------------ files

    /// Editor state that belongs to one project (called when the project is swapped).
    pub fn reset_editor_state(&mut self) {
        self.sel = Sel::None;
        self.group.clear();
        self.highlight_net = None;
        self.wire_draw = None;
        self.route = None;
        self.drag = None;
        self.zone_draft.clear();
        self.mode = Mode::Schematic;
        self.fit_sch = true;
        self.fit_pcb = true;
        self.analysis_dirty = true;
        self.external_change = false;
    }

    pub fn take_project_dir(&mut self) -> Option<PathBuf> {
        self.project_dir.take()
    }

    pub fn restore_project_dir(&mut self, d: Option<PathBuf>) {
        self.project_dir = d;
    }

    fn save_to(&mut self, path: &str) {
        if self.blocks.editing.is_some() {
            // Ctrl+S while editing a block saves the block, not the project.
            self.save_block_edit();
            return;
        }
        let p = PathBuf::from(path);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        match std::fs::write(&p, serde_json::to_string_pretty(&self.project).unwrap()) {
            Ok(_) => {
                self.path = path.to_string();
                self.dirty = false;
                self.status = format!("Saved {}", path);
                self.project_dir = project_folder(path);
                self.remember(path);
                self.track_source(path);
            }
            Err(e) => self.status = format!("Couldn't save: {}", e),
        }
    }

    fn open_from(&mut self, path: &str) {
        if self.blocks.editing.is_some() {
            self.status = "Close the block editor first.".into();
            return;
        }
        match load_any(path) {
            Ok((p, notes)) => {
                self.checkpoint();
                self.project = p;
                let kicad = crate::kicad::is_kicad_path(std::path::Path::new(path));
                self.path = if kicad { String::new() } else { path.to_string() };
                self.dirty = kicad;
                self.fit_sch = true;
                self.fit_pcb = true;
                self.sel = Sel::None;
                self.mode = Mode::Schematic;
                self.analysis_dirty = true;
                self.status = notes.first().cloned().unwrap_or_else(|| format!("Opened {}", path));
                self.project_dir = project_folder(path);
                self.remember(path);
                self.track_source(path);
                for n in notes {
                    self.push_msg(Role::Note, n);
                }
            }
            Err(e) => {
                self.status = format!("Couldn't open {}: {}", path, e);
                self.push_msg(Role::Error, self.status.clone());
            }
        }
    }

    fn suggested_path(&self) -> String {
        if !self.path.is_empty() {
            return self.path.clone();
        }
        let name: String = self.project.name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        default_dir()
            .join(format!("{}.fcad", if name.is_empty() { "circuit".to_string() } else { name }))
            .to_string_lossy()
            .to_string()
    }

    // ------------------------------------------------------------ AI

    fn push_msg(&mut self, role: Role, raw: String) {
        let (text, actions) = if role == Role::Assistant { ai::parse_actions(&raw) } else { (raw.clone(), vec![]) };
        let refs = if role == Role::Assistant { ai::mentioned_refs(&text, &self.project) } else { vec![] };
        let applied = vec![false; actions.len()];
        self.chat.push(ChatMsg { role, raw, text, actions, applied, refs });
    }

    pub fn ask_ai(&mut self, q: String) {
        if self.pending.is_some() || q.trim().is_empty() {
            return;
        }
        if self.analysis_dirty {
            self.analyze();
            self.analysis_dirty = false;
        }
        self.push_msg(Role::User, q.clone());
        let issues = self.all_issues();
        if !self.ai.online() {
            let a = ai::offline_answer(&q, &self.project, &issues, &self.sim);
            self.push_msg(Role::Assistant, a);
            return;
        }
        let report = ai::circuit_report(&self.project, &self.nl, &issues, &self.sim, &self.conn);
        self.ai.project_dir = self.project_dir.clone();
        let system = ai::system_prompt(&report) + &self.ai.files_note();
        let mut hist: History = vec![];
        for m in &self.chat {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                _ => continue,
            };
            if hist.is_empty() && role == "assistant" {
                continue;
            }
            if let Some(last) = hist.last_mut() {
                if last.0 == role {
                    last.1.push_str("\n\n");
                    last.1.push_str(&m.raw);
                    continue;
                }
            }
            hist.push((role.to_string(), m.raw.clone()));
        }
        while hist.len() > 16 {
            hist.remove(0);
        }
        while hist.first().map_or(false, |h| h.0 != "user") {
            hist.remove(0);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let settings = self.ai.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let r = ai::ask(&settings, &system, &hist);
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.pending = Some(rx);
    }

    pub fn ctx_handle(&self) -> egui::Context {
        self.ctx.clone()
    }

    /// Where generated files go by default: the project's folder, else ~/Documents/FerrisCad.
    pub fn project_dir_for_output(&self) -> PathBuf {
        self.project_dir.clone().unwrap_or_else(default_dir)
    }

    pub fn chat_note(&mut self, role: Role, text: String) {
        self.push_msg(role, text);
    }

    pub fn waiting_for_ai(&self) -> bool {
        self.pending.is_some()
    }

    pub fn poll_ai(&mut self) {
        let Some(rx) = &self.pending else { return };
        match rx.try_recv() {
            Ok(Ok(text)) => {
                self.pending = None;
                self.push_msg(Role::Assistant, text);
            }
            Ok(Err(e)) => {
                self.pending = None;
                self.push_msg(Role::Error, e);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.push_msg(Role::Error, "The AI request stopped unexpectedly.".into());
            }
        }
    }

    // ------------------------------------------------------------ keyboard

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        if (cmd && pressed(Key::R)) || pressed(Key::F5) {
            self.reload_from_disk(false);
            return;
        }
        if cmd && pressed(Key::O) {
            self.open_dialog(Purpose::Open);
        }
        if cmd && pressed(Key::S) {
            if self.path.is_empty() || shift {
                self.open_dialog(Purpose::Save);
            } else {
                let p = self.path.clone();
                self.save_to(&p);
            }
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        if cmd && pressed(Key::Z) {
            if shift {
                self.redo()
            } else {
                self.undo()
            }
            return;
        }
        if cmd && pressed(Key::Y) {
            self.redo();
            return;
        }
        if cmd && pressed(Key::D) {
            self.duplicate_selection();
            return;
        }
        if cmd && pressed(Key::A) {
            self.select_all();
            return;
        }
        if cmd {
            return;
        }
        if pressed(Key::Escape) {
            self.cancel_tool();
        }
        if pressed(Key::Delete) || pressed(Key::Backspace) {
            self.delete_selection();
        }
        if pressed(Key::R) {
            self.rotate_selection();
        }
        if pressed(Key::Home) {
            match self.mode {
                Mode::Schematic => self.fit_sch = true,
                Mode::Pcb => self.fit_pcb = true,
                Mode::View3D => self.cam3d = Default::default(),
            }
        }
        match self.mode {
            Mode::Schematic => {
                if pressed(Key::W) {
                    self.sch_tool = SchTool::Wire;
                }
                if pressed(Key::L) {
                    self.sch_tool = SchTool::Label;
                }
                if pressed(Key::Q) {
                    self.sch_tool = SchTool::NoConnect;
                }
                if pressed(Key::S) {
                    self.sch_tool = SchTool::Select;
                }
                for (key, kind) in [(Key::I, NoteKind::Line), (Key::B, NoteKind::Box), (Key::T, NoteKind::Text)] {
                    if pressed(key) {
                        self.sch_tool = SchTool::Note(kind);
                    }
                }
            }
            Mode::View3D => {}
            Mode::Pcb => {
                if pressed(Key::X) {
                    self.pcb_tool = PcbTool::Route;
                }
                if pressed(Key::T) {
                    self.pcb_tool = PcbTool::Text;
                }
                if pressed(Key::Z) {
                    self.pcb_tool = PcbTool::Zone;
                }
                if pressed(Key::V) {
                    if self.route.is_some() {
                        let next = self.route_layer.other();
                        self.switch_route_layer(next);
                    } else {
                        self.pcb_tool = PcbTool::Via;
                    }
                }
                if pressed(Key::F) {
                    self.route_layer = Layer::Front;
                }
                if pressed(Key::B) {
                    self.route_layer = Layer::Back;
                }
                // PageUp / PageDown: previous / next copper layer.
                let layers = self.project.board.copper();
                let cur = layers.iter().position(|l| *l == self.route_layer).unwrap_or(0);
                if pressed(Key::PageUp) && cur > 0 {
                    self.switch_route_layer(layers[cur - 1]);
                }
                if pressed(Key::PageDown) && cur + 1 < layers.len() {
                    self.switch_route_layer(layers[cur + 1]);
                }
                if pressed(Key::S) {
                    self.pcb_tool = PcbTool::Select;
                }
            }
        }
    }

    // ------------------------------------------------------------ panels

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        if self.frame == 3 && std::env::var_os("FERRISCAD_TEST_NEW").is_some() {
            self.new_project();
        }
        if let Ok(q) = std::env::var("FERRISCAD_TEST_SEARCH") {
            if self.frame == 3 {
                self.online_query = q;
                self.online_search();
            }
            if self.frame > 3 && self.online_job.is_none() && !self.online_hits.is_empty() && !self.online_status.starts_with("Added") {
                let h = self.online_hits[0].clone();
                self.online_fetch(&h);
            }
        }
        if self.frame == 3 && std::env::var_os("FERRISCAD_TEST_TEXT").is_some() {
            self.project.texts.push(BoardText { text: "FerrisCad LED v1.0".into(), pos: (30.0, 3.5), size: 2.0, rot: 0, layer: TextLayer::SilkFront });
            self.project.texts.push(BoardText { text: "Made with Rust".into(), pos: (30.0, 38.0), size: 1.5, rot: 0, layer: TextLayer::SilkBack });
            self.project.texts.push(BoardText { text: "GND 5V".into(), pos: (56.0, 20.0), size: 1.5, rot: 1, layer: TextLayer::Copper(Layer::Front) });
            self.sel = Sel::Text(0);
            self.analysis_dirty = true;
        }
        if self.frame == 3 && std::env::var_os("FERRISCAD_TEST_LAYERS").is_some() {
            self.project.board.layers = 4;
            let pad = |p: &Project, r: &str, pin: &str| {
                let c = &p.components[p.find_ref(r).unwrap()];
                let inf = c.info();
                c.pad_pos(inf.pins.iter().find(|x| x.name == pin).unwrap())
            };
            let (a, b) = (pad(&self.project, "BT1", "-"), pad(&self.project, "C1", "-"));
            self.project.tracks.push(Track { a, b, width: 0.6, layer: Layer::Inner(1) });
            let (a, b) = (pad(&self.project, "U1", "GND"), pad(&self.project, "C1", "-"));
            self.project.tracks.push(Track { a, b, width: 0.6, layer: Layer::Inner(2) });
            self.route_layer = Layer::Inner(1);
            self.analysis_dirty = true;
        }
        if self.frame == 3 && std::env::var_os("FERRISCAD_TEST_JLC").is_some() {
            self.jlc.open = true;
            self.jlc.out_dir = std::env::temp_dir().join("vp-jlc-ui").to_string_lossy().to_string();
        }
        if self.frame == 5 && std::env::var_os("FERRISCAD_TEST_JLC").is_some() {
            self.jlc_auto_public();
        }
        if self.frame == 3 && std::env::var_os("FERRISCAD_TEST_BAND").is_some() {
            let items = self.band_items((410.0, 60.0), (820.0, 285.0));
            self.set_selection(items);
        }
        if self.frame == 3 && std::env::var_os("FERRISCAD_TEST_OPEN").is_some() {
            self.open_dialog(Purpose::Open);
        }
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New empty project").clicked() {
                    self.new_project();
                    ui.close_menu();
                }
                if ui.button("Open the example").clicked() {
                    self.checkpoint();
                    self.project = demo_project();
                    self.path.clear();
                    self.fit_sch = true;
                    self.fit_pcb = true;
                    ui.close_menu();
                }
                if ui.button("Open…          Ctrl+O").clicked() {
                    self.open_dialog(Purpose::Open);
                    ui.close_menu();
                }
                if ui.button("Open a KiCad project…").clicked() {
                    self.open_dialog(Purpose::OpenKicad);
                    ui.close_menu();
                }
                if ui
                    .add_enabled(self.source.is_some(), egui::Button::new("Reload from disk    Ctrl+R"))
                    .on_hover_text("Read the open file again (e.g. after Claude changed it). Ctrl+Z goes back.")
                    .clicked()
                {
                    self.reload_from_disk(false);
                    ui.close_menu();
                }
                let recent = self.recent.clone();
                ui.add_enabled_ui(!recent.is_empty(), |ui| {
                    ui.menu_button("Open recent", |ui| {
                        for r in recent.iter().take(10) {
                            let name = PathBuf::from(r).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| r.clone());
                            if ui.button(name).on_hover_text(r).clicked() {
                                self.open_from(r);
                                ui.close_menu();
                            }
                        }
                    });
                });
                ui.separator();
                if ui.button("Save           Ctrl+S").clicked() {
                    if self.path.is_empty() {
                        self.open_dialog(Purpose::Save);
                    } else {
                        let p = self.path.clone();
                        self.save_to(&p);
                    }
                    ui.close_menu();
                }
                if ui.button("Save as…   Ctrl+Shift+S").clicked() {
                    self.open_dialog(Purpose::Save);
                    ui.close_menu();
                }
                if ui
                    .button("Export to KiCad…")
                    .on_hover_text("Write a KiCad project (.kicad_pro, .kicad_sch, .kicad_pcb) that KiCad 8 and 9 can open")
                    .clicked()
                {
                    self.open_dialog(Purpose::ExportKicad);
                    ui.close_menu();
                }
                if ui.button("Copy netlist to clipboard").clicked() {
                    let issues = self.all_issues();
                    let r = ai::circuit_report(&self.project, &self.nl, &issues, &self.sim, &self.conn);
                    ui.output_mut(|o| o.copied_text = r);
                    self.status = "Netlist and check results copied.".into();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo      Ctrl+Z")).clicked() {
                    self.undo();
                    ui.close_menu();
                }
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo      Ctrl+Y")).clicked() {
                    self.redo();
                    ui.close_menu();
                }
                if ui.button("Rotate    R").clicked() {
                    self.rotate_selection();
                    ui.close_menu();
                }
                if ui.button("Delete    Del").clicked() {
                    self.delete_selection();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Blocks…").on_hover_text("Save part of the circuit as a reusable block, or insert one").clicked() {
                    self.blocks.open = true;
                    ui.close_menu();
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Schematic").clicked() {
                    self.mode = Mode::Schematic;
                    ui.close_menu();
                }
                if ui.button("Board layout").clicked() {
                    self.mode = Mode::Pcb;
                    ui.close_menu();
                }
                if ui.button("Zoom to fit    Home").clicked() {
                    self.fit_sch = true;
                    self.fit_pcb = true;
                    ui.close_menu();
                }
                ui.checkbox(&mut self.show_voltages, "Show simulated voltages");
            });
            ui.menu_button("Board", |ui| {
                if ui.button("Manufacture with JLCPCB…").clicked() {
                    self.jlc.open = true;
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Arrange parts on the board").clicked() {
                    self.checkpoint();
                    self.project.auto_place();
                    self.fit_pcb = true;
                    ui.close_menu();
                }
                if ui.button("Remove all tracks").clicked() {
                    self.checkpoint();
                    self.project.tracks.clear();
                    self.project.vias.clear();
                    ui.close_menu();
                }
            });
            ui.menu_button("AI", |ui| {
                if ui.button("Review my circuit").clicked() {
                    self.ask_ai("Review my circuit: what's wrong with it?".into());
                    ui.close_menu();
                }
                if ui.button("Connect an AI…").clicked() {
                    self.show_ai_settings = true;
                    ui.close_menu();
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            for (m, label) in [(Mode::Schematic, "📐  Schematic"), (Mode::Pcb, "🔲  Board layout"), (Mode::View3D, "🔄  3D view")] {
                let sel = self.mode == m;
                let txt = RichText::new(label).size(15.0).strong();
                let b = egui::Button::new(if sel { txt.color(Color32::WHITE) } else { txt })
                    .fill(if sel { Color32::from_rgb(40, 90, 170) } else { Color32::from_rgb(225, 228, 236) })
                    .min_size(Vec2::new(130.0, 30.0));
                if ui.add(b).clicked() {
                    self.mode = m;
                    self.wire_draw = None;
                    self.route = None;
                }
            }
            ui.separator();
            if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("⟲ Undo")).clicked() {
                self.undo();
            }
            if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("⟳ Redo")).clicked() {
                self.redo();
            }
            if ui.button("🔄 Rotate").on_hover_text("Rotate the selected part (R)").clicked() {
                self.rotate_selection();
            }
            if ui.button("🗑 Delete").on_hover_text("Delete the selection (Del)").clicked() {
                self.delete_selection();
            }
            if ui.button("⛶ Fit").on_hover_text("Zoom to fit (Home)").clicked() {
                self.fit_sch = true;
                self.fit_pcb = true;
            }
            ui.toggle_value(&mut self.show_voltages, "⚡ Voltages").on_hover_text("Show the simulated DC voltage on each net");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(6.0);
                let issues = self.all_issues();
                let e = issues.iter().filter(|i| i.severity == Severity::Error).count();
                let w = issues.iter().filter(|i| i.severity == Severity::Warning).count();
                let chip = |ui: &mut egui::Ui, text: String, bg: Color32| {
                    egui::Frame::none().fill(bg).rounding(10.0).inner_margin(egui::Margin::symmetric(9.0, 3.0)).show(ui, |ui| {
                        ui.label(RichText::new(text).color(Color32::WHITE).strong());
                    });
                };
                chip(
                    ui,
                    format!("{} warning{}", w, if w == 1 { "" } else { "s" }),
                    if w > 0 { Color32::from_rgb(214, 140, 20) } else { Color32::from_rgb(120, 160, 120) },
                );
                chip(
                    ui,
                    format!("{} error{}", e, if e == 1 { "" } else { "s" }),
                    if e > 0 { Color32::from_rgb(200, 50, 50) } else { Color32::from_rgb(60, 150, 80) },
                );
                let simtxt = if self.sim.ok { "DC sim ✔" } else { "DC sim —" };
                chip(ui, simtxt.into(), Color32::from_rgb(70, 110, 170));
                ui.label(RichText::new(format!("{}{}", self.project.name, if self.dirty { " •" } else { "" })).color(Color32::DARK_GRAY));
            });
        });
    }

    fn tool_button(ui: &mut egui::Ui, selected: bool, label: &str, hint: &str) -> bool {
        let b = egui::Button::new(RichText::new(label).size(13.5))
            .fill(if selected { Color32::from_rgb(196, 220, 255) } else { Color32::from_rgb(232, 234, 240) })
            .stroke(if selected { Stroke::new(1.0_f32, Color32::from_rgb(40, 90, 170)) } else { Stroke::NONE })
            .min_size(Vec2::new(ui.available_width(), 24.0));
        ui.add(b).on_hover_text(hint).clicked()
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(4.0);
            if self.sel != Sel::None {
                egui::Frame::none()
                    .fill(Color32::from_rgb(232, 238, 250))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(170, 190, 225)))
                    .rounding(8.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        self.properties(ui);
                    });
                ui.add_space(6.0);
            } else {
                ui.label(
                    RichText::new(match self.mode {
                        Mode::Schematic => "Click a part to edit it, double-click to change its value. Pick a part below to add one.",
                        Mode::Pcb => "Click a footprint or track to edit it.",
                        Mode::View3D => "Turn the board with the mouse. Edit parts in the schematic or board layout.",
                    })
                    .small()
                    .color(Color32::GRAY),
                );
                ui.add_space(4.0);
            }
            if matches!(self.mode, Mode::Schematic | Mode::Pcb) && ui.button("📦  Blocks…").on_hover_text("Reuse a saved part of a circuit, or save the selection as a block").clicked() {
                self.blocks.open = true;
            }
            match self.mode {
                Mode::Schematic => {
                    ui.label(RichText::new("TOOLS").small().strong().color(Color32::GRAY));
                    if Self::tool_button(ui, self.sch_tool == SchTool::Select, "⬉  Select / move", "Select and drag parts (S)") {
                        self.sch_tool = SchTool::Select;
                    }
                    if Self::tool_button(ui, self.sch_tool == SchTool::Wire, "✏  Draw wire", "Click to start, click to bend, click a pin to finish (W)") {
                        self.sch_tool = SchTool::Wire;
                    }
                    if Self::tool_button(ui, self.sch_tool == SchTool::Label, "🏷  Net label", "Wires with the same label are connected (L)") {
                        self.sch_tool = SchTool::Label;
                    }
                    if Self::tool_button(ui, self.sch_tool == SchTool::NoConnect, "❌  No connect", "Click a pin that is left open on purpose, so the checks stop reporting it (Q). Click the mark again to remove it.") {
                        self.sch_tool = SchTool::NoConnect;
                    }
                    if self.sch_tool == SchTool::Label {
                        ui.horizontal(|ui| {
                            ui.label("Name");
                            ui.add(egui::TextEdit::singleline(&mut self.label_text).desired_width(110.0));
                        });
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new("ANNOTATE").small().strong().color(Color32::GRAY));
                    for (kind, title, help) in [
                        (NoteKind::Line, "➖  Line", "Separate parts of the circuit: click each point, right-click or Esc ends (I)"),
                        (NoteKind::Box, "📝  Text box", "Frame a section or write notes: drag, or click two corners (B)"),
                        (NoteKind::Text, "T  Text", "Write on the schematic: click where it starts (T)"),
                    ] {
                        if Self::tool_button(ui, self.sch_tool == SchTool::Note(kind), title, help) {
                            self.sch_tool = SchTool::Note(kind);
                        }
                    }
                    if let SchTool::Note(kind) = self.sch_tool {
                        crate::annot::note_style_controls(ui, kind, &mut self.note_style);
                    }
                    ui.add_space(6.0);
                    self.online_panel(ui);
                    ui.add_space(6.0);
                    ui.label(RichText::new("PARTS").small().strong().color(Color32::GRAY));
                    for (group, kinds) in LIBRARY {
                        ui.label(RichText::new(*group).strong());
                        let w = (ui.available_width() - 6.0) / 2.0;
                        egui::Grid::new(*group).num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
                            for (i, &k) in kinds.iter().enumerate() {
                                let inf = info(k);
                                let sel = self.sch_tool == SchTool::Place(k);
                                let short = match k {
                                    Kind::CapPolarized => "Polarized cap",
                                    Kind::Mcu8 => "Microcontroller",
                                    Kind::Regulator => "Regulator",
                                    Kind::Connector2 => "Connector",
                                    Kind::Power => "Power rail",
                                    Kind::Gnd => "Ground",
                                    Kind::Npn => "NPN",
                                    Kind::Pnp => "PNP",
                                    Kind::Nmos => "N-MOSFET",
                                    _ => inf.label,
                                };
                                let b = egui::Button::new(RichText::new(short).size(12.5))
                                    .fill(if sel { Color32::from_rgb(196, 220, 255) } else { Color32::from_rgb(232, 234, 240) })
                                    .min_size(Vec2::new(w, 24.0));
                                if ui.add(b).on_hover_text(format!("{}: click, then click on the sheet to place it. R rotates.", inf.label)).clicked() {
                                    self.sch_tool = SchTool::Place(k);
                                    self.ghost_rot = 0;
                                    self.sel = Sel::None;
                                }
                                if i % 2 == 1 {
                                    ui.end_row();
                                }
                            }
                        });
                    }
                }
                Mode::View3D => self.view3d_panel(ui),
                Mode::Pcb => {
                    ui.label(RichText::new("TOOLS").small().strong().color(Color32::GRAY));
                    if Self::tool_button(ui, self.pcb_tool == PcbTool::Select, "⬉  Select / move", "Select and drag footprints (S)") {
                        self.pcb_tool = PcbTool::Select;
                    }
                    if Self::tool_button(
                        ui,
                        self.pcb_tool == PcbTool::Route,
                        "✏  Route track",
                        "Click a pad, click to bend, click the target pad (X). V adds a via.",
                    ) {
                        self.pcb_tool = PcbTool::Route;
                    }
                    if Self::tool_button(ui, self.pcb_tool == PcbTool::Via, "○  Add via", "Connects all copper layers (through via)") {
                        self.pcb_tool = PcbTool::Via;
                    }
                    if Self::tool_button(ui, self.pcb_tool == PcbTool::Text, "T  Add text", "Write text on the silkscreen or copper (T)") {
                        self.pcb_tool = PcbTool::Text;
                    }
                    if Self::tool_button(ui, self.pcb_tool == PcbTool::Zone, "▨  Copper zone", "Draw a copper pour (Z): click the corners, double-click or Enter to close") {
                        self.pcb_tool = PcbTool::Zone;
                    }
                    if self.pcb_tool == PcbTool::Zone {
                        let mut names: Vec<String> = self.nl.nets.iter().filter(|n| !n.pins.is_empty()).map(|n| n.name.clone()).collect();
                        names.sort();
                        names.dedup();
                        if !names.contains(&self.zone_net) {
                            self.zone_net = names.iter().find(|n| *n == "GND").or(names.first()).cloned().unwrap_or_default();
                        }
                        egui::Frame::none().fill(Color32::from_rgb(232, 238, 250)).rounding(6.0).inner_margin(6.0).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Net");
                                egui::ComboBox::from_id_salt("new_zone_net").selected_text(self.zone_net.clone()).show_ui(ui, |ui| {
                                    for n in &names {
                                        ui.selectable_value(&mut self.zone_net, n.clone(), n);
                                    }
                                });
                            });
                            ui.label(RichText::new(format!("Layer: {} (pick it in the layer list below).", self.route_layer.name())).small().color(Color32::GRAY));
                            ui.label(RichText::new("Click the corners, double-click or Enter to close, Esc cancels.").small().color(Color32::GRAY));
                        });
                    }
                    if self.pcb_tool == PcbTool::Text {
                        egui::Frame::none().fill(Color32::from_rgb(232, 238, 250)).rounding(6.0).inner_margin(6.0).show(ui, |ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.text_input).hint_text("Text to write").desired_width(ui.available_width()));
                            ui.horizontal(|ui| {
                                ui.label("Height");
                                ui.add(egui::DragValue::new(&mut self.text_size).speed(0.05).range(0.5..=20.0).suffix(" mm"));
                            });
                            ui.horizontal_wrapped(|ui| {
                                for l in text_layers(&self.project.board) {
                                    ui.selectable_value(&mut self.text_layer, l, l.name());
                                }
                            });
                            ui.label(RichText::new("Click on the board to place it. Back-side text is mirrored.").small().color(Color32::GRAY));
                        });
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Clearance");
                        let mut c = self.project.board.clearance;
                        if ui.add(egui::DragValue::new(&mut c).speed(0.01).range(0.05..=2.0).suffix(" mm")).on_hover_text("Minimum copper-to-copper gap checked by the DRC").changed() {
                            self.project.board.clearance = c;
                            self.analysis_dirty = true;
                            self.dirty = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("COPPER LAYERS").small().strong().color(Color32::GRAY));
                        let mut n = self.project.board.layers;
                        egui::ComboBox::from_id_salt("layer_count")
                            .selected_text(format!("{} layers", n))
                            .width(90.0)
                            .show_ui(ui, |ui| {
                                for k in [2u8, 4, 6, 8, 10, 12, 14, 16] {
                                    ui.selectable_value(&mut n, k, format!("{} layers", k));
                                }
                            });
                        if n != self.project.board.layers {
                            self.set_layer_count(n);
                        }
                    });
                    let mut toggle: Option<Layer> = None;
                    let mut pick: Option<Layer> = None;
                    for l in self.project.board.copper() {
                        ui.horizontal(|ui| {
                            let shown = !self.hidden_layers.contains(&l);
                            if ui
                                .small_button(if shown { "👁" } else { "–" })
                                .on_hover_text(if shown { "Hide this layer" } else { "Show this layer" })
                                .clicked()
                            {
                                toggle = Some(l);
                            }
                            let key = match l {
                                Layer::Front => " (F)".to_string(),
                                Layer::Back => " (B)".to_string(),
                                _ => String::new(),
                            };
                            let tracks = self.project.tracks.iter().filter(|t| t.layer == l).count();
                            let text = RichText::new(format!("■ {}{}", l.short(), key)).color(crate::pcb::layer_color(l));
                            if ui.radio(self.route_layer == l, text).on_hover_text("Route new tracks on this layer").clicked() {
                                pick = Some(l);
                            }
                            if tracks > 0 {
                                ui.label(RichText::new(format!("{} tracks", tracks)).small().color(Color32::GRAY));
                            }
                        });
                    }
                    if let Some(l) = toggle {
                        if let Some(i) = self.hidden_layers.iter().position(|x| *x == l) {
                            self.hidden_layers.remove(i);
                        } else {
                            self.hidden_layers.push(l);
                        }
                    }
                    if let Some(l) = pick {
                        self.switch_route_layer(l);
                    }
                    ui.label(RichText::new("PageUp / PageDown change layer while routing; V drops a via.").small().color(Color32::GRAY));
                    ui.label(RichText::new("TRACK WIDTH").small().strong().color(Color32::GRAY));
                    ui.horizontal(|ui| {
                        for w in [0.25f32, 0.5, 1.0] {
                            if ui.selectable_label((self.track_width - w).abs() < 1e-3, format!("{} mm", w)).clicked() {
                                self.track_width = w;
                            }
                        }
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new("BOARD SIZE").small().strong().color(Color32::GRAY));
                    let mut w = self.project.board.x1 - self.project.board.x0;
                    let mut h = self.project.board.y1 - self.project.board.y0;
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        changed |= ui.add(egui::DragValue::new(&mut w).speed(0.5).suffix(" mm")).changed();
                        ui.label("×");
                        changed |= ui.add(egui::DragValue::new(&mut h).speed(0.5).suffix(" mm")).changed();
                    });
                    if changed {
                        self.project.board.x1 = self.project.board.x0 + w.clamp(5.0, 500.0);
                        self.project.board.y1 = self.project.board.y0 + h.clamp(5.0, 500.0);
                        self.analysis_dirty = true;
                        self.dirty = true;
                    }
                    ui.add_space(4.0);
                    if ui.button("Arrange parts on the board").clicked() {
                        self.checkpoint();
                        self.project.auto_place();
                    }
                    let b = egui::Button::new(RichText::new("🏭 Manufacture with JLCPCB…").color(Color32::WHITE).strong())
                        .fill(Color32::from_rgb(30, 110, 60));
                    if ui.add(b).clicked() {
                        self.jlc.open = true;
                    }
                    if ui.button("Remove all tracks").clicked() {
                        self.checkpoint();
                        self.project.tracks.clear();
                        self.project.vias.clear();
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!(
                            "{} connection{} left to route",
                            self.conn.unrouted,
                            if self.conn.unrouted == 1 { "" } else { "s" }
                        ))
                        .color(if self.conn.unrouted == 0 { Color32::from_rgb(40, 140, 60) } else { Color32::from_rgb(180, 110, 0) }),
                    );
                }
            }

        });
    }

    fn properties(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("PROPERTIES").small().strong().color(Color32::GRAY));
        let mut ask: Option<String> = None;
        if !self.group.is_empty() {
            let g = &self.group;
            let n = |f: fn(&Sel) -> bool| g.iter().filter(|s| f(s)).count();
            let parts = n(|s| matches!(s, Sel::Comp(_)));
            let wires = n(|s| matches!(s, Sel::Wire(_)));
            let labels = n(|s| matches!(s, Sel::Label(_)));
            let tracks = n(|s| matches!(s, Sel::Track(_)));
            let vias = n(|s| matches!(s, Sel::Via(_)));
            let notes = n(|s| matches!(s, Sel::Note(_)));
            ui.label(RichText::new(format!("{} items selected", g.len())).strong().size(15.0));
            let mut lines = vec![];
            for (k, label) in [(parts, "part"), (wires, "wire"), (labels, "label"), (tracks, "track"), (vias, "via"), (notes, "annotation")] {
                if k > 0 {
                    lines.push(format!("{} {}{}", k, label, if k > 1 { "s" } else { "" }));
                }
            }
            ui.label(lines.join(", "));
            ui.label(
                RichText::new("Drag any selected item to move them together. Shift+click adds or removes an item.")
                    .small()
                    .color(Color32::GRAY),
            );
            ui.horizontal(|ui| {
                if ui.button("🗑 Delete all").on_hover_text("Del").clicked() {
                    self.delete_group();
                }
                if ui.button("Clear selection").on_hover_text("Esc").clicked() {
                    self.group.clear();
                    self.sel = Sel::None;
                }
            });
            return;
        }
        match self.sel {
            Sel::None => {
                ui.label(RichText::new("Nothing selected. Click a part, wire or track.").color(Color32::GRAY));
            }
            Sel::Text(i) => {
                ui.label(RichText::new("Board text").strong().size(15.0));
                if i < self.project.texts.len() {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.project.texts[i].text).desired_width(ui.available_width()));
                    if r.gained_focus() {
                        self.undo.push(self.project.clone());
                    }
                    if r.changed() {
                        self.dirty = true;
                    }
                    let mut size = self.project.texts[i].size;
                    ui.horizontal(|ui| {
                        ui.label("Height");
                        if ui.add(egui::DragValue::new(&mut size).speed(0.05).suffix(" mm")).changed() {
                            self.project.texts[i].size = size.clamp(0.5, 20.0);
                            self.dirty = true;
                        }
                    });
                    let current = self.project.texts[i].layer;
                    let mut pick = None;
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Layer");
                        for l in text_layers(&self.project.board) {
                            if ui.selectable_label(current == l, l.name()).clicked() && current != l {
                                pick = Some(l);
                            }
                        }
                    });
                    if let Some(l) = pick {
                        self.checkpoint();
                        self.project.texts[i].layer = l;
                    }
                    ui.horizontal(|ui| {
                        if ui.button("🔄 Rotate").on_hover_text("R").clicked() {
                            self.rotate_selection();
                        }
                        if ui.button("🗑 Delete").on_hover_text("Del").clicked() {
                            self.delete_selection();
                        }
                    });
                }
            }
            Sel::Comp(id) => {
                let Some(ci) = self.project.find(id) else { return };
                let inf = self.project.components[ci].info();
                ui.label(RichText::new(format!("{} — {}", self.project.components[ci].reference, inf.label)).strong().size(15.0));
                let kind = self.project.components[ci].kind;
                let mut flip: Option<&str> = None;
                let mut jlc_find: Option<u32> = None;
                let mut pick_fp: Option<u32> = None;
                egui::Grid::new("props").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    if !kind.is_power() {
                        ui.label("Reference");
                        let r = ui.add(egui::TextEdit::singleline(&mut self.project.components[ci].reference).desired_width(110.0));
                        if r.gained_focus() {
                            self.undo.push(self.project.clone());
                        }
                        if r.changed() {
                            self.analysis_dirty = true;
                            self.dirty = true;
                        }
                        ui.end_row();
                    }
                    ui.label(if kind == Kind::Power { "Rail" } else { "Value" });
                    if kind == Kind::Switch {
                        let closed = !self.project.components[ci].value.to_lowercase().contains("open");
                        let label = if closed { "closed (on)" } else { "open (off)" };
                        if ui.button(label).on_hover_text("Click to flip the switch").clicked() {
                            flip = Some(if closed { "open" } else { "closed" });
                        }
                    } else if kind != Kind::Gnd {
                        let r = ui.add(egui::TextEdit::singleline(&mut self.project.components[ci].value).desired_width(110.0));
                        if self.focus_value {
                            r.request_focus();
                            self.focus_value = false;
                        }
                        if r.gained_focus() {
                            self.undo.push(self.project.clone());
                        }
                        if r.changed() {
                            self.analysis_dirty = true;
                            self.dirty = true;
                        }
                    } else {
                        ui.label("GND");
                    }
                    ui.end_row();
                    if kind.has_footprint() {
                        ui.label("JLCPCB part");
                        ui.horizontal(|ui| {
                            let r = ui.add(egui::TextEdit::singleline(&mut self.project.components[ci].lcsc).hint_text("C…").desired_width(80.0));
                            if r.gained_focus() {
                                self.undo.push(self.project.clone());
                            }
                            if r.changed() {
                                self.dirty = true;
                            }
                            if ui.small_button("Find").on_hover_text("Search the JLCPCB catalogue").clicked() {
                                jlc_find = Some(id);
                            }
                        });
                        ui.end_row();
                        ui.label("Footprint");
                        let short = inf.footprint.rsplit(':').next().unwrap_or("").to_string();
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(short).truncate()).on_hover_text(&inf.footprint);
                            if ui.small_button("Change…").on_hover_text("Pick another footprint (package size, KiCad library)").clicked() {
                                pick_fp = Some(id);
                            }
                        });
                        ui.end_row();
                    }
                });
                if let Some(v) = flip {
                    self.set_value(id, v);
                }
                if let Some(fid) = pick_fp {
                    self.open_fp_picker(fid);
                }
                if let Some(fid) = jlc_find {
                    if let Some(i) = self.project.find(fid) {
                        let q = crate::jlc::query_for(&self.project.components[i]);
                        let ids: Vec<u32> = crate::jlc::bom(&self.project).into_iter().find(|l| l.ids.contains(&fid)).map(|l| l.ids).unwrap_or(vec![fid]);
                        self.jlc.open = true;
                        self.jlc.query = q.clone();
                        self.jlc.line = Some(ids);
                        self.jlc.results.clear();
                        self.jlc_open_search(q);
                    }
                }
                let common: &[&str] = match kind {
                    Kind::Resistor => &["100", "220", "330", "1k", "4.7k", "10k", "47k", "100k"],
                    Kind::Capacitor => &["10p", "100n", "1u", "10u"],
                    Kind::CapPolarized => &["10u", "47u", "100u", "470u"],
                    Kind::Inductor => &["1u", "10u", "100u"],
                    Kind::Led => &["red", "green", "yellow", "blue", "white"],
                    Kind::Regulator => &["3.3V", "5V", "12V"],
                    Kind::Battery => &["1.5V", "3V", "3.7V", "9V", "12V"],
                    Kind::Power => &["+3V3", "+5V", "+12V", "VCC"],
                    _ => &[],
                };
                if !common.is_empty() {
                    let mut pick = None;
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Common:").small().color(Color32::GRAY));
                        for v in common {
                            if ui.small_button(*v).clicked() {
                                pick = Some(*v);
                            }
                        }
                    });
                    if let Some(v) = pick {
                        self.set_value(id, v);
                    }
                }
                if let Some(n) = self.sim.notes.get(&id) {
                    ui.label(RichText::new(format!("⚡ {}", n)).color(Color32::from_rgb(150, 90, 0)));
                }
                if !kind.is_power() {
                    ui.add_space(2.0);
                    let mut hl = None;
                    egui::Grid::new("pins").striped(true).num_columns(3).show(ui, |ui| {
                        ui.label(RichText::new("Pin").small().strong());
                        ui.label(RichText::new("Net").small().strong());
                        ui.label(RichText::new("DC").small().strong());
                        ui.end_row();
                        for (pi, pd) in inf.pins.iter().enumerate() {
                            ui.label(&pd.name);
                            let n = self.nl.net_of(ci, pi);
                            let name = n.map(|n| self.nl.nets[n].name.clone()).unwrap_or_default();
                            if ui.link(name).clicked() {
                                hl = Some(n);
                            }
                            let v = n.and_then(|n| self.sim.net_v.get(n).copied().flatten());
                            ui.label(v.map(|v| fmt_si(v, "V")).unwrap_or_default());
                            ui.end_row();
                        }
                    });
                    if let Some(h) = hl {
                        self.highlight_net = h;
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("🔄 Rotate").on_hover_text("R").clicked() {
                        self.rotate_selection();
                    }
                    if ui.button("Duplicate").on_hover_text("Ctrl+D").clicked() {
                        self.duplicate_selection();
                    }
                    if ui.button("🗑 Delete").on_hover_text("Del").clicked() {
                        self.delete_selection();
                    }
                });
                if !kind.is_power() {
                    let r = self.project.components.get(ci).map(|c| c.reference.clone()).unwrap_or_default();
                    if ui.button(format!("💬 Ask the AI about {}", r)).clicked() {
                        ask = Some(format!("Tell me about {} in my circuit: is it connected and sized correctly? What should I check?", r));
                    }
                }
            }
            Sel::Wire(i) => {
                let n = self.nl.wire_net.get(i).copied();
                let name = n.map(|n| self.nl.nets[n].name.clone()).unwrap_or_default();
                ui.label(RichText::new("Wire").strong().size(15.0));
                ui.label(format!("Net: {}", name));
                if let Some(v) = n.and_then(|n| self.sim.net_v.get(n).copied().flatten()) {
                    ui.label(format!("DC voltage: {}", fmt_si(v, "V")));
                }
                if ui.button("🗑 Delete wire").clicked() {
                    self.delete_selection();
                }
                if ui.button(format!("💬 Ask the AI about net {}", name)).clicked() {
                    ask = Some(format!("Explain what net {} does in my circuit and whether its voltage is right.", name));
                }
            }
            Sel::Label(i) => {
                ui.label(RichText::new("Net label").strong().size(15.0));
                if i < self.project.labels.len() {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.project.labels[i].text).desired_width(140.0));
                    if r.gained_focus() {
                        self.undo.push(self.project.clone());
                    }
                    if r.changed() {
                        self.analysis_dirty = true;
                    }
                }
                if ui.button("🗑 Delete label").clicked() {
                    self.delete_selection();
                }
            }
            Sel::Track(i) => {
                ui.label(RichText::new("Track").strong().size(15.0));
                let net = self.conn.track_net(i).map(|n| self.nl.nets[n].name.clone()).unwrap_or_else(|| "(none)".into());
                ui.label(format!("Net: {}", net));
                if let Some(t) = self.project.tracks.get_mut(i) {
                    ui.label(format!("Layer: {}", t.layer.name()));
                    let mut w = t.width;
                    ui.horizontal(|ui| {
                        ui.label("Width");
                        if ui.add(egui::DragValue::new(&mut w).speed(0.05).suffix(" mm")).changed() {
                            t.width = w.clamp(0.1, 5.0);
                            self.analysis_dirty = true;
                        }
                    });
                    let layers = self.project.board.copper();
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Move to");
                        for l in layers {
                            if l != t.layer && ui.small_button(l.short()).clicked() {
                                t.layer = l;
                                self.analysis_dirty = true;
                            }
                        }
                    });
                }
                if ui.button("🗑 Delete track").clicked() {
                    self.delete_selection();
                }
            }
            Sel::Zone(i) => self.zone_properties(ui, i),
            Sel::Note(i) => self.note_properties(ui, i),
            Sel::Via(i) => {
                ui.label(RichText::new("Via").strong().size(15.0));
                let net = self.conn.via_net(i).map(|n| self.nl.nets[n].name.clone()).unwrap_or_else(|| "(none)".into());
                ui.label(format!("Net: {}", net));
                if ui.button("🗑 Delete via").clicked() {
                    self.delete_selection();
                }
            }
        }
        if let Some(q) = ask {
            self.ask_ai(q);
        }
    }

    fn ai_panel(&mut self, ui: &mut egui::Ui) {
        let mut acts: Vec<Act> = vec![];
        ui.add_space(4.0);
        let mut clear = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("💬 AI Assistant").size(17.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("⚙").on_hover_text("AI settings").clicked() {
                    self.show_ai_settings = true;
                }
                if ui.small_button("Clear").on_hover_text("Start a new conversation").clicked() {
                    clear = true;
                }
            });
        });
        if clear {
            self.chat.clear();
            self.welcome();
        }
        let online = self.ai.online();
        if online {
            ui.label(RichText::new(format!("✔ {}", self.ai.describe())).small().color(Color32::from_rgb(30, 140, 70)));
        } else {
            egui::Frame::none()
                .fill(Color32::from_rgb(255, 243, 205))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(230, 180, 60)))
                .rounding(8.0)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("No AI connected yet: answers come from the built-in checker.").color(Color32::from_rgb(120, 80, 0)));
                    let b = egui::Button::new(RichText::new("Connect an AI…").color(Color32::WHITE).strong()).fill(Color32::from_rgb(40, 90, 170));
                    if ui.add(b).clicked() {
                        self.show_ai_settings = true;
                    }
                });
        }
        ui.horizontal_wrapped(|ui| {
            for (label, q) in [
                ("🔍 Review my circuit", "Review my circuit: what's wrong with it?"),
                (
                    "🔧 Why doesn't it work?",
                    "My circuit doesn't work as expected. Walk me through debugging it step by step, starting with the most likely causes.",
                ),
                (
                    "⚡ Check my values",
                    "Check the values of my parts (resistors, capacitors, supply voltages). Are they right for what the circuit does?",
                ),
                (
                    "🔲 Help with the board",
                    "Look at my board layout: what is left to route and what should I fix before ordering the PCB?",
                ),
            ] {
                if ui.add_enabled(self.pending.is_none(), egui::Button::new(RichText::new(label).size(12.5))).clicked() {
                    acts.push(Act::Ask(q.into()));
                }
            }
        });
        ui.separator();
        let pending = self.pending.is_some();
        egui::TopBottomPanel::bottom("ai_input").frame(egui::Frame::none()).show_inside(ui, |ui| {
            ui.add_space(4.0);
            let te = egui::TextEdit::multiline(&mut self.input)
                .hint_text("Ask about your circuit… e.g. \"Why is D2 not lighting up?\"")
                .desired_rows(3)
                .desired_width(f32::INFINITY);
            let r = ui.add(te);
            let send_key = r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && i.modifiers.command);
            ui.horizontal(|ui| {
                if pending {
                    ui.spinner();
                    ui.label("Thinking…");
                } else {
                    ui.label(RichText::new("Ctrl+Enter to send").small().color(Color32::GRAY));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let b = egui::Button::new(RichText::new("Send").color(Color32::WHITE).strong()).fill(Color32::from_rgb(40, 90, 170));
                    if (ui.add_enabled(!pending, b).clicked() || send_key) && !self.input.trim().is_empty() {
                        acts.push(Act::Ask(std::mem::take(&mut self.input).trim().to_string()));
                    }
                });
            });
            ui.add_space(4.0);
        });
        egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            for (mi, m) in self.chat.iter().enumerate() {
                match m.role {
                    Role::User => {
                        ui.with_layout(Layout::top_down(Align::Max), |ui| {
                            egui::Frame::none().fill(Color32::from_rgb(40, 90, 170)).rounding(10.0).inner_margin(8.0).show(ui, |ui| {
                                ui.set_max_width(ui.available_width() * 0.85);
                                ui.label(RichText::new(&m.text).color(Color32::WHITE));
                            });
                        });
                    }
                    Role::Assistant => {
                        egui::Frame::none()
                            .fill(Color32::WHITE)
                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(220, 222, 230)))
                            .rounding(10.0)
                            .inner_margin(8.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                render_markdown(ui, &m.text);
                                if !m.actions.is_empty() {
                                    ui.add_space(4.0);
                                    let left = m.applied.iter().filter(|d| !**d).count();
                                    if m.actions.len() > 1 && left > 0 {
                                        let b = egui::Button::new(
                                            RichText::new(format!("✔ Apply all {} changes", left)).color(Color32::WHITE).strong(),
                                        )
                                        .fill(Color32::from_rgb(30, 110, 60));
                                        if ui.add(b).on_hover_text("One step: Ctrl+Z undoes them all").clicked() {
                                            acts.push(Act::ApplyAll(mi));
                                        }
                                    }
                                    for (k, a) in m.actions.iter().enumerate() {
                                        if m.applied.get(k).copied().unwrap_or(false) {
                                            ui.label(RichText::new(format!("✔ Done: {}", a.label())).small().color(Color32::from_rgb(40, 120, 60)));
                                        } else {
                                            let b = egui::Button::new(RichText::new(format!("Apply: {}", a.label())).color(Color32::WHITE))
                                                .fill(Color32::from_rgb(40, 140, 80));
                                            if ui.add(b).clicked() {
                                                acts.push(Act::Apply(mi, k));
                                            }
                                        }
                                    }
                                }
                                if !m.refs.is_empty() {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(RichText::new("Show:").small().color(Color32::GRAY));
                                        for r in &m.refs {
                                            if ui.small_button(format!("📍 {}", r)).clicked() {
                                                acts.push(Act::Focus(r.clone()));
                                            }
                                        }
                                    });
                                }
                            });
                    }
                    Role::Note => {
                        ui.label(RichText::new(format!("• {}", m.text)).italics().small().color(Color32::from_rgb(90, 110, 90)));
                    }
                    Role::Error => {
                        ui.label(RichText::new(format!("⚠ {}", m.text)).color(Color32::from_rgb(190, 40, 40)));
                    }
                }
                ui.add_space(6.0);
            }
        });
        self.apply_acts(acts);
    }

    fn online_search(&mut self) {
        let q = self.online_query.trim().to_string();
        if q.is_empty() || self.online_job.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        let q2 = q.clone();
        std::thread::spawn(move || {
            let _ = tx.send(OnlineMsg::Hits(q2.clone(), crate::online::search(&q2)));
            ctx.request_repaint();
        });
        self.online_job = Some(rx);
        self.online_status = format!("Searching the KiCad library for \"{}\"…", q);
    }

    fn online_fetch(&mut self, hit: &crate::online::Hit) {
        if self.online_job.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        let (lib, name) = (hit.lib.clone(), hit.name.clone());
        std::thread::spawn(move || {
            let r = crate::online::fetch_part(&lib, &name);
            let _ = tx.send(OnlineMsg::Part(format!("{}:{}", lib, name), r));
            ctx.request_repaint();
        });
        self.online_job = Some(rx);
        self.online_status = format!("Downloading {}…", hit.name);
    }

    fn poll_online(&mut self) {
        let Some(rx) = &self.online_job else { return };
        let msg = match rx.try_recv() {
            Ok(m) => m,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.online_job = None;
                self.online_status = "The download stopped unexpectedly.".into();
                return;
            }
        };
        self.online_job = None;
        match msg {
            OnlineMsg::Hits(q, Ok(h)) => {
                self.online_status = if h.is_empty() {
                    format!("Nothing found for \"{}\". Try the part family (e.g. LM78, NE555) or Library:Name.", q)
                } else {
                    format!("{} part{} found for \"{}\"", h.len(), if h.len() == 1 { "" } else { "s" }, q)
                };
                self.online_hits = h;
            }
            OnlineMsg::Hits(_, Err(e)) => self.online_status = format!("Search failed: {}", e),
            OnlineMsg::Part(id, Ok(part)) => {
                self.checkpoint();
                let rect = self.canvas_rect;
                let w = self.sch_view.w(rect.min, rect.center());
                let start = ((w.0 / GRID as f32).round() as i32 * GRID, (w.1 / GRID as f32).round() as i32 * GRID);
                let pos = self.free_spot_for(start, part.kind, 0, Some(&part.custom));
                let notes = part.notes.clone();
                let desc = part.description.clone();
                let cid = self.project.add_part(part, pos);
                self.mode = Mode::Schematic;
                self.center_on(Mode::Schematic, (pos.0 as f32, pos.1 as f32));
                self.sch_tool = SchTool::Select;
                self.sel = Sel::Comp(cid);
                let r = self.project.find(cid).map(|i| self.project.components[i].reference.clone()).unwrap_or_default();
                self.online_status = format!("Added {} ({}). Drag it into place and wire it.", r, id);
                self.status = self.online_status.clone();
                let mut note = format!("Added {} from the KiCad library: {}", r, id);
                if !desc.is_empty() {
                    note.push_str(&format!(" — {}", desc));
                }
                self.push_msg(Role::Note, note);
                for n in notes {
                    self.push_msg(Role::Note, n);
                }
            }
            OnlineMsg::Part(id, Err(e)) => self.online_status = format!("Couldn't download {}: {}", id, e),
        }
    }

    fn online_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("ONLINE LIBRARY (KiCad)").small().strong().color(Color32::GRAY));
        let mut go = false;
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.online_query)
                    .hint_text("Search a part: LM7805, NE555…")
                    .desired_width(ui.available_width() - 34.0),
            );
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                go = true;
            }
            if ui.add_enabled(self.online_job.is_none(), egui::Button::new("🔍")).on_hover_text("Search the official KiCad libraries online").clicked() {
                go = true;
            }
        });
        if go {
            self.online_search();
        }
        if self.online_job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(&self.online_status).small());
            });
        } else if !self.online_status.is_empty() {
            ui.label(RichText::new(&self.online_status).small().color(Color32::DARK_GRAY));
        }
        if !self.online_hits.is_empty() {
            let mut fetch = None;
            let mut clear = false;
            egui::ScrollArea::vertical().id_salt("online_hits").max_height(220.0).show(ui, |ui| {
                for h in &self.online_hits {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(self.online_job.is_none(), egui::Button::new("Add").small()).on_hover_text("Download and add to the schematic").clicked() {
                            fetch = Some(h.clone());
                        }
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&h.name).strong().size(12.5));
                            ui.label(RichText::new(&h.lib).small().color(Color32::GRAY));
                        });
                    });
                }
            });
            if ui.small_button("Clear results").clicked() {
                clear = true;
            }
            if let Some(h) = fetch {
                self.online_fetch(&h);
            }
            if clear {
                self.online_hits.clear();
                self.online_status.clear();
            }
        }
    }

    /// Apply one (or all remaining) design changes proposed in an AI message, as one undo step.
    pub fn apply_ai_changes(&mut self, mi: usize, only: Option<usize>) {
        let Some(m) = self.chat.get(mi) else { return };
        let todo: Vec<(usize, ai::AiAction)> = m
            .actions
            .iter()
            .cloned()
            .enumerate()
            .filter(|(k, _)| only.map_or(true, |o| o == *k) && !m.applied.get(*k).copied().unwrap_or(false))
            .collect();
        if todo.is_empty() {
            return;
        }
        self.checkpoint();
        let mut done = vec![];
        let mut failed = vec![];
        for (k, a) in todo {
            // Later changes see earlier ones (e.g. CONNECT after ADD).
            self.analyze();
            match self.apply_ai_action(&a) {
                Ok(msg) => {
                    done.push(msg);
                    if let Some(f) = self.chat.get_mut(mi).and_then(|m| m.applied.get_mut(k)) {
                        *f = true;
                    }
                }
                Err(e) => failed.push(format!("{}: {}", a.label(), e)),
            }
        }
        self.analysis_dirty = true;
        if !done.is_empty() {
            self.status = format!("Applied: {}", done.join("; "));
            self.push_msg(Role::Note, format!("Applied: {}.", done.join("; ")));
            self.fit_sch = done.len() > 1;
        }
        for f in failed {
            self.push_msg(Role::Error, format!("Couldn't apply {}", f));
        }
    }

    fn apply_acts(&mut self, acts: Vec<Act>) {
        if acts.is_empty() {
            return;
        }
        let issues = self.all_issues();
        for a in acts {
            match a {
                Act::Ask(q) => self.ask_ai(q),
                Act::Apply(mi, k) => self.apply_ai_changes(mi, Some(k)),
                Act::ApplyAll(mi) => self.apply_ai_changes(mi, None),
                Act::Fix(id, v) => self.set_value(id, &v),
                Act::Focus(r) => {
                    if let Some(ci) = self.project.find_ref(&r) {
                        let id = self.project.components[ci].id;
                        self.focus_comp(id);
                    }
                }
                Act::ShowIssue(i) => {
                    if let Some(is) = issues.get(i) {
                        self.show_issue(is);
                    }
                }
            }
        }
    }

    fn problems_panel(&mut self, ui: &mut egui::Ui) {
        let issues = self.all_issues();
        let mut acts = vec![];
        let counts = [Severity::Error, Severity::Warning, Severity::Info].map(|s| issues.iter().filter(|i| i.severity == s).count());
        let pending = self.pending.is_some();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Problems").strong().size(15.0));
            ui.toggle_value(&mut self.show_filter[0], RichText::new(format!("{} error{}", counts[0], if counts[0] == 1 { "" } else { "s" })).color(Color32::from_rgb(200, 50, 50)));
            ui.toggle_value(&mut self.show_filter[1], RichText::new(format!("{} warning{}", counts[1], if counts[1] == 1 { "" } else { "s" })).color(Color32::from_rgb(200, 130, 0)));
            ui.toggle_value(&mut self.show_filter[2], RichText::new(format!("{} note{}", counts[2], if counts[2] == 1 { "" } else { "s" })).color(Color32::from_rgb(60, 110, 180)));
            ui.label(RichText::new("· checked live: wiring (ERC), DC simulation (SIM), board (DRC)").small().color(Color32::GRAY));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(!pending && !issues.is_empty(), egui::Button::new("💬 Explain these to me")).clicked() {
                    acts.push(Act::Ask("Explain the problems the checker found, most important first, and tell me how to fix each one.".into()));
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if issues.is_empty() {
                ui.label(RichText::new("✔ No problems found.").color(Color32::from_rgb(40, 140, 60)));
            }
            for (i, is) in issues.iter().enumerate() {
                let fi = match is.severity {
                    Severity::Error => 0,
                    Severity::Warning => 1,
                    Severity::Info => 2,
                };
                if !self.show_filter[fi] {
                    continue;
                }
                let col = [Color32::from_rgb(200, 50, 50), Color32::from_rgb(200, 130, 0), Color32::from_rgb(60, 110, 180)][fi];
                ui.horizontal(|ui| {
                    if ui.small_button("Show").clicked() {
                        acts.push(Act::ShowIssue(i));
                    }
                    if ui.add_enabled(!pending, egui::Button::new("Ask AI").small()).clicked() {
                        acts.push(Act::Ask(format!("Explain this problem and how to fix it: \"{}\"", is.message)));
                    }
                    if let Some((id, v)) = &is.fix {
                        if ui.small_button(format!("Fix: {}", v)).clicked() {
                            acts.push(Act::Fix(*id, v.clone()));
                        }
                    }
                    ui.label(RichText::new("⚠").color(col));
                    ui.label(RichText::new(is.source).small().monospace().color(Color32::GRAY));
                    ui.label(&is.message);
                });
            }
        });
        self.apply_acts(acts);
    }

    pub fn new_project(&mut self) {
        if self.blocks.editing.is_some() {
            self.status = "Close the block editor first.".into();
            return;
        }
        self.checkpoint();
        self.project = Project { name: "New circuit".into(), ..Default::default() };
        self.path.clear();
        self.sel = Sel::None;
        self.highlight_net = None;
        self.wire_draw = None;
        self.route = None;
        self.drag = None;
        self.fit_sch = true;
        self.fit_pcb = true;
        self.status = "New empty project. Pick a part on the left to start.".into();
    }

    /// Show the system's file dialog (or the built-in browser if there is none).
    pub fn open_dialog(&mut self, purpose: Purpose) {
        if self.blocks.editing.is_some() {
            if purpose.saving() {
                self.save_block_edit();
            } else {
                self.status = "Close the block editor first.".into();
            }
            return;
        }
        if self.native.is_some() || self.browser.is_some() {
            return;
        }
        let sugg = PathBuf::from(self.suggested_path());
        let mut dir = if !self.path.is_empty() {
            sugg.parent().map(|p| p.to_path_buf()).unwrap_or_else(default_dir)
        } else if let Some(r) = self.recent.first() {
            PathBuf::from(r).parent().map(|p| p.to_path_buf()).unwrap_or_else(default_dir)
        } else {
            default_dir()
        };
        if purpose.saving() {
            let _ = std::fs::create_dir_all(&dir);
        }
        if !dir.is_dir() {
            dir = dir.parent().map(|p| p.to_path_buf()).filter(|p| p.is_dir()).unwrap_or_else(|| PathBuf::from("."));
        }
        let mut name = sugg.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "circuit.fcad".into());
        if purpose == Purpose::ExportKicad {
            let stem = PathBuf::from(&name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "circuit".into());
            name = format!("{}.kicad_pro", stem);
        }
        if std::env::var_os("FERRISCAD_BUILTIN_DIALOG").is_some() {
            self.browser = Some(crate::filedlg::Browser::new(purpose, dir, name));
        } else {
            self.native = Some(crate::filedlg::start_native(purpose, dir, name, self.ctx.clone()));
        }
    }

    fn picked(&mut self, purpose: Purpose, path: PathBuf) {
        // Work with the real location, not the dialog's one-file "document portal" path.
        let path = crate::filedlg::host_path(&path);
        let s = path.to_string_lossy().to_string();
        match purpose {
            Purpose::Save => {
                let p = if s.ends_with(".fcad") || s.ends_with(".vpcb") { s } else { format!("{}.fcad", s) };
                self.save_to(&p);
            }
            Purpose::ExportKicad => self.export_kicad(&s),
            _ => self.open_from(&s),
        }
    }

    /// Write the project as a KiCad project (.kicad_pro + .kicad_sch + .kicad_pcb).
    pub fn export_kicad(&mut self, path: &str) {
        let base = path.trim_end_matches(".kicad_pro").trim_end_matches(".kicad_sch").trim_end_matches(".kicad_pcb");
        let mut pro = PathBuf::from(format!("{}.kicad_pro", base));
        if crate::filedlg::is_portal_path(&pro) {
            // The dialog only opened access to this one file. Create it, then ask the portal
            // where it really lives, so the schematic and board go into the same real folder.
            let _ = std::fs::write(&pro, "{}");
            let real = crate::filedlg::host_path(&pro);
            if crate::filedlg::is_portal_path(&real) {
                self.status = "The file dialog only gave access to one file, so the schematic and board can't be written next to it. \
                               Choose a folder in your home directory, or use the built-in browser."
                    .into();
                self.push_msg(Role::Error, self.status.clone());
                return;
            }
            pro = real;
        }
        if self.analysis_dirty {
            self.analyze();
            self.analysis_dirty = false;
        }
        match crate::kicad_export::export(&self.project, &self.nl, &self.conn, &pro) {
            Ok(files) => {
                let missing: Vec<String> = files.iter().filter(|f| !f.is_file()).map(|f| f.display().to_string()).collect();
                if !missing.is_empty() {
                    self.status = format!("KiCad export incomplete: {} wasn't created.", missing.join(", "));
                    self.push_msg(Role::Error, self.status.clone());
                    return;
                }
                let names: Vec<String> = files.iter().filter_map(|f| f.file_name().map(|n| n.to_string_lossy().to_string())).collect();
                self.status = format!("Exported to KiCad: {}", names.join(", "));
                self.push_msg(
                    Role::Note,
                    format!(
                        "Exported to KiCad in {}: {}. Open the .kicad_pro in KiCad 8 or 9.",
                        pro.parent().map(|d| d.display().to_string()).unwrap_or_default(),
                        names.join(", ")
                    ),
                );
            }
            Err(e) => {
                self.status = format!("Couldn't export to KiCad: {}", e);
                self.push_msg(Role::Error, self.status.clone());
            }
        }
    }

    fn file_dialogs(&mut self, ctx: &egui::Context) {
        if let Some(n) = &self.native {
            let purpose = n.purpose;
            let quick = n.started.elapsed().as_millis() < 400;
            match n.rx.try_recv() {
                Ok(Some(p)) => {
                    self.native = None;
                    self.picked(purpose, p);
                }
                Ok(None) => {
                    self.native = None;
                    if quick {
                        // No native dialog answered: use the built-in browser.
                        self.native = None;
                        std::env::set_var("FERRISCAD_BUILTIN_DIALOG", "1");
                        self.open_dialog(purpose);
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.native = None;
                    std::env::set_var("FERRISCAD_BUILTIN_DIALOG", "1");
                    self.open_dialog(purpose);
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(b) = &mut self.browser {
            let purpose = b.purpose;
            match b.ui(ctx, &self.recent) {
                crate::filedlg::Outcome::Pending => {}
                crate::filedlg::Outcome::Cancel => self.browser = None,
                crate::filedlg::Outcome::Pick(p) => {
                    self.browser = None;
                    self.picked(purpose, p);
                }
            }
        }
    }

    fn remember(&mut self, path: &str) {
        let p = std::fs::canonicalize(path).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| path.to_string());
        self.recent.retain(|r| r != &p);
        self.recent.insert(0, p);
        self.recent.truncate(12);
        if let Some(d) = ai::config_dir() {
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join("recent.json"), serde_json::to_string_pretty(&self.recent).unwrap_or_default());
        }
    }

    fn ai_settings_ui(&mut self, ctx: &egui::Context) {
        if !self.show_ai_settings {
            self.ai_found = None;
            return;
        }
        if self.ai_found.is_none() {
            self.ai_found = Some((ai::find_claude_cli().is_some(), ai::ollama_models()));
        }
        let (has_cli, models) = self.ai_found.clone().unwrap_or_default();
        if let Some(rx) = &self.folder_pick {
            match rx.try_recv() {
                Ok(Some(p)) => {
                    let s = p.to_string_lossy().to_string();
                    if !self.ai.doc_dirs.contains(&s) {
                        self.ai.doc_dirs.push(s);
                    }
                    self.folder_pick = None;
                }
                Ok(None) | Err(TryRecvError::Disconnected) => self.folder_pick = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let mut open = true;
        let mut close = false;
        let mut test = false;
        let ok = Color32::from_rgb(30, 140, 70);
        let muted = Color32::GRAY;
        egui::Window::new("Connect an AI")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_width(520.0);
                ui.label(RichText::new("Choose the AI that helps you debug your circuit.").strong());
                ui.add_space(4.0);

                ui.radio_value(&mut self.ai.provider, Provider::ClaudeCode, RichText::new("Claude Code on this computer — no key needed").strong());
                ui.indent("cc", |ui| {
                    if has_cli {
                        ui.label(RichText::new("✔ Found. It uses the Claude account you already signed in with.").color(ok));
                    } else {
                        ui.label(RichText::new("Not installed. Install it from claude.com/claude-code, run `claude` once to sign in, then reopen this window.").color(muted));
                    }
                    if self.ai.provider == Provider::ClaudeCode {
                        ui.horizontal(|ui| {
                            ui.label("Model:");
                            for (m, label) in [("", "default"), ("sonnet", "Sonnet"), ("opus", "Opus"), ("haiku", "Haiku (fastest)")] {
                                if ui.selectable_label(self.ai.cli_model == m, label).clicked() {
                                    self.ai.cli_model = m.into();
                                }
                            }
                        });
                        ui.add_space(4.0);
                        ui.checkbox(&mut self.ai.allow_web, RichText::new("Let the AI search the internet (datasheets, KiCad library parts)").strong());
                        ui.checkbox(&mut self.ai.allow_files, RichText::new("Let the AI read documentation on this computer (read-only)").strong());
                        if self.ai.allow_files {
                            ui.indent("docs", |ui| {
                                let proj = self.project_dir.as_ref().map(|p| p.display().to_string());
                                ui.checkbox(
                                    &mut self.ai.include_project,
                                    format!("The project's folder{}", proj.map(|p| format!(": {}", p)).unwrap_or_else(|| " (open a project first)".into())),
                                );
                                let mut unshare = None;
                                for (i, d) in self.ai.doc_dirs.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        if ui.small_button("✕").on_hover_text("Stop sharing this folder").clicked() {
                                            unshare = Some(i);
                                        }
                                        let ok = std::path::Path::new(d).is_dir();
                                        ui.label(RichText::new(d).color(if ok { Color32::DARK_GRAY } else { Color32::from_rgb(190, 40, 40) }));
                                        if !ok {
                                            ui.label(RichText::new("(not found)").small().color(Color32::from_rgb(190, 40, 40)));
                                        }
                                    });
                                }
                                if let Some(i) = unshare {
                                    let gone = self.ai.doc_dirs[i].clone();
                                    self.ai.doc_dirs.retain(|d| *d != gone);
                                }
                                ui.horizontal(|ui| {
                                    if ui.add_enabled(self.folder_pick.is_none(), egui::Button::new("Add a folder…")).clicked() {
                                        self.folder_pick = Some(crate::filedlg::start_pick_folder("Folder the AI may read", ctx.clone()));
                                    }
                                    ui.add(egui::TextEdit::singleline(&mut self.new_doc_dir).hint_text("or type a path").desired_width(220.0));
                                    let p = self.new_doc_dir.trim().to_string();
                                    if ui.add_enabled(std::path::Path::new(&p).is_dir(), egui::Button::new("Add")).clicked() {
                                        if !self.ai.doc_dirs.contains(&p) {
                                            self.ai.doc_dirs.push(p);
                                        }
                                        self.new_doc_dir.clear();
                                    }
                                });
                                ui.label(
                                    RichText::new(
                                        "The AI can open datasheets (PDF), notes and KiCad files in these folders to answer. \
                                         It only reads: it never changes your files. What it reads is sent to Claude.",
                                    )
                                    .small()
                                    .color(muted),
                                );
                            });
                        }
                    }
                });
                ui.add_space(4.0);

                ui.radio_value(&mut self.ai.provider, Provider::Claude, RichText::new("Claude with an API key").strong());
                if self.ai.provider == Provider::Claude {
                    ui.indent("api", |ui| {
                        ui.horizontal(|ui| {
                            ui.label("1. Create a key at");
                            ui.hyperlink_to("console.anthropic.com", "https://console.anthropic.com/settings/keys");
                        });
                        ui.label("2. Paste it here:");
                        ui.add(egui::TextEdit::singleline(&mut self.ai.api_key).password(true).hint_text("sk-ant-…").desired_width(360.0));
                        ui.checkbox(&mut self.ai.remember_key, "Remember the key on this computer");
                        ui.horizontal(|ui| {
                            ui.label("Model:");
                            for m in ["claude-sonnet-5-5", "claude-opus-5-5", "claude-haiku-5-5"] {
                                if ui.selectable_label(self.ai.model == m, m).clicked() {
                                    self.ai.model = m.into();
                                }
                            }
                        });
                    });
                }
                ui.add_space(4.0);

                ui.radio_value(&mut self.ai.provider, Provider::OpenAiCompatible, RichText::new("A local model (Ollama) or another OpenAI-compatible service").strong());
                if self.ai.provider == Provider::OpenAiCompatible {
                    ui.indent("oai", |ui| {
                        if models.is_empty() {
                            ui.label(RichText::new("No Ollama models found running on this computer.").color(muted));
                        } else {
                            ui.label(RichText::new(format!("✔ Ollama is running with {} model(s). Pick one:", models.len())).color(ok));
                            ui.horizontal_wrapped(|ui| {
                                for m in &models {
                                    if ui.selectable_label(self.ai.model == *m, m).clicked() {
                                        self.ai.model = m.clone();
                                        self.ai.base_url = "http://localhost:11434/v1".into();
                                    }
                                }
                            });
                            ui.label(RichText::new("Small or code-only models give weak electronics advice.").small().color(muted));
                        }
                        egui::Grid::new("oaigrid").num_columns(2).show(ui, |ui| {
                            ui.label("Address");
                            ui.add(egui::TextEdit::singleline(&mut self.ai.base_url).desired_width(300.0));
                            ui.end_row();
                            ui.label("Model");
                            ui.add(egui::TextEdit::singleline(&mut self.ai.model).desired_width(300.0));
                            ui.end_row();
                            ui.label("Key (if needed)");
                            ui.add(egui::TextEdit::singleline(&mut self.ai.api_key).password(true).desired_width(300.0));
                            ui.end_row();
                        });
                    });
                }
                ui.add_space(4.0);
                ui.radio_value(&mut self.ai.provider, Provider::Offline, "No AI — built-in checker only");
                ui.add_space(6.0);
                ui.label(
                    RichText::new("When you ask a question, a text description of your circuit (parts, connections, check results) is sent to the AI you chose.")
                        .small()
                        .color(muted),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    let b = egui::Button::new(RichText::new("Save and test").color(Color32::WHITE).strong()).fill(Color32::from_rgb(40, 90, 170));
                    if ui.add_enabled(self.ai.online() && self.pending.is_none(), b).clicked() {
                        test = true;
                        close = true;
                    }
                    if ui.button("Save").clicked() {
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.ai = AiSettings::load();
                        self.show_ai_settings = false;
                    }
                    if !self.ai.online() && self.ai.provider != Provider::Offline {
                        ui.label(RichText::new("Not ready yet").color(Color32::from_rgb(180, 110, 0)));
                    }
                });
            });
        if close {
            match self.ai.save() {
                Ok(_) => self.status = format!("AI: {}", self.ai.describe()),
                Err(e) => self.status = format!("Couldn't save AI settings: {}", e),
            }
            self.show_ai_settings = false;
        }
        if !open {
            self.show_ai_settings = false;
        }
        if test {
            self.ask_ai("Hello! Can you see my circuit? In two sentences, tell me what it is and the first thing you'd check.".into());
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let hint = match (self.mode, self.sch_tool, self.pcb_tool) {
                (Mode::Schematic, SchTool::Wire, _) => "Wire: click to start, click to bend, click a pin or double-click to finish · Esc ends",
                (Mode::Schematic, SchTool::NoConnect, _) => "No connect: click a pin left open on purpose · click the cross again to remove it · Esc stops",
                (Mode::Schematic, SchTool::Label, _) => "Label: click on a wire to name its net",
                (Mode::Schematic, SchTool::Note(NoteKind::Line), _) => "Line: click each point · right-click or Esc ends",
                (Mode::Schematic, SchTool::Note(NoteKind::Box), _) => "Text box: drag, or click two opposite corners · Esc cancels",
                (Mode::Schematic, SchTool::Note(NoteKind::Text), _) => "Text: click where it starts, then type · Esc stops",
                (Mode::Schematic, SchTool::Place(_), _) => "Click to place · R rotates · Esc stops",
                (Mode::Pcb, _, PcbTool::Route) => "Route: click a pad, click to bend, click the target pad · V via + switch layer · Esc ends",
                (Mode::Pcb, _, PcbTool::Via) => "Click to drop a via",
                (Mode::Pcb, _, PcbTool::Zone) => "Zone: click the corners · double-click or Enter closes · Esc cancels",
                (Mode::Pcb, _, PcbTool::Text) => "Click to place the text · R rotates the selected text · Esc stops",
                (Mode::Schematic, _, _) => "Drag to move · drag on empty space to select several · Shift+click adds · Ctrl+A all · double-click edits a value",
                _ => "Drag to move · drag on empty space to select several · Shift+click adds · scroll to zoom · right-drag to pan",
            };
            let text = if !self.hover_info.is_empty() { self.hover_info.clone() } else { self.status.clone() };
            ui.label(RichText::new(text).color(Color32::from_rgb(40, 40, 60)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some((x, y)) = self.cursor {
                    let txt = match self.mode {
                        Mode::Schematic => format!("X {:.0}  Y {:.0}", x, y),
                        Mode::Pcb | Mode::View3D => format!("X {:.2} mm  Y {:.2} mm", x, y),
                    };
                    ui.label(RichText::new(txt).monospace());
                }
                ui.separator();
                ui.label(RichText::new(hint).small().color(Color32::GRAY));
            });
        });
    }

    fn take_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        self.frame += 1;
        if std::env::var_os("FERRISCAD_DEBUG").is_some() {
            eprintln!("frame {} at {:?}", self.frame, std::time::Instant::now());
        }
        ctx.request_repaint();
        // Wait (a little) for downloads and AI answers so the picture shows the result.
        let busy = self.online_job.is_some() || self.pending.is_some() || self.jlc.job.is_some();
        let started = *self.shot_started.get_or_insert_with(std::time::Instant::now);
        if self.frame >= 120 && !self.shot_requested && (!busy || started.elapsed().as_secs() > 20) {
            self.shot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if let Err(e) = image::save_buffer(&path, &bytes, w as u32, h as u32, image::ColorType::Rgba8) {
                eprintln!("could not save screenshot: {}", e);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.frame > 900 && started.elapsed().as_secs() > 40 {
            eprintln!("screenshot timed out");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// Screenshot runs used by automatic checks shouldn't change the person's recent list,
/// except in explicit dialog tests.
fn screenshot_free_of_side_effects(app: &App) -> bool {
    app.screenshot.is_none() || std::env::var_os("FERRISCAD_TEST_OPEN").is_some()
}

/// The folder a project file lives in (or the folder itself for a KiCad project folder).
fn project_folder(path: &str) -> Option<PathBuf> {
    let p = std::fs::canonicalize(path).ok()?;
    if p.is_dir() {
        Some(p)
    } else {
        p.parent().map(|d| d.to_path_buf())
    }
}

/// Layers a board text can be on: silkscreens first, then every copper layer.
pub fn text_layers(b: &Board) -> Vec<TextLayer> {
    let mut v = vec![TextLayer::SilkFront, TextLayer::SilkBack];
    v.extend(b.copper().into_iter().map(TextLayer::Copper));
    v
}

fn load_recent() -> Vec<String> {
    ai::config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("recent.json")).ok())
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| std::path::Path::new(p).exists())
        .collect()
}

/// Open a FerrisCad project or import a KiCad one; returns notes about the import.
pub fn load_any(path: &str) -> Result<(Project, Vec<String>), String> {
    let pb = std::path::Path::new(path);
    if crate::kicad::is_kicad_path(pb) {
        let im = crate::kicad::import(pb)?;
        return Ok((im.project, im.notes));
    }
    let s = std::fs::read_to_string(pb).map_err(|e| e.to_string())?;
    let p = serde_json::from_str::<Project>(&s).map_err(|e| e.to_string())?;
    Ok((p, vec![]))
}

/// Very small markdown renderer for assistant answers.
fn render_markdown(ui: &mut egui::Ui, text: &str) {
    let mut in_code = false;
    for line in text.lines() {
        let t = line.trim_end();
        if t.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            ui.label(RichText::new(t).monospace().small());
            continue;
        }
        if t.trim().is_empty() {
            ui.add_space(3.0);
            continue;
        }
        let clean = t.replace("**", "").replace('`', "");
        let s = clean.trim_start();
        if let Some(h) = s.strip_prefix('#') {
            ui.label(RichText::new(h.trim_start_matches('#').trim()).strong().size(14.5));
        } else if let Some(b) = s.strip_prefix("- ").or_else(|| s.strip_prefix("* ")) {
            let indent = (clean.len() - s.len()) as f32 * 4.0;
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent);
                ui.label("•");
                ui.label(b);
            });
        } else {
            ui.label(s);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.analysis_dirty {
            self.analyze();
            self.analysis_dirty = false;
        }
        self.poll_ai();
        self.poll_online();
        self.poll_jlc();
        self.watch_source();
        self.ai.project_dir = self.project_dir.clone();
        self.handle_keys(ctx);
        self.hover_info.clear();
        self.cursor = None;

        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu_bar(ui));
        egui::TopBottomPanel::top("toolbar")
            .frame(egui::Frame::none().fill(Color32::from_rgb(248, 249, 252)).inner_margin(egui::Margin::symmetric(4.0, 6.0)))
            .show(ctx, |ui| self.toolbar(ui));
        self.reload_banner(ctx);
        self.block_banner(ctx);
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        egui::TopBottomPanel::bottom("problems").resizable(true).default_height(150.0).min_height(60.0).show(ctx, |ui| {
            ui.add_space(4.0);
            self.problems_panel(ui);
        });
        egui::SidePanel::left("tools").resizable(true).default_width(240.0).min_width(200.0).max_width(360.0).show(ctx, |ui| self.left_panel(ui));
        egui::SidePanel::right("ai").resizable(true).default_width(390.0).min_width(280.0).show(ctx, |ui| self.ai_panel(ui));
        if self.analysis_dirty {
            // A panel above may have replaced or edited the project this frame.
            self.analyze();
            self.analysis_dirty = false;
        }
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| match self.mode {
            Mode::Schematic => self.sch_canvas(ui),
            Mode::Pcb => self.pcb_canvas(ui),
            Mode::View3D => self.view3d_canvas(ui),
        });
        self.file_dialogs(ctx);
        self.ai_settings_ui(ctx);
        self.jlc_window(ctx);
        self.blocks_window(ctx);
        self.fp_window(ctx);
        self.take_screenshot(ctx);
    }
}

/// Zoom with the wheel around the cursor; pan with middle or right drag.
pub fn zoom_pan(view: &mut View, resp: &egui::Response, ui: &egui::Ui, rect: Rect, minz: f32, maxz: f32) {
    if let Some(hp) = resp.hover_pos() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let mut f = pinch;
        if scroll != 0.0 {
            f *= (scroll * 0.0018).exp();
        }
        if (f - 1.0).abs() > 1e-4 {
            let nz = (view.zoom * f).clamp(minz, maxz);
            let local = hp - rect.min;
            view.offset = local - (local - view.offset) * (nz / view.zoom);
            view.zoom = nz;
        }
    }
    if resp.dragged_by(egui::PointerButton::Middle) || resp.dragged_by(egui::PointerButton::Secondary) {
        view.offset += resp.drag_delta();
    }
}
