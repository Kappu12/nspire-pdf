# nspire-pdf

Convert TI-Nspire Notes documents to printable A4 PDFs. The Rust CLI reads the
archive, decodes TI method 13 entries, and translates Notes into LaTeX. Tectonic
compiles the LaTeX with selectable text and vector math. The converter sharpens
small embedded screenshots for print.

## Desktop app

Build the desktop app with:

    cargo build --release --features gui

Launch nspire-pdf-gui from target/release. Pick LaTeX or Lean for the math,
choose or drop a .tns file, and select Convert to PDF. Open PDF opens the
result in your default viewer.

The app looks for Tectonic next to its executable or on PATH. The TECTONIC
environment variable takes precedence.

## Command line

Install Rust and [Tectonic](https://tectonic-typesetting.github.io/). Put
Tectonic on PATH or set the TECTONIC environment variable to the absolute path
of its executable. Tectonic may download its LaTeX bundle on first use.

    cargo run --release -- "path/to/notes.tns" "path/to/notes.pdf"
    cargo run --release -- --lean "path/to/notes.tns" "path/to/notes-Lean.pdf"

Omit the output argument to write beside the input. LaTeX is the default and
writes .pdf, while Lean writes -Lean.pdf.
The CLI refuses to overwrite an existing file. To build a standalone CLI
executable, run cargo build --release. Tectonic is still required at runtime.

LaTeX uses stacked fractions and conventional equation typography. Lean uses
Lean 4 to format linear expressions and calculator fragments as code-style
math. Both modes use Tectonic for page layout, and both preserve the saved
calculator results. Lean mode formats syntax without evaluating or proving it.
Install Lean 4 on PATH or set LEAN to the absolute path of the Lean executable.

## Release builds

Push a new version tag to publish the desktop and CLI packages:

    git tag -a v0.1.0 -m "Release v0.1.0"
    git push origin v0.1.0

The workflow publishes Windows x64, Linux x64, and macOS packages for Apple
Silicon and Intel. Tags such as v0.1.0-rc.1 publish as prereleases. Packages
go directly to GitHub Releases.

Install Tectonic separately before converting a file. On macOS, the app bundle
is ad-hoc signed and lacks notarization. If Tectonic is available only in your
terminal PATH, launch the app from the extracted folder with:

    TECTONIC="$(command -v tectonic)" "./Nspire PDF.app/Contents/MacOS/nspire-pdf-gui"

## Supported content

- TI-Nspire .tns containers with stored, deflated, or TI method 13 entries.
- Notes prose, fractions, powers, subscripts, common math symbols, and
  calculator results saved inside Notes math boxes.
- Embedded BMP, PNG, and JPEG images. The converter reflows Notes for print
  and keeps embedded screenshots as images.
- Other TI-Nspire app widgets are skipped with a warning. Unsupported Notes
  structures produce an error. The converter uses the saved results and
  does not evaluate the original calculations.
- PublishView .tnsp is a separate format. Use
  [TI-Nspire's conversion workflow](https://education.ti.com/en/customer-support/knowledge-base/ti-nspire-family/product-usage/32332)
  to save it as .tns first. PublishView-specific objects may be dropped by
  that conversion.

The method 13 and TIXC implementation follows
[TnsTools](https://github.com/MaksimirKurtov/TnsTools). See LICENSE-TnsTools
for its MIT notice.
