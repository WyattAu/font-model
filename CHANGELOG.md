# Changelog

All notable changes to this project are documented in this file. Format:
[Keep a Changelog](https://keepachangelog.com/); versions follow
[semver](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-10-06

### Added

- **`outline`** — resolved contours: `Outline`, `Path`, `Command`
  (`MoveTo`/`LineTo`/`QuadTo`/`CubicTo`/`Close`), `Bbox`, `Winding`,
  `Subpath`, `FlatSegment`. Every constructor is absolute; the `*_by` family is
  relative to the current point and resolves to the identical absolute
  command. `Bbox` is **exact**: quadratic and cubic segments solve their
  derivative for axis extrema, so the box holds the curve rather than its
  control hull. `signed_area` integrates `x dy − y dx` in the power basis, so
  four cubic arcs of a unit circle give π to 1e-3 — a tolerance set by the
  shape approximation, not the integral. `reverse` rebuilds each subpath with
  its segments in the opposite order, preserving geometry and flipping
  winding.
- **`cmap`** — `Cmap`, `CmapSubtable` (formats 4 and 12), `CodePointSet`,
  `Segment`. Both formats **encode and decode**. A format-4 encode groups
  codepoints into maximal runs: consecutive codepoints with consecutive glyphs
  become a delta segment, anything else a `glyphIndexArray` with its holes; the
  mandatory `U+FFFF` terminator is always emitted and never maps. Format 12
  spans the astral planes and splits a non-consecutive run back into groups it
  can express. `CodePointSet` is the sorted, mergeable set of mapped
  codepoints with `union`/`intersection`/`difference`/`is_subset_of`.
- **`glyph`** — `GlyphId` (a `u16` newtype with checked construction and
  numeric ordering) and `Glyph` (id, advance, left side bearing, outline), with
  `recompute_metrics` deriving the metrics from the ink.
- **`metrics`** — `Metrics` (the per-glyph `hmtx` pair, with
  `right_side_bearing` against an outline), `FontMetrics` (the `hhea` header,
  with the horizontal extents derived **exactly** from a glyph store — a
  positive minimum is reported as positive, not clamped to the field's zero),
  `VerticalMetrics`, and `MetricsBuilder` (accumulates the same extents, so a
  built header and a derived one agree).
- **`font`** — `Font`, `FontBuilder`, `Kerning`. `Font::validate` is the single
  gate and every `ModelError` variant names the offending glyph and field:
  `InvalidUnitsPerEm`, `ZeroAdvance`, `UnclosedContour`,
  `NonFiniteCoordinate`, `NonMonotonicCmap`, `UnknownGlyph`,
  `AdvanceOverflow`, `UnsupportedCmapFormat`, `Parse`. `subset` keeps
  `.notdef` plus the requested codepoints, remaps ids densely and in ascending
  original order, remaps kerning, carries metadata and opaque tables, pulls the
  ascender back to the surviving ink, and re-validates the result. Layout and
  other uninterpreted tables ride through as opaque `(tag, bytes)`, so a
  read-modify-write round trip is lossless.
- **`name` / `os2`** — owned `NameRecord` with the well-known name ids and a
  preference helper; owned `Os2Metrics` with the weight/width classes and
  embedding permissions.
- **`parse`** — `from_sfnt` / `from_font_ref`, the one function that takes
  foreign bytes: it runs `font-parse` and resolves its borrowed views into the
  model, converting TrueType contours (including the implied on-curve
  midpoints between adjacent controls) into proper segment streams.
- **`serde`** — behind the off-by-default `serde` feature, every owned type
  round-trips through a config format: glyph ids as raw `u16`, commands as
  `(opcode, [f32; 6])`, a `cmap` as its per-subtable entry lists.

[Unreleased]: https://github.com/WyattAu/font-model/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/WyattAu/font-model/releases/tag/v0.1.0