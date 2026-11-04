use anyhow::{Result, bail, ensure};
use nspire_pdf::MathStyle;
use std::{env, path::PathBuf};

const USAGE: &str = "Usage: nspire-pdf [--lean | --latex] INPUT.tns [OUTPUT.pdf]";

fn main() {
    if let Err(error) = run() {
        eprintln!("nspire-pdf: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args_os().skip(1).peekable();
    if args
        .peek()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!(
            "{USAGE}\nSet TECTONIC to override the PDF compiler and LEAN to override the Lean executable."
        );
        return Ok(());
    }
    let style = match args.next_if(|arg| arg == "--lean" || arg == "--latex") {
        Some(arg) if arg == "--lean" => MathStyle::Lean,
        _ => MathStyle::Latex,
    };
    let Some(input) = args.next().map(PathBuf::from) else {
        bail!("{USAGE}")
    };
    let output = args.next().map(PathBuf::from).unwrap_or_else(|| {
        if style == MathStyle::Lean {
            let mut name = input.file_stem().unwrap_or_default().to_os_string();
            name.push("-Lean.pdf");
            input.with_file_name(name)
        } else {
            input.with_extension("pdf")
        }
    });
    ensure!(args.next().is_none(), "{USAGE}");
    let stats = nspire_pdf::convert_with_style(&input, &output, style)?;
    println!(
        "{} ({} Notes widgets, {} math boxes, {} image placements)",
        output.display(),
        stats.notes,
        stats.math,
        stats.images
    );
    Ok(())
}
