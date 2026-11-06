// Typst template for the book's PDFs.
//
// Used by tools/render_pdf.py, which converts the Markdown to a Typst body with pandoc
// (and tools/pdf/filters.lua) and lays that body out with either `doc` (one chapter, one
// PDF) or `book` (every chapter: cover, contents, chapters).
//
// Copied from the template of the Rust course this book's style follows, so the two courses'
// PDFs look like one set. Keep changes small and note them here.
//
// A4, serif body, sans headings, a running header (left: the book title; right: the
// current chapter) and the page number in the footer.
//
// Fonts (changed from the original template, which used macOS system fonts): only openly
// licensed ones, so that the published PDF can embed them. Libertinus Serif and DejaVu Sans
// Mono ship with Typst; Libertinus Sans is in tools/pdf/fonts/, under the SIL Open Font
// License (OFL.txt there). render_pdf.py passes --ignore-system-fonts, so no other font can
// end up in the PDF.

#let serif = ("Libertinus Serif",)
#let sans = ("Libertinus Sans",)
#let mono = ("DejaVu Sans Mono",)

#let accent = rgb("#1f5f99")
#let ink = rgb("#1b1f24")
#let muted = rgb("#57606a")
#let code-bg = rgb("#f6f8fa")
#let code-border = rgb("#d8dee4")
#let quote-color = rgb("#0969da")

// ------------------------------------------------------------------------------
// Blocks used by the converted Markdown
// ------------------------------------------------------------------------------

// `---` in the Markdown. Pandoc emits `divider()` (Typst 0.15+) or, before 3.8,
// `horizontalrule`; defining both keeps the body independent of either version.
#let divider(..args) = block(above: 1.4em, below: 1.4em, line(length: 100%, stroke: 0.5pt + code-border))
#let horizontalrule = divider()

// A link to a heading in another chapter of the book. The filter cannot see the other
// chapters, so it cannot tell whether the target exists; this decides at layout time and
// falls back to the plain text instead of failing the build on a stale anchor.
#let xref(target, body) = context {
  let found = query(label(target))
  if found.len() > 0 { link(found.first().location(), body) } else { body }
}

// The book uses block quotes as callouts ("> **Pitfall:** …"): a tinted box with a rule
// on the left. Short ones are kept on one page; one taller than a third of a page may
// break, because a box that cannot break and does not fit overflows the page instead.
#let callout(body) = layout(size => {
  let frame(breakable) = block(
    width: 100%,
    breakable: breakable,
    fill: quote-color.lighten(93%),
    stroke: (left: 2.5pt + quote-color),
    radius: (right: 3pt),
    inset: (left: 11pt, right: 10pt, top: 8pt, bottom: 9pt),
    above: 1.1em,
    below: 1.1em,
  )[
    #set par(spacing: 0.75em)
    #body
  ]
  frame(measure(frame(true), width: size.width).height > 8cm)
})

// ------------------------------------------------------------------------------
// Shared styling
// ------------------------------------------------------------------------------

// A heading's number as written in the source ("2.1", "A"), in the accent colour.
#let heading-number(it) = if it.numbering != none {
  text(fill: accent, counter(heading).display(it.numbering))
  h(0.6em)
}

