# nspire-pdf

Convert TI-Nspire Notes documents to printable A4 PDFs. The Rust CLI reads the
archive, decodes TI method 13 entries, and translates Notes into LaTeX. Tectonic
compiles the LaTeX with selectable text and vector math. The converter sharpens
small embedded screenshots for print.

## Install

Install Rust 1.95 or newer and [Tectonic](https://tectonic-typesetting.github.io/).
Put Tectonic on PATH or set the TECTONIC environment variable to its absolute
path. The install commands compile the app and CLI on your machine.

### Windows (PowerShell)

With the MSVC Rust toolchain, install Visual Studio Build Tools with the
Desktop development with C++ workload.

    cargo install --locked --git https://github.com/Kappu12/nspire-pdf --tag v0.1.2 --features gui
    nspire-pdf-gui.exe

### macOS

Install the Xcode Command Line Tools if needed.

    xcode-select --install
    cargo install --locked --git https://github.com/Kappu12/nspire-pdf --tag v0.1.2 --features gui
    nspire-pdf-gui

### Linux (Debian or Ubuntu)

Install the desktop build dependencies:

    sudo apt-get install build-essential pkg-config libgl1-mesa-dev libwayland-dev libxkbcommon-dev
    cargo install --locked --git https://github.com/Kappu12/nspire-pdf --tag v0.1.2 --features gui
    nspire-pdf-gui

In the app, choose or drop a .tns file, then select Convert to PDF.

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
  vectors, matrices, equation systems, and saved calculator results.
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
