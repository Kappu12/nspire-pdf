# nspire-pdf

Convert TI-Nspire Notes documents to printable A4 PDFs. The Rust CLI reads the
archive, decodes TI method 13 entries, and translates Notes into LaTeX. Tectonic
compiles the LaTeX with selectable text and vector math. The converter sharpens
small embedded screenshots for print.

## Install

The Cargo install commands compile the app and CLI on your machine. For those
builds, install Rust 1.95 or newer and [Tectonic](https://tectonic-typesetting.github.io/).
Put Tectonic on PATH or set the TECTONIC environment variable to its absolute
path.

### Windows (PowerShell)

Download the prebuilt Windows x64 app from the
[v0.1.5 release](https://github.com/Kappu12/nspire-pdf/releases/tag/v0.1.5).
The EXE includes Tectonic. On first use, Tectonic may download its TeX resources.

To build from source with the MSVC Rust toolchain, install Visual Studio Build
Tools with the Desktop development with C++ workload.

    cargo install --locked --git https://github.com/Kappu12/nspire-pdf --tag v0.1.5 --features gui
    nspire-pdf-gui.exe

To build an EXE with Tectonic included, set NSPIRE_PDF_EMBED_TECTONIC to the
absolute path to a Tectonic executable for the target Windows architecture,
then run cargo build --locked --release --features gui.

### macOS

Install the Xcode Command Line Tools if needed.

    xcode-select --install
    cargo install --locked --git https://github.com/Kappu12/nspire-pdf --tag v0.1.5 --features gui
    nspire-pdf-gui

### Linux (Debian or Ubuntu)

Install the desktop build dependencies:

    sudo apt-get install build-essential pkg-config libgl1-mesa-dev libwayland-dev libxkbcommon-dev
    cargo install --locked --git https://github.com/Kappu12/nspire-pdf --tag v0.1.5 --features gui
    nspire-pdf-gui

In the app, choose or drop one or more .tns files. Drag the rows or use the
arrows to set the PDF order. Edit the output filename and PDF title, and add
optional Markdown notes at the start of the PDF. Select Convert to PDF to
choose where to save it.

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

## Supported content

- TI-Nspire .tns containers with stored, deflated, or TI method 13 entries.
- Notes prose and math, including fractions, roots, powers, subscripts,
  vectors, matrices, equation systems, piecewise expressions, calculus
  templates, units, conversion operators, and saved calculator results.
- White and black font colors, including separately colored equation parts.
  Other font colors are converted to black for print.
- Embedded BMP, PNG, and JPEG images. The converter reflows Notes for print
  and keeps embedded screenshots as images.
- Interactive TI-Nspire widgets, including Lua apps, are skipped by default.
  The desktop checkbox can mark their locations in the PDF. Unsupported
  Notes structures produce an error. The converter uses the saved results
  and does not evaluate the original calculations.
- PublishView .tnsp is a separate format. Use
  [TI-Nspire's conversion workflow](https://education.ti.com/en/customer-support/knowledge-base/ti-nspire-family/product-usage/32332)
  to save it as .tns first. PublishView-specific objects may be dropped by
  that conversion.

This project is available under the [MIT License](LICENSE).

The method 13 and TIXC implementation follows
[TnsTools](https://github.com/MaksimirKurtov/TnsTools). Its copyright notice
is included in the project license.
