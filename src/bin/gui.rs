#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, RichText};
use nspire_pdf::{ConversionStage, ExportOptions, MathStyle, OutputPolicy, Stats};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::{
    env,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

const TITLE: &str = "Nspire PDF";
const FOREGROUND: Color32 = Color32::WHITE;
const MUTED: Color32 = Color32::from_gray(170);
const LINE: Color32 = Color32::from_gray(46);
const SURFACE: Color32 = Color32::from_gray(19);
const BACKGROUND: Color32 = Color32::from_gray(9);
const ERROR: Color32 = Color32::from_rgb(255, 161, 152);
const SUCCESS: Color32 = Color32::from_rgb(106, 218, 137);
const SOURCE_ROW_HEIGHT: f32 = 30.0;

fn main() -> eframe::Result {
    let mut app = Converter::default();
    let paths: Vec<_> = env::args_os().skip(1).map(PathBuf::from).collect();
    if !paths.is_empty() {
        app.add_sources(paths);
    }
    eframe::run_native(
        TITLE,
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([600.0, 520.0])
                .with_min_inner_size([475.0, 440.0])
                .with_drag_and_drop(true),
            ..Default::default()
        },
        Box::new(|cc| {
            let mut visuals = egui::Visuals::dark();
            visuals.override_text_color = Some(FOREGROUND);
            visuals.selection.bg_fill = LINE;
            cc.egui_ctx.set_visuals(visuals);
            Ok(Box::new(app))
        }),
    )
}

type Outcome = Result<(PathBuf, Stats), String>;

enum WorkerUpdate {
    Progress(ConversionStage),
    Finished(Outcome),
}

#[derive(Default)]
enum Status {
    #[default]
    Idle,
    Working {
        receiver: Receiver<WorkerUpdate>,
        progress: ConversionProgress,
    },
    Done {
        path: PathBuf,
        stats: Stats,
        finished_at: Instant,
        open_error: Option<String>,
    },
    Failed(String),
}

struct ConversionProgress {
    stage: ConversionStage,
    stage_started: Instant,
    last_frame: Instant,
    fill: f32,
}

impl ConversionProgress {
    fn new(now: Instant) -> Self {
        Self {
            stage: ConversionStage::Reading,
            stage_started: now,
            last_frame: now,
            fill: 0.0,
        }
    }

    fn enter(&mut self, stage: ConversionStage, now: Instant) {
        self.stage = stage;
        self.stage_started = now;
    }

    fn advance(&mut self, now: Instant) -> f32 {
        let elapsed = now.duration_since(self.stage_started).as_secs_f32();
        // Tectonic reports no fraction complete. Reserve the end of the bar for the save.
        let estimate = match self.stage {
            ConversionStage::Reading => 0.18 * elapsed / (elapsed + 0.8),
            ConversionStage::Typesetting => 0.18 + 0.77 * elapsed / (elapsed + 7.0),
            ConversionStage::Saving => 0.98,
        };
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        self.fill += (estimate - self.fill).max(0.0) * (1.0 - (-dt / 0.22).exp());
        self.fill
    }
}

#[derive(Clone, Copy)]
struct SourceDrag {
    from: usize,
    grab_offset: egui::Vec2,
}

#[derive(Default)]
struct Converter {
    sources: Vec<PathBuf>,
    source_drag: Option<SourceDrag>,
    output_name: Option<String>,
    status: Status,
    style: MathStyle,
    show_widget_placeholders: bool,
}

impl Converter {
    fn add_sources(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        let mut valid = false;
        for path in paths.into_iter().filter(|path| is_tns(path)) {
            valid = true;
            if !self.sources.contains(&path) {
                self.sources.push(path);
                self.status = Status::Idle;
            }
        }
        if !valid && !matches!(self.status, Status::Done { .. }) {
            self.status =
                Status::Failed("Choose a TI-Nspire Notes file with a .tns extension.".into());
        }
    }

    fn accept_drops(&mut self, files: &[egui::DroppedFile]) {
        if !files.is_empty() {
            self.add_sources(files.iter().filter_map(|file| file.path.clone()));
        }
    }

    fn default_output_path(&self) -> Option<PathBuf> {
        let first = self.sources.first()?;
        Some(if self.sources.len() > 1 {
            first.with_file_name("Combined Notes.pdf")
        } else {
            default_destination(first, self.style)
        })
    }

