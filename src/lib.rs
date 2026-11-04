mod archive;
mod latex;
mod lean;
mod markdown;
mod tixc;

pub use latex::Stats;

use anyhow::{Context, Result, bail, ensure};
use archive::Archive;
use std::{
    env, fs,
    io::{self, Read},
    path::Path,
    process::Command,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum MathStyle {
    #[default]
    Latex,
    Lean,
}

#[derive(Clone, Copy, Default)]
pub struct ExportOptions {
    pub style: MathStyle,
    pub show_widget_placeholders: bool,
}

#[derive(Clone, Copy)]
pub enum OutputPolicy {
    CreateNew,
    ReplaceExisting,
}

pub struct PdfIntro<'a> {
    pub title: &'a str,
    pub markdown: &'a str,
}

#[derive(Clone, Copy)]
pub enum ConversionStage {
    Reading,
    Typesetting,
    Saving,
}

pub fn convert(input: &Path, output: &Path) -> Result<Stats> {
    convert_with_style(input, output, MathStyle::Latex)
}

pub fn convert_with_style(input: &Path, output: &Path, style: MathStyle) -> Result<Stats> {
    convert_with_progress(input, output, style, |_| {})
}

pub fn convert_with_progress(
    input: &Path,
    output: &Path,
    style: MathStyle,
    report: impl FnMut(ConversionStage),
) -> Result<Stats> {
    convert_with_options(
        input,
        output,
        ExportOptions {
            style,
            ..ExportOptions::default()
        },
        report,
    )
}

pub fn convert_with_options(
    input: &Path,
    output: &Path,
    options: ExportOptions,
    report: impl FnMut(ConversionStage),
) -> Result<Stats> {
    convert_files_with_options(&[input], output, options, report)
}

pub fn convert_files_with_options(
    inputs: &[impl AsRef<Path>],
    output: &Path,
    options: ExportOptions,
    report: impl FnMut(ConversionStage),
) -> Result<Stats> {
    convert_files_with_output_policy(inputs, output, options, OutputPolicy::CreateNew, report)
}

pub fn convert_files_with_output_policy(
    inputs: &[impl AsRef<Path>],
    output: &Path,
    options: ExportOptions,
    output_policy: OutputPolicy,
    report: impl FnMut(ConversionStage),
) -> Result<Stats> {
    convert_files_with_intro(inputs, output, options, output_policy, None, report)
}

pub fn convert_files_with_intro(
    inputs: &[impl AsRef<Path>],
    output: &Path,
    options: ExportOptions,
    output_policy: OutputPolicy,
    intro: Option<PdfIntro<'_>>,
    mut report: impl FnMut(ConversionStage),
) -> Result<Stats> {
    ensure!(!inputs.is_empty(), "choose at least one TI-Nspire file");
    for input in inputs {
        if input
            .as_ref()
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("tnsp"))
        {
            bail!("PublishView .tnsp is a separate format. Convert it to .tns in TI-Nspire first.");
        }
    }
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
        !matches!(output_policy, OutputPolicy::CreateNew) || !output.exists(),
        "output already exists: {}",
        output.display()
    );

    report(ConversionStage::Reading);
    let work = tempfile::tempdir()?;
    let mut renderer = latex::Renderer::new(work.path(), options);
    if let Some(intro) = &intro {
        renderer.write_intro(intro, inputs.len() == 1)?;
    }
    for input in inputs {
        let input = input.as_ref();
        let mut append = || {
            let source = read_input(input)?;
            let archive = Archive::parse(&source)?;
            let title = input
                .file_stem()
                .context("input needs a file name")?
                .to_string_lossy();
            let heading = intro.is_none().then_some(title.as_ref());
            renderer.append(&archive, heading)
        };
        if inputs.len() > 1 {
            append().with_context(|| format!("converting {}", input.display()))?;
        } else {
            append()?;
        }
    }
    let (tex, stats) = renderer.finish()?;
    fs::write(work.path().join("document.tex"), tex)?;

    report(ConversionStage::Typesetting);
    compile(work.path())?;

    report(ConversionStage::Saving);
    let pdf = fs::File::open(work.path().join("document.pdf"))?;
    save_pdf(pdf, output, output_dir, output_policy)?;
    Ok(stats)
}

