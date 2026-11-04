use anyhow::{Result, bail, ensure};
use std::{env, path::PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("nspire-pdf: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let Some(input) = args.next() else {
        bail!("usage: nspire-pdf INPUT.tns [OUTPUT.pdf]");
    };
    if input == "--help" || input == "-h" {
        println!(
            "Usage: nspire-pdf INPUT.tns [OUTPUT.pdf]\nSet TECTONIC to override the LaTeX compiler executable."
        );
        return Ok(());
    }
    let input = PathBuf::from(input);
    let output = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| input.with_extension("pdf"));
    ensure!(
        args.next().is_none(),
        "usage: nspire-pdf INPUT.tns [OUTPUT.pdf]"
    );
    let stats = nspire_pdf::convert(&input, &output)?;
    println!(
        "{} ({} Notes widgets, {} math boxes, {} image placements)",
        output.display(),
        stats.notes,
        stats.math,
        stats.images
    );
    Ok(())
}
