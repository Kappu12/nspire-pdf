use crate::archive::Archive;
use crate::{ExportOptions, MathStyle, lean};
use anyhow::{Context, Result, bail, ensure};
use roxmltree::{Document, Node};
use std::{collections::BTreeMap, fmt::Write, io::Cursor, path::Path};

const PREAMBLE: &str = r"\documentclass[11pt,a4paper]{article}
\usepackage[margin=23mm,top=24mm,bottom=24mm]{geometry}
\usepackage{amsmath,unicode-math,graphicx,xcolor,needspace}
\setmathfont{latinmodern-math.otf}
\definecolor{ink}{HTML}{101720}
\definecolor{accent}{HTML}{245166}
\definecolor{rulecolor}{HTML}{BDC9CF}
\setlength{\parindent}{0pt}
\setlength{\parskip}{5pt plus 1pt}
\setlength{\emergencystretch}{2em}
\setlength{\jot}{5pt}
\newcommand{\problem}[1]{%
  \Needspace{6\baselineskip}%
  \par\addvspace{14pt}%
  {\color{accent}\sffamily\large\bfseries #1\par}%
  \vspace{3pt}{\color{rulecolor}\hrule}\vspace{4pt}}
\newcommand{\mathline}[1]{\par{\raggedright\leftskip=1em #1\par}}
\newcommand{\noteimage}[2]{%
  \par\smallskip{\centering\includegraphics[width=.94\linewidth,height=#1\textheight,keepaspectratio]{#2}\par}\smallskip}
\newcommand{\omittedwidget}[1]{\par{\small\color{accent} Interactive widget omitted: #1\par}}
\begin{document}
\color{ink}
";

#[derive(Default)]
pub struct Stats {
    pub notes: usize,
    pub math: usize,
    pub images: usize,
    pub skipped_widgets: usize,
}

struct Renderer<'a> {
    archive: &'a Archive<'a>,
    work: &'a Path,
    assets: BTreeMap<String, String>,
    stats: Stats,
    style: MathStyle,
    expressions: Vec<String>,
}

fn name<'a>(node: Node<'a, '_>) -> &'a str {
    node.attribute("name").unwrap_or("")
}

fn children<'a, 'input>(node: Node<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children().filter(Node::is_element)
}

pub fn render(
    archive: &Archive<'_>,
    title: &str,
    work: &Path,
    options: ExportOptions,
) -> Result<(String, Stats)> {
    let style = options.style;
    let mut renderer = Renderer {
        archive,
        work,
        assets: BTreeMap::new(),
        stats: Stats::default(),
        style,
        expressions: Vec::new(),
    };
    let mut tex = PREAMBLE.to_owned();
    writeln!(tex, r"{{\sffamily\huge\bfseries {}}}\par", escape(title))?;
    let label = if style == MathStyle::Lean {
        "Lean notation"
    } else {
        "LaTeX equations"
    };
    writeln!(tex, r"{{\sffamily\small\color{{accent}} {label}}}\par")?;
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
        for widget in xml.descendants().filter(|n| n.has_tag_name("wdgt")) {
            let widget_type = widget.attribute("type").unwrap_or("unknown");
            let supported = widget_type == "TI.Notepad";
            if !supported {
                renderer.stats.skipped_widgets += 1;
                eprintln!("warning: {file}: skipping widget type {widget_type}");
                if !options.show_widget_placeholders {
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
                let label = if widget_type == "TI.ScriptApp" {
                    "TI-Nspire Lua app"
                } else {
                    widget_type
                };
                writeln!(tex, r"\omittedwidget{{{}}}", escape(label))?;
                continue;
            }
            let source = children(widget)
                .find(|node| node.has_tag_name("fmtxt"))
                .and_then(|n| n.text())
                .context("Notes widget has no formatted text")?;
            renderer
                .notes(source, &mut tex)
                .with_context(|| format!("rendering {file}"))?;
        }
    }
    ensure!(renderer.stats.notes > 0, "no TI-Nspire Notes widgets found");
    tex.push_str("\\end{document}\n");
    if style == MathStyle::Lean {
        tex = lean::format_document(&tex, &renderer.expressions, work)?;
    }
    Ok((tex, renderer.stats))
}

impl Renderer<'_> {
    fn notes(&mut self, source: &str, tex: &mut String) -> Result<()> {
        let xml = Document::parse(source)?;
        let root = xml.root_element();
        let document = children(root)
            .find(|node| name(*node) == "1doc")
            .context("unsupported Notes document structure")?;
        let formats: BTreeMap<_, _> = root
            .descendants()
            .filter(|n| n.has_tag_name("formatEntry"))
            .filter_map(|n| {
                Some((
                    n.attribute("entryID")?,
                    n.attribute("fst")?.parse::<u8>().ok()?,
                ))
            })
            .collect();
        let image_height = if document
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
                        let word = node.text().unwrap_or("");
                        let body = escape(word);
                        let style = formats
                            .get(node.attribute("id0").unwrap_or(""))
                            .copied()
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
                        let text = node.text().unwrap_or("");
                        let symbol = text.trim();
                        if !symbol.is_empty() {
                            line.push_str(&escape(&text[..text.len() - text.trim_start().len()]));
                            let expression = if self.style == MathStyle::Lean {
                                format!("vec {}", lean::operand(&lean::text(symbol, false)))
                            } else {
                                format!(r"\overrightarrow{{{}}}", math_text(symbol, false)?)
                            };
                            self.write_math(&mut line, expression)?;
                            line.push_str(&escape(&text[text.trim_end().len()..]));
                            math_count += 1;
                        }
                    }
                    "1widgetNode" => {
                        flush_line(tex, &mut line, has_words, math_count)?;
                        has_words = false;
                        math_count = 0;
                        self.image(node, image_height, tex)?;
                    }
                    "0el" => {
                        let parts = math_box(node, self.style)?;
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
        if self.style == MathStyle::Lean {
            self.expressions.push(expression.trim().to_owned());
            line.push(lean::SLOT);
        } else {
            write!(line, r"\(\displaystyle {expression}\)")?;
        }
        Ok(())
    }

    fn image(&mut self, node: Node<'_, '_>, height: &str, tex: &mut String) -> Result<()> {
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
            let mut bytes = self
                .archive
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
            let filename = format!("image-{}.png", self.assets.len() + 1);
            sharpen_image(image).save(self.work.join(&filename))?;
            self.assets.insert(key.clone(), filename);
        }
        self.stats.images += 1;
        writeln!(tex, r"\noteimage{{{height}}}{{{}}}", self.assets[&key])?;
        Ok(())
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

fn math_box(node: Node<'_, '_>, style: MathStyle) -> Result<Vec<String>> {
    let parts: Vec<_> = children(node)
        .filter(|n| !n.has_tag_name("mbox"))
        .map(|node| math_element(node, false, style))
        .collect::<Result<_>>()?;
    Ok(parts.into_iter().filter(|s| !s.trim().is_empty()).collect())
}

fn math_element(node: Node<'_, '_>, inside_subscript: bool, style: MathStyle) -> Result<String> {
    let args = |inside_subscript| {
        children(node)
            .map(|child| math_element(child, inside_subscript, style))
            .collect::<Result<Vec<_>>>()
    };
    let kind = name(node);
    Ok(match kind {
        "0el" => args(inside_subscript)?.join(""),
        "0text" | "0paren" => {
            let text = node.text().unwrap_or("");
            if style == MathStyle::Lean {
                lean::text(text, inside_subscript)
            } else {
                math_text(text, inside_subscript)?
            }
        }
        "0fraction" => {
            let values = args(false)?;
            let [top, bottom] = values.as_slice() else {
                bail!("expected two fraction operands");
            };
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
                .filter_map(|n| n.text())
                .collect();
            if style == MathStyle::Lean {
                text.replace(char::is_whitespace, " ")
            } else {
                format!(
                    r"\text{{\parbox[t]{{.78\linewidth}}{{\raggedright {}}}}}",
                    escape(&text)
                )
            }
        }
        other => math_template(node, other, &args(false)?, style)?,
    })
}

fn math_template(
    node: Node<'_, '_>,
    kind: &str,
    args: &[String],
    style: MathStyle,
) -> Result<String> {
    use MathStyle::{Latex, Lean};
    Ok(match (kind, args, style) {
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
        ("0integral", [x, var, lower, upper], Latex) => {
            format!(r"\int_{{{lower}}}^{{{upper}}} {x}\,\mathrm{{d}}{var}")
        }
        ("0integral", [x, var, lower, upper], Lean) => {
            format!("integral ({lower}) ({upper}) (fun {var} => {x})")
        }
        ("0sum", [x, var, lower, upper], Latex) => {
            format!(r"\sum_{{{var}={lower}}}^{{{upper}}} {x}")
        }
        ("0sum", [x, var, lower, upper], Lean) => {
            format!("sum ({lower}) ({upper}) (fun {var} => {x})")
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

pub(crate) fn escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str(r"\textbackslash{}"),
            '~' => out.push_str(r"\textasciitilde{}"),
            '^' => out.push_str(r"\textasciicircum{}"),
            '⇔' => out.push_str(r"\ensuremath{\Leftrightarrow}"),
            '⇒' => out.push_str(r"\ensuremath{\Rightarrow}"),
            '−' => out.push_str(r"\ensuremath{-}"),
            '#' | '$' | '%' | '&' | '_' | '{' | '}' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
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
        match ch {
            '*' if chars.peek() == Some(&'_') && {
                let token: String = chars
                    .clone()
                    .skip(1)
                    .take_while(char::is_ascii_alphabetic)
                    .collect();
                display_unit(&token).is_some()
            } =>
            {
                out.push_str(r"\,")
            }
            '*' | '·' => out.push_str(r"\cdot "),
            '×' => out.push_str(r"\times "),
            '−' => out.push('-'),
            '▶' => out.push_str(r"\triangleright "),
            '\u{f03f}' => out.push_str(r"\mathrm{e}"),
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
            '\u{f041}' => out.push('c'),
            '\u{f042}' => out.push('n'),
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
            'µ' => out.push_str(r"\mu "),
            '∆' => out.push_str(r"\Delta "),
            '°' => out.push_str(r"^{\circ}"),
            'Α'..='Ω'
            | 'α'..='ω'
            | '±'
            | '∈'
            | '∉'
            | 'ℤ'
            | 'ℝ'
            | '∑'
            | '→'
            | '⇒'
            | '∥'
            | '⊥'
            | '²'
            | '⁺'
            | '⁻' => out.push(ch),
            '_' if chars.peek().is_some_and(char::is_ascii_alphabetic) => {
                let token: String =
                    std::iter::from_fn(|| chars.next_if(char::is_ascii_alphabetic)).collect();
                if out.ends_with(|ch: char| ch.is_ascii_digit()) {
                    out.push_str(r"\,");
                }
                let token = display_unit(&token).unwrap_or(&token);
                write!(out, r"\mathrm{{{token}}}")?;
            }
            'a'..='z' | 'A'..='Z' => {
                let word: String = std::iter::once(ch)
                    .chain(std::iter::from_fn(|| {
                        chars.next_if(char::is_ascii_alphabetic)
                    }))
                    .collect();
                match word.as_str() {
                    "ln" | "log" | "sin" | "cos" | "tan" | "exp" => write!(out, "\\{word} ")?,
                    "solve" => out.push_str(r"\operatorname{solve}"),
                    "and" | "or" => write!(out, r"\quad\text{{{word}}}\quad")?,
                    "nm" | "pm" | "keV" | "eV" => write!(out, r"\mathrm{{{word}}}")?,
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
            ch => write!(out, r"\text{{{}}}", escape(&ch.to_string()))?,
        }
    }
    Ok(out)
}

pub(crate) fn decode_subscript(ch: char) -> Option<char> {
    match ch {
        '₀'..='₉' => char::from_u32(ch as u32 - '₀' as u32 + '0' as u32),
        '\u{f841}'..='\u{f85a}' => char::from_u32(ch as u32 - 0xf841 + 'A' as u32),
        '\u{f861}'..='\u{f87a}' => char::from_u32(ch as u32 - 0xf861 + 'a' as u32),
        '\u{f8b1}' => Some('α'),
        '\u{f8b2}' => Some('β'),
        '\u{f8bc}' => Some('μ'),
        '₊' => Some('+'),
        '₋' => Some('-'),
        _ => None,
    }
}

pub(crate) fn display_unit(token: &str) -> Option<&str> {
    match token {
        "gm" => Some("g"),
        "kev" => Some("keV"),
        "m" | "cm" | "mm" | "nm" | "pm" | "km" | "kg" | "eV" | "keV" | "MeV" => Some(token),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
