mod archive;
mod latex;
mod tixc;

use anyhow::{Context, Result, bail, ensure};
use archive::Archive;
use std::{env, fs, io, path::PathBuf, process::Command};

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
    if input
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tnsp"))
    {
        bail!("PublishView .tnsp is a separate format. Convert it to .tns in TI-Nspire first.");
    }
    let output = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| input.with_extension("pdf"));
    ensure!(
        args.next().is_none(),
        "usage: nspire-pdf INPUT.tns [OUTPUT.pdf]"
    );
    ensure!(
        output
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf")),
        "output must have a .pdf extension"
    );
    let output_dir = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    ensure!(
        output_dir.is_dir(),
        "output directory does not exist: {}",
        output_dir.display()
    );
    ensure!(
        !output.exists(),
        "output already exists: {}",
        output.display()
    );

    let source = fs::read(&input).with_context(|| format!("reading {}", input.display()))?;
    ensure!(
        source.len() <= archive::LIMIT,
        "input exceeds the size limit"
    );
    let archive = Archive::parse(&source)?;
    let work = tempfile::tempdir()?;
    let title = input
        .file_stem()
        .context("input needs a file name")?
        .to_string_lossy();
    let (tex, stats) = latex::render(&archive, &title, work.path())?;
    fs::write(work.path().join("document.tex"), tex)?;

    let compiler = env::var_os("TECTONIC").unwrap_or_else(|| "tectonic".into());
    let status = Command::new(compiler)
        .current_dir(work.path())
        .args(["-X", "compile", "--untrusted", "document.tex"])
        .status()
        .context("starting Tectonic. Install it on PATH or set TECTONIC to its absolute path")?;
    ensure!(status.success(), "LaTeX compilation failed ({status})");

    let mut pdf = fs::File::open(work.path().join("document.pdf"))?;
    let mut staging = tempfile::NamedTempFile::new_in(output_dir)?;
    io::copy(&mut pdf, staging.as_file_mut())?;
    staging.persist_noclobber(&output)?;
    println!(
        "{} ({} Notes widgets, {} math boxes, {} image placements)",
        output.display(),
        stats.notes,
        stats.math,
        stats.images
    );
    Ok(())
}
