//! egui GUI for hexz_k — Pack and Extract tabs (Keka-like)

use crate::cmd::pack::{self, PackOptions, ProgressTracker};
use std::time::Instant;

fn load_icon() -> Option<egui::IconData> {
    let icon_bytes = include_bytes!("../../assets/hexz.png");
    let img = image::load_from_memory(icon_bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width: w,
        height: h,
    })
}

pub fn run_gui() -> anyhow::Result<()> {
    let mut vp = egui::ViewportBuilder::default()
        .with_inner_size([460.0, 280.0])
        .with_resizable(false);
    if let Some(icon) = load_icon() {
        vp = vp.with_icon(icon);
    }
    eframe::run_native(
        "hexz_k",
        eframe::NativeOptions { viewport: vp, ..Default::default() },
        Box::new(|_cc| Ok(Box::new(App::default()))),
    ).map_err(|e| anyhow::anyhow!("GUI error: {e}"))
}

struct App {
    tab: usize, // 0=Pack, 1=Extract
    // Pack
    input_dir: String, output_file: String, compression: usize,
    encrypt: bool, password: String,
    // Extract
    archive_in: String, extract_out: String, ext_password: String,
    ext_encrypted: bool,
    // Shared
    status: String, busy: bool,
    progress: (u64, u64), start_time: Option<Instant>,
    worker: Option<std::thread::JoinHandle<anyhow::Result<()>>>,
    tracker: Option<ProgressTracker>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            tab: 0, compression: 1, progress: (0, 0),
            input_dir: String::new(), output_file: String::new(),
            encrypt: false, password: String::new(),
            archive_in: String::new(), extract_out: String::new(),
            ext_password: String::new(), ext_encrypted: false,
            status: String::new(), busy: false,
            start_time: None, worker: None, tracker: None,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll worker
        if let Some(ref t) = self.tracker {
            let (done, total, finished) = t.get();
            self.progress = (done, total);
            if finished {
                self.status = if self.worker.take().unwrap().join().is_ok() {
                    "Done.".into()
                } else { "Error.".into() };
                self.busy = false; self.tracker = None;
            }
        }

        // Gray visuals
        let mut v = egui::Visuals::dark();
        v.widgets.active.bg_fill = egui::Color32::from_gray(90);
        v.widgets.hovered.bg_fill = egui::Color32::from_gray(110);
        v.widgets.inactive.bg_fill = egui::Color32::from_gray(60);
        v.selection.bg_fill = egui::Color32::from_gray(100);
        v.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(140));
        ctx.set_visuals(v);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
            ui.heading("hexz_k");
            ui.add_space(2.0);

            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, 0, "Pack");
                ui.selectable_value(&mut self.tab, 1, "Extract");
            });
            ui.separator();

            let disabled = self.busy;
            ui.add_enabled_ui(!disabled, |ui| {
                if self.tab == 0 { self.pack_page(ui); }
                else { self.extract_page(ui); }
            });

            // Space-filler pushes status bar to bottom
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.separator();
                if self.busy {
                    if self.progress.1 > 0 {
                        let frac = self.progress.0 as f32 / self.progress.1 as f32;
                        ui.add(egui::ProgressBar::new(frac).desired_width(ui.available_width()).text(format!("{:.0}%", frac * 100.0)));
                        if let Some(s) = self.start_time {
                            let el = s.elapsed().as_secs_f64();
                            if self.progress.0 > 0 && el > 0.3 {
                                let eta = el / self.progress.0 as f64 * self.progress.1 as f64 - el;
                                ui.label(format!("{:.1}/{:.1} MB  ETA {:.0}s",
                                    self.progress.0 as f64/1_048_576.0, self.progress.1 as f64/1_048_576.0, eta));
                            }
                        }
                    } else {
                        ui.label(egui::RichText::new("Working...").small());
                    }
                } else if !self.status.is_empty() {
                    ui.label(egui::RichText::new(&self.status).small());
                }
            });

            if self.busy {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        });
    }
}

