//! egui GUI for hexz_k — Pack + Browse tabs
#![allow(deprecated)]
#![allow(clippy::collapsible_if)]

use crate::cmd::pack::{self, PackOptions, ProgressTracker};
use hexz_k::{FileCategory, PackMetadata, ResourcePack, TreeNode, bench, format_size};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ── helpers ──────────────────────────────────────────────────────────────

fn load_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../../assets/hexz.png");
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width: w,
        height: h,
    })
}

fn try_cjk_font() -> Option<Vec<u8>> {
    for path in [
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "C:/Windows/Fonts/msyh.ttc",
    ] {
        if let Ok(data) = std::fs::read(path)
            && !data.is_empty()
        {
            return Some(data);
        }
    }
    None
}

fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let prefix: String = s.chars().take(max - 1).collect();
        format!("{prefix}…")
    }
}

fn cat_icon(c: Option<&FileCategory>) -> &str {
    match c {
        Some(FileCategory::Image) => "🖼",
        Some(FileCategory::Audio) => "🎵",
        Some(FileCategory::Video) => "🎬",
        Some(FileCategory::Script) => "📜",
        Some(FileCategory::Data) => "📊",
        Some(FileCategory::Text) => "📄",
        Some(FileCategory::Font) => "🔤",
        Some(FileCategory::Archive) => "📦",
        _ => "📎",
    }
}

fn find_directory<'a>(root: &'a TreeNode, path: &str) -> Option<&'a TreeNode> {
    path.split('/')
        .filter(|part| !part.is_empty())
        .try_fold(root, |directory, part| {
            directory
                .children
                .iter()
                .find(|child| child.is_dir && child.name == part)
        })
}

fn safe_output_path(root: &std::path::Path, archive_path: &str) -> Option<std::path::PathBuf> {
    use std::path::Component;

    let path = std::path::Path::new(archive_path);
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Some(root.join(path))
    } else {
        None
    }
}

// ── entry point ───────────────────────────────────────────────────────────

/// Launch the egui GUI window.
pub fn run_gui() -> anyhow::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([720.0, 500.0])
        .with_resizable(true)
        .with_min_inner_size([520.0, 360.0]);
    let viewport = match load_icon() {
        Some(icon) => viewport.with_icon(icon),
        None => viewport,
    };

    eframe::run_native(
        "hexz_k",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| {
            let gray = |level| egui::Color32::from_gray(level);
            let mut visuals = egui::Visuals::dark();
            visuals.widgets.active.bg_fill = gray(90);
            visuals.widgets.hovered.bg_fill = gray(110);
            visuals.widgets.inactive.bg_fill = gray(60);
            visuals.selection.bg_fill = gray(100);
            visuals.selection.stroke = egui::Stroke::new(1.0, gray(140));
            cc.egui_ctx.set_visuals(visuals);
            if let Some(font_data) = try_cjk_font() {
                let mut fonts = egui::FontDefinitions::default();
                fonts
                    .font_data
                    .insert("cjk".into(), egui::FontData::from_owned(font_data).into());
                for family in fonts.families.values_mut() {
                    family.push("cjk".into());
                }
                cc.egui_ctx.set_fonts(fonts);
            }
            Ok(Box::new(App::default()))
        }),
    )
    .map_err(|e| anyhow::anyhow!("GUI error: {e}"))
}

// ── state ────────────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum Tab {
    Pack,
    Browse,
    Bench,
}

#[derive(PartialEq, Clone, Copy)]
enum BrowseMode {
    List,
    Grid,
}

#[derive(Default)]
struct Archive {
    path: String,
    password: String,
    encrypted: bool,
    pack: Option<ResourcePack>,
    meta: Option<Arc<PackMetadata>>,
    category_chips: Vec<(FileCategory, usize, u64)>,
    error: Option<String>,
    file_size: u64,
    open_dirs: HashSet<String>,
    selected: HashSet<String>,
    ctx_menu: Option<String>,
}

struct LoadedArchive {
    pack: ResourcePack,
    meta: Arc<PackMetadata>,
    category_chips: Vec<(FileCategory, usize, u64)>,
    file_size: u64,
}

struct PackForm {
    input: String,
    output: String,
    compression: usize, // 0 = LZ4, 1 = Zstd
    encrypt: bool,
    password: String,
}

impl Default for PackForm {
    fn default() -> Self {
        Self {
            input: String::new(),
            output: String::new(),
            compression: 1, // default to Zstd
            encrypt: false,
            password: String::new(),
        }
    }
}

#[derive(Clone, Default)]
struct BenchEntry {
    label: String,
    pack_ms: u128,
    ratio: f64,
    seq_mbps: f64,
    iops: f64,
}

#[derive(Clone, Default)]
struct BenchProgress {
    step: usize,
    total: usize,
    msg: String,
}

