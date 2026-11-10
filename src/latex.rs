use crate::archive::Archive;
use crate::{ExportOptions, MathStyle, PdfIntro, lean, markdown};
use anyhow::{Context, Result, bail, ensure};
use roxmltree::{Document, Node};
use std::{collections::BTreeMap, fmt::Write, io::Cursor, path::Path};

const PREAMBLE: &str = r"\documentclass[11pt,a4paper]{article}
\usepackage[margin=23mm,top=24mm,bottom=24mm]{geometry}
\usepackage{amsmath,unicode-math,graphicx,xcolor,needspace,fontawesome5}
\setmathfont{latinmodern-math.otf}
\newfontface\nspiresymbolfont{DejaVuSans.ttf}
\newcommand{\nspiresymbol}[1]{{\nspiresymbolfont #1}}
\definecolor{accent}{HTML}{245166}
\definecolor{rulecolor}{HTML}{BDC9CF}
\setlength{\parindent}{0pt}
\setlength{\parskip}{5pt plus 1pt}
\setlength{\emergencystretch}{2em}
\hyphenpenalty=10000
\setlength{\jot}{5pt}
\newcommand{\problem}[1]{%
  \Needspace{6\baselineskip}%
  \par\addvspace{14pt}%
  {\color{accent}\sffamily\large\bfseries #1\par}%
  \vspace{3pt}{\color{rulecolor}\hrule}\vspace{4pt}}
\newcommand{\mathline}[1]{\par{\raggedright\leftskip=1em #1\par}}
\newsavebox{\notemathbox}
\newcommand{\notemath}[1]{%
  \sbox{\notemathbox}{\(\displaystyle #1\)}%
  \ifdim\wd\notemathbox>\dimexpr\linewidth-1em\relax
    \resizebox{\dimexpr\linewidth-1em\relax}{!}{\usebox{\notemathbox}}%
  \else
    \(\displaystyle #1\)%
  \fi}
\newcommand{\noteimage}[3]{%
  \par\smallskip{\centering\includegraphics[width=#1\linewidth,height=#2\textheight,keepaspectratio]{#3}\par}\smallskip}
\newcommand{\omittedwidget}[1]{\par{\small\color{accent} Interactive widget omitted: #1\par}}
\begin{document}
\color{black}
";

#[derive(Default)]
pub struct Stats {
    pub notes: usize,
    pub math: usize,
    pub images: usize,
    pub skipped_widgets: usize,
}

pub(crate) struct Renderer<'a> {
    work: &'a Path,
    tex: String,
    assets: BTreeMap<String, String>,
    stats: Stats,
    options: ExportOptions,
    expressions: Vec<String>,
}

fn name<'a>(node: Node<'a, '_>) -> &'a str {
    node.attribute("name").unwrap_or("")
}

fn children<'a, 'input>(node: Node<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children().filter(Node::is_element)
}

fn leaf_text(node: Node<'_, '_>) -> String {
    // TI-Nspire can save a cursor element before or inside a text run.
    node.children()
        .filter(Node::is_text)
        .filter_map(|child| child.text())
        .collect()
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum NoteColor {
    #[default]
    Black,
    White,
}

#[derive(Default)]
struct NoteFormat {
    flags: u8,
    color: Option<NoteColor>,
}

type NoteFormats<'a> = BTreeMap<&'a str, NoteFormat>;

fn note_formats<'a>(root: Node<'a, '_>) -> NoteFormats<'a> {
    root.descendants()
        .filter(|node| node.has_tag_name("formatEntry"))
        .filter_map(|node| {
            Some((
                node.attribute("entryID")?,
                NoteFormat {
                    flags: node
                        .attribute("fst")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0),
                    color: node
                        .attribute("tc")
                        .and_then(|s| s.parse::<u32>().ok())
                        .map(|tc| {
                            // TI's palette stores white as 0xfffffe and black as 1.
                            if matches!(tc, 0xfffffe | 0xffffff) {
                                NoteColor::White
                            } else {
                                NoteColor::Black
                            }
                        }),
                },
            ))
        })
        .collect()
}

fn note_color(node: Node<'_, '_>, formats: &NoteFormats<'_>) -> NoteColor {
    node.ancestors()
        .find_map(|node| formats.get(node.attribute("id0")?)?.color)
        .unwrap_or_default()
}

fn colored(body: String, color: NoteColor, parent: NoteColor, style: MathStyle) -> String {
    if body.trim().is_empty() || color == parent {
        return body;
    }
    if style == MathStyle::Lean {
        let start = if color == NoteColor::White {
            lean::WHITE
        } else {
            lean::BLACK
        };
        format!("{start}{body}{}", lean::END_COLOR)
    } else {
        let color = if color == NoteColor::White {
            "white"
        } else {
            "black"
        };
        // TeX groups preserve operator spacing and allow scripts to attach to
        // the preceding atom, unlike wrapping each colored run in math braces.
        format!(r"\begingroup\color{{{color}}}{body}\endgroup ")
    }
}

fn app_panes<'a, 'input>(problem: Node<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    problem
        .descendants()
        .filter(|node| node.has_tag_name("card"))
        .flat_map(children)
        .filter(|node| node.has_tag_name("wdgt"))
}

fn write_document_heading(tex: &mut String, title: &str, style: MathStyle) -> Result<()> {
    writeln!(tex, r"{{\sffamily\huge\bfseries {}}}\par", escape(title))?;
    let label = if style == MathStyle::Lean {
        "Lean notation"
    } else {
        "LaTeX equations"
    };
    writeln!(tex, r"{{\sffamily\small\color{{accent}} {label}}}\par")?;
    Ok(())
}

impl<'a> Renderer<'a> {
    pub(crate) fn new(work: &'a Path, options: ExportOptions) -> Self {
        Self {
            work,
            tex: PREAMBLE.to_owned(),
            assets: BTreeMap::new(),
            stats: Stats::default(),
            options,
            expressions: Vec::new(),
        }
    }

    pub(crate) fn write_intro(&mut self, intro: &PdfIntro<'_>, single_source: bool) -> Result<()> {
        let title = intro.title.replace(lean::SLOT, "");
        if single_source {
            write_document_heading(&mut self.tex, &title, self.options.style)?;
        } else {
            writeln!(
                self.tex,
                r"{{\sffamily\huge\bfseries {}}}\par",
                escape(&title)
            )?;
        }
        markdown::render(intro.markdown, &mut self.tex)?;
        if !single_source {
            self.tex.push_str("\\par\\medskip\n");
        }
        Ok(())
    }

    pub(crate) fn append(&mut self, archive: &Archive<'_>, title: Option<&str>) -> Result<()> {
        let previous_notes = self.stats.notes;
        self.assets.clear();
        let mut tex = String::new();
        if previous_notes > 0 {
            tex.push_str("\\clearpage\n");
        }
        if let Some(title) = title {
            write_document_heading(&mut tex, title, self.options.style)?;
        }
        let mut problems: Vec<_> = archive
            .0
            .iter()
            .filter_map(|(&file, entry)| {
                let number: u32 = file
                    .strip_prefix("Problem")?
                    .strip_suffix(".xml")?
                    .parse()
                    .ok()?;
                Some((number, file, entry))
            })
            .collect();
        problems.sort_unstable_by_key(|(number, _, _)| *number);
        let mut wrote_problem = false;
        for (number, file, entry) in problems {
            let decoded = entry.decode().with_context(|| format!("decoding {file}"))?;
            let xml = Document::parse(str::from_utf8(&decoded)?)
                .with_context(|| format!("parsing {file}"))?;
            let title = xml.root_element().attribute("pbname").unwrap_or("").trim();
            let heading = if title.is_empty() {
                format!("Problem {number}")
            } else {
                format!("Problem {number} / {}", escape(title))
            };
            let mut heading_written = false;
            for widget in app_panes(xml.root_element()) {
                let widget_type = widget.attribute("type").unwrap_or("unknown");
                let supported = widget_type == "TI.Notepad";
                if !supported {
                    self.stats.skipped_widgets += 1;
                    eprintln!("warning: {file}: skipping widget type {widget_type}");
                    if !self.options.show_widget_placeholders {
                        continue;
                    }
                }
                if !heading_written {
                    if wrote_problem {
                        tex.push_str("\\clearpage\n");
                    }
                    writeln!(tex, r"\problem{{{heading}}}")?;
                    heading_written = true;
                    wrote_problem = true;
                }
                if !supported {
                    let label = match widget_type {
                        "TI.ScriptApp" => "TI-Nspire Lua app",
                        "TI.GeoGrapher" => "TI-Nspire Graphs & Geometry",
                        _ => widget_type,
                    };
                    writeln!(tex, r"\omittedwidget{{{}}}", escape(label))?;
                    continue;
                }
                let source = children(widget)
                    .find(|node| node.has_tag_name("fmtxt"))
                    .and_then(|n| n.text())
                    .context("Notes widget has no formatted text")?;
                self.notes(archive, source, &mut tex)
                    .with_context(|| format!("rendering {file}"))?;
            }
        }
        ensure!(
            self.stats.notes > previous_notes,
            "no TI-Nspire Notes widgets found"
        );
        self.tex.push_str(&tex);
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<(String, Stats)> {
        self.tex.push_str("\\end{document}\n");
        if self.options.style == MathStyle::Lean {
            self.tex = lean::format_document(&self.tex, &self.expressions, self.work)?;
        }
        Ok((self.tex, self.stats))
    }

    fn notes(&mut self, archive: &Archive<'_>, source: &str, tex: &mut String) -> Result<()> {
        let xml = Document::parse(source)?;
        let root = xml.root_element();
        let document = children(root)
            .find(|node| name(*node) == "1doc")
            .context("unsupported Notes document structure")?;
        let formats = note_formats(root);
        let fallback_image_height = if document
            .descendants()
            .filter(|n| name(*n) == "1widgetNode")
            .count()
            >= 3
        {
            ".245"
        } else {
            ".30"
        };
        self.stats.notes += 1;
        for para in children(document) {
            ensure!(
                name(para) == "1para",
                "unsupported Notes block {}",
                name(para)
            );
            let mut line = String::new();
            let mut has_words = false;
            let mut math_count = 0;
            // TI wraps long paragraphs into calculator-width rtlines. Reflow them for print.
            for node in children(para).flat_map(children) {
                match name(node) {
                    "1word" => {
                        let word = leaf_text(node);
                        let body = colored(
                            escape_note_word(&word),
                            note_color(node, &formats),
                            NoteColor::Black,
                            MathStyle::Latex,
                        );
                        let style = formats
                            .get(node.attribute("id0").unwrap_or(""))
                            .map(|format| format.flags)
                            .unwrap_or(0);
                        has_words |= !word.trim().is_empty();
                        if !body.trim().is_empty() && style & 1 != 0 {
                            write!(line, r"\textbf{{{body}}}")?;
                        } else if !body.trim().is_empty() && style & 2 != 0 {
                            write!(line, r"\textit{{{body}}}")?;
                        } else {
                            line.push_str(&body);
                        }
                    }
                    "1vector" => {
                        let text = leaf_text(node);
                        let symbol = text.trim();
                        if !symbol.is_empty() {
                            line.push_str(&escape(&text[..text.len() - text.trim_start().len()]));
                            let expression = if self.options.style == MathStyle::Lean {
                                format!("vec {}", lean::operand(&lean::text(symbol, false)))
                            } else {
                                format!(r"\overrightarrow{{{}}}", math_text(symbol, false)?)
                            };
                            self.write_math(
                                &mut line,
                                colored(
                                    expression,
                                    note_color(node, &formats),
                                    NoteColor::Black,
                                    self.options.style,
                                ),
                            )?;
                            line.push_str(&escape(&text[text.trim_end().len()..]));
                            math_count += 1;
                        }
                    }
                    "1widgetNode" => {
                        flush_line(tex, &mut line, has_words, math_count)?;
                        has_words = false;
                        math_count = 0;
                        self.image(archive, node, fallback_image_height, tex)?;
                    }
                    "0el" => {
                        let parts = math_box(node, self.options.style, &formats)?;
                        for (i, part) in parts.into_iter().enumerate() {
                            if i > 0 {
                                line.push_str(r"\allowbreak\quad\(\mapsto\)\nobreak\;");
                            } else if !line.is_empty() && !line.ends_with(char::is_whitespace) {
                                line.push_str(r"\allowbreak\ ");
                            }
                            self.write_math(&mut line, part)?;
                        }
                        math_count += 1;
                        self.stats.math += 1;
                    }
                    other => bail!("unsupported Notes element {other}"),
                }
            }
            flush_line(tex, &mut line, has_words, math_count)?;
        }
        Ok(())
    }

    fn write_math(&mut self, line: &mut String, expression: String) -> Result<()> {
        if self.options.style == MathStyle::Lean {
            self.expressions.push(expression.trim().to_owned());
            line.push(lean::SLOT);
        } else {
            write!(line, r"\notemath{{{expression}}}")?;
        }
        Ok(())
    }

    fn image(
        &mut self,
        archive: &Archive<'_>,
        node: Node<'_, '_>,
        fallback_height: &str,
        tex: &mut String,
    ) -> Result<()> {
        let widget = children(node)
            .find(|n| n.has_tag_name("wdgt"))
            .context("embedded widget is missing its data")?;
        ensure!(
            widget.attribute("type") == Some("Image"),
            "unsupported embedded widget type"
        );
        let name = children(widget)
            .find(|n| n.has_tag_name("name"))
            .and_then(|n| n.text())
            .context("embedded image has no name")?;
        let crop = children(widget).find(|n| n.has_tag_name("crop"));
        let key = format!(
            "{name}:{:?}",
            crop.map(|n| [
                n.attribute("x"),
                n.attribute("y"),
                n.attribute("w"),
                n.attribute("h")
            ])
        );
        if !self.assets.contains_key(&key) {
            let mut bytes = archive
                .0
                .get(name)
                .with_context(|| format!("missing embedded image {name}"))?
                .decode()
                .with_context(|| format!("decoding image {name}"))?;
            normalize_ti_bitmap(&mut bytes);
            let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(12_000);
            limits.max_image_height = Some(12_000);
            reader.limits(limits);
            let mut image = reader
                .decode()
                .with_context(|| format!("reading image {name}"))?;
            if let Some(crop) = crop {
                let ratio = |attr, default| {
                    crop.attribute(attr)
                        .unwrap_or(default)
                        .parse::<f64>()
                        .context("invalid image crop")
                };
                let (x, y, w, h) = (
                    ratio("x", "0")?,
                    ratio("y", "0")?,
                    ratio("w", "1")?,
                    ratio("h", "1")?,
                );
                ensure!(
                    [x, y, w, h].into_iter().all(f64::is_finite)
                        && x >= 0.
                        && y >= 0.
                        && w > 0.
                        && h > 0.,
                    "invalid image crop rectangle"
                );
                let width = f64::from(image.width());
                let height = f64::from(image.height());
                let (sx, sy) = if x + w <= 1.00001 && y + h <= 1.00001 {
                    (width, height)
                } else {
                    (1., 1.)
                };
                let left = (x * sx).round() as u32;
                let top = (y * sy).round() as u32;
                let right = ((x + w) * sx).min(width).round() as u32;
                let bottom = ((y + h) * sy).min(height).round() as u32;
                ensure!(right > left && bottom > top, "empty image crop");
                image = image.crop_imm(left, top, right - left, bottom - top);
            }
            let filename = format!("image-{}.png", self.stats.images + 1);
            sharpen_image(image).save(self.work.join(&filename))?;
            self.assets.insert(key.clone(), filename);
        }
        self.stats.images += 1;
        let (width, height) = note_image_dimensions(widget, fallback_height);
        writeln!(
            tex,
            r"\noteimage{{{width:.6}}}{{{height}}}{{{}}}",
            self.assets[&key]
        )?;
        Ok(())
    }
}

fn note_image_dimensions<'a>(widget: Node<'_, '_>, fallback_height: &'a str) -> (f64, &'a str) {
    const MAX_WIDTH: f64 = 0.94;
    let width_ratio = children(widget)
        .find(|node| node.has_tag_name("ratiow"))
        .and_then(|node| node.text())
        .and_then(|text| text.trim().parse::<f64>().ok())
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0);
    match width_ratio {
        // Map the saved fraction of the Notes pane to the printable image width.
        Some(ratio) => ((ratio.min(1.0) * MAX_WIDTH).max(0.000001), ".90"),
        None => (MAX_WIDTH, fallback_height),
    }
}

fn sharpen_image(image: image::DynamicImage) -> image::DynamicImage {
    // Preserve the source alpha channel and avoid enlarging already large images.
    if image.color().has_alpha() {
        return image;
    }
    let image = if image.width().max(image.height()) <= 1200 {
        image.resize_exact(
            image.width() * 2,
            image.height() * 2,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        image
    };
    image.unsharpen(0.9, 8)
}

fn normalize_ti_bitmap(bytes: &mut [u8]) {
    // Some Nspire BMPs store RGB565 pixels with the masks shifted into the high word.
    if bytes.starts_with(b"BM")
        && bytes.get(14..18) == Some(&52_u32.to_le_bytes())
        && bytes.get(28..34) == Some(&[16, 0, 3, 0, 0, 0])
        && bytes.get(54..66) == Some(&[0, 0, 0, 248, 0, 0, 224, 7, 0, 0, 31, 0])
    {
        bytes[54..66].copy_from_slice(&[0, 248, 0, 0, 224, 7, 0, 0, 31, 0, 0, 0]);
    }
}

fn flush_line(
    tex: &mut String,
    line: &mut String,
    has_words: bool,
    math_count: usize,
) -> Result<()> {
    let content = line.trim();
    if !content.is_empty() {
        if math_count > 0 && !has_words {
            writeln!(tex, r"\mathline{{{content}}}")?;
        } else {
            writeln!(tex, "{content}\\par")?;
        }
    }
    line.clear();
    Ok(())
}

fn math_box(
    node: Node<'_, '_>,
    style: MathStyle,
    formats: &NoteFormats<'_>,
) -> Result<Vec<String>> {
    let color = note_color(node, formats);
    let parts: Vec<_> = children(node)
        .filter(|n| !n.has_tag_name("mbox"))
        .map(|node| {
            Ok(colored(
                math_element_formatted(node, false, style, formats, color)?,
                color,
                NoteColor::Black,
                style,
            ))
        })
        .collect::<Result<_>>()?;
    Ok(parts.into_iter().filter(|s| !s.trim().is_empty()).collect())
}

fn math_element(node: Node<'_, '_>, inside_subscript: bool, style: MathStyle) -> Result<String> {
    math_element_formatted(
        node,
        inside_subscript,
        style,
        &NoteFormats::new(),
        NoteColor::Black,
    )
}

fn math_element_formatted(
    node: Node<'_, '_>,
    inside_subscript: bool,
    style: MathStyle,
    formats: &NoteFormats<'_>,
    parent: NoteColor,
) -> Result<String> {
    let color = formats
        .get(node.attribute("id0").unwrap_or(""))
        .and_then(|f| f.color)
        .unwrap_or(parent);
    let args = |inside_subscript| {
        children(node)
            .map(|child| math_element_formatted(child, inside_subscript, style, formats, color))
            .collect::<Result<Vec<_>>>()
    };
    let kind = name(node);
    let body = match kind {
        "0el" => {
            let mut output = String::new();
            let mut previous_ends_in_unicode_subscript = false;
            for child in children(node) {
                let value = math_element_formatted(child, inside_subscript, style, formats, color)?;
                // A TI subscript template can extend a Unicode subscript already
                // present in the preceding text run (for example Eₖ followed by
                // a particle label). Keep the entire label in one TeX script.
                if style == MathStyle::Latex
                    && !inside_subscript
                    && name(child) == "0mathSubscript"
                    && previous_ends_in_unicode_subscript
                    && output.ends_with('}')
                    && value.starts_with("_{")
                {
                    output.pop();
                    output.push_str(&value[2..]);
                } else {
                    output.push_str(&value);
                }
                if !value.is_empty() {
                    previous_ends_in_unicode_subscript = matches!(name(child), "0text" | "0paren")
                        && leaf_text(child)
                            .chars()
                            .last()
                            .and_then(decode_subscript)
                            .is_some();
                }
            }
            output
        }
        "0text" | "0paren" => {
            let text = leaf_text(node);
            if style == MathStyle::Lean {
                lean::text(&text, inside_subscript)
            } else {
                math_text(&text, inside_subscript)?
            }
        }
        "0fraction" => {
            let values = args(false)?;
            let [top, bottom] = values.as_slice() else {
                bail!("expected two fraction operands");
            };
            let (top, bottom) = (math_slot(top, style), math_slot(bottom, style));
            if style == MathStyle::Lean {
                format!("({} / {})", lean::operand(top), lean::operand(bottom))
            } else {
                format!(r"\frac{{{top}}}{{{bottom}}}")
            }
        }
        "0power" | "0mathSubscript" => {
            let operator = if kind == "0power" { '^' } else { '_' };
            let values = args(operator == '_')?;
            let [value] = values.as_slice() else {
                bail!("expected one operand for {kind}");
            };
            let value = math_slot(value, style);
            if style == MathStyle::Lean {
                if operator == '^' {
                    format!(" ^ {}", lean::operand(value))
                } else {
                    format!("_{value}")
                }
            } else {
                format!("{operator}{{{value}}}")
            }
        }
        "0mline" => {
            let text: String = node
                .descendants()
                .filter(|n| n.has_tag_name("leaf"))
                .map(|leaf| {
                    let text = leaf_text(leaf);
                    let text = if style == MathStyle::Lean {
                        text.replace(char::is_whitespace, " ")
                    } else {
                        escape(&text)
                    };
                    colored(text, note_color(leaf, formats), color, style)
                })
                .collect();
            if style == MathStyle::Lean {
                text
            } else {
                format!(r"\text{{\parbox[t]{{.78\linewidth}}{{\raggedright {text}}}}}")
            }
        }
        other => math_template(node, other, &args(false)?, style)?,
    };
    Ok(colored(body, color, parent, style))
}

fn math_template(
    node: Node<'_, '_>,
    kind: &str,
    args: &[String],
    style: MathStyle,
) -> Result<String> {
    use MathStyle::{Latex, Lean};
    let values: Vec<_> = args.iter().map(|arg| math_slot(arg, style)).collect();
    Ok(match (kind, values.as_slice(), style) {
        ("0sqrt", [x], Latex) => format!(r"\sqrt{{{x}}}"),
        ("0sqrt", [x], Lean) => format!("sqrt ({x})"),
        ("0nroot", [x, index], Latex) => format!(r"\sqrt[{index}]{{{x}}}"),
        ("0nroot", [x, index], Lean) => format!("root ({x}) ({index})"),
        ("0abs", [x], Latex) => format!(r"\left|{x}\right|"),
        ("0abs", [x], Lean) => format!("abs ({x})"),
        ("0logn", [x, base], Latex) => format!(r"\log_{{{base}}}\left({x}\right)"),
        ("0logn", [x, base], Lean) => format!("logBase ({base}) ({x})"),
        ("0list", [x], Latex) => format!(r"\left\{{{x}\right\}}"),
        ("0list", [x], Lean) => format!("[{x}]"),
        ("0deriv", [x, var], Latex) => {
            format!(r"\frac{{\mathrm{{d}}}}{{\mathrm{{d}}{var}}}\left({x}\right)")
        }
        ("0deriv", [x, var], Lean) => format!("derivative ({var}) ({x})"),
        ("0deriv", [x, var, order], Latex) => {
            format!(
                r"\frac{{\mathrm{{d}}^{{{order}}}}}{{\mathrm{{d}}{var}^{{{order}}}}}\left({x}\right)"
            )
        }
        ("0deriv", [x, var, order], Lean) => format!("derivativeN ({order}) ({var}) ({x})"),
        ("0integral", [x, var], Latex) => format!(r"\int {x}\,\mathrm{{d}}{var}"),
        ("0integral", [x, var], Lean) => format!("antiderivative (fun {var} => {x})"),
        ("0integral", [x, var, lower, upper], Latex) => {
            format!(r"\int_{{{lower}}}^{{{upper}}} {x}\,\mathrm{{d}}{var}")
        }
        ("0integral", [x, var, lower, upper], Lean) => {
            format!("integral ({lower}) ({upper}) (fun {var} => {x})")
        }
        ("0sum" | "0product", [x, var, lower, upper], Latex) => {
            let operator = if kind == "0sum" { r"\sum" } else { r"\prod" };
            format!(r"{operator}_{{{var}={lower}}}^{{{upper}}} {x}")
        }
        ("0sum" | "0product", [x, var, lower, upper], Lean) => {
            let operator = if kind == "0sum" { "sum" } else { "product" };
            format!("{operator} ({lower}) ({upper}) (fun {var} => {x})")
        }
        ("0limit", [x, var, point, ..], Latex) if matches!(args.len(), 3 | 4) => {
            // This operand is numeric metadata, not printed equation content.
            let raw_direction = children(node)
                .nth(3)
                .map(|child| math_element(child, false, Latex))
                .transpose()?;
            let direction = raw_direction
                .as_deref()
                .or_else(|| args.get(3).map(String::as_str))
                .unwrap_or("")
                .trim();
            let side = match direction.parse::<f64>().ok() {
                Some(n) if n < 0.0 => "^{-}".to_owned(),
                Some(n) if n > 0.0 => "^{+}".to_owned(),
                Some(_) => String::new(),
                None if direction.is_empty() => String::new(),
                None => format!(r"^{{[{direction}]}}"),
            };
            format!(r"\lim_{{{var}\to {point}{side}}} {x}")
        }
        ("0limit", [x, var, point, ..], Lean) if matches!(args.len(), 3 | 4) => {
            let direction = args.get(3).map_or("", |arg| arg.trim());
            if direction.is_empty() {
                format!("limit ({point}) (fun {var} => {x})")
            } else {
                format!("limitFrom ({direction}) ({point}) (fun {var} => {x})")
            }
        }
        ("0dms", [degrees, minutes, seconds], Latex) => format!(
            r"{{{degrees}}}^{{\circ}}\,{{{minutes}}}^{{\prime}}\,{{{seconds}}}^{{\prime\prime}}"
        ),
        ("0dms", [degrees, minutes, seconds], Lean) => {
            format!("dms ({degrees}) ({minutes}) ({seconds})")
        }
        ("0piecewise", expressions, Latex) if !expressions.is_empty() => {
            let rows: Vec<_> = expressions
                .chunks(2)
                .map(|pair| match pair {
                    [expr, condition] => format!("{expr} & {condition}"),
                    [expr] => format!(r"{expr} & \text{{otherwise}}"),
                    _ => unreachable!(),
                })
                .collect();
            format!(r"\begin{{cases}}{}\end{{cases}}", rows.join(r"\\ "))
        }
        ("0piecewise", expressions, Lean) if !expressions.is_empty() => {
            format!("piecewise ({})", expressions.join(", "))
        }
        ("0system", rows, Latex) if !rows.is_empty() => format!(
            r"\left\{{\begin{{gathered}}{}\end{{gathered}}\right.",
            rows.join(r"\\ ")
        ),
        ("0system", rows, Lean) if !rows.is_empty() => format!("system [{}]", rows.join(", ")),
        ("0matrix", cells, style) => {
            let columns: usize = node
                .attribute("data")
                .context("missing matrix column count")?
                .parse()?;
            ensure!(
                columns > 0 && !cells.is_empty() && cells.len().is_multiple_of(columns),
                "invalid matrix dimensions"
            );
            if style == Latex {
                let rows: Vec<_> = cells.chunks(columns).map(|row| row.join(" & ")).collect();
                format!(r"\begin{{bmatrix}}{}\end{{bmatrix}}", rows.join(r"\\ "))
            } else {
                let rows: Vec<_> = cells
                    .chunks(columns)
                    .map(|row| format!("[{}]", row.join(", ")))
                    .collect();
                format!("[{}]", rows.join(", "))
            }
        }
        _ => bail!(
            "unsupported math element {kind} with {} operands",
            args.len()
        ),
    })
}

fn math_slot(value: &str, style: MathStyle) -> &str {
    if value.trim().is_empty() {
        match style {
            MathStyle::Latex => "□",
            MathStyle::Lean => "_",
        }
    } else {
        value
    }
}

fn escape_note_word(word: &str) -> String {
    // TI escapes literal backslashes in rich-text words before serializing them.
    if word.contains(r"\\") {
        escape(&word.replace(r"\\", r"\"))
    } else {
        escape(word)
    }
}

pub(crate) fn escape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(first) = decode_superscript(ch) {
            let script: String = std::iter::once(first)
                .chain(std::iter::from_fn(|| {
                    chars
                        .next_if(|&c| decode_superscript(c).is_some())
                        .and_then(decode_superscript)
                }))
                .collect();
            out.push_str(r"\ensuremath{{}^{");
            out.push_str(&script);
            out.push_str("}}");
            continue;
        }
        match ch {
            '\\' => out.push_str(r"\textbackslash{}"),
            '\'' => out.push_str(r"\textquotesingle{}"),
            '"' => out.push_str(r"\textquotedbl{}"),
            '`' => out.push_str(r"\char96\relax"),
            '~' => out.push_str(r"\textasciitilde{}"),
            '^' => out.push_str(r"\textasciicircum{}"),
            '⇔' => out.push_str(r"\ensuremath{\Leftrightarrow}"),
            '⇒' => out.push_str(r"\ensuremath{\Rightarrow}"),
            '−' => out.push_str(r"\ensuremath{-}"),
            '\u{f000}' => out.push('E'),
            '\u{2009}' => out.push_str(r"\,"),
            '\u{2005}' => out.push_str(r"\hspace{0.25em}"),
            'ĸ' | 'ŉ' | 'Ŧ' | 'ŧ' | '‣' => {
                out.push_str(r"\nspiresymbol{");
                out.push(ch);
                out.push('}');
            }
            '#' | '$' | '%' | '&' | '_' | '{' | '}' => {
                out.push('\\');
                out.push(ch);
            }
            ch if decode_subscript(ch).is_some() => {
                out.push_str(r"\ensuremath{{}_{");
                out.extend(decode_subscript(ch));
                out.push_str("}}");
            }
            ch => match math_symbol(ch) {
                Some(symbol) => {
                    out.push_str(r"\ensuremath{");
                    out.push_str(symbol);
                    out.push('}');
                }
                None if is_math_symbol(ch) => {
                    out.push_str(r"\ensuremath{");
                    out.push(ch);
                    out.push('}');
                }
                None => out.push(ch),
            },
        }
    }
    out
}

fn math_symbol(ch: char) -> Option<&'static str> {
    Some(match ch {
        // TI's palette uses private-use characters for these glyphs.
        '\u{f001}' => r"{}^{\mathrm{r}}",
        '\u{f002}' => r"{}^{\mathsf{T}}",
        '\u{f003}' => r"\bar{x}",
        '\u{f004}' | 'ȳ' => r"\bar{y}",
        '\u{f005}' => r"{}^{-1}",
        '\u{f008}' => r"\mathrm{d}",
        '\u{f021}' => r"\mathsf{F}",
        '\u{f02f}' | 'ⅈ' => r"\mathrm{i}",
        '\u{f03f}' | 'ℯ' => r"\mathrm{e}",
        '\u{f040}' => r"{}^{\mathrm{g}}",
        '\u{f041}' => "c",
        '\u{f042}' => "n",
        '\u{f044}' => r"\hat{p}",
        '\u{f045}' => "10",
        '\u{f112}' => r"\text{\nspiresymbol{↳}}",
        '\u{f100}' => r"\text{\faIcon{wrench}}",
        '\u{f101}' => r"\text{\faIcon{mouse-pointer}}",
        '\u{f102}' => r"\text{\faIcon{hand-paper}}",
        '\u{f103}' => r"\text{\nspiresymbol{⚐}}",
        '\u{f104}' => r"\text{\faIcon{book-open}}",
        '\u{f105}' => r"\text{\nspiresymbol{◑▸}}",
        '\u{f106}' => r"\text{\nspiresymbol{◂◐}}",
        'ŷ' => r"\hat{y}",
        'µ' => r"\mu ",
        '∆' => r"\Delta ",
        '√' => r"\text{\nspiresymbol{√}}",
        '⇥' => r"\mathrel{\text{\nspiresymbol{⇥}}}",
        '◿' => r"\text{\nspiresymbol{◿}}",
        '▴' => r"\text{\nspiresymbol{▴}}",
        '▸' => r"\text{\nspiresymbol{▸}}",
        '▾' => r"\text{\nspiresymbol{▾}}",
        '◂' => r"\text{\nspiresymbol{◂}}",
        '◆' => r"\text{\nspiresymbol{◆}}",
        '►' => r"\text{\nspiresymbol{►}}",
        '☺' => r"\text{\nspiresymbol{☺}}",
        '★' => r"\text{\nspiresymbol{★}}",
        '⫽' => r"\mathbin{/\mkern-4mu/}",
        '¼' => r"\tfrac{1}{4}",
        '½' => r"\tfrac{1}{2}",
        '¾' => r"\tfrac{3}{4}",
        _ => return None,
    })
}

fn is_math_symbol(ch: char) -> bool {
    matches!(
        ch,
        'Α'..='Ρ' | 'Σ'..='Ω' | 'α'..='ω' | 'ϑ' | 'ϕ' | 'ϖ' | 'ϱ' | 'ϵ'
            | 'ℂ' | 'ℎ' | 'ℏ' | 'ℕ' | 'ℙ' | 'ℚ' | 'ℝ' | 'ℤ'
            | '¬' | '±' | '′' | '″'
            | '\u{2190}'..='\u{22ff}'
            | '▪' | '▫' | '▲' | '△' | '▶' | '▷' | '▼' | '◀' | '✓' | '□'
    )
}

fn math_text(text: &str, inside_subscript: bool) -> Result<String> {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(subscript) = decode_subscript(ch) {
            if !inside_subscript {
                out.push_str("_{");
            }
            out.push(subscript);
            while let Some(next) = chars.next_if(|c| decode_subscript(*c).is_some()) {
                out.extend(decode_subscript(next));
            }
            if !inside_subscript {
                out.push('}');
            }
            continue;
        }
        if let Some(first) = decode_superscript(ch) {
            out.push_str("^{");
            out.push(first);
            while let Some(next) = chars.next_if(|&c| decode_superscript(c).is_some()) {
                out.extend(decode_superscript(next));
            }
            out.push('}');
            continue;
        }
        match ch {
            '*' if chars.peek() == Some(&'_') && {
                let token: String = chars
                    .clone()
                    .skip(1)
                    .take_while(|&ch| unit_character(ch))
                    .collect();
                display_unit(&token).is_some()
            } =>
            {
                out.push_str(r"\,")
            }
            '*' | '·' | '•' => out.push_str(r"\cdot "),
            '×' => out.push_str(r"\times "),
            '÷' => out.push_str(r"\div "),
            '\'' => {
                out.push_str(r"{}^{\prime");
                while chars.next_if_eq(&'\'').is_some() {
                    out.push_str(r"\prime");
                }
                out.push('}');
            }
            '−' => out.push('-'),
            '▶' | '►' | '▸' => out.push_str(r"\mathbin{\triangleright}"),
            '\u{f000}' => {
                let sign = chars.next_if(|c| matches!(c, '+' | '-' | '−'));
                let exponent: String =
                    std::iter::from_fn(|| chars.next_if(char::is_ascii_digit)).collect();
                if exponent.is_empty() {
                    out.push_str(r"\mathrm{E}");
                    out.extend(sign.map(|c| if c == '−' { '-' } else { c }));
                } else {
                    let sign = sign.map_or("", |c| if c == '+' { "+" } else { "-" });
                    write!(out, r"\times 10^{{{sign}{exponent}}}")?;
                }
            }
            '\u{f005}' => out.push_str("^{-1}"),
            'λ' => out.push_str(r"\lambda "),
            'μ' => out.push_str(r"\mu "),
            'ρ' => out.push_str(r"\rho "),
            'π' => out.push_str(r"\pi "),
            'θ' => out.push_str(r"\theta "),
            '∞' => out.push_str(r"\infty "),
            '≤' => out.push_str(r"\leq "),
            '≥' => out.push_str(r"\geq "),
            '≠' => out.push_str(r"\ne "),
            '≈' => out.push_str(r"\approx "),
            '⇔' => out.push_str(r"\Leftrightarrow "),
            '°' => out.push_str(r"^{\circ}"),
            '_' if chars.next_if_eq(&'°').is_some() => {
                let unit: String =
                    std::iter::from_fn(|| chars.next_if(char::is_ascii_alphabetic)).collect();
                write!(out, r"\,{{}}^{{\circ}}\mathrm{{{unit}}}")?;
            }
            '_' if chars
                .peek()
                .is_some_and(|&c| unit_character(c) && !c.is_ascii_digit()) =>
            {
                let token: String =
                    std::iter::from_fn(|| chars.next_if(|&c| unit_character(c))).collect();
                if out.ends_with(|ch: char| ch.is_ascii_digit()) {
                    out.push_str(r"\,");
                }
                let token = display_unit(&token).unwrap_or(&token);
                if token.is_ascii() {
                    write!(out, r"\mathrm{{{token}}}")?;
                } else {
                    // U+00B5 and Å are not available in the math font. Use
                    // Greek mu and the text font's ringed A for these units.
                    let token = token.replace('µ', "μ").replace('Å', r"\text{\AA}");
                    write!(out, r"\symup{{{token}}}")?;
                }
            }
            'a'..='z' | 'A'..='Z' => {
                let mut word: String = std::iter::once(ch)
                    .chain(std::iter::from_fn(|| {
                        chars.next_if(char::is_ascii_alphanumeric)
                    }))
                    .collect();
                if let Some(mark) = chars.next_if(|c| matches!(c, '\u{0302}' | '\u{0304}')) {
                    let base = word.pop().expect("nonempty ASCII word");
                    let accent = if mark == '\u{0302}' { "hat" } else { "bar" };
                    write!(out, r"{word}\{accent}{{{base}}}")?;
                    continue;
                }
                match word.as_str() {
                    "ln" | "log" | "sin" | "cos" | "tan" | "exp" => write!(out, "\\{word} ")?,
                    "solve" => out.push_str(r"\operatorname{solve}"),
                    "and" | "or" | "xor" | "not" => write!(out, r"\quad\text{{{word}}}\quad")?,
                    "nm" | "pm" | "keV" | "eV" => write!(out, r"\mathrm{{{word}}}")?,
                    _ if word.len() > 1
                        && chars.clone().find(|c| !c.is_whitespace()) == Some('(') =>
                    {
                        write!(out, r"\operatorname{{{word}}}")?;
                    }
                    _ => out.push_str(&word),
                }
            }
            ch if ch.is_whitespace() => out.push_str(r"\,"),
            '0'..='9'
            | '+'
            | '-'
            | '='
            | '/'
            | '.'
            | ','
            | ':'
            | ';'
            | '!'
            | '?'
            | '('
            | ')'
            | '['
            | ']'
            | '<'
            | '>'
            | '|' => out.push(ch),
            ch => match math_symbol(ch) {
                Some(symbol) => out.push_str(symbol),
                None if is_math_symbol(ch) => out.push(ch),
                None => write!(out, r"\text{{{}}}", escape(&ch.to_string()))?,
            },
        }
    }
    Ok(out)
}

pub(crate) fn decode_subscript(ch: char) -> Option<char> {
    match ch {
        '₀'..='₉' => char::from_u32(ch as u32 - '₀' as u32 + '0' as u32),
        '\u{f841}'..='\u{f85a}' => char::from_u32(ch as u32 - 0xf841 + 'A' as u32),
        '\u{f861}'..='\u{f87a}' => char::from_u32(ch as u32 - 0xf861 + 'a' as u32),
        '\u{f891}'..='\u{f8a1}' | '\u{f8a3}'..='\u{f8a9}' | '\u{f8b1}'..='\u{f8c9}' => {
            char::from_u32(ch as u32 - 0xf500)
        }
        '₊' => Some('+'),
        '₋' => Some('-'),
        '₍' => Some('('),
        '₎' => Some(')'),
        '\u{f043}' => Some('n'),
        '\u{f046}' => Some('L'),
        '\u{f047}' => Some('b'),
        _ => None,
    }
}

fn decode_superscript(ch: char) -> Option<char> {
    match ch {
        '⁰' => Some('0'),
        '¹' => Some('1'),
        '²' => Some('2'),
        '³' => Some('3'),
        '⁴'..='⁹' => char::from_u32(ch as u32 - '⁴' as u32 + '4' as u32),
        '⁺' => Some('+'),
        '⁻' => Some('-'),
        '⁼' => Some('='),
        '⁽' => Some('('),
        '⁾' => Some(')'),
        'ⁱ' => Some('i'),
        'ⁿ' => Some('n'),
        _ => None,
    }
}

pub(crate) fn unit_character(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, 'µ' | 'μ' | 'ε' | 'σ' | 'φ' | 'ϕ' | 'Å')
}

pub(crate) fn display_unit(token: &str) -> Option<&str> {
    match token {
        "gm" => Some("g"),
        "kev" => Some("keV"),
        "m" | "cm" | "mm" | "dm" | "nm" | "pm" | "km" | "µm" | "μm" | "Å" | "in" | "ft" | "yd"
        | "mi" | "kg" | "mg" | "µg" | "μg" | "lb" | "oz" | "s" | "ms" | "min" | "hr" | "day"
        | "L" | "mL" | "mol" | "A" | "Hz" | "J" | "K" | "N" | "Pa" | "V" | "W" | "eV" | "keV"
        | "MeV" => Some(token),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_cursor_does_not_drop_notes_text_or_a_power_base() {
        let source = r#"<r2dtotree><node name="1doc"><node name="1para"><node name="1rtline">
            <leaf name="1word">Res<cursor index="3"/>ult: </leaf>
            <node name="0el"><node name="0el">
                <leaf name="0text"><cursor index="0"/>2.5*10</leaf>
                <node name="0power"><node name="0el"><leaf name="0text">6</leaf></node></node>
                <leaf name="0text">_eV</leaf>
            </node></node>
        </node></node></node></r2dtotree>"#;
        let work = tempfile::tempdir().unwrap();
        let mut renderer = Renderer::new(work.path(), ExportOptions::default());
        let mut tex = String::new();
        renderer
            .notes(&Archive(BTreeMap::new()), source, &mut tex)
            .unwrap();
        assert!(tex.contains(r"Result: \notemath{2.5\cdot 10^{6}\mathrm{eV}}"));
    }

    const COLOR_FORMATS: &str = r#"<formatManager>
        <formatEntry entryID="black" tc="1" fst="0"/>
        <formatEntry entryID="white" tc="16777214" fst="1"/>
        <formatEntry entryID="red" tc="16711680" fst="2"/>
        <formatEntry entryID="plain" fst="0"/>
    </formatManager>"#;

    #[test]
    fn fraction_color_is_independent_of_its_operands() {
        let source = format!(
            r#"<r2dtotree>{COLOR_FORMATS}
            <node name="0el" id0="white"><mbox/>
              <node name="0el"><node name="0fraction">
                <node name="0el"><leaf name="0text" id0="black">1</leaf></node>
                <node name="0el" id0="red"><leaf name="0text">2</leaf></node>
              </node></node>
            </node></r2dtotree>"#
        );
        let xml = Document::parse(&source).unwrap();
        let root = xml.root_element();
        let formats = note_formats(root);
        let equation = children(root).find(|n| name(*n) == "0el").unwrap();
        let parts = math_box(equation, MathStyle::Latex, &formats).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(
            parts[0],
            r"\begingroup\color{white}\frac{\begingroup\color{black}1\endgroup }{\begingroup\color{black}2\endgroup }\endgroup "
        );

        let lean = math_box(equation, MathStyle::Lean, &formats).unwrap();
        assert_eq!(
            lean[0],
            format!(
                "{}({}1{} / {}2{}){}",
                lean::WHITE,
                lean::BLACK,
                lean::END_COLOR,
                lean::BLACK,
                lean::END_COLOR,
                lean::END_COLOR
            )
        );
    }

    #[test]
    fn prose_and_vectors_keep_white_without_leaking_to_the_next_run() {
        let source = format!(
            r#"<r2dtotree>{COLOR_FORMATS}
            <node name="1doc"><node name="1para"><node name="1rtline">
                <leaf name="1word" id0="white">white &amp; </leaf>
                <leaf name="1word" id0="red">red </leaf>
                <leaf name="1word">default </leaf>
                <leaf name="1vector" id0="white">v</leaf>
            </node></node></node></r2dtotree>"#
        );
        let work = tempfile::tempdir().unwrap();
        for style in [MathStyle::Latex, MathStyle::Lean] {
            let mut renderer = Renderer::new(
                work.path(),
                ExportOptions {
                    style,
                    ..ExportOptions::default()
                },
            );
            let mut tex = String::new();
            renderer
                .notes(&Archive(BTreeMap::new()), &source, &mut tex)
                .unwrap();
            assert!(tex.contains(r"\textbf{\begingroup\color{white}white \& \endgroup }"));
            assert!(tex.contains(r"\textit{red }default "));
            assert!(!tex.contains("16711680"));
            if style == MathStyle::Latex {
                assert!(
                    tex.contains(
                        r"\notemath{\begingroup\color{white}\overrightarrow{v}\endgroup }"
                    )
                );
            } else {
                assert!(renderer.expressions[0].starts_with(lean::WHITE));
            }
        }
    }

    #[test]
    fn colors_inherit_and_every_non_white_color_becomes_black() {
        let source = format!(
            r#"<root>{COLOR_FORMATS}<node id0="white">
            <leaf id0="plain"/><leaf id0="unknown"/><leaf id0="red"/>
        </node></root>"#
        );
        let xml = Document::parse(&source).unwrap();
        let formats = note_formats(xml.root_element());
        let colors: Vec<_> = xml
            .descendants()
            .filter(|n| n.has_tag_name("leaf"))
            .map(|n| note_color(n, &formats) == NoteColor::White)
            .collect();
        assert_eq!(colors, [true, true, false]);
        for tc in [
            "0", "1", "255", "8421504", "16711680", "16777213", "invalid",
        ] {
            let source =
                format!(r#"<root><formatEntry entryID="c" tc="{tc}"/><leaf id0="c"/></root>"#);
            let xml = Document::parse(&source).unwrap();
            assert!(
                note_color(
                    xml.descendants().find(|n| n.has_tag_name("leaf")).unwrap(),
                    &note_formats(xml.root_element())
                ) == NoteColor::Black
            );
        }
    }

    #[test]
    fn colored_limit_direction_remains_numeric_metadata() {
        let source = format!(
            r#"<root>{COLOR_FORMATS}<node name="0limit">
            <node name="0el"><leaf name="0text">f(x)</leaf></node>
            <node name="0el"><leaf name="0text">x</leaf></node>
            <node name="0el"><leaf name="0text">0</leaf></node>
            <node name="0el" id0="white"><leaf name="0text">-1</leaf></node>
        </node></root>"#
        );
        let xml = Document::parse(&source).unwrap();
        let root = xml.root_element();
        let limit = children(root).find(|n| name(*n) == "0limit").unwrap();
        let rendered = math_element_formatted(
            limit,
            false,
            MathStyle::Latex,
            &note_formats(root),
            NoteColor::Black,
        )
        .unwrap();
        assert_eq!(rendered, r"\lim_{x\to 0^{-}} f(x)");
    }

    #[test]
    fn image_size_uses_the_saved_pane_width_with_a_legacy_fallback() {
        for (xml, expected) in [
            ("<wdgt><ratiow>0.5</ratiow></wdgt>", (0.47, ".90")),
            ("<wdgt><ratiow>1.25</ratiow></wdgt>", (0.94, ".90")),
            ("<wdgt><ratiow>NaN</ratiow></wdgt>", (0.94, ".245")),
            ("<wdgt/>", (0.94, ".245")),
        ] {
            let widget = Document::parse(xml).unwrap();
            assert_eq!(
                note_image_dimensions(widget.root_element(), ".245"),
                expected
            );
        }
    }

    #[test]
    fn nested_graph_controls_are_not_app_panes() {
        let xml = Document::parse(
            r#"<prob xmlns="urn:TI.Problem"><card>
                <wdgt type="TI.Notepad"/>
                <wdgt type="TI.GeoGrapher"><wdgt type="EditingLine"/></wdgt>
            </card></prob>"#,
        )
        .unwrap();
        let types: Vec<_> = app_panes(xml.root_element())
            .filter_map(|pane| pane.attribute("type"))
            .collect();
        assert_eq!(types, ["TI.Notepad", "TI.GeoGrapher"]);
    }

    #[test]
    fn renders_ti_scientific_notation_as_a_power() {
        assert_eq!(
            math_text("1-7.504516\u{f000}−6", false).unwrap(),
            r"1-7.504516\times 10^{-6}"
        );
    }

    #[test]
    fn distinguishes_units_from_constants() {
        assert_eq!(
            math_text("_c*_h=20.4*_cm", false).unwrap(),
            r"\mathrm{c}\cdot \mathrm{h}=20.4\,\mathrm{cm}"
        );
    }

    #[test]
    fn preserves_micro_and_angstrom_units_and_digit_grouping_spaces() {
        assert_eq!(
            math_text("5*_µm+2*_Å", false).unwrap(),
            r"5\,\symup{μm}+2\,\symup{\text{\AA}}"
        );
        assert_eq!(escape("123\u{2005}456"), r"123\hspace{0.25em}456");
    }

    #[test]
    fn renders_palette_symbols_in_notes_and_equations() {
        assert_eq!(
            escape("\u{f02f}\u{f002} α∈ℝ x⁻¹ \u{f043}"),
            r"\ensuremath{\mathrm{i}}\ensuremath{{}^{\mathsf{T}}} \ensuremath{α}\ensuremath{∈}\ensuremath{ℝ} x\ensuremath{{}^{-1}} \ensuremath{{}_{n}}"
        );
        assert_eq!(
            math_text("A\u{f002}x\u{f8c9}²+p\u{0302}", false).unwrap(),
            r"A{}^{\mathsf{T}}x_{ω}^{2}+\hat{p}"
        );
    }

    #[test]
    fn renders_catalog_names_and_unicode_constants() {
        assert_eq!(
            math_text("ANOVA2way(x)+5*_μm+_ε0", false).unwrap(),
            r"\operatorname{ANOVA2way}(x)+5\,\symup{μm}+\symup{ε0}"
        );
    }

    #[test]
    fn unescapes_backslashes_in_ti_notes_words() {
        assert_eq!(
            escape_note_word(r"\\ \\\\"),
            r"\textbackslash{} \textbackslash{}\textbackslash{}"
        );
    }

    #[test]
    fn subscript_node_accepts_already_subscripted_digits() {
        let xml = Document::parse(r#"<node name="0mathSubscript"><node name="0el"><leaf name="0text">₀</leaf></node></node>"#).unwrap();
        assert_eq!(
            math_element(xml.root_element(), false, MathStyle::Latex).unwrap(),
            "_{0}"
        );
        assert_eq!(
            math_element(xml.root_element(), false, MathStyle::Lean).unwrap(),
            "_0"
        );
    }

    #[test]
    fn subscript_template_extends_a_unicode_subscript() {
        let xml = Document::parse(r#"<node name="0el"><leaf name="0text">E</leaf><node name="0mathSubscript"><node name="0el"><leaf name="0text">Th₂₂₈</leaf></node></node><leaf name="0text">+x</leaf><node name="0mathSubscript"><node name="0el"><leaf name="0text">1</leaf></node></node></node>"#).unwrap();
        assert_eq!(
            math_element(xml.root_element(), false, MathStyle::Latex).unwrap(),
            "E_{kTh228}+x_{1}"
        );
    }

    #[test]
    fn matrix_column_count_controls_row_breaks() {
        let xml = Document::parse(r#"<node name="0matrix" data="2"/>"#).unwrap();
        let cells = ["a", "b", "c", "d"].map(str::to_owned);
        assert_eq!(
            math_template(xml.root_element(), "0matrix", &cells, MathStyle::Latex).unwrap(),
            r"\begin{bmatrix}a & b\\ c & d\end{bmatrix}"
        );
        assert_eq!(
            math_template(xml.root_element(), "0matrix", &cells, MathStyle::Lean).unwrap(),
            "[[a, b], [c, d]]"
        );
    }

    #[test]
    fn calculus_templates_keep_ti_operand_order() {
        let xml = Document::parse("<node/>").unwrap();
        let operands = ["f(x)", "x", "0", "1"].map(str::to_owned);
        assert_eq!(
            math_template(xml.root_element(), "0integral", &operands, MathStyle::Latex).unwrap(),
            r"\int_{0}^{1} f(x)\,\mathrm{d}x"
        );
        assert_eq!(
            math_template(xml.root_element(), "0sum", &operands, MathStyle::Latex).unwrap(),
            r"\sum_{x=0}^{1} f(x)"
        );
    }

    #[test]
    fn limits_and_blank_indefinite_integrals_keep_their_slots() {
        let xml = Document::parse("<node/>").unwrap();
        let limit = ["f(x)", "x", "0", "-1"].map(str::to_owned);
        assert_eq!(
            math_template(xml.root_element(), "0limit", &limit, MathStyle::Latex).unwrap(),
            r"\lim_{x\to 0^{-}} f(x)"
        );
        let integral = ["", "x"].map(str::to_owned);
        assert_eq!(
            math_template(xml.root_element(), "0integral", &integral, MathStyle::Latex).unwrap(),
            r"\int □\,\mathrm{d}x"
        );
    }

    #[test]
    fn catalog_conversion_keeps_operator_and_constant() {
        assert_eq!(
            math_text("12•_m►_ft+_c", false).unwrap(),
            r"12\cdot \mathrm{m}\mathbin{\triangleright}\mathrm{ft}+\mathrm{c}"
        );
    }
}
