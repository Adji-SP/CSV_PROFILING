# CSV Profiler + ESP32 OTA handbook

This directory contains beginner-friendly English and Bahasa Indonesia LaTeX handbooks for the project. Their visual language is based on the supplied `beautybook.pdf` template: a graphic cover, Roman-numbered front matter, part dividers, running headers, and textbook-style callouts.

## Build the PDF

From this directory, run three direct passes so the table of contents and PDF bookmarks settle:

```powershell
pdflatex --disable-installer -interaction=nonstopmode -halt-on-error csv-profiler-ota-handbook.tex
pdflatex --disable-installer -interaction=nonstopmode -halt-on-error csv-profiler-ota-handbook.tex
pdflatex --disable-installer -interaction=nonstopmode -halt-on-error csv-profiler-ota-handbook.tex
```

If Perl and `latexmk` are installed, this is an equivalent convenience command:

```powershell
latexmk -pdf -interaction=nonstopmode -halt-on-error csv-profiler-ota-handbook.tex
```

The generated file is `csv-profiler-ota-handbook.pdf`.

For the complete Bahasa Indonesia edition, run the same three passes with its main source:

```powershell
pdflatex --disable-installer -interaction=nonstopmode -halt-on-error csv-profiler-ota-handbook-id.tex
pdflatex --disable-installer -interaction=nonstopmode -halt-on-error csv-profiler-ota-handbook-id.tex
pdflatex --disable-installer -interaction=nonstopmode -halt-on-error csv-profiler-ota-handbook-id.tex
```

The generated file is `csv-profiler-ota-handbook-id.pdf`.

## Sources

- `csv-profiler-ota-handbook.tex` controls front matter and chapter order.
- `csv-profiler-ota-handbook-id.tex` controls the Bahasa Indonesia edition.
- `beautybook-inspired.sty` contains the reusable visual design.
- `chapters/` contains the English teaching material.
- `chapters-id/` contains the complete Bahasa Indonesia teaching material.

The handbook documents the source tree in this repository. When an API or embedded contract changes, update the corresponding chapter and rebuild the PDF.
