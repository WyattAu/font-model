# Security Policy — font-model

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | ✅        |

## Reporting a vulnerability

Report privately via [GitHub security advisories] for this repository, or
email **wyatt_au@protonmail.com**. Do **not** open a public issue for
security reports.

You will receive an acknowledgement within **72 hours**. Coordinated
disclosure: we ask for up to 90 days before public disclosure while a
patch ships.

## Scope notes

`font-model` is a pure data-structure library. Security considerations for
integrators:

- **No network, filesystem, or process state** — the crate is pure computation
  over caller-owned buffers and has no attack surface beyond its inputs. The
  one filesystem touch is in the `inventory` example, not the library.
- **`from_sfnt` is the untrusted-input edge.** It runs `font-parse` (whose own
  contract is *never panics on font bytes*) and then resolves the borrowed
  views into owned data. Everything after that is total: `validate`, `subset`,
  and every accessor return a value or a typed `ModelError`, never a panic.
  The crate denies `clippy::unwrap_used`, `expect_used`, `panic`, and
  `indexing_slicing`, and `#![forbid(unsafe_code)]`.
- **Bounded work on hostile input.** `from_sfnt` enumerates a parsed `cmap`
  across the full Unicode range (0x110 000 codepoint lookups) to recover its
  entry list, and allocates one owned outline per glyph. A caller bounding an
  untrusted font should bound its `numGlyphs` before loading; the fuzz target
  caps point, contour, and glyph counts for the same reason.
- **Non-finite coordinates are rejected, not sanitised.** `validate` reports
  a `NonFiniteCoordinate` naming the glyph, contour, command, and field. A
  caller that wants a *renderable* model must call `validate` first — the
  constructors deliberately do not silently repair a NaN, because a repaired
  coordinate is a wrong outline that looks plausible.
- **Bearing saturation is silent by design.** `Metrics::advance_for` saturates
  a bearing outside `i16` rather than failing, so a far-off-origin outline
  yields a clamped bearing with an honest advance width. That is a
  representability choice, not a validation one; call `validate` if a clamped
  bearing would be a defect for you.
- **`serde` is off by default.** Enabling it adds a dependency to the model's
  surface; the deserialisers are structural and will reject a malformed
  payload with a `serde` error rather than building an invalid model.

[GitHub security advisories]:
    https://github.com/WyattAu/font-model/security/advisories/new