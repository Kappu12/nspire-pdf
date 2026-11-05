use anyhow::{Context, Result, ensure};
use std::{env, fs, path::Path, process::Command};

const FORMATTER: &str = include_str!("format_notes.lean");
pub const SLOT: char = '\u{1f}';
// Noncharacters carry nested colors through Lean's fragment formatter without
// mixing TeX commands into the notation. They are removed before typesetting.
pub(crate) const BLACK: char = '\u{fdd0}';
pub(crate) const WHITE: char = '\u{fdd1}';
pub(crate) const END_COLOR: char = '\u{fdd2}';

fn escape_colored(expression: &str) -> String {
    let mut output = String::new();
    let mut start = 0;
    for (i, ch) in expression.char_indices() {
        let command = match ch {
            BLACK => r"{\color{black}",
            WHITE => r"{\color{white}",
            END_COLOR => "}",
            _ => continue,
        };
        output.push_str(&crate::latex::escape(&expression[start..i]));
        output.push_str(command);
        start = i + ch.len_utf8();
    }
    output.push_str(&crate::latex::escape(&expression[start..]));
    output
}

pub fn format_document(tex: &str, expressions: &[String], work: &Path) -> Result<String> {
    if expressions.is_empty() {
        return Ok(tex.to_owned());
    }
    fs::write(work.join("format_notes.lean"), FORMATTER)?;
    fs::write(work.join("lean-input.txt"), expressions.join("\n"))?;
    let program = env::var_os("LEAN")
        .or_else(|| {
            let exe = env::current_exe().ok()?;
            let sibling = exe.parent()?.join("lean/bin").join(if cfg!(windows) {
                "lean.exe"
            } else {
                "lean"
            });
            sibling.is_file().then(|| sibling.into_os_string())
        })
        .unwrap_or_else(|| "lean".into());
    let mut command = Command::new(program);
    command.current_dir(work).arg("format_notes.lean");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let result = command.output().context(
        "starting Lean 4. Install it on PATH or set LEAN to its absolute executable path",
    )?;
    ensure!(
        result.status.success(),
        "Lean formatting failed: {}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let formatted = fs::read_to_string(work.join("lean-output.txt"))?;
    let mut expressions = formatted.lines();
    let mut output = String::with_capacity(tex.len());
    let mut parts = tex.split(SLOT);
    output.push_str(parts.next().unwrap_or_default());
    for part in parts {
        let expression = expressions.next().context("missing output from Lean")?;
        output.push_str(r"\texttt{");
        output.push_str(&escape_colored(expression));
        output.push('}');
        output.push_str(part);
    }
    ensure!(expressions.next().is_none(), "extra output from Lean");
    Ok(output)
}

pub fn text(input: &str, in_subscript: bool) -> String {
    let mut result = String::new();
    let mut chars = input.chars().peekable();
    let mut previous = None;
    while let Some(ch) = chars.next() {
        if let Some(subscript) = crate::latex::decode_subscript(ch) {
            if !in_subscript && previous.and_then(crate::latex::decode_subscript).is_none() {
                result.push('_');
            }
            result.push(subscript);
            previous = Some(ch);
            continue;
        }
        match ch {
            '−' | '-' => result.push_str(
                if previous.is_none_or(|p: char| !p.is_alphanumeric() && p != ')') {
                    "-"
                } else {
                    " - "
                },
            ),
            '+' | '=' | '*' | '/' | '<' | '>' | '≤' | '≥' | '≠' | '≈' | '⇔' => {
                result.extend([' ', ch, ' ']);
            }
            '·' | '×' | '•' => result.push_str(" * "),
            '÷' => result.push_str(" / "),
            '▶' | '►' | '▸' => result.push_str(" ▷ "),
            '\u{f03f}' => result.push('e'),
            '\u{f005}' => result.push_str(" ^ (-1)"),
            '\u{f041}' => result.push('c'),
            '\u{f042}' => result.push('n'),
            '\u{f000}' => {
                let sign = chars.next_if(|c| matches!(c, '+' | '-' | '−'));
                let exponent: String =
                    std::iter::from_fn(|| chars.next_if(char::is_ascii_digit)).collect();
                if exponent.is_empty() {
                    result.push('E');
                    result.extend(sign.map(|c| if c == '−' { '-' } else { c }));
                } else {
                    let sign = sign.map_or("", |c| if c == '+' { "+" } else { "-" });
                    result.push_str(&format!("e{sign}{exponent}"));
                }
            }
            '_' if chars
                .peek()
                .is_some_and(|&c| crate::latex::unit_character(c) && !c.is_ascii_digit()) =>
            {
                let name: String =
                    std::iter::from_fn(|| chars.next_if(|&c| crate::latex::unit_character(c)))
                        .collect();
                if previous.is_some_and(|c: char| c.is_ascii_digit()) {
                    result.push_str(" * ");
                }
                result.push_str(crate::latex::display_unit(&name).unwrap_or(&name));
            }
            ',' => result.push_str(", "),
            c if c.is_whitespace() => result.push(' '),
            _ => result.push(ch),
        }
        previous = Some(ch);
    }
    result.replace(" and ", " ∧ ").replace(" or ", " ∨ ")
}

pub fn operand(value: &str) -> String {
    let value = value.trim();
    let plain: String = value
        .chars()
        .filter(|ch| !matches!(*ch, BLACK | WHITE | END_COLOR))
        .collect();
    let is_atom = plain
        .chars()
        .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '.' | '%'));
    let mut depth = 0;
    let is_group = plain.starts_with('(')
        && plain.char_indices().all(|(i, ch)| {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            depth > 0 || (depth == 0 && i + 1 == plain.len())
        })
        && depth == 0;
    if is_atom || is_group {
        value.to_owned()
    } else {
        format!("({value})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colored_operands_keep_their_grouping_and_escape_literal_tex() {
        let value = format!("{WHITE}({BLACK}x_1{END_COLOR} / 2){END_COLOR}");
        assert_eq!(operand(&value), value);
        assert_eq!(
            escape_colored(&value),
            r"{\color{white}({\color{black}x\_1} / 2)}"
        );
        let atom = format!("{BLACK}12{END_COLOR}");
        assert_eq!(operand(&atom), atom);
    }

    #[test]
    fn grouping_keeps_nested_operations_in_order() {
        assert_eq!(operand("I_0"), "I_0");
        assert_eq!(operand("((a + b) / c)"), "((a + b) / c)");
        assert_eq!(operand("(a) * b"), "((a) * b)");
        assert_eq!(operand("1 / (x + 2)"), "(1 / (x + 2))");
    }
}