struct App {
    tab: Tab,
    browse_mode: BrowseMode,
    grid_skip_frames: u8,
    grid_path: String,
    grid_drag_start: Option<egui::Pos2>,
    grid_drag_now: Option<egui::Pos2>,
    bench_results: Arc<Mutex<Vec<BenchEntry>>>,
    bench_running: bool,
    bench_progress: Arc<Mutex<BenchProgress>>,
    arc: Archive,
    form: PackForm,
    status: String,
    busy: bool,
    progress: (u64, u64),
    start: Option<Instant>,
    worker: Option<std::thread::JoinHandle<anyhow::Result<String>>>,
    open_worker: Option<std::thread::JoinHandle<anyhow::Result<LoadedArchive>>>,
    tracker: Option<ProgressTracker>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            tab: Tab::Pack,
            browse_mode: BrowseMode::List,
            grid_skip_frames: 0,
            grid_path: String::new(),
            grid_drag_start: None,
            grid_drag_now: None,
            bench_results: Arc::new(Mutex::new(Vec::new())),
            bench_running: false,
            bench_progress: Arc::new(Mutex::new(BenchProgress::default())),
            arc: Archive::default(),
            form: PackForm::default(),
            status: "Ready.".into(),
            busy: false,
            progress: (0, 0),
            start: None,
            worker: None,
            open_worker: None,
            tracker: None,
        }
    }
}