    fn suggested_destination(&self) -> Option<PathBuf> {
        let default = self.default_output_path()?;
        let Some(name) = self
            .output_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            return Some(default);
        };
        let filename = Path::new(name);
        if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
            || filename.file_name() != Some(filename.as_os_str())
        {
            return None;
        }
        Some(
            if filename
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
            {
                default.with_file_name(filename)
            } else {
                default.with_file_name(format!("{name}.pdf"))
            },
        )
    }

    fn choose_destination(&mut self) -> Option<(PathBuf, OutputPolicy)> {
        let Some(suggested) = self.suggested_destination() else {
            self.status = Status::Failed("Enter a valid PDF filename without a folder.".into());
            return None;
        };
        let mut dialog = FileDialog::new().add_filter("PDF document", &["pdf"]);
        if let Some(parent) = suggested.parent().filter(|p| !p.as_os_str().is_empty()) {
            dialog = dialog.set_directory(parent);
        }
        if let Some(name) = suggested.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        let mut path = dialog.save_file()?;
        let needs_extension = path.extension().is_none();
        if needs_extension {
            path.set_extension("pdf");
        }
        let policy = if path.exists() {
            // A Save As confirmation only covers the exact filename returned by the dialog.
            if needs_extension
                && MessageDialog::new()
                    .set_title("Confirm Save As")
                    .set_level(MessageLevel::Warning)
                    .set_description(format!("{} already exists. Replace it?", path.display()))
                    .set_buttons(MessageButtons::YesNo)
                    .show()
                    != MessageDialogResult::Yes
            {
                return None;
            }
            OutputPolicy::ReplaceExisting
        } else {
            OutputPolicy::CreateNew
        };
        Some((path, policy))
    }

    fn start(&mut self, ctx: &egui::Context, destination: PathBuf, output_policy: OutputPolicy) {
        let sources = self.sources.clone();
        let ctx = ctx.clone();
        let options = ExportOptions {
            style: self.style,
            show_widget_placeholders: self.show_widget_placeholders,
        };
        let (send, receive) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("pdf-converter".into())
            .spawn(move || {
                let result = nspire_pdf::convert_files_with_output_policy(
                    &sources,
                    &destination,
                    options,
                    output_policy,
                    |stage| {
                        let _ = send.send(WorkerUpdate::Progress(stage));
                        ctx.request_repaint();
                    },
                )
                .map(|stats| (destination, stats))
                .map_err(|error| format!("{error:#}"));
                let _ = send.send(WorkerUpdate::Finished(result));
                ctx.request_repaint();
            });
        self.status = match worker {
            Ok(_) => Status::Working {
                receiver: receive,
                progress: ConversionProgress::new(Instant::now()),
            },
            Err(error) => Status::Failed(format!("Could not start conversion. {error}")),
        };
    }

    fn update_status(&mut self) {
        while let Status::Working { receiver, progress } = &mut self.status {
            self.status = match receiver.try_recv() {
                Ok(WorkerUpdate::Progress(current)) => {
                    progress.enter(current, Instant::now());
                    continue;
                }
                Ok(WorkerUpdate::Finished(Ok((path, stats)))) => Status::Done {
                    path,
                    stats,
                    finished_at: Instant::now(),
                    open_error: None,
                },
                Ok(WorkerUpdate::Finished(Err(error))) => Status::Failed(error),
                Err(TryRecvError::Disconnected) => {
                    Status::Failed("The converter stopped unexpectedly.".into())
                }
                Err(TryRecvError::Empty) => return,
            };
        }
    }

    fn sources_ui(&mut self, ui: &mut egui::Ui) {
        let count = self.sources.len();
        let stride = SOURCE_ROW_HEIGHT + ui.spacing().item_spacing.y;
        let height = SOURCE_ROW_HEIGHT + stride * (count - 1) as f32;
        let (list, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        let (pointer, origin, released, cancelled) = ui.input(|input| {
            (
                input.pointer.interact_pos(),
                input.pointer.press_origin(),
                input.pointer.primary_released(),
                input.key_pressed(egui::Key::Escape)
                    || !input.focused
                    || (!input.pointer.primary_down() && !input.pointer.primary_released()),
            )
        });
        if cancelled {
            self.source_drag = None;
        }
        let dragged = self.source_drag.zip(pointer).map(|(drag, pointer)| {
            let top_left = pointer - drag.grab_offset;
            let target = ((top_left.y - list.top()) / stride)
                .round()
                .clamp(0.0, (count - 1) as f32) as usize;
            (drag, top_left, target)
        });
        if let Some((_, _, target)) = dragged {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            ui.painter().rect_stroke(
                egui::Rect::from_min_size(
                    list.min + egui::vec2(0.0, target as f32 * stride),
                    egui::vec2(list.width(), SOURCE_ROW_HEIGHT),
                ),
                6,
                egui::Stroke::new(1.0, LINE),
                egui::StrokeKind::Inside,
            );
        }

        let mut reorder = None;
        let mut remove = None;
        for (index, path) in self.sources.iter().enumerate() {
            let id = ui.id().with(("source-row", path));
            let floating = dragged.filter(|(drag, _, _)| drag.from == index);
            let top_left = if let Some((_, top_left, _)) = floating {
                // Seed the animations so the row settles from its release position.
                ui.ctx()
                    .animate_value_with_time(id.with("x"), top_left.x - list.left(), 0.0);
                ui.ctx().animate_value_with_time(
                    id.with("slot"),
                    (top_left.y - list.top()) / stride,
                    0.0,
                );
                top_left
            } else {
                let slot = match dragged {
                    Some((drag, _, target)) if drag.from < index && index <= target => index - 1,
                    Some((drag, _, target)) if target <= index && index < drag.from => index + 1,
                    _ => index,
                };
                let x = ui.ctx().animate_value_with_time(id.with("x"), 0.0, 0.14);
                let y = ui
                    .ctx()
                    .animate_value_with_time(id.with("slot"), slot as f32, 0.14);
                list.min + egui::vec2(x, y * stride)
            };
            let rect =
                egui::Rect::from_min_size(top_left, egui::vec2(list.width(), SOURCE_ROW_HEIGHT));
            let mut builder = egui::UiBuilder::new().id_salt(id).max_rect(rect);
            if floating.is_some() {
                builder = builder.layer_id(egui::LayerId::new(egui::Order::Tooltip, id));
            }
            let mut row_ui = ui.new_child(builder);
            egui::Frame::new()
                .fill(if floating.is_some() { LINE } else { SURFACE })
                .corner_radius(6)
                .inner_margin(6)
                .show(&mut row_ui, |ui| {
                    ui.horizontal(|ui| {
                        let (handle, _) =
                            ui.allocate_exact_size(egui::vec2(12.0, 18.0), egui::Sense::hover());
                        let center = handle.center();
                        for offset in [-3.0, 0.0, 3.0] {
                            ui.painter().hline(
                                center.x - 3.0..=center.x + 3.0,
                                center.y + offset,
                                egui::Stroke::new(1.0, MUTED),
                            );
                        }
                        ui.label(RichText::new(format!("{}.", index + 1)).color(MUTED));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("×").on_hover_text("Remove file").clicked() {
                                remove = Some(index);
                            }
                            if move_button(ui, index + 1 < self.sources.len(), false) {
                                reorder = Some((index, index + 1));
                            }
                            if move_button(ui, index > 0, true) {
                                reorder = Some((index, index - 1));
                            }
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    let name = path.file_name().unwrap_or(path.as_os_str());
                                    let filename = ui
                                        .add(egui::Label::new(name.to_string_lossy()).truncate())
                                        .on_hover_text(path.display().to_string());
                                    if dragged.is_none() && count > 1 {
                                        let response = ui
                                            .interact(
                                                handle.union(filename.rect),
                                                id.with("drag"),
                                                egui::Sense::drag(),
                                            )
                                            .on_hover_cursor(egui::CursorIcon::Grab);
                                        if response.drag_started()
                                            && let Some(origin) = origin
                                        {
                                            self.source_drag = Some(SourceDrag {
                                                from: index,
                                                grab_offset: origin - rect.min,
                                            });
                                            ui.ctx().request_repaint();
                                        }
                                    }
                                },
                            );
                        });
                    });
                });
        }
        if released && let Some((drag, _, target)) = dragged {
            self.source_drag = None;
            reorder = Some((drag.from, target));
            ui.ctx().request_repaint();
        }
        if let Some(index) = remove {
            self.sources.remove(index);
            if self.sources.is_empty() {
                self.output_name = None;
            }
            self.status = Status::Idle;
        } else if let Some((from, to)) = reorder
            && from != to
            && from < self.sources.len()
        {
            let source = self.sources.remove(from);
            self.sources.insert(to, source);
            self.status = Status::Idle;
        }
    }

    fn output_ui(&mut self, ui: &mut egui::Ui) {
        let Some(default) = self.default_output_path() else {
            return;
        };
        let default_name = default
            .file_name()
            .unwrap_or(default.as_os_str())
            .to_string_lossy();
        let mut name = self
            .output_name
            .clone()
            .unwrap_or_else(|| default_name.to_string());
        let editor_id = ui.make_persistent_id("output-name");
        egui::Frame::new()
            .fill(SURFACE)
            .corner_radius(6)
            .inner_margin(8)
            .show(ui, |ui| {
                let label = ui.label(RichText::new("Output filename").color(MUTED));
                let width = ui.available_width();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut name)
                            .id(editor_id)
                            .hint_text(default_name.as_ref())
                            .desired_width(width),
                    )
                    .labelled_by(label.id)
                    .changed()
                {
                    self.output_name = Some(name);
                    self.status = Status::Idle;
                }
            });
        let path = match &self.status {
            Status::Done { path, .. } => Some(path.clone()),
            _ => self.suggested_destination(),
        };
        if let Some(path) = path {
            let displayed = path.display().to_string();
            ui.add(egui::Label::new(RichText::new(&displayed).color(MUTED)).truncate())
                .on_hover_text(displayed);
        }
    }

    fn status_ui(&mut self, ui: &mut egui::Ui) {
        match &mut self.status {
            Status::Idle => {
                ui.label(
                    RichText::new("A4 PDF with selectable text and typeset math.").color(MUTED),
                );
            }
            Status::Working { .. } => {}
            Status::Done {
                path,
                stats,
                open_error,
                ..
            } => {
                let clicked = ui
                    .horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} Notes sections · {} math boxes · {} images",
                                    stats.notes, stats.math, stats.images,
                                ))
                                .color(MUTED),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.button("Open PDF").clicked()
                        })
                        .inner
                    })
                    .inner;
                if clicked {
                    *open_error = open::that(&*path)
                        .err()
                        .map(|error| format!("PDF saved, but it could not be opened. {error}"));
                }
                if let Some(error) = open_error {
                    ui.label(RichText::new(error.as_str()).color(ERROR));
                }
            }
            Status::Failed(error) => {
                ui.label(RichText::new(error.as_str()).color(ERROR));
            }
        }
    }
}

