// The repository's social preview image (1280x640), in the book's colours and open fonts.
// Regenerate with:
//   typst compile --font-path tools/pdf/fonts --ignore-system-fonts --ppi 144 --format png \
//     .github/social-preview.typ .github/social-preview.png
#let accent = rgb("#1f5f99")
#let ink = rgb("#1b1f24")
#let muted = rgb("#57606a")
#let sans = ("Libertinus Sans",)
#let mono = ("DejaVu Sans Mono",)
#set page(width: 640pt, height: 320pt, margin: (x: 40pt, top: 38pt, bottom: 30pt), fill: white)
#set par(justify: false)
#set text(font: sans, fill: ink)
#text(size: 11pt, weight: "semibold", fill: accent, tracking: 0.16em, upper[A practical course])
#v(2pt)
#text(size: 40pt, weight: "bold")[Async PostgreSQL in Rust]
#v(4pt)
#block(width: 92%, text(size: 15.5pt, fill: muted)[tokio-postgres from the first query to a production-shaped axum service: pipelining, COPY, LISTEN/NOTIFY, cancellation, TLS, pooling and testing.])
#v(1fr)
#line(length: 100%, stroke: 1.2pt + accent)
#v(3pt)
#grid(columns: (1fr, auto), column-gutter: 1em,
  text(size: 10.5pt, fill: muted)[19 chapters #sym.dot.c 156 examples run against PostgreSQL 18 #sym.dot.c a capstone service],
  text(font: mono, size: 10pt, fill: accent)[tokio-postgres 0.7 #sym.dot.c axum 0.8],
)
