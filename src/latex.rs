use crate::archive::Archive;
use anyhow::{Context, Result, bail, ensure};
use roxmltree::{Document, Node};
use std::{collections::BTreeMap, fmt::Write, io::Cursor, path::Path};

const PREAMBLE: &str = r"\documentclass[11pt,a4paper]{article}
\usepackage[margin=23mm,top=24mm,bottom=24mm]{geometry}
\usepackage{fontspec,amsmath,graphicx,xcolor,needspace}
\definecolor{ink}{HTML}{182432}
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
\begin{document}
\color{ink}
";

#[derive(Default)]
pub struct Stats {
    pub notes: usize,
    pub math: usize,
    pub images: usize,
}

struct Renderer<'a> {
    archive: &'a Archive<'a>,
    work: &'a Path,
    assets: BTreeMap<String, String>,
    stats: Stats,
}

fn name<'a>(node: Node<'a, '_>) -> &'a str {
    node.attribute("name").unwrap_or("")
}

fn children<'a, 'input>(node: Node<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children().filter(Node::is_element)
}

pub fn render(archive: &Archive<'_>, title: &str, work: &Path) -> Result<(String, Stats)> {
    let mut renderer = Renderer {
        archive,
        work,
        assets: BTreeMap::new(),
        stats: Stats::default(),
    };
    let mut tex = PREAMBLE.to_owned();
    writeln!(tex, r"{{\sffamily\huge\bfseries {}}}\par", escape(title))?;
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
        let notes_before = renderer.stats.notes;
        for widget in xml.descendants().filter(|n| n.has_tag_name("wdgt")) {
            let widget_type = widget.attribute("type").unwrap_or("unknown");
            if widget_type != "TI.Notepad" {
                eprintln!("warning: {file}: skipping widget type {widget_type}");
                continue;
            }
            if renderer.stats.notes == notes_before {
                if notes_before > 0 {
                    tex.push_str("\\clearpage\n");
                }
                writeln!(tex, r"\problem{{{heading}}}")?;
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
                    "1widgetNode" => {
                        flush_line(tex, &mut line, has_words, math_count)?;
                        has_words = false;
                        math_count = 0;
                        self.image(node, image_height, tex)?;
                    }
                    "0el" => {
                        let parts = math_box(node)?;
                        for (i, part) in parts.iter().enumerate() {
                            if i > 0 {
                                line.push_str(r"\allowbreak\quad\(\mapsto\)\nobreak\;");
                            } else if !line.is_empty() && !line.ends_with(char::is_whitespace) {
                                line.push_str(r"\allowbreak\ ");
                            }
                            write!(line, r"\(\displaystyle {part}\)")?;
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
            image.save(self.work.join(&filename))?;
            self.assets.insert(key.clone(), filename);
        }
        self.stats.images += 1;
        writeln!(tex, r"\noteimage{{{height}}}{{{}}}", self.assets[&key])?;
        Ok(())
    }
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

fn math_box(node: Node<'_, '_>) -> Result<Vec<String>> {
    let parts: Vec<_> = children(node)
        .filter(|n| !n.has_tag_name("mbox"))
        .map(math_node)
        .collect::<Result<_>>()?;
    Ok(parts.into_iter().filter(|s| !s.is_empty()).collect())
}

fn math_node(node: Node<'_, '_>) -> Result<String> {
    math_element(node, false)
}

fn math_element(node: Node<'_, '_>, inside_subscript: bool) -> Result<String> {
    let args = |inside_subscript| {
        children(node)
            .map(|child| math_element(child, inside_subscript))
            .collect::<Result<Vec<_>>>()
    };
    let kind = name(node);
    Ok(match kind {
        "0el" => args(inside_subscript)?.join(""),
        "0text" | "0paren" => math_text(node.text().unwrap_or(""), inside_subscript)?,
        "0fraction" => {
            let values = args(false)?;
            let [top, bottom] = values.as_slice() else {
                bail!("expected two fraction operands");
            };
            format!(r"\frac{{{top}}}{{{bottom}}}")
        }
        "0power" | "0mathSubscript" => {
            let operator = if kind == "0power" { '^' } else { '_' };
            let values = args(operator == '_')?;
            let [value] = values.as_slice() else {
                bail!("expected one operand for {kind}");
            };
            format!("{operator}{{{value}}}")
        }
        other => bail!("unsupported math element {other}"),
    })
}

fn escape(text: &str) -> String {
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
            '\u{f86d}' => out.push_str(r"_{\mathrm{m}}"),
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
            '₀'..='₉' => {
                let digit = u32::from(ch) - u32::from('₀');
                if inside_subscript {
                    write!(out, "{digit}")?;
                } else {
                    let rest: String =
                        std::iter::from_fn(|| chars.next_if(|c| ('₀'..='₉').contains(c)))
                            .map(|c| (b'0' + (u32::from(c) - u32::from('₀')) as u8) as char)
                            .collect();
                    write!(out, "_{{{digit}{rest}}}")?;
                }
            }
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

fn display_unit(token: &str) -> Option<&str> {
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
        assert_eq!(math_node(xml.root_element()).unwrap(), "_{0}");
    }
}