impl eframe::App for Converter {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.update_status();
        let progress = match &mut self.status {
            Status::Working { progress, .. } => {
                Some((progress.stage, progress.advance(Instant::now())))
            }
            _ => None,
        };
        let working = progress.is_some();
        let file_hover = !working
            && ctx.input(|input| {
                self.accept_drops(&input.raw.dropped_files);
                !input.raw.hovered_files.is_empty()
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(26))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading(RichText::new("Convert to PDF").size(29.0).strong());
                    ui.label(
                        RichText::new("Choose or drop .tns files to make a print-ready PDF.")
                            .color(MUTED),
                    );
                    ui.add_space(17.0);

                    ui.add_enabled_ui(!working, |ui| {
                        let previous = self.style;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Math style").strong());
                            ui.selectable_value(&mut self.style, MathStyle::Latex, "LaTeX");
                            ui.selectable_value(&mut self.style, MathStyle::Lean, "Lean");
                        });
                        if self.style != previous {
                            self.status = Status::Idle;
                        }
                        if ui
                            .checkbox(
                                &mut self.show_widget_placeholders,
                                "Mark skipped app panes in PDF",
                            )
                            .changed()
                        {
                            self.status = Status::Idle;
                        }
                        ui.add_space(9.0);
                        if source_target(ui, !self.sources.is_empty(), file_hover)
                            && let Some(paths) = FileDialog::new()
                                .add_filter("TI-Nspire document", &["tns"])
                                .pick_files()
                        {
                            self.add_sources(paths);
                        }
                        if !self.sources.is_empty() {
                            ui.add_space(9.0);
                            self.sources_ui(ui);
                            ui.add_space(9.0);
                            self.output_ui(ui);
                        }
                    });
                    ui.add_space(17.0);
                    let ready = !working && self.source_drag.is_none() && !self.sources.is_empty();
                    let converted = match self.status {
                        Status::Done { finished_at, .. } => Some(finished_at.elapsed()),
                        _ => None,
                    };
                    if convert_button(ui, ready, progress, converted)
                        && let Some((destination, output_policy)) = self.choose_destination()
                    {
                        self.start(ctx, destination, output_policy);
                    }
                    ui.add_space(12.0);
                    self.status_ui(ui);
                });
            });
    }
}

