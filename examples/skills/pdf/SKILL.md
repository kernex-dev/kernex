---
name = "pdf"
description = "Read, extract text, and manipulate PDF files."
requires = ["pdftotext"]
homepage = "https://poppler.freedesktop.org/"
trigger = "pdf|extract pdf|read pdf|convert pdf|pdf text"

[toolbox.pdf_extract]
description = "Extract text from a PDF file."
command = "pdftotext"
args = []
parameters = { type = "object", properties = { file = { type = "string", description = "Path to the PDF file" }, output = { type = "string", description = "Output text file path (optional, defaults to stdout)" } }, required = ["file"] }
---

# PDF Processing

Extract text and work with PDF files using local CLI tools. This skill uses
`pdftotext` from the Poppler utilities for reliable PDF text extraction.

## Tools available

- `pdf_extract` — Extract text content from a PDF file

## Setup

### macOS
```bash
brew install poppler
```

### Ubuntu/Debian
```bash
sudo apt-get install poppler-utils
```

### Windows
Download from https://github.com/oschwartz10612/poppler-windows/releases

## Usage examples

**Extract all text from a PDF:**
```
Extract the text from report.pdf
```

**Extract specific pages:**
```bash
pdftotext -f 1 -l 5 document.pdf  # Pages 1-5 only
```

## Limitations

- Scanned PDFs (images) require OCR — this skill extracts embedded text only
- Complex layouts may have formatting issues in extracted text
- Password-protected PDFs require the password to be provided

## Alternatives

For more advanced PDF operations, consider:
- `pdfinfo` — Get PDF metadata
- `pdftk` — Merge, split, rotate PDFs
- `qpdf` — PDF transformations and encryption
