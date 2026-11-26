//! Footprint picker: change the footprint of a part by choosing one from the KiCad libraries
//! (downloaded once, then cached) or by loading a `.kicad_mod` file.

use crate::app::App;
use crate::model::*;
use eframe::egui;
use std::sync::mpsc::{Receiver, TryRecvError};

pub enum FpMsg {
    Libs(Result<Vec<String>, String>),
    Names(String, Result<Vec<String>, String>),
    Got(u32, Result<FpOver, String>),
}

#[derive(Default)]
pub struct FpPicker {
    pub target: Option<u32>,
    pub lib_filter: String,
    pub lib: String,
    pub libs: Vec<String>,
    pub names: Vec<String>,
    pub filter: String,
    pub manual: String,
    pub msg: String,
    pub job: Option<Receiver<FpMsg>>,
}

/// Footprints worth offering first for a kind of part.
fn suggestions(kind: Kind) -> &'static [(&'static str, &'static str)] {
    match kind {
        Kind::Resistor => &[
            ("0402", "Resistor_SMD:R_0402_1005Metric"),
            ("0603", "Resistor_SMD:R_0603_1608Metric"),
            ("0805", "Resistor_SMD:R_0805_2012Metric"),
            ("1206", "Resistor_SMD:R_1206_3216Metric"),
            ("2512", "Resistor_SMD:R_2512_6332Metric"),
            ("THT 0.25W", "Resistor_THT:R_Axial_DIN0207_L6.3mm_D2.5mm_P10.16mm_Horizontal"),
        ],
        Kind::Capacitor => &[
            ("0402", "Capacitor_SMD:C_0402_1005Metric"),
            ("0603", "Capacitor_SMD:C_0603_1608Metric"),
            ("0805", "Capacitor_SMD:C_0805_2012Metric"),
            ("1206", "Capacitor_SMD:C_1206_3216Metric"),
            ("THT 5mm", "Capacitor_THT:C_Disc_D5.0mm_W2.5mm_P5.00mm"),
        ],
        Kind::CapPolarized => &[
            ("Radial 5mm", "Capacitor_THT:CP_Radial_D5.0mm_P2.00mm"),
            ("Radial 6.3mm", "Capacitor_THT:CP_Radial_D6.3mm_P2.50mm"),
            ("Tantalum A", "Capacitor_Tantalum_SMD:CP_EIA-3216-18_Kemet-A"),
            ("Tantalum B", "Capacitor_Tantalum_SMD:CP_EIA-3528-21_Kemet-B"),
        ],
        Kind::Inductor => &[
            ("0603", "Inductor_SMD:L_0603_1608Metric"),
            ("0805", "Inductor_SMD:L_0805_2012Metric"),
            ("1206", "Inductor_SMD:L_1206_3216Metric"),
            ("1210", "Inductor_SMD:L_1210_3225Metric"),
        ],
        Kind::Diode => &[
            ("SOD-123", "Diode_SMD:D_SOD-123"),
            ("SOD-323", "Diode_SMD:D_SOD-323"),
            ("SMA", "Diode_SMD:D_SMA"),
            ("DO-35", "Diode_THT:D_DO-35_SOD27_P7.62mm_Horizontal"),
        ],
        Kind::Led => &[
            ("0603", "LED_SMD:LED_0603_1608Metric"),
            ("0805", "LED_SMD:LED_0805_2012Metric"),
            ("3mm", "LED_THT:LED_D3.0mm"),
            ("5mm", "LED_THT:LED_D5.0mm"),
        ],
        Kind::Npn | Kind::Pnp | Kind::Nmos => &[
            ("SOT-23", "Package_TO_SOT_SMD:SOT-23"),
            ("TO-92", "Package_TO_SOT_THT:TO-92_Inline"),
            ("SOT-223", "Package_TO_SOT_SMD:SOT-223-3_TabPin2"),
        ],
        Kind::Regulator => &[
            ("SOT-223", "Package_TO_SOT_SMD:SOT-223-3_TabPin2"),
            ("SOT-23", "Package_TO_SOT_SMD:SOT-23"),
            ("TO-220", "Package_TO_SOT_THT:TO-220-3_Vertical"),
        ],
        Kind::OpAmp | Kind::Mcu8 => &[
            ("SOIC-8", "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm"),
            ("DIP-8", "Package_DIP:DIP-8_W7.62mm"),
            ("TSSOP-8", "Package_SO:TSSOP-8_4.4x3mm_P0.65mm"),
        ],
        Kind::Connector2 => &[
            ("Pin header 1x2", "Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm_Vertical"),
            ("JST PH", "Connector_JST:JST_PH_B2B-PH-K_1x02_P2.00mm_Vertical"),
            ("Screw 5mm", "TerminalBlock:TerminalBlock_bornier-2_P5.08mm"),
        ],
        _ => &[],
    }
}

impl App {
    pub fn open_fp_picker(&mut self, id: u32) {
        let fp = &mut self.fp_pick;
        if fp.target != Some(id) {
            fp.msg.clear();
            fp.manual.clear();
        }
        fp.target = Some(id);
    }

    fn fp_start(&mut self, f: impl FnOnce() -> FpMsg + Send + 'static) {
        if self.fp_pick.job.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
        self.fp_pick.job = Some(rx);
    }

    fn fp_apply(&mut self, id: u32, id_text: String) {
        self.fp_pick.msg = format!("Loading {}…", id_text);
        self.fp_start(move || FpMsg::Got(id, crate::online::fetch_footprint(&id_text)));
    }