impl App {
    fn pack_page(&mut self, ui: &mut egui::Ui) {
        // Input
        ui.horizontal(|ui| {
            ui.label("Input:");
            let w = ui.available_width() - 30.0;
            ui.add_sized([w.max(60.0), 18.0], egui::TextEdit::singleline(&mut self.input_dir));
            if ui.button("...").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() { self.input_dir = p.display().to_string(); }
            }
        });
        // Output
        ui.horizontal(|ui| {
            ui.label("Output:");
            let w = ui.available_width() - 30.0;
            ui.add_sized([w.max(60.0), 18.0], egui::TextEdit::singleline(&mut self.output_file));
            if ui.button("...").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Hexz", &["hxz"]).save_file() { self.output_file = p.display().to_string(); }
            }
        });
        // Options
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.compression, 1, "Zstd");
            ui.selectable_value(&mut self.compression, 0, "LZ4");
            ui.separator();
            ui.checkbox(&mut self.encrypt, "AES-256");
            if self.encrypt {
                ui.label("Key:");
                ui.add_sized([80.0, 18.0], egui::TextEdit::singleline(&mut self.password).password(true));
            }
        });
        ui.add_space(4.0);
        // Button
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ok = !self.input_dir.is_empty() && !self.output_file.is_empty()
                && (!self.encrypt || !self.password.is_empty());
            if ui.add_enabled(ok, egui::Button::new("Pack Archive").min_size([130.0, 28.0].into())).clicked() {
                self.busy = true; self.status.clear(); self.progress = (0, 0); self.start_time = Some(Instant::now());
                let opts = PackOptions {
                    input: self.input_dir.clone(), output: self.output_file.clone(),
                    compression: if self.compression == 0 { "lz4".into() } else { "zstd".into() },
                    encrypt: self.encrypt, block_size: 65536,
                    password: if self.encrypt { Some(self.password.clone()) } else { None },
                };
                let t = ProgressTracker::new(); let t2 = t.clone();
                self.tracker = Some(t);
                self.worker = Some(std::thread::spawn(move || pack::pack_directory_with_progress(&opts, t2)));
            }
        });
    }

    fn extract_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Archive:");
            let w = ui.available_width() - 30.0;
            ui.add_sized([w.max(60.0), 18.0], egui::TextEdit::singleline(&mut self.archive_in));
            if ui.button("...").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Hexz", &["hxz"]).pick_file() { self.archive_in = p.display().to_string(); }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Output:");
            let w = ui.available_width() - 30.0;
            ui.add_sized([w.max(60.0), 18.0], egui::TextEdit::singleline(&mut self.extract_out));
            if ui.button("...").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() { self.extract_out = p.display().to_string(); }
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.ext_encrypted, "AES-256");
            if self.ext_encrypted {
                ui.label("Key:");
                ui.add_sized([80.0, 18.0], egui::TextEdit::singleline(&mut self.ext_password).password(true));
            }
        });
        ui.add_space(4.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ok = !self.archive_in.is_empty() && !self.extract_out.is_empty()
                && (!self.ext_encrypted || !self.ext_password.is_empty());
            if ui.add_enabled(ok, egui::Button::new("Extract Archive").min_size([130.0, 28.0].into())).clicked() {
                self.busy = true; self.status = "Extracting...".into();
                let arc = self.archive_in.clone(); let out = self.extract_out.clone();
                let pw = if self.ext_encrypted { Some(self.ext_password.clone()) } else { None };
                let t = ProgressTracker::new(); let t2 = t.clone();
                self.tracker = Some(t);
                self.progress = (0, 0); self.start_time = Some(Instant::now());
                self.worker = Some(std::thread::spawn(move || {
                    let result = crate::cmd::read::extract_all(&arc, &out, pw.as_deref());
                    t2.inner.lock().unwrap().2 = true; // signal finish
                    result
                }));
            }
        });
    }
}