#let base-style(body) = {
  set text(font: serif, size: 10.5pt, fill: ink, lang: "en", hyphenate: true)
  set par(justify: true, leading: 0.68em, spacing: 1.05em)

  // Headings ------------------------------------------------------------------
  show heading: set text(font: sans, fill: ink, hyphenate: false)
  show heading.where(level: 2): it => {
    set text(size: 15pt, weight: "bold")
    block(above: 1.9em, below: 0.9em, sticky: true)[#heading-number(it)#it.body]
  }
  show heading.where(level: 3): it => {
    set text(size: 12pt, weight: "bold")
    block(above: 1.5em, below: 0.75em, sticky: true)[#heading-number(it)#it.body]
  }
  show heading.where(level: 4): it => {
    set text(size: 10.5pt, weight: "bold", style: "italic")
    block(above: 1.3em, below: 0.6em, sticky: true, it.body)
  }

  // Code ---------------------------------------------------------------------------
  show raw: set text(font: mono)
  // A highlight, not a box: a box cannot break, and these docs put long paths and
  // environment variable names in narrow table cells.
  show raw.where(block: false): it => highlight(
    fill: code-bg,
    radius: 2pt,
    extent: 1.5pt,
    top-edge: "ascender",
    bottom-edge: "descender",
    text(size: 0.92em, it),
  )
  show raw.where(block: true): it => block(
    width: 100%,
    fill: code-bg,
    stroke: 0.5pt + code-border,
    radius: 3pt,
    inset: (x: 9pt, y: 8pt),
    breakable: true,
    above: 1em,
    below: 1.1em,
  )[
    #set par(justify: false, leading: 0.5em)
    #set text(size: 7.9pt)
    #it
  ]

  // Links, lists, quotes, tables -----------------------------------------------------
  // Every link, in-document ones included: these docs navigate by them ("Contents").
  show link: set text(fill: accent)
  set list(indent: 0.6em, body-indent: 0.5em, spacing: 0.75em)
  set enum(indent: 0.6em, body-indent: 0.5em, spacing: 0.75em)
  show quote.where(block: true): it => callout(it.body)
  set table(
    stroke: (x, y) => if y == 0 { (bottom: 0.8pt + ink) } else { (bottom: 0.4pt + code-border) },
    inset: (x: 6pt, y: 5pt),
  )
  show table: set text(size: 9.2pt)
  show table: set par(justify: false)
  show table.cell.where(y: 0): set text(weight: "bold", font: sans, size: 8.8pt)
  // A table is wrapped in a figure, and a figure does not break by default: these docs
  // have tables several pages long. The header row repeats on every page.
  show figure.where(kind: table): set block(breakable: true, above: 1.4em, below: 1.4em)
  show footnote.entry: set text(size: 8.5pt)
  body
}

// Document / chapter title (level-1 headings) -----------------------------------------
// "# Chapter 2 — Ownership" arrives from the filter as a heading numbered "2" with the
// supplement [Chapter]: the label goes above the title, the title stands alone.
#let title-style(body) = {
  show heading.where(level: 1): it => {
    pagebreak(weak: true)
    v(1.4cm)
    if it.numbering != none {
      let kicker = [#it.supplement #counter(heading).display(it.numbering)]
      text(font: sans, size: 11pt, weight: "semibold", fill: accent, tracking: 0.12em, upper(kicker))
      v(0.25em)
    }
    block(text(font: sans, size: 25pt, weight: "bold", fill: ink, hyphenate: false, it.body))
    v(0.35em)
    line(length: 100%, stroke: 0.8pt + accent)
    v(1.4em)
  }
  body
}

// Running header. With a `series` (the course): the series left, the chapter right.
// Without one: the document's title left, the section the page is in right.
#let running-header(series) = context {
  let page = here().page()
  // No header on a page that opens a document or a chapter: its title says it all.
  let titles = query(heading.where(level: 1))
  if titles.any(h => h.location().page() == page) { return }
  let current(level) = {
    let starts = query(heading.where(level: level)).filter(h => h.location().page() == page)
    let before = query(selector(heading.where(level: level)).before(here()))
    if starts.len() > 0 { starts.first().body } else if before.len() > 0 { before.last().body }
  }
  let (lhs, rhs) = if series != none {
    (series, current(1))
  } else {
    (if titles.len() > 0 { titles.first().body }, current(2))
  }
  set text(font: sans, size: 8.5pt, fill: muted)
  set par(justify: false)
  grid(columns: (1fr, 1fr), column-gutter: 1.5em, align: (left, right), lhs, rhs)
  v(-0.3em)
  line(length: 100%, stroke: 0.4pt + code-border)
}

#let page-setup(series, body) = {
  set page(
    paper: "a4",
    margin: (top: 2.6cm, bottom: 2.4cm, inside: 2.5cm, outside: 2.3cm),
    header: running-header(series),
    footer: context {
      set text(font: sans, size: 8.5pt, fill: muted)
      align(center, counter(page).display(page.numbering))
    },
    numbering: "1",
  )
  body
}

// ------------------------------------------------------------------------------
// Documents
// ------------------------------------------------------------------------------

// One document as a standalone PDF. `series` names the collection it belongs to (the
// course), for the running header. No creation date, so the same Markdown renders to the
// same bytes.
#let doc(title: none, series: none, body) = {
  set document(title: title, date: none)
  show: base-style
  show: title-style
  show: page-setup.with(series)
  body
}

// The whole course. The cover and the contents are placed by the generated main file
// between `book` and the chapters.
#let book(title: none, body) = {
  set document(title: title, date: none)
  show: base-style
  show: title-style
  show: page-setup.with(title)
  body
}

#let title-page(kicker, title, subtitle, left-note, right-note) = {
  set page(header: none, footer: none, numbering: none)
  set par(justify: false)
  v(3.2cm)
  text(font: sans, size: 13pt, weight: "semibold", fill: accent, tracking: 0.16em, upper(kicker))
  v(0.6em)
  text(font: sans, size: 38pt, weight: "bold", fill: ink, title)
  v(0.4em)
  block(width: 85%, text(font: sans, size: 16pt, fill: muted, subtitle))
  v(1fr)
  line(length: 100%, stroke: 1pt + accent)
  v(0.4em)
  grid(
    columns: (1fr, auto),
    column-gutter: 1.5em,
    text(font: sans, size: 10pt, fill: muted, left-note),
    text(font: sans, size: 10pt, fill: muted, right-note),
  )
  pagebreak()
}

#let toc() = {
  pagebreak(weak: true)
  set page(numbering: "i")
  heading(level: 1, outlined: false)[Contents]
  show outline.entry.where(level: 1): it => {
    v(0.35em)
    strong(it)
  }
  set outline.entry(fill: repeat(text(fill: code-border)[.], gap: 0.25em))
  outline(title: none, depth: 2, indent: auto)
}