fn is_tns(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tns"))
}

fn move_button(ui: &mut egui::Ui, enabled: bool, up: bool) -> bool {
    let label = if up { "Move up" } else { "Move down" };
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("").min_size(egui::vec2(22.0, 18.0)),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), label)
    });
    let center = response.rect.center();
    let tip = if up { -2.0 } else { 2.0 };
    let stroke = ui.style().interact(&response).fg_stroke;
    for x in [-3.5, 3.5] {
        ui.painter().line_segment(
            [center + egui::vec2(x, -tip), center + egui::vec2(0.0, tip)],
            stroke,
        );
    }
    response.on_hover_text(label).clicked()
}

fn convert_button(
    ui: &mut egui::Ui,
    ready: bool,
    progress: Option<(ConversionStage, f32)>,
    converted: Option<Duration>,
) -> bool {
    let size = egui::vec2(ui.available_width(), 38.0);
    if let Some(elapsed) = converted {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "Converted"));
        let painter = ui.painter();
        painter.rect_filled(rect, 8, LINE);
        let label = painter.layout_no_wrap(
            "Converted".into(),
            egui::TextStyle::Button.resolve(ui.style()),
            MUTED,
        );
        let icon = rect.center() - egui::vec2((label.size().x + 7.0) / 2.0, 0.0);
        paint_conversion_check(ui, icon, elapsed);
        let origin = icon + egui::vec2(13.0, -label.size().y / 2.0);
        painter.galley(origin, label, MUTED);
        return false;
    }
    let label = match progress.map(|(stage, _)| stage) {
        None => "Convert to PDF",
        Some(ConversionStage::Reading) => "Reading notes…",
        Some(ConversionStage::Typesetting) => "Typesetting PDF…",
        Some(ConversionStage::Saving) => "Saving PDF…",
    };
    let (fill, text) = if progress.is_some() {
        (SURFACE, FOREGROUND)
    } else {
        (FOREGROUND, BACKGROUND)
    };
    let button = egui::Button::new((
        egui::Atom::grow(),
        RichText::new(label).strong().color(text),
        egui::Atom::grow(),
    ))
    .fill(fill)
    .corner_radius(8)
    .min_size(size);
    let response = ui.add_enabled(ready, button);
    if let Some((_, fraction)) = progress {
        let edge = response.rect.shrink2(egui::vec2(7.0, 1.0));
        ui.painter()
            .hline(edge.x_range(), edge.bottom(), egui::Stroke::new(2.0, LINE));
        ui.painter().hline(
            edge.left()..=edge.left() + edge.width() * fraction,
            edge.bottom(),
            egui::Stroke::new(2.0, FOREGROUND),
        );
        ui.ctx().request_repaint_after(Duration::from_millis(16));
        response.on_disabled_hover_text("Estimated progress");
        return false;
    }
    response.clicked()
}

