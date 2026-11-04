#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, RichText};
use nspire_pdf::{MathStyle, Stats};
use rfd::FileDialog;
use std::{
    env,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
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
    if let Some(path) = env::args_os().nth(1) {
        app.select_source(path.into());
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

#[derive(Default)]
enum Status {
    #[default]
    Idle,
    Working(Receiver<Outcome>),
    Done(PathBuf, Stats),
    Failed(String),
}

#[derive(Default)]
struct Converter {
    source: Option<PathBuf>,
    destination: Option<PathBuf>,
    status: Status,
    style: MathStyle,
}

impl Converter {
    fn select_source(&mut self, path: PathBuf) {
        if !is_tns(&path) {
            self.status =
                Status::Failed("Choose a TI-Nspire Notes file with a .tns extension.".into());
            return;
        }
        self.destination = Some(default_destination(&path, self.style));
        self.source = Some(path);
        self.status = Status::Idle;
    }

    fn accept_drops(&mut self, files: &[egui::DroppedFile]) {
        if let Some(path) = files
            .iter()
            .filter_map(|file| file.path.as_deref())
            .find(|path| is_tns(path))
        {
            self.select_source(path.to_owned());
        } else if !files.is_empty() {
            self.status =
                Status::Failed("Drop a TI-Nspire Notes file with a .tns extension.".into());
        }
    }

    fn choose_destination(&mut self) {
        let mut dialog = FileDialog::new().add_filter("PDF document", &["pdf"]);
        if let Some(path) = &self.destination {
            if let Some(parent) = path.parent() {
                dialog = dialog.set_directory(parent);
            }
            if let Some(name) = path.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy());
            }
        }
        if let Some(mut path) = dialog.save_file() {
            if path.extension().is_none() {
                path.set_extension("pdf");
            }
            self.destination = Some(path);
            self.status = Status::Idle;
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let (Some(source), Some(destination)) = (self.source.clone(), self.destination.clone())
        else {
            return;
        };
        let ctx = ctx.clone();
        let style = self.style;
        let (send, receive) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("pdf-converter".into())
            .spawn(move || {
                let result = nspire_pdf::convert_with_style(&source, &destination, style)
                    .map(|stats| (destination, stats))
                    .map_err(|error| format!("{error:#}"));
                let _ = send.send(result);
                ctx.request_repaint();
            });
        self.status = match worker {
            Ok(_) => Status::Working(receive),
            Err(error) => Status::Failed(format!("Could not start conversion. {error}")),
        };
    }

    fn update_status(&mut self) {
        if let Status::Working(receiver) = &self.status {
            self.status = match receiver.try_recv() {
                Ok(Ok((path, stats))) => Status::Done(path, stats),
                Ok(Err(error)) => Status::Failed(error),
                Err(TryRecvError::Disconnected) => {
                    Status::Failed("The converter stopped unexpectedly.".into())
                }
                Err(TryRecvError::Empty) => return,
            };
        }
    }

    fn status_ui(&mut self, ui: &mut egui::Ui) {
        match &self.status {
            Status::Idle => {
                ui.label(
                    RichText::new("A4 PDF with selectable text and typeset math.").color(MUTED),
                );
            }
            Status::Working(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Preparing your PDF…");
                });
            }
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
        let working = matches!(self.status, Status::Working(_));
        let file_hover = !working
            && ctx.input(|input| {
                self.accept_drops(&input.raw.dropped_files);
                !input.raw.hovered_files.is_empty()
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(26))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(RichText::new("TI-NSPIRE NOTES").size(12.0).strong());
                    ui.heading(RichText::new("Convert to PDF").size(29.0).strong());
                    ui.label(
                        RichText::new("Choose or drop a .tns file to make a print-ready PDF.")
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
                        if self.style != previous
                            && let Some(source) = &self.source
                        {
                            self.destination = Some(default_destination(source, self.style));
                            self.status = Status::Idle;
                        }
                        ui.add_space(9.0);
                        if source_target(ui, self.source.as_deref(), file_hover)
                            && let Some(path) = FileDialog::new()
                                .add_filter("TI-Nspire document", &["tns"])
                                .pick_file()
                        {
                            self.select_source(path);
                        }

                        ui.add_space(9.0);
                        ui.add_enabled_ui(self.source.is_some(), |ui| {
                            if file_row(
                                ui,
                                "Save PDF as",
                                self.destination.as_deref(),
                                "Choose a source first",
                                "Change…",
                            ) {
                                self.choose_destination();
                            }
                        });
                    });
                    ui.add_space(17.0);
                    let convert = egui::Button::new((
                        egui::Atom::grow(),
                        RichText::new(if working {
                            "Converting…"
                        } else {
                            "Convert to PDF"
                        })
                        .strong()
                        .color(BACKGROUND),
                        egui::Atom::grow(),
                    ))
                    .fill(FOREGROUND)
                    .corner_radius(8)
                    .min_size(egui::vec2(ui.available_width(), 38.0));
                    let ready = !working && self.source.is_some() && self.destination.is_some();
                    if ui.add_enabled(ready, convert).clicked() {
                        self.start(ctx);
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

fn source_target(ui: &mut egui::Ui, path: Option<&Path>, file_hover: bool) -> bool {
    let title = if file_hover {
        "Drop your .tns file"
    } else if path.is_some() {
        "Drop another .tns file"
    } else {
        "Drop a .tns file here"
    };
    let mut label = egui::text::LayoutJob {
        halign: egui::Align::Center,
        ..Default::default()
    };
    label.append(
        title,
        0.0,
        egui::TextFormat::simple(egui::FontId::proportional(19.0), FOREGROUND),
    );
    label.append(
        "\nor click to browse",
        0.0,
        egui::TextFormat::simple(egui::FontId::proportional(14.0), MUTED),
    );
    let button = egui::Button::new((egui::Atom::grow(), label, egui::Atom::grow()))
        .fill(if file_hover { LINE } else { SURFACE })
        .stroke(egui::Stroke::new(
            1.0,
            if file_hover { FOREGROUND } else { LINE },
        ))
        .corner_radius(10)
        .min_size(egui::vec2(ui.available_width(), 112.0));
    let clicked = ui
        .add(button)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked();
    if let Some(path) = path {
        let filename = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        ui.add(
            egui::Label::new(RichText::new(format!("Selected: {filename}")).color(MUTED))
                .truncate(),
        )
        .on_hover_text(path.display().to_string());
    }
    clicked
}

fn default_destination(source: &Path, style: MathStyle) -> PathBuf {
    let suffix = match style {
        MathStyle::Lean => "-Lean.pdf",
        MathStyle::Latex if source.with_extension("pdf").exists() => "-LaTeX.pdf",
        MathStyle::Latex => ".pdf",
    };
    let mut filename = source.file_stem().unwrap_or_default().to_os_string();
    filename.push(suffix);
    source.with_file_name(filename)
}

fn file_row(
    ui: &mut egui::Ui,
    label: &str,
    path: Option<&Path>,
    empty: &str,
    button: &str,
) -> bool {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, LINE))
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            let clicked = ui
                .horizontal(|ui| {
                    ui.label(RichText::new(label).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.button(button).clicked()
                    })
                    .inner
                })
                .inner;
            let displayed = path.map(|p| p.display().to_string());
            ui.add(
                egui::Label::new(
                    RichText::new(displayed.as_deref().unwrap_or(empty))
                        .size(13.0)
                        .color(MUTED),
                )
                .truncate(),
            )
            .on_hover_text(displayed.as_deref().unwrap_or(empty));
            clicked
        })
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_selects_the_first_tns_file_in_a_mixed_batch() {
        let mut app = Converter::default();
        let files = ["readme.txt", "notes.TNS", "other.tns"].map(|path| egui::DroppedFile {
            path: Some(path.into()),
            ..Default::default()
        });
        app.accept_drops(&files);
        assert_eq!(app.source.as_deref(), Some(Path::new("notes.TNS")));
        assert!(matches!(app.status, Status::Idle));

        app.accept_drops(&files[..1]);
        assert_eq!(app.source.as_deref(), Some(Path::new("notes.TNS")));
        assert!(matches!(app.status, Status::Failed(_)));
    }
}
