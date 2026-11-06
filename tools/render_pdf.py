"""Render the book's Markdown to print-ready PDFs (plus provenance stamps).

Pipeline: Markdown --(pandoc + tools/pdf/filters.lua)--> Typst body
                   --(typst + tools/pdf/doc.typ)--> PDF.

Ported from the reference course's renderer, so the two
courses' PDFs come out of the same template. `tools/book.py` is the only caller:

    python3 tools/book.py pdf <chapter-id>     one chapter
    python3 tools/book.py pdf --stale          every `current` chapter whose PDF is behind
    python3 tools/book.py book                 the whole book as one PDF

Beside each PDF it writes `<out>.source-sha256`, standard `shasum -a 256` output for the
Markdown it rendered, so `book.py list` can tell a fresh PDF from a stale one.

Prerequisites (one-time):  brew install pandoc typst   (pandoc 3.x, typst 0.13 or newer)

No network is used. The fonts are macOS system fonts (Charter, Avenir Next, Menlo);
elsewhere Typst falls back to the fonts it embeds and says so.
"""

import hashlib
import os
import re
import shutil
import subprocess
import sys
import tempfile

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PDF_DIR = os.path.join(REPO_ROOT, "tools", "pdf")
TEMPLATE = os.path.join(PDF_DIR, "doc.typ")
FILTER = os.path.join(PDF_DIR, "filters.lua")

BOOK_TITLE = "Async PostgreSQL in Rust"

# GitHub-flavoured Markdown, minus the extensions that misread these docs: `$…$` would
# turn shell variables (and SQL's `$1`) into math, and a `---` rule followed by
# `key: value` lines would be taken for a YAML metadata block.
READER = "gfm-tex_math_dollars-tex_math_gfm-yaml_metadata_block"


def run(argv, stdin=None):
    """Run a tool, return its stdout; its warnings go to stderr, a failure raises."""
    try:
        res = subprocess.run(argv, input=stdin, capture_output=True, text=True)
    except FileNotFoundError:
        raise RuntimeError(f"{argv[0]} not found — install it with `brew install pandoc typst`.")
    if res.stderr.strip():
        sys.stderr.write(res.stderr)
    if res.returncode != 0:
        raise RuntimeError(f"{os.path.basename(argv[0])} failed (exit {res.returncode}).")
    return res.stdout


def typst_str(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def to_typst(text, **meta):
    """Markdown -> a Typst body for doc.typ. `meta` goes to the filter (see filters.lua)."""
    argv = ["pandoc", "-f", READER, "-t", "typst", "--wrap=preserve", "--lua-filter", FILTER]
    for key, value in meta.items():
        argv += ["-M", f"{key}={value}"]
    body = run(argv, stdin=text)

    # Pandoc sets only the horizontal alignment of a table cell; centre them vertically.
    def fix(m):
        items = [x.strip() for x in m.group(1).split(",") if x.strip()]
        return "align: (" + ", ".join(("left" if x == "auto" else x) + " + horizon" for x in items) + ",)"

    return re.sub(r"align: \(((?:auto|left|center|right|,)+)\)", fix, body)


def compile_typst(source, out):
    """Compile a main file (which imports doc.typ) to `out`."""
    tmpdir = tempfile.mkdtemp(prefix="bookpdf-")
    try:
        shutil.copy(TEMPLATE, os.path.join(tmpdir, "doc.typ"))
        main = os.path.join(tmpdir, "main.typ")
        with open(main, "w", encoding="utf-8") as f:
            f.write(source)
        run(["typst", "compile", main, out])
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
    if not os.path.exists(out):
        raise RuntimeError(f"typst produced no {out}.")


def title_of(text, fallback):
    """The document's `# ` heading, which is also the PDF's title."""
    m = re.search(r"^# (.+?)\s*$", text, re.MULTILINE)
    return m.group(1) if m else fallback


def stamp_for(text, src):
    """The provenance stamp line for `text`, in `shasum -a 256` format, repo-relative so
    `shasum -c book/<chapter>.pdf.source-sha256` works from the repository root."""
    digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
    return f"{digest}  {os.path.relpath(src, REPO_ROOT)}\n"


def render(src, out=None):
    """Render one chapter to a PDF and write `<out>.source-sha256` beside it.

    Returns `(pdf_path, stamp_path)`; raises `RuntimeError` if no PDF was produced.
    """
    src = os.path.abspath(src)
    out = os.path.abspath(out or os.path.splitext(src)[0] + ".pdf")
    with open(src, encoding="utf-8") as f:
        text = f.read()
    title = title_of(text, os.path.splitext(os.path.basename(src))[0])
    compile_typst(
        '#import "doc.typ": *\n'
        f"#show: doc.with(title: {typst_str(title)}, series: {typst_str(BOOK_TITLE)})\n"
        + to_typst(text),
        out,
    )
    stamp_path = out + ".source-sha256"
    with open(stamp_path, "w", encoding="utf-8") as f:
        f.write(stamp_for(text, src))
    return out, stamp_path


def render_book(chapters, out, title, kicker, subtitle, left_note, right_note):
    """Render several chapters as one book: cover, contents, then the chapters.

    `chapters` is a list of `(stem, markdown)`, `stem` being the chapter's file name
    without `.md`. Each chapter is converted on its own, with its heading ids prefixed by
    its stem, so ids stay unique across the book and links between chapters become links
    inside the book. The caller writes the provenance stamp. Returns the PDF path.
    """
    stems = ",".join(stem for stem, _ in chapters)
    body = "\n".join(
        to_typst(text, idprefix=f"{stem}-", chapterid=stem, chapters=stems)
        for stem, text in chapters
    )
    labels = set(re.findall(r"^<([^>\s]+)>$", body, re.MULTILINE))
    labels |= set(re.findall(r'#label\("([^"]+)"\)', body))
    for target in sorted(set(re.findall(r'#xref\("([^"]+)"\)', body)) - labels):
        sys.stderr.write(f"warning: link to {target}: no heading in the book has that id\n")
    compile_typst(
        '#import "doc.typ": *\n'
        f"#show: book.with(title: {typst_str(title)})\n"
        f"#title-page({typst_str(kicker)}, {typst_str(title)}, {typst_str(subtitle)}, "
        f"{typst_str(left_note)}, {typst_str(right_note)})\n"
        "#toc()\n"
        '#set page(numbering: "1")\n'
        "#counter(page).update(1)\n"
        + body,
        out,
    )
    return out
