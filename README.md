# nspire-pdf

Convert TI-Nspire Notes documents to printable A4 PDFs. The Rust CLI reads the
archive, decodes TI method 13 entries, and translates Notes into LaTeX. Tectonic
compiles the LaTeX with selectable text and vector math. The converter sharpens
small embedded screenshots for print.

## Desktop app

Build the desktop app with:

    cargo build --release --features gui

Launch nspire-pdf-gui from target/release. Choose a .tns file or drop one onto
the window, check the PDF destination, and select Convert to PDF. Open PDF
opens the result in your default viewer.

The app looks for Tectonic next to its executable or on PATH. The TECTONIC
environment variable takes precedence.

## Command line

Install Rust and [Tectonic](https://tectonic-typesetting.github.io/). Put
Tectonic on PATH or set the TECTONIC environment variable to the absolute path
of its executable. Tectonic may download its LaTeX bundle on first use.

    cargo run --release -- "path/to/notes.tns" "path/to/notes.pdf"

Omit the output argument to write beside the input with a .pdf extension.
The CLI refuses to overwrite an existing file. To build a standalone CLI
executable, run cargo build --release. Tectonic is still required at runtime.

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