    fn fp_poll(&mut self) {
        let Some(rx) = &self.fp_pick.job else { return };
        let m = match rx.try_recv() {
            Ok(m) => m,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.fp_pick.job = None;
                self.fp_pick.msg = "The download stopped unexpectedly.".into();
                return;
            }
        };
        self.fp_pick.job = None;
        match m {
            FpMsg::Libs(Ok(l)) => {
                self.fp_pick.libs = l;
                self.fp_pick.msg.clear();
            }
            FpMsg::Libs(Err(e)) => self.fp_pick.msg = e,
            FpMsg::Names(lib, Ok(n)) => {
                self.fp_pick.lib = lib;
                self.fp_pick.names = n;
                self.fp_pick.msg.clear();
            }
            FpMsg::Names(_, Err(e)) => self.fp_pick.msg = e,
            FpMsg::Got(id, Ok(over)) => {
                if let Some(ci) = self.project.find(id) {
                    self.checkpoint();
                    let name = over.name.clone();
                    self.project.components[ci].fp_over = Some(Box::new(over));
                    self.fp_pick.msg = format!("Footprint set to {}.", name);
                    self.fp_pick.target = None;
                }
            }
            FpMsg::Got(_, Err(e)) => self.fp_pick.msg = e,
        }
    }

    pub fn fp_window(&mut self, ctx: &egui::Context) {
        self.fp_poll();
        let Some(id) = self.fp_pick.target else { return };
        let Some(ci) = self.project.find(id) else {
            self.fp_pick.target = None;
            return;
        };
        let (kind, reference, current, has_over) = {
            let c = &self.project.components[ci];
            (c.kind, c.reference.clone(), c.info().footprint, c.fp_over.is_some())
        };
        let mut open = true;
        let mut apply: Option<String> = None;
        let mut reset = false;
        let mut want_libs = false;
        let mut want_names: Option<String> = None;
        egui::Window::new(format!("Footprint of {}", reference)).open(&mut open).default_width(420.0).show(ctx, |ui| {
            ui.label(format!("Now: {}", current));
            ui.label(egui::RichText::new("Pads are matched to the part's pins by number; check the result in the PCB view.").small().color(egui::Color32::GRAY));
            let sug = suggestions(kind);
            if !sug.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("COMMON CHOICES").small().strong().color(egui::Color32::GRAY));
                ui.horizontal_wrapped(|ui| {
                    for (label, full) in sug {
                        if ui.button(*label).on_hover_text(*full).clicked() {
                            apply = Some(full.to_string());
                        }
                    }
                });
            }
            ui.separator();
            ui.label(egui::RichText::new("BY NAME").small().strong().color(egui::Color32::GRAY));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.fp_pick.manual).hint_text("Library:Name  or  /path/file.kicad_mod").desired_width(290.0));
                if ui.add_enabled(!self.fp_pick.manual.trim().is_empty(), egui::Button::new("Use")).clicked() {
                    apply = Some(self.fp_pick.manual.clone());
                }
            });
            ui.separator();
            ui.label(egui::RichText::new("BROWSE THE KICAD LIBRARIES").small().strong().color(egui::Color32::GRAY));
            if self.fp_pick.libs.is_empty() {
                if ui.button("Load the library list").clicked() {
                    want_libs = true;
                }
            } else {
                ui.horizontal(|ui| {
                    ui.label("Library");
                    ui.add(egui::TextEdit::singleline(&mut self.fp_pick.lib_filter).hint_text("filter, e.g. SO").desired_width(120.0));
                });
                let f = self.fp_pick.lib_filter.to_lowercase();
                egui::ScrollArea::vertical().id_salt("fplibs").max_height(110.0).show(ui, |ui| {
                    for l in self.fp_pick.libs.iter().filter(|l| l.to_lowercase().contains(&f)).take(80) {
                        if ui.selectable_label(self.fp_pick.lib == *l, l).clicked() {
                            want_names = Some(l.clone());
                        }
                    }
                });
            }
            if !self.fp_pick.names.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(format!("{} footprints", self.fp_pick.lib));
                    ui.add(egui::TextEdit::singleline(&mut self.fp_pick.filter).hint_text("filter").desired_width(120.0));
                });
                let f = self.fp_pick.filter.to_lowercase();
                egui::ScrollArea::vertical().id_salt("fpnames").max_height(160.0).show(ui, |ui| {
                    for n in self.fp_pick.names.iter().filter(|n| n.to_lowercase().contains(&f)).take(150) {
                        if ui.selectable_label(false, n).clicked() {
                            apply = Some(format!("{}:{}", self.fp_pick.lib, n));
                        }
                    }
                });
            }
            if self.fp_pick.job.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Working…");
                });
            }
            if !self.fp_pick.msg.is_empty() {
                ui.label(egui::RichText::new(&self.fp_pick.msg).color(egui::Color32::from_rgb(150, 90, 0)));
            }
            if has_over {
                ui.separator();
                if ui.button("↩ Back to the original footprint").clicked() {
                    reset = true;
                }
            }
        });
        if !open {
            self.fp_pick.target = None;
        }
        if want_libs {
            self.fp_pick.msg = "Loading the library list…".into();
            self.fp_start(|| FpMsg::Libs(crate::online::fp_libraries()));
        }
        if let Some(l) = want_names {
            self.fp_pick.msg = format!("Loading {}…", l);
            self.fp_start(move || {
                let r = crate::online::fp_names(&l);
                FpMsg::Names(l, r)
            });
        }
        if let Some(t) = apply {
            self.fp_apply(id, t);
        }
        if reset {
            self.checkpoint();
            self.project.components[ci].fp_over = None;
            self.fp_pick.target = None;
        }
    }
}