fn save_pdf(
    mut pdf: impl Read,
    output: &Path,
    output_dir: &Path,
    output_policy: OutputPolicy,
) -> Result<()> {
    let mut staging = tempfile::NamedTempFile::new_in(output_dir)?;
    io::copy(&mut pdf, staging.as_file_mut())?;
    match output_policy {
        OutputPolicy::CreateNew => staging.persist_noclobber(output),
        OutputPolicy::ReplaceExisting => staging.persist(output),
    }
    .with_context(|| format!("saving {}", output.display()))?;
    Ok(())
}

fn read_input(input: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(input)
        .with_context(|| format!("opening {}", input.display()))?
        .take(archive::LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", input.display()))?;
    ensure!(
        bytes.len() <= archive::LIMIT,
        "input exceeds the size limit"
    );
    Ok(bytes)
}

fn compile(work: &Path) -> Result<()> {
    let compiler = match env::var_os("TECTONIC") {
        Some(program) => program,
        None => default_compiler(work)?,
    };
    let mut command = Command::new(compiler);
    command
        .current_dir(work)
        .args(["-X", "compile", "--untrusted", "document.tex"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let result = command.output().context(
        "starting Tectonic. Put it next to the app, install it on PATH, or set TECTONIC",
    )?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let stdout = String::from_utf8_lossy(&result.stdout);
        let details: Vec<_> = stderr
            .lines()
            .chain(stdout.lines())
            .filter(|line| line.starts_with("error:"))
            .collect();
        let detail = if details.is_empty() {
            stderr.lines().next_back().unwrap_or("").to_owned()
        } else {
            details.join("\n")
        };
        bail!("LaTeX compilation failed ({}). {}", result.status, detail);
    }
    Ok(())
}

#[cfg(embedded_tectonic)]
fn default_compiler(work: &Path) -> Result<std::ffi::OsString> {
    let executable = work.join("tectonic.exe");
    fs::write(
        &executable,
        include_bytes!(env!("NSPIRE_PDF_TECTONIC_PATH")),
    )
    .context("extracting the built-in Tectonic compiler")?;
    Ok(executable.into_os_string())
}

#[cfg(not(embedded_tectonic))]
fn default_compiler(_work: &Path) -> Result<std::ffi::OsString> {
    Ok(env::current_exe()
        .ok()
        .and_then(|exe| {
            let sibling = exe.with_file_name(if cfg!(windows) {
                "tectonic.exe"
            } else {
                "tectonic"
            });
            sibling.is_file().then(|| sibling.into_os_string())
        })
        .unwrap_or_else(|| "tectonic".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmed_overwrite_replaces_only_after_the_new_pdf_is_copied() {
        struct BrokenReader;

        impl Read for BrokenReader {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("simulated read failure"))
            }
        }

        let folder = tempfile::tempdir().unwrap();
        let output = folder.path().join("notes.pdf");
        fs::write(&output, b"original PDF").unwrap();
        assert!(
            save_pdf(
                &b"new PDF"[..],
                &output,
                folder.path(),
                OutputPolicy::CreateNew
            )
            .is_err()
        );
        let partial = (&b"partial PDF"[..]).chain(BrokenReader);
        assert!(
            save_pdf(
                partial,
                &output,
                folder.path(),
                OutputPolicy::ReplaceExisting
            )
            .is_err()
        );
        assert_eq!(fs::read(&output).unwrap(), b"original PDF");

        save_pdf(
            &b"new PDF"[..],
            &output,
            folder.path(),
            OutputPolicy::ReplaceExisting,
        )
        .unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"new PDF");
    }
}
