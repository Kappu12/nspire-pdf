use crate::{latex::escape, lean};
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use std::fmt::{self, Write};

pub(crate) fn render(source: &str, tex: &mut String) -> fmt::Result {
    let source = source.replace(lean::SLOT, "");
    let mut lists = Vec::new();
    let mut links = Vec::new();
    let mut code = None::<String>;
    for event in Parser::new(&source) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let size = match level {
                    HeadingLevel::H1 => r"\Large",
                    HeadingLevel::H2 => r"\large",
                    _ => r"\normalsize",
                };
                write!(tex, r"\par\medskip{{\sffamily\bfseries{size} ")?;
            }
            Event::End(TagEnd::Heading(_)) => tex.push_str("}\\par\n"),
            Event::End(TagEnd::Paragraph) => tex.push_str("\\par\n"),
            Event::Start(Tag::Emphasis) => tex.push_str(r"\textit{"),
            Event::Start(Tag::Strong) => tex.push_str(r"\textbf{"),
            Event::End(TagEnd::Emphasis | TagEnd::Strong) => tex.push('}'),
            Event::Start(Tag::BlockQuote(_)) => {
                tex.push_str(r"\par\begingroup\advance\leftskip by 1em\itshape ");
            }
            Event::End(TagEnd::BlockQuote(_)) => tex.push_str("\\par\\endgroup\n"),
            Event::Start(Tag::List(start)) => lists.push(start),
            Event::End(TagEnd::List(_)) => {
                lists.pop();
                tex.push('\n');
            }
            Event::Start(Tag::Item) => {
                tex.push_str(r"\par\begingroup\advance\leftskip by 1.25em\noindent ");
                if let Some(Some(number)) = lists.last_mut() {
                    write!(tex, r"{number}.\enspace ")?;
                    *number = number.saturating_add(1);
                } else {
                    tex.push_str(r"\textbullet\enspace ");
                }
            }
            Event::End(TagEnd::Item) => tex.push_str("\\par\\endgroup\n"),
            Event::Start(Tag::Link { dest_url, .. }) => links.push(dest_url),
            Event::End(TagEnd::Link) => {
                if let Some(url) = links.pop() {
                    write!(tex, " ({})", escape(&url))?;
                }
            }
            Event::Start(Tag::CodeBlock(_)) => code = Some(String::new()),
            Event::End(TagEnd::CodeBlock) => {
                tex.push_str("\\par\\smallskip\\begingroup\\ttfamily\\raggedright\n");
                for line in code.take().unwrap_or_default().lines() {
                    writeln!(tex, r"\noindent\strut {}\par", monospace(line))?;
                }
                tex.push_str("\\endgroup\\smallskip\n");
            }
            Event::Text(text) => match &mut code {
                Some(code) => code.push_str(&text),
                None => tex.push_str(&escape(&text)),
            },
            Event::Code(text) => write!(tex, r"\texttt{{{}}}", monospace(&text))?,
            Event::Html(html) | Event::InlineHtml(html) => tex.push_str(&escape(&html)),
            Event::SoftBreak => tex.push(' '),
            Event::HardBreak => tex.push_str("\\newline\n"),
            Event::Rule => tex.push_str("\\par\\smallskip\\hrule\\smallskip\n"),
            _ => {}
        }
    }
    Ok(())
}

fn monospace(text: &str) -> String {
    text.replace('\t', "    ")
        .split(' ')
        .map(escape)
        .collect::<Vec<_>>()
        .join(r"\ ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_markdown_with_literal_tex_special_characters() {
        let mut tex = String::new();
        render(
            "# Review\n\n3. **Use** `x_2` & *compare*.\n\n> Keep 100%.",
            &mut tex,
        )
        .unwrap();
        assert!(tex.contains(r"\Large Review"));
        assert!(tex.contains(r"3.\enspace \textbf{Use} \texttt{x\_2} \& \textit{compare}."));
        assert!(tex.contains(r"\itshape Keep 100\%."));
    }
}