fn paint_conversion_check(ui: &egui::Ui, center: egui::Pos2, elapsed: Duration) {
    let seconds = elapsed.as_secs_f32();
    let painter = ui.painter();
    let radius = 8.0;
    let outline = egui::Stroke::new(1.2, SUCCESS.gamma_multiply(0.8));
    let sweep = (seconds / 0.28).min(1.0);
    if sweep >= 1.0 {
        painter.circle_stroke(center, radius, outline);
    } else if sweep > 0.0 {
        let points = (0..=24)
            .map(|step| {
                let angle = std::f32::consts::TAU * sweep * step as f32 / 24.0
                    - std::f32::consts::FRAC_PI_2;
                center + egui::Vec2::angled(angle) * radius
            })
            .collect();
        painter.add(egui::Shape::line(points, outline));
    }

    let check = ((seconds - 0.16) / 0.22).clamp(0.0, 1.0);
    if check > 0.0 {
        let start = center + egui::vec2(-3.8, 0.0);
        let bend = center + egui::vec2(-1.0, 2.7);
        let end = center + egui::vec2(4.0, -2.9);
        let points = if check < 0.35 {
            vec![start, start.lerp(bend, check / 0.35)]
        } else {
            vec![start, bend, bend.lerp(end, (check - 0.35) / 0.65)]
        };
        painter.add(egui::Shape::line(points, egui::Stroke::new(1.6, SUCCESS)));
    }
    if check < 1.0 {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
}

fn source_target(ui: &mut egui::Ui, has_sources: bool, file_hover: bool) -> bool {
    let title = if file_hover {
        "Drop your .tns files"
    } else if has_sources {
        "Drop more .tns files"
    } else {
        "Drop .tns files here"
    };
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 112.0),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            "Browse for .tns files",
        )
    });
    let highlight = file_hover || response.hovered() || response.has_focus();
    let painter = ui.painter();
    painter.rect(
        rect,
        10,
        if highlight { LINE } else { SURFACE },
        egui::Stroke::new(1.0, if highlight { FOREGROUND } else { LINE }),
        egui::StrokeKind::Inside,
    );
    let center = rect.center();
    painter.text(
        center - egui::vec2(0.0, 10.0),
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(19.0),
        FOREGROUND,
    );
    painter.text(
        center + egui::vec2(0.0, 13.0),
        egui::Align2::CENTER_CENTER,
        "or click to browse",
        egui::FontId::proportional(14.0),
        MUTED,
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn default_destination(source: &Path, style: MathStyle) -> PathBuf {
    let suffix = match style {
        MathStyle::Lean => "-Lean.pdf",
        MathStyle::Latex => ".pdf",
    };
    let mut filename = source.file_stem().unwrap_or_default().to_os_string();
    filename.push(suffix);
    source.with_file_name(filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_estimate_keeps_filling_without_jumping_at_stage_changes() {
        let start = Instant::now();
        let mut progress = ConversionProgress::new(start);
        let typesetting = start + Duration::from_millis(500);
        let reading_fill = progress.advance(typesetting);
        assert!(reading_fill > 0.0);
        progress.enter(ConversionStage::Typesetting, typesetting);
        assert_eq!(progress.advance(typesetting), reading_fill);

        let mut previous = reading_fill;
        for seconds in [1, 2, 10, 60, 120] {
            let fill = progress.advance(typesetting + Duration::from_secs(seconds));
            assert!(fill > previous && fill < 0.95);
            previous = fill;
        }
        let saving = typesetting + Duration::from_secs(120);
        progress.enter(ConversionStage::Saving, saving);
        assert_eq!(progress.advance(saving), previous);
        let fill = progress.advance(saving + Duration::from_millis(200));
        assert!(fill > previous && fill < 1.0);
    }

    #[test]
    fn successful_export_shows_a_passive_converted_bar_until_a_file_is_added() {
        let mut app = Converter::default();
        app.add_sources([PathBuf::from("first.tns")]);
        let (send, receiver) = mpsc::channel();
        app.status = Status::Working {
            receiver,
            progress: ConversionProgress::new(Instant::now()),
        };
        send.send(WorkerUpdate::Finished(Ok((
            PathBuf::from("first.pdf"),
            Stats::default(),
        ))))
        .ok()
        .unwrap();
        app.update_status();
        app.add_sources([PathBuf::from("first.tns")]);
        app.add_sources([PathBuf::from("ignored.txt")]);
        assert!(matches!(app.status, Status::Done { .. }));

        let ctx = egui::Context::default();
        let completed_frame = |elapsed| {
            ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    assert!(!convert_button(ui, true, None, Some(elapsed)));
                });
            })
        };
        let starting = completed_frame(Duration::from_millis(100));
        assert!(starting.shapes.iter().any(|shape| matches!(
            &shape.shape,
            egui::Shape::Path(path) if path.points.len() > 3
        )));
        let output = completed_frame(Duration::from_millis(500));
        assert!(output.shapes.iter().any(|shape| matches!(
            &shape.shape,
            egui::Shape::Circle(circle) if circle.radius == 8.0
        )));
        assert!(output.shapes.iter().any(|shape| matches!(
            &shape.shape,
            egui::Shape::Text(text) if text.galley.text() == "Converted"
        )));
        assert!(output.shapes.iter().any(|shape| matches!(
            &shape.shape,
            egui::Shape::Path(path) if path.points.len() == 3
                && matches!(&path.stroke.color, egui::epaint::ColorMode::Solid(color) if *color == SUCCESS)
        )));
        app.add_sources([PathBuf::from("second.tns")]);
        assert!(matches!(app.status, Status::Idle));
    }

    #[test]
    fn dragged_row_follows_pointer_and_other_rows_animate_into_place() {
        let mut app = Converter::default();
        app.add_sources(["first.tns", "second.tns", "third.tns"].map(PathBuf::from));
        let ctx = egui::Context::default();
        let mut frame = |time, events| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(475.0, 440.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            };
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.sources_ui(ui));
            })
        };
        let filename_position = |frame: &egui::FullOutput, filename| {
            frame
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == filename => Some(text.pos),
                    _ => None,
                })
                .unwrap()
        };
        let mouse_button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let initial = frame(0.0, vec![]);
        let first = filename_position(&initial, "first.tns");
        let second = filename_position(&initial, "second.tns");
        let third = filename_position(&initial, "third.tns");
        let grab = first + egui::vec2(4.0, 4.0);
        let destination = grab + egui::vec2(16.0, third.y - first.y);
        frame(
            0.02,
            vec![egui::Event::PointerMoved(grab), mouse_button(grab, true)],
        );
        let moving = frame(0.04, vec![egui::Event::PointerMoved(destination)]);
        let floating = filename_position(&moving, "first.tns");
        assert!((floating - (first + (destination - grab))).length() < 1.0);

        let shifting = frame(0.10, vec![]);
        let second_y = filename_position(&shifting, "second.tns").y;
        assert!(first.y < second_y && second_y < second.y);

        frame(0.12, vec![mouse_button(destination, false)]);
        frame(0.14, vec![]);
        let settled = frame(0.36, vec![]);
        assert!((filename_position(&settled, "first.tns") - third).length() < 1.0);
        assert_eq!(
            app.sources,
            ["second.tns", "third.tns", "first.tns"].map(PathBuf::from)
        );
    }

    #[test]
    fn drop_adds_each_tns_file_in_order_without_duplicates() {
        let mut app = Converter::default();
        let files = ["readme.txt", "notes.TNS", "other.tns"].map(|path| egui::DroppedFile {
            path: Some(path.into()),
            ..Default::default()
        });
        app.accept_drops(&files);
        assert_eq!(
            app.sources,
            [PathBuf::from("notes.TNS"), PathBuf::from("other.tns")]
        );
        assert!(matches!(app.status, Status::Idle));

        app.accept_drops(&files);
        assert_eq!(app.sources.len(), 2);
        app.accept_drops(&files[..1]);
        assert_eq!(app.sources.len(), 2);
        assert!(matches!(app.status, Status::Failed(_)));
    }

    #[test]
    fn output_suggestion_keeps_the_source_name_and_respects_custom_names() {
        let folder = tempfile::tempdir().unwrap();
        let existing = folder.path().join("notes.pdf");
        std::fs::write(&existing, []).unwrap();
        let mut app = Converter::default();
        app.add_sources([folder.path().join("notes.tns")]);
        assert_eq!(app.suggested_destination(), Some(existing));

        app.add_sources([folder.path().join("other.tns")]);
        assert_eq!(
            app.suggested_destination(),
            Some(folder.path().join("Combined Notes.pdf"))
        );
        app.output_name = Some("Revision.pdf".to_owned());
        assert_eq!(
            app.suggested_destination(),
            Some(folder.path().join("Revision.pdf"))
        );
    }

    #[test]
    fn filename_editor_accepts_text_and_keeps_the_source_folder() {
        let mut app = Converter {
            output_name: Some(String::new()),
            ..Default::default()
        };
        app.add_sources([PathBuf::from("school/notes.tns")]);
        let ctx = egui::Context::default();
        for events in [vec![], vec![egui::Event::Text("Revision".to_owned())]] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(475.0, 440.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let editor_id = ui.make_persistent_id("output-name");
                    ui.memory_mut(|memory| memory.request_focus(editor_id));
                    app.output_ui(ui);
                });
            });
        }
        assert_eq!(
            app.suggested_destination(),
            Some(PathBuf::from("school").join("Revision.pdf"))
        );
    }
}