// ── main loop ────────────────────────────────────────────────────────────

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_open_worker();
        self.poll_worker();

        let mut load_trigger = false;
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Enter) && !i.modifiers.any() {
                load_trigger = true;
            }
        });

        // header
        egui::TopBottomPanel::top("head")
            .min_height(28.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("hexz_k");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.selectable_value(&mut self.tab, Tab::Bench, " Bench ");
                        ui.selectable_value(&mut self.tab, Tab::Browse, " Browse ");
                        ui.selectable_value(&mut self.tab, Tab::Pack, "  Pack  ");
                    });
                });
            });

        // main
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_enabled_ui(!self.busy, |ui| match self.tab {
                Tab::Pack => self.pack_page(ui),
                Tab::Browse => self.browse_page(ui, load_trigger),
                Tab::Bench => self.bench_page(ui),
            });
        });

        // status bar
        egui::TopBottomPanel::bottom("status")
            .min_height(22.0)
            .show(ctx, |ui| {
                if self.busy {
                    self.render_busy_status(ui);
                } else {
                    let mut line = String::new();
                    if let Some(ref meta) = self.arc.meta {
                        let ratio = if self.arc.file_size > 0 {
                            meta.total_size as f64 / self.arc.file_size as f64
                        } else {
                            1.0
                        };
                        line.push_str(&format!(
                            "{} files | {} disk → {} | {:.1}x | ",
                            meta.total_files,
                            format_size(self.arc.file_size),
                            format_size(meta.total_size),
                            ratio,
                        ));
                    }
                    line.push_str(&self.status);
                    ui.label(line);
                }
            });

        if self.busy || self.bench_running {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

impl App {
    fn poll_open_worker(&mut self) {
        let Some(worker) = self.open_worker.as_ref() else {
            return;
        };
        if !worker.is_finished() {
            return;
        }

        let result = self.open_worker.take().unwrap().join();
        match result {
            Ok(Ok(loaded)) => {
                self.arc.file_size = loaded.file_size;
                self.arc.category_chips = loaded.category_chips;
                self.arc.meta = Some(loaded.meta);
                self.arc.pack = Some(loaded.pack);
                self.status = "Loaded.".into();
            }
            Ok(Err(error)) => {
                let message = error.to_string();
                if message.contains("encrypt")
                    || message.contains("password")
                    || message.contains("auth")
                {
                    self.arc.encrypted = true;
                    self.arc.error = Some("Wrong password or encrypted — check Key.".into());
                } else {
                    self.arc.error = Some(format!("Failed: {message}"));
                }
                self.status = "Open failed.".into();
            }
            Err(_) => {
                self.arc.error = Some("Archive loader panicked.".into());
                self.status = "Open failed.".into();
            }
        }
        self.busy = false;
    }

    fn poll_worker(&mut self) {
        let Some(ref tracker) = self.tracker else {
            return;
        };
        let (done, total, finished) = tracker.get();
        self.progress = (done, total);
        let worker_finished = self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished());
        if !finished && !worker_finished {
            return;
        }
        self.status = match self.worker.take().unwrap().join() {
            Ok(Ok(message)) => message,
            Ok(Err(error)) => format!("Error: {error:#}"),
            Err(_) => "Error: background worker panicked".into(),
        };
        self.busy = false;
        self.tracker = None;
    }

    fn render_busy_status(&self, ui: &mut egui::Ui) {
        if self.progress.1 > 0 {
            let frac = self.progress.0 as f32 / self.progress.1 as f32;
            ui.add(
                egui::ProgressBar::new(frac)
                    .desired_width(ui.available_width())
                    .text(format!("{:.0}%", frac * 100.0)),
            );
            if let Some(start) = self.start {
                let elapsed = start.elapsed().as_secs_f64();
                if self.progress.0 > 0 && elapsed > 0.3 {
                    let eta = elapsed / self.progress.0 as f64 * self.progress.1 as f64 - elapsed;
                    ui.label(format!(
                        "{:.1}/{:.1} MB  ETA {:.0}s",
                        self.progress.0 as f64 / 1_048_576.0,
                        self.progress.1 as f64 / 1_048_576.0,
                        eta,
                    ));
                }
            }
        } else {
            ui.label("Working…");
        }
    }

    // ── Pack tab ─────────────────────────────────────────────────────────

    fn pack_page(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.heading("Pack Archive");

        let row_h = 22.0;

        // Input
        ui.horizontal(|ui| {
            ui.label("Input:");
            let w = ui.available_width() - 34.0;
            ui.add_sized(
                [w.max(60.0), row_h],
                egui::TextEdit::singleline(&mut self.form.input).hint_text("source directory…"),
            );
            if ui.button("…").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.form.input = path.display().to_string();
                }
            }
        });

        // Output
        ui.horizontal(|ui| {
            ui.label("Output:");
            let w = ui.available_width() - 34.0;
            ui.add_sized(
                [w.max(60.0), row_h],
                egui::TextEdit::singleline(&mut self.form.output).hint_text("output.hxz…"),
            );
            if ui.button("…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Hexz", &["hxz"])
                    .save_file()
                {
                    self.form.output = path.display().to_string();
                }
            }
        });

        // Options
        ui.horizontal(|ui| {
            ui.label("Options:");
            ui.selectable_value(&mut self.form.compression, 1, "Zstd");
            ui.selectable_value(&mut self.form.compression, 0, "LZ4");
            ui.separator();
            ui.checkbox(&mut self.form.encrypt, "AES-256");
            if self.form.encrypt {
                ui.add_sized(
                    [120.0, row_h],
                    egui::TextEdit::singleline(&mut self.form.password)
                        .password(true)
                        .hint_text("password"),
                );
            }
        });

        ui.add_space(24.0);

        // Pack button
        ui.horizontal_centered(|ui| {
            let ok = !self.form.input.is_empty()
                && !self.form.output.is_empty()
                && (!self.form.encrypt || !self.form.password.is_empty());
            let button = ui.add_enabled(
                ok,
                egui::Button::new("Pack Archive").min_size([160.0, 30.0].into()),
            );
            if button.clicked() {
                self.do_pack();
            }
        });
    }

    fn do_pack(&mut self) {
        self.busy = true;
        self.status.clear();
        self.progress = (0, 0);
        self.start = Some(Instant::now());

        let compression = if self.form.compression == 0 {
            "lz4"
        } else {
            "zstd"
        };
        let password = self.form.encrypt.then(|| self.form.password.clone());
        let opts = PackOptions {
            input: self.form.input.clone(),
            output: self.form.output.clone(),
            compression: compression.into(),
            encrypt: self.form.encrypt,
            block_size: 65536,
            password,
        };

        let tracker = ProgressTracker::new();
        let clone = tracker.clone();
        self.tracker = Some(tracker);
        self.worker = Some(std::thread::spawn(move || {
            pack::pack_directory_with_progress(&opts, clone)?;
            Ok("Packed archive.".into())
        }));
    }

    // ── Bench tab ────────────────────────────────────────────────────────

    fn bench_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Benchmark");

        let can_run = !self.bench_running;
        if ui
            .add_enabled(can_run, egui::Button::new("▶ Run Full Benchmark"))
            .clicked()
        {
            self.bench_running = true;
            self.bench_results.lock().unwrap().clear();
            *self.bench_progress.lock().unwrap() = BenchProgress {
                step: 0,
                total: bench::GAME_BENCH_GROUPS.len() + bench::BENCH_CONFIGS.len() + 1,
                msg: "Generating test data…".into(),
            };
            let results = self.bench_results.clone();
            let progress = self.bench_progress.clone();
            std::thread::spawn(move || {
                let tmp = std::env::temp_dir().join("hexz_gui_bench");
                let _ = std::fs::remove_dir_all(&tmp);
                let _ = std::fs::create_dir_all(&tmp);
                let input_dir = tmp.join("input");
                let output_dir = tmp.join("output");
                let _ = std::fs::create_dir_all(&input_dir);
                let _ = std::fs::create_dir_all(&output_dir);

                // Step 0: generate
                {
                    let mut p = progress.lock().unwrap();
                    p.step = 0;
                    p.total = bench::GAME_BENCH_GROUPS.len() + bench::BENCH_CONFIGS.len() + 1;
                    p.msg = "Generating test data…".into();
                }
                let total = match bench::generate_game_test_files(&input_dir) {
                    Ok(t) => t,
                    Err(e) => {
                        let mut p = progress.lock().unwrap();
                        p.msg = format!("Data generation failed: {e}");
                        p.step = p.total;
                        return;
                    }
                };

                for (i, (comp, bs)) in bench::BENCH_CONFIGS.iter().enumerate() {
                    let step = bench::GAME_BENCH_GROUPS.len() + i + 1;
                    let total_steps =
                        bench::GAME_BENCH_GROUPS.len() + bench::BENCH_CONFIGS.len() + 1;
                    {
                        let mut p = progress.lock().unwrap();
                        p.step = step;
                        p.total = total_steps;
                        p.msg = format!("Packing {comp} {}KiB…", bs / 1024);
                    }

                    let label = format!("{comp} {}KiB", bs / 1024);
                    let archive = output_dir.join(format!("bench_{comp}_{bs}.hxz"));

                    const ROUNDS: usize = 3;
                    let mut sum_pack = 0u128;
                    let mut sum_seq = 0f64;
                    let mut sum_iops = 0f64;

                    for _ in 0..ROUNDS {
                        let t0 = Instant::now();
                        let opts = crate::cmd::pack::PackOptions {
                            input: input_dir.to_string_lossy().to_string(),
                            output: archive.to_string_lossy().to_string(),
                            compression: comp.to_string(),
                            encrypt: false,
                            block_size: *bs,
                            password: None,
                        };
                        if crate::cmd::pack::pack_directory(&opts).is_err() {
                            break; // skip remaining rounds on pack failure
                        }
                        sum_pack += t0.elapsed().as_millis();

                        // Use shared measurement (same as CLI bench)
                        if let Ok((seq, iops)) = bench::measure_reads(&archive) {
                            sum_seq += seq;
                            sum_iops += iops;
                        }
                    }

                    let archive_sz = std::fs::metadata(&archive).map(|m| m.len()).unwrap_or(1);
                    let entry = BenchEntry {
                        label: label.clone(),
                        pack_ms: if ROUNDS > 0 {
                            sum_pack / ROUNDS as u128
                        } else {
                            0
                        },
                        ratio: total as f64 / archive_sz as f64,
                        seq_mbps: sum_seq / ROUNDS as f64,
                        iops: sum_iops / ROUNDS as f64,
                    };

                    {
                        let mut out = results.lock().unwrap();
                        out.push(entry);
                    }
                }

                let _ = std::fs::remove_dir_all(&tmp);
                let mut p = progress.lock().unwrap();
                p.step = p.total;
                p.msg = "Done.".into();
            });
        }

        // Progress
        if self.bench_running {
            // Use try_lock to never block the UI
            if let Ok(p) = self.bench_progress.try_lock() {
                if p.total > 0 {
                    let frac = p.step as f32 / p.total as f32;
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .text(format!("{}/{}  {}", p.step, p.total, p.msg)),
                    );
                } else {
                    ui.label(&p.msg);
                }
                if p.step >= p.total && p.total > 0 {
                    self.bench_running = false;
                }
            }
        }

        // Results + chart — also use try_lock
        if let Ok(entries) = self.bench_results.try_lock() {
            if !entries.is_empty() {
                ui.separator();
                self.draw_bench_chart(ui, &entries);
            }
        }
    }

    fn draw_bench_chart(&self, ui: &mut egui::Ui, entries: &[BenchEntry]) {
        let max_read = entries
            .iter()
            .map(|e| e.seq_mbps)
            .fold(0.0, f64::max)
            .max(1.0);
        let max_iops = entries.iter().map(|e| e.iops).fold(0.0, f64::max).max(1.0);
        let bar_w = 36.0;
        let max_h = 100.0;
        let gap = 14.0;

        // Two bar groups side by side: Read MB/s | IOPS
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 24.0;
            // ── Read MB/s ──
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Sequential Read  MB/s").strong());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for e in entries {
                        let h = (e.seq_mbps / max_read * max_h as f64).max(2.0) as f32;
                        let info = format!("{}ms {:.0}x", e.pack_ms, e.ratio);
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(format!("{:.0}", e.seq_mbps)).size(13.0));
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(bar_w, max_h),
                                egui::Sense::hover(),
                            );
                            let bottom = rect.max.y;
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(rect.min.x, bottom - h),
                                    egui::vec2(bar_w, h),
                                ),
                                2.0,
                                egui::Color32::from_gray(140),
                            );
                            ui.label(egui::RichText::new(ellipsize(&e.label, 10)).size(12.0));
                            ui.label(
                                egui::RichText::new(&info)
                                    .size(11.0)
                                    .color(egui::Color32::from_gray(160)),
                            );
                        });
                    }
                });
            });

            // ── IOPS ──
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Random IOPS").strong());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for e in entries {
                        let h = (e.iops / max_iops * max_h as f64).max(2.0) as f32;
                        let info = format!("{}ms {:.0}x", e.pack_ms, e.ratio);
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(format!("{:.0}", e.iops)).size(13.0));
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(bar_w, max_h),
                                egui::Sense::hover(),
                            );
                            let bottom = rect.max.y;
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(rect.min.x, bottom - h),
                                    egui::vec2(bar_w, h),
                                ),
                                2.0,
                                egui::Color32::from_gray(100),
                            );
                            ui.label(egui::RichText::new(ellipsize(&e.label, 10)).size(12.0));
                            ui.label(
                                egui::RichText::new(&info)
                                    .size(11.0)
                                    .color(egui::Color32::from_gray(160)),
                            );
                        });
                    }
                });
            });
        });
    }

    // ── Browse tab ───────────────────────────────────────────────────────

    fn browse_page(&mut self, ui: &mut egui::Ui, load_trigger: bool) {
        ui.spacing_mut().item_spacing.y = 4.0;
        let row_h = 22.0;

        // address bar
        ui.horizontal(|ui| {
            ui.label("Path:");
            let response = ui.add_sized(
                [ui.available_width() - 34.0, row_h],
                egui::TextEdit::singleline(&mut self.arc.path).hint_text("archive.hxz…"),
            );
            if ui.button("…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Hexz", &["hxz"])
                    .pick_file()
                {
                    self.arc.path = path.display().to_string();
                }
            }
            let enter_pressed =
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if load_trigger || enter_pressed {
                self.open_archive();
            }
        });

        // encryption
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.arc.encrypted, "AES-256");
            if self.arc.encrypted {
                ui.add_sized(
                    [120.0, row_h],
                    egui::TextEdit::singleline(&mut self.arc.password)
                        .password(true)
                        .hint_text("password"),
                );
            }
        });

        // error
        if let Some(ref err) = self.arc.error {
            ui.colored_label(egui::Color32::from_rgb(255, 100, 100), err);
        }

        // loaded → show file list
        if self.arc.pack.is_some() {
            self.render_browse_content(ui);
        } else {
            // not loaded → Load button at bottom
            ui.add_space(24.0);
            ui.horizontal_centered(|ui| {
                let can_load = !self.arc.path.is_empty();
                if ui
                    .add_enabled(
                        can_load,
                        egui::Button::new("Load Archive").min_size([160.0, 30.0].into()),
                    )
                    .clicked()
                {
                    self.open_archive();
                }
            });
        }
    }

    fn open_archive(&mut self) {
        let path = self.arc.path.clone();
        let password = if self.arc.encrypted && !self.arc.password.is_empty() {
            Some(self.arc.password.clone())
        } else {
            None
        };
        self.arc.error = None;
        self.arc.meta = None;
        self.arc.category_chips.clear();
        self.arc.pack = None;
        self.arc.open_dirs.clear();
        self.arc.selected.clear();
        self.arc.ctx_menu = None;
        self.arc.file_size = 0;
        self.grid_path.clear();
        self.grid_drag_start = None;
        self.grid_drag_now = None;
        self.busy = true;
        self.status = "Loading archive…".into();
        self.open_worker = Some(std::thread::spawn(move || {
            let file_size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            let pack = ResourcePack::open(&path, password.as_deref())?;
            let meta = Arc::new(pack.build_metadata());
            let mut category_chips: Vec<_> = meta
                .category_counts
                .iter()
                .map(|(category, (count, size))| (*category, *count, *size))
                .collect();
            category_chips.sort_unstable_by_key(|(category, _, _)| *category);
            Ok(LoadedArchive {
                pack,
                meta,
                category_chips,
                file_size,
            })
        }));
    }

    fn render_browse_content(&mut self, ui: &mut egui::Ui) {
        ui.separator();

        // category chips
        let cats = &self.arc.category_chips;
        if !cats.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (cat, count, size) in cats {
                    let total = self.arc.meta.as_ref().map(|m| m.total_size).unwrap_or(1);
                    let pct = if total > 0 {
                        *size as f64 / total as f64 * 100.0
                    } else {
                        0.0
                    };
                    ui.label(format!("{cat} {count}({pct:.0}%)"));
                    ui.label("·");
                }
            });
        }

        // toolbar: actions + view toggle
        let n = self.arc.selected.len();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    n > 0,
                    egui::Button::new(format!("Extract Selected ({n})"))
                        .min_size([160.0, 26.0].into()),
                )
                .clicked()
            {
                self.extract_selected();
            }
            if n > 0 && ui.button("Clear").clicked() {
                self.arc.selected.clear();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let toggled = ui
                    .selectable_value(&mut self.browse_mode, BrowseMode::Grid, " ▦ Grid ")
                    .clicked()
                    || ui
                        .selectable_value(&mut self.browse_mode, BrowseMode::List, " ☰ List ")
                        .clicked();
                if toggled {
                    self.grid_skip_frames = 2;
                }
                ui.separator();
                if ui.button("Extract All").clicked() {
                    self.extract_all();
                }
            });
        });

        ui.separator();

        let meta = Arc::clone(self.arc.meta.as_ref().unwrap());
        let tree = &meta.file_tree;
        match self.browse_mode {
            BrowseMode::List => {
                egui::ScrollArea::vertical()
                    .id_salt("tree")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for child in &tree.children {
                            self.render_node(ui, child, 0, "");
                        }
                    });
            }
            BrowseMode::Grid => {
                egui::ScrollArea::vertical()
                    .id_salt("grid")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.render_grid(ui, tree);
                    });
            }
        }
        self.ctx_menu(ui);
    }

    // ── list tree rendering ──────────────────────────────────────────────

    fn render_node(&mut self, ui: &mut egui::Ui, node: &TreeNode, depth: usize, parent: &str) {
        let indent = (depth * 12) as f32;
        let full_path = if parent.is_empty() {
            node.name.clone()
        } else {
            format!("{parent}/{}", node.name)
        };
        let key = format!("{depth}:{}", node.name);

        if node.is_dir {
            self.render_dir_node(ui, node, indent, &full_path, &key, depth);
        } else {
            self.render_file_node(ui, node, indent, &full_path);
        }
    }

    fn render_dir_node(
        &mut self,
        ui: &mut egui::Ui,
        node: &TreeNode,
        indent: f32,
        full_path: &str,
        key: &str,
        depth: usize,
    ) {
        let is_open = self.arc.open_dirs.contains(key);
        let arrow = if is_open { "▾" } else { "▸" };
        let name = ellipsize(&node.name, 60);
        let selected = self.arc.selected.contains(full_path);

        let (checkbox_changed, folder_clicked) = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add_space(indent);
                let mut s = selected;
                let cb = ui.checkbox(&mut s, "");
                ui.label(arrow);
                let clicked = ui.selectable_label(false, format!("📁 {name}/")).clicked();
                (cb.changed(), clicked)
            })
            .inner;

        if checkbox_changed {
            if selected {
                self.arc.selected.remove(full_path);
            } else {
                self.arc.selected.insert(full_path.to_string());
            }
        }
        if folder_clicked {
            if is_open {
                self.arc.open_dirs.remove(key);
            } else {
                self.arc.open_dirs.insert(key.to_string());
            }
        }
        if is_open {
            for child in &node.children {
                self.render_node(ui, child, depth + 1, full_path);
            }
        }
    }

    fn render_file_node(
        &mut self,
        ui: &mut egui::Ui,
        node: &TreeNode,
        indent: f32,
        full_path: &str,
    ) {
        let name = ellipsize(&node.name, 48);
        let size_str = format_size(node.size.unwrap_or(0));
        let category = node.category.as_ref();
        let category_str = category.map(|c| c.to_string()).unwrap_or_default();
        let selected = self.arc.selected.contains(full_path);

        let (checkbox_changed, double_clicked, right_clicked) = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add_space(indent);

                let mut s = selected;
                let cb = ui.checkbox(&mut s, "");

                let label = format!("{} {}", cat_icon(category), name);
                let rich_text = if selected {
                    egui::RichText::new(label).background_color(egui::Color32::from_gray(80))
                } else {
                    egui::RichText::new(label)
                };
                let name_response = ui.label(rich_text);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(category_str);
                    ui.add_space(12.0);
                    ui.label(size_str);
                });

                (
                    cb.changed(),
                    name_response.double_clicked(),
                    name_response.secondary_clicked(),
                )
            })
            .inner;

        if checkbox_changed {
            if selected {
                self.arc.selected.remove(full_path);
            } else {
                self.arc.selected.insert(full_path.to_string());
            }
        }
        if double_clicked {
            self.extract_one(full_path);
        }
        if right_clicked {
            self.arc.ctx_menu = Some(full_path.to_string());
        }
    }

    // ── grid view ───────────────────────────────────────────────────────

    fn render_grid(&mut self, ui: &mut egui::Ui, root: &TreeNode) {
        let directory = find_directory(root, &self.grid_path).unwrap_or(root);
        let items: Vec<_> = directory
            .children
            .iter()
            .map(|node| {
                let full_path = if self.grid_path.is_empty() {
                    node.name.clone()
                } else {
                    format!("{}/{}", self.grid_path, node.name)
                };
                (full_path, node)
            })
            .collect();

        // Breadcrumb
        ui.horizontal(|ui| {
            if !self.grid_path.is_empty() && ui.button("⬆ ..").clicked() {
                self.grid_path = self
                    .grid_path
                    .rsplit_once('/')
                    .map_or_else(String::new, |(parent, _)| parent.to_owned());
            }
            let label = if self.grid_path.is_empty() {
                "/ (root)"
            } else {
                &self.grid_path
            };
            ui.label(label);
        });
        ui.separator();

        let mut card_rects: Vec<(egui::Rect, String)> = Vec::new();

        let spacing = 8.0;
        let min_card_w = 90.0;

        if self.grid_skip_frames > 0 {
            self.grid_skip_frames -= 1;
        }

        ui.spacing_mut().item_spacing = egui::vec2(spacing, spacing);

        let pointer = ui.input(|i| i.pointer.hover_pos());
        let skip_input = self.grid_skip_frames > 0;
        let primary_pressed = ui.input(|i| i.pointer.primary_pressed()) && !skip_input;
        let primary_down = ui.input(|i| i.pointer.primary_down()) && !skip_input;
        let primary_released = ui.input(|i| i.pointer.primary_released()) && !skip_input;
        let modifiers = ui.input(|i| i.modifiers);

        // Render cards with manual row layout for perfect fill
        let _response = ui.vertical(|ui| {
            let available_w = ui.available_width();
            let cols = ((available_w + spacing) / (min_card_w + spacing)).max(2.0) as usize;
            let card_w = (available_w - spacing * (cols as f32 - 1.0)) / cols as f32;
            let card_h = card_w * 0.92;
            let size = egui::vec2(card_w, card_h);

            let mut chunk: Vec<&(String, &TreeNode)> = Vec::with_capacity(cols);
            for item in &items {
                chunk.push(item);
                if chunk.len() >= cols {
                    self.render_grid_row(ui, &chunk, size, &mut card_rects, modifiers);
                    chunk.clear();
                }
            }
            if !chunk.is_empty() {
                self.render_grid_row(ui, &chunk, size, &mut card_rects, modifiers);
            }
        });

        // ── drag-to-select ──
        let on_card = pointer.is_some_and(|p| card_rects.iter().any(|(r, _)| r.contains(p)));
        if primary_pressed && !on_card && !modifiers.ctrl && !modifiers.command {
            self.grid_drag_start = pointer;
            self.grid_drag_now = pointer;
        }
        if primary_down && self.grid_drag_start.is_some() {
            self.grid_drag_now = pointer;
        }
        if primary_released {
            if let (Some(p0), Some(p1)) = (self.grid_drag_start, self.grid_drag_now) {
                let dist = (p1 - p0).length();
                if dist > 4.0 {
                    let sel_rect = egui::Rect::from_two_pos(p0, p1);
                    if !modifiers.ctrl && !modifiers.command {
                        self.arc.selected.clear();
                    }
                    for (rect, full) in &card_rects {
                        if sel_rect.intersects(*rect) {
                            self.arc.selected.insert(full.clone());
                        }
                    }
                } else if !on_card && !modifiers.ctrl && !modifiers.command {
                    self.arc.selected.clear();
                }
            } else if !on_card && !modifiers.ctrl && !modifiers.command {
                self.arc.selected.clear();
            }
            self.grid_drag_start = None;
            self.grid_drag_now = None;
        }

        // Draw selection rectangle
        if let (Some(p0), Some(p1)) = (self.grid_drag_start, self.grid_drag_now)
            && (p1 - p0).length() > 4.0
        {
            let rect = egui::Rect::from_two_pos(p0, p1);
            let fill = egui::Color32::from_rgba_premultiplied(60, 60, 60, 40);
            let stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(160));
            ui.painter()
                .rect(rect, 0.0, fill, stroke, egui::StrokeKind::Inside);
        }
    }

    fn render_grid_row(
        &mut self,
        ui: &mut egui::Ui,
        items: &[&(String, &TreeNode)],
        size: egui::Vec2,
        card_rects: &mut Vec<(egui::Rect, String)>,
        modifiers: egui::Modifiers,
    ) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for (full_path, node) in items {
                let full = full_path.to_string();
                let selected = self.arc.selected.contains(&full);
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                card_rects.push((rect, full.clone()));
                let bg = if selected {
                    egui::Color32::from_gray(80)
                } else {
                    egui::Color32::from_gray(50)
                };
                ui.painter().rect_filled(rect, 4.0, bg);
                ui.allocate_ui_at_rect(rect, |ui| {
                    ui.style_mut().interaction.selectable_labels = false;
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add_space(6.0);
                    ui.vertical_centered(|ui| {
                        if node.is_dir {
                            ui.label(egui::RichText::new("📁").size(28.0));
                        } else {
                            ui.label(
                                egui::RichText::new(cat_icon(node.category.as_ref())).size(28.0),
                            );
                        }
                        ui.label(egui::RichText::new(ellipsize(&node.name, 16)).size(11.0));
                        if !node.is_dir {
                            ui.label(
                                egui::RichText::new(format_size(node.size.unwrap_or(0)))
                                    .size(10.0)
                                    .weak(),
                            );
                        }
                    });
                });
                if response.double_clicked() {
                    if node.is_dir {
                        self.grid_path = full;
                    } else {
                        self.extract_one(&full);
                    }
                } else if response.secondary_clicked() {
                    self.arc.ctx_menu = Some(full);
                } else if response.clicked() {
                    if modifiers.ctrl || modifiers.command {
                        if selected {
                            self.arc.selected.remove(&full);
                        } else {
                            self.arc.selected.insert(full);
                        }
                    } else {
                        self.arc.selected.clear();
                        self.arc.selected.insert(full);
                    }
                }
            }
        });
    }

    // ── context menu ─────────────────────────────────────────────────────

    fn ctx_menu(&mut self, ui: &mut egui::Ui) {
        let Some(ref path) = self.arc.ctx_menu.clone() else {
            return;
        };
        let pc = path.clone();

        let area = egui::Area::new("ctx_menu".into())
            .fixed_pos(ui.ctx().pointer_latest_pos().unwrap_or_default())
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(160.0);
                    if ui.button("📤 Extract this file…").clicked() {
                        self.extract_one(&pc);
                        self.arc.ctx_menu = None;
                    }
                    let label = if self.arc.selected.contains(&pc) {
                        "☐ Deselect"
                    } else {
                        "☑ Select"
                    };
                    if ui.button(label).clicked() {
                        if self.arc.selected.contains(&pc) {
                            self.arc.selected.remove(&pc);
                        } else {
                            self.arc.selected.insert(pc.clone());
                        }
                        self.arc.ctx_menu = None;
                    }
                });
            });

        if area.response.clicked_elsewhere() {
            self.arc.ctx_menu = None;
        }
    }

    // ── extraction ───────────────────────────────────────────────────────

    fn extract_one(&mut self, path: &str) {
        let Some(ref pack) = self.arc.pack else {
            return;
        };
        match pack.read_file(path) {
            Ok(data) => {
                let default_name = std::path::Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("extracted");
                if let Some(save_path) = rfd::FileDialog::new()
                    .set_file_name(default_name)
                    .save_file()
                {
                    match std::fs::write(&save_path, &data) {
                        Ok(_) => self.status = format!("Extracted: {}", save_path.display()),
                        Err(e) => self.status = format!("Save error: {e}"),
                    }
                }
            }
            Err(e) => self.status = format!("Read error: {e}"),
        }
    }

    fn extract_selected(&mut self) {
        if self.arc.selected.is_empty() {
            return;
        }
        let Some(ref pack) = self.arc.pack else {
            return;
        };
        let Some(out_dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };

        let selected = self.arc.selected.clone();
        let mut targets = std::collections::BTreeSet::new();
        let mut directory_prefixes = Vec::new();
        for path in selected {
            if pack.contains_file(&path) {
                targets.insert(path);
            } else {
                directory_prefixes.push(format!("{path}/"));
            }
        }
        if !directory_prefixes.is_empty() {
            for path in pack.iter_files() {
                if directory_prefixes
                    .iter()
                    .any(|prefix| path.starts_with(prefix))
                {
                    targets.insert(path.to_owned());
                }
            }
        }

        self.arc.selected.clear();
        if targets.is_empty() {
            self.status = "No files matched the selection.".into();
            return;
        }

        self.busy = true;
        self.status = "Extracting selected files…".into();
        self.progress = (0, 0);
        self.start = Some(Instant::now());
        let pack = pack.clone();
        let tracker = ProgressTracker::new();
        let completion = tracker.clone();
        self.tracker = Some(tracker);
        self.worker = Some(std::thread::spawn(move || {
            let mut ok = 0usize;
            let mut fail = 0usize;
            for file_path in targets {
                let Some(destination) = safe_output_path(&out_dir, &file_path) else {
                    fail += 1;
                    continue;
                };
                let operation = pack.read_file(&file_path).and_then(|data| {
                    if let Some(parent) = destination.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(destination, data)?;
                    Ok(())
                });
                if operation.is_ok() {
                    ok += 1;
                } else {
                    fail += 1;
                }
            }
            completion.inner.lock().unwrap().2 = true;
            Ok(format!("Extracted {ok} files, {fail} failed"))
        }));
    }

    fn extract_all(&mut self) {
        let Some(ref _pack) = self.arc.pack else {
            return;
        };
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };

        self.busy = true;
        self.status = "Extracting...".into();
        let archive = self.arc.path.clone();
        let output = dir.display().to_string();
        let password = if self.arc.encrypted && !self.arc.password.is_empty() {
            Some(self.arc.password.clone())
        } else {
            None
        };

        let tracker = ProgressTracker::new();
        let clone = tracker.clone();
        self.tracker = Some(tracker);
        self.progress = (0, 0);
        self.start = Some(Instant::now());

        self.worker = Some(std::thread::spawn(move || {
            let result = crate::cmd::read::extract_all(&archive, &output, password.as_deref());
            clone.inner.lock().unwrap().2 = true;
            result.map(|()| "Extraction complete.".into())
        }));
    }
}
