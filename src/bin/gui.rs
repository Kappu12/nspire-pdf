#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, RichText};
use nspire_pdf::{ConversionStage, ExportOptions, MathStyle, Stats};
use rfd::FileDialog;
use std::{
    env,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::Duration,
};

const TITLE: &str = "Nspire PDF";
const FOREGROUND: Color32 = Color32::WHITE;
const MUTED: Color32 = Color32::from_gray(170);
const LINE: Color32 = Color32::from_gray(46);
const SURFACE: Color32 = Color32::from_gray(19);
const BACKGROUND: Color32 = Color32::from_gray(9);
const ERROR: Color32 = Color32::from_rgb(255, 161, 152);

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
        stage: ConversionStage,
    },
    Done(PathBuf, Stats),
    Failed(String),
}

#[derive(Default)]
struct Converter {
    sources: Vec<PathBuf>,
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
        if !valid {
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

    fn choose_destination(&mut self) -> Option<PathBuf> {
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
        if path.extension().is_none() {
            path.set_extension("pdf");
        }
        Some(path)
    }

    fn start(&mut self, ctx: &egui::Context, destination: PathBuf) {
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
                let result = nspire_pdf::convert_files_with_options(
                    &sources,
                    &destination,
                    options,
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
                stage: ConversionStage::Reading,
            },
            Err(error) => Status::Failed(format!("Could not start conversion. {error}")),
        };
    }

    fn update_status(&mut self) {
        while let Status::Working { receiver, stage } = &mut self.status {
            self.status = match receiver.try_recv() {
                Ok(WorkerUpdate::Progress(current)) => {
                    *stage = current;
                    continue;
                }
                Ok(WorkerUpdate::Finished(Ok((path, stats)))) => Status::Done(path, stats),
                Ok(WorkerUpdate::Finished(Err(error))) => Status::Failed(error),
                Err(TryRecvError::Disconnected) => {
                    Status::Failed("The converter stopped unexpectedly.".into())
                }
                Err(TryRecvError::Empty) => return,
            };
        }
    }

    fn sources_ui(&mut self, ui: &mut egui::Ui) {
        let mut reorder = None;
        let mut remove = None;
        for (index, path) in self.sources.iter().enumerate() {
            let row = egui::Frame::new()
                .fill(SURFACE)
                .corner_radius(6)
                .inner_margin(6)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.dnd_drag_source(ui.id().with(path), index, |ui| {
                            let (response, painter) =
                                ui.allocate_painter(egui::vec2(12.0, 18.0), egui::Sense::hover());
                            let center = response.rect.center();
                            for offset in [-3.0, 0.0, 3.0] {
                                painter.hline(
                                    center.x - 3.0..=center.x + 3.0,
                                    center.y + offset,
                                    egui::Stroke::new(1.0, MUTED),
                                );
                            }
                            response.on_hover_text("Drag to reorder");
                        });
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
                                    ui.add(egui::Label::new(name.to_string_lossy()).truncate())
                                        .on_hover_text(path.display().to_string());
                                },
                            );
                        });
                    });
                })
                .response;
            if row.dnd_hover_payload::<usize>().is_some() {
                ui.painter().rect_stroke(
                    row.rect,
                    6,
                    egui::Stroke::new(1.0, FOREGROUND),
                    egui::StrokeKind::Inside,
                );
            }
            if let Some(from) = row.dnd_release_payload::<usize>() {
                reorder = Some((*from, index));
            }
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
            Status::Done(path, _) => Some(path.clone()),
            _ => self.suggested_destination(),
        };
        if let Some(path) = path {
            let displayed = path.display().to_string();
            ui.add(egui::Label::new(RichText::new(&displayed).color(MUTED)).truncate())
                .on_hover_text(displayed);
        }
    }

    fn status_ui(&mut self, ui: &mut egui::Ui) {
        match &self.status {
            Status::Idle => {
                ui.label(
                    RichText::new("A4 PDF with selectable text and typeset math.").color(MUTED),
                );
            }
            Status::Working { .. } => {}
            Status::Done(path, stats) => {
                let clicked = ui
                    .horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("PDF saved").strong());
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
                if clicked && let Err(error) = open::that(path) {
                    self.status =
                        Status::Failed(format!("PDF saved, but it could not be opened. {error}"));
                }
            }
            Status::Failed(error) => {
                ui.label(RichText::new(error).color(ERROR));
            }
        }
    }
}

impl eframe::App for Converter {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.update_status();
        let stage = match self.status {
            Status::Working { stage, .. } => Some(stage),
            _ => None,
        };
        let working = stage.is_some();
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
                        ui.checkbox(
                            &mut self.show_widget_placeholders,
                            "Mark skipped app panes in PDF",
                        );
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
                    let ready = !working && !self.sources.is_empty();
                    if convert_button(ui, ready, stage)
                        && let Some(destination) = self.choose_destination()
                    {
                        self.start(ctx, destination);
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

fn convert_button(ui: &mut egui::Ui, ready: bool, stage: Option<ConversionStage>) -> bool {
    let label = match stage {
        None => "Convert to PDF",
        Some(ConversionStage::Reading) => "Reading notes…",
        Some(ConversionStage::Typesetting) => "Typesetting PDF…",
        Some(ConversionStage::Saving) => "Saving PDF…",
    };
    let (fill, text) = if stage.is_some() {
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
    .min_size(egui::vec2(ui.available_width(), 38.0));
    let response = ui.add_enabled(ready, button);
    if stage.is_some() {
        let edge = response.rect.shrink2(egui::vec2(7.0, 1.0));
        // The compiler provides no completion estimate, so animate an activity line.
        let time = ui.input(|input| input.time);
        let angle = time * std::f64::consts::TAU / 2.4;
        let position = (0.5 - 0.5 * angle.cos()) as f32;
        let length = edge.width() * 0.22;
        let start = edge.left() + (edge.width() - length) * position;
        ui.painter().hline(
            start..=start + length,
            edge.bottom(),
            egui::Stroke::new(2.0, FOREGROUND),
        );
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
    response.clicked()
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
