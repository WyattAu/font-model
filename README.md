# font-model

Typed, owned, editable font model — glyph outlines, `cmap` formats 4 and 12,
metrics, validation, subsetting.

**Layer:** L1 — substrate · **Estate deps:** `font-parse` (L0) · **Runtime
deps:** `font-parse`, `thiserror`, `serde` (optional, off by default) ·
`#![no_std]` + `alloc`.

## What this layer is

[`font-parse`] reads font files **in place**: a `FontRef` is a borrowed view
over `&[u8]`, zero-copy and allocation-free. That is the right shape for
*reading*. It is the wrong shape for *editing* — reshaping a glyph, dropping a
codepoint, or serialising metrics — because you cannot mutate a borrowed view
and you cannot hand a `&[u8]` to a config file.

`font-model` is that owned, mutable form. It is the write path's data
structure:

```text
  font bytes ──font_parse::FontRef──► resolved glyphs, cmap, metrics
                                            │  own them
                                            ▼
                                         Font ◄── editor mutates
                                            │
                        ┌───────────────────┴───────────────────┐
                        ▼                                       ▼
                  subset(codepoints)                   writer serialises
```

## What's here

| Module | Contents |
|---|---|
| [`outline`](https://docs.rs/font-model/latest/font_model/outline/index.html) | [`Outline`], [`Path`], [`Command`], [`Bbox`], [`Winding`], [`FlatSegment`] — resolved contours in font units, absolute and relative constructors, **exact** curve bounds and areas |
| [`cmap`](https://docs.rs/font-model/latest/font_model/cmap/index.html) | [`Cmap`], [`CmapSubtable`], [`CodePointSet`], [`Segment`] — format 4 and 12 encode *and* decode |
| [`glyph`](https://docs.rs/font-model/latest/font_model/glyph/index.html) | [`GlyphId`], [`Glyph`] — the glyph store's element |
| [`metrics`](https://docs.rs/font-model/latest/font_model/metrics/index.html) | [`Metrics`], [`FontMetrics`], [`VerticalMetrics`], [`MetricsBuilder`] |
| [`font`](https://docs.rs/font-model/latest/font_model/font/index.html) | [`Font`], [`FontBuilder`], [`Kerning`] — the model, validation, subsetting |
| [`name`](https://docs.rs/font-model/latest/font_model/name/index.html) | [`NameRecord`] and the well-known name ids |
| [`os2`](https://docs.rs/font-model/latest/font_model/os2/index.html) | [`Os2Metrics`] |
| [`parse`](https://docs.rs/font-model/latest/font_model/parse/index.html) | [`from_sfnt`] — the bridge from `font-parse`'s borrowed views |

## Guarantees

- **Geometry is exact, not approximated.** `Path::bbox` solves each curve's
  derivative for its axis extrema, so the box holds the *curve* rather than its
  control hull — a cubic's real maximum is found, and
  `bbox.contains(point)` holds for every point on the path. `Path::signed_area`
  integrates `x dy − y dx` in the Bézier's power basis: four cubic arcs of a
  unit circle give **π to 1e-3** (the tolerance is the *shape* approximation,
  not the integral), and the quadratic form gives its exact value.
- **`cmap` round-trips.** Format 4 ends with the mandatory `U+FFFF` terminator;
  a contiguous run with consecutive glyphs becomes a delta segment and anything
  else a `glyphIndexArray`, holes included. U+FFFF is reserved by the spec, so
  it never maps. Format 12 spans the astral planes. `proptest` runs 500
  arbitrary sorted codepoint sets through both.
- **Invariants are enforced, not assumed.** `Font::validate` is the single
  gate, and every error names the offending glyph and field:

  | Invariant | Error |
  |---|---|
  | `units_per_em` in `16..=16384` | `InvalidUnitsPerEm { units_per_em }` |
  | ink implies non-zero advance | `ZeroAdvance { glyph }` |
  | contours closed | `UnclosedContour { glyph, contour }` |
  | coordinates finite | `NonFiniteCoordinate { glyph, contour, command, field }` |
  | `cmap` strictly ascending | `NonMonotonicCmap { subtable, previous, current }` |
  | every reference resolves | `UnknownGlyph { glyph, glyph_count }` |

  `FontBuilder::build` surfaces the same errors at the same granularity.
- **Round-trip is lossless.** `GSUB`/`GPOS`/`GDEF` — and anything else the
  model does not interpret — ride along as opaque `(tag, bytes)`, so a
  read-modify-write through the model loses nothing.
- **Subsetting always yields a valid font.** `.notdef` plus every glyph a
  requested codepoint maps to, remapped densely and in ascending original
  order; kerning is remapped, metadata and opaque tables carry over, and the
  result is re-validated before it is returned.
- **Totality under untrusted input.** The crate denies
  `clippy::unwrap_used`/`expect_used`/`panic`/`indexing_slicing` and
  `#![forbid(unsafe_code)]`; the `font_model_fuzz` target drives arbitrary
  bytes through construction, `cmap` round trips, and subsetting.

## Example

```rust
use font_model::{Font, Glyph, GlyphId, Outline, Path};

let mut outline = Outline::new();
let mut p = Path::starting_at(80.0, 0.0);
p.line_to(520.0, 0.0);
p.line_to(520.0, 700.0);
p.line_to(80.0, 700.0);
p.close();
outline.push_contour(p);

let mut cmap = font_model::Cmap::new();
cmap.subtables_mut()
    .push(font_model::from_entries(&[(u32::from('A'), GlyphId::new(1))], 4));

let font = Font::builder()
    .units_per_em(1000)
    .glyph(Glyph::new(GlyphId::NOTDEF, 600, 80, outline.clone()))
    .glyph(Glyph::new(GlyphId::new(1), 600, 80, outline))
    .cmap(cmap)
    .build()?;

assert_eq!(font.glyph_for(u32::from('A')), Some(GlyphId::new(1)));

let mut keep = font_model::CodePointSet::new();
keep.insert(u32::from('A'));
let sub = font.subset(&keep)?;   // .notdef + 'A'
assert_eq!(sub.glyph_count(), 2);
# Ok::<(), font_model::ModelError>(())
```

## Serde

The owned types carry `Serialize`/`Deserialize` behind the **off-by-default**
`serde` feature, so a model can round-trip through a config format without
forcing the dependency on every consumer. A font survives the trip with every
part intact: glyph ids as raw `u16`, commands as `(opcode, floats)`, the
`cmap` as its per-subtable entry lists.

## Not in scope

CFF (PostScript) outline decoding, `GSUB`/`GPOS` lookup interpretation (they ride
along opaquely), glyph-instruction (hinting bytecode) interpretation, `gvar`
variation application, and SFNT *serialisation* — this crate models, `font-shape`
draws, and a writer crate consumes.

## Layer

L1 — substrate. Its only estate-internal dependency is `font-parse` (L0), so
`font-parse` → `font-model` → `font-shape` is a legal chain.

[`font-parse`]: https://docs.rs/font-parse
[`from_sfnt`]: https://docs.rs/font-model/latest/font_model/parse/fn.from_sfnt.html
[`Outline`]: https://docs.rs/font-model/latest/font_model/outline/struct.Outline.html
[`Path`]: https://docs.rs/font-model/latest/font_model/outline/struct.Path.html
[`Command`]: https://docs.rs/font-model/latest/font_model/outline/enum.Command.html
[`Bbox`]: https://docs.rs/font-model/latest/font_model/outline/struct.Bbox.html
[`Winding`]: https://docs.rs/font-model/latest/font_model/outline/enum.Winding.html
[`FlatSegment`]: https://docs.rs/font-model/latest/font_model/outline/struct.FlatSegment.html
[`Cmap`]: https://docs.rs/font-model/latest/font_model/cmap/struct.Cmap.html
[`CmapSubtable`]: https://docs.rs/font-model/latest/font_model/cmap/enum.CmapSubtable.html
[`CodePointSet`]: https://docs.rs/font-model/latest/font_model/cmap/struct.CodePointSet.html
[`Segment`]: https://docs.rs/font-model/latest/font_model/cmap/struct.Segment.html
[`GlyphId`]: https://docs.rs/font-model/latest/font_model/glyph/struct.GlyphId.html
[`Glyph`]: https://docs.rs/font-model/latest/font_model/glyph/struct.Glyph.html
[`Metrics`]: https://docs.rs/font-model/latest/font_model/metrics/struct.Metrics.html
[`FontMetrics`]: https://docs.rs/font-model/latest/font_model/metrics/struct.FontMetrics.html
[`VerticalMetrics`]: https://docs.rs/font-model/latest/font_model/metrics/struct.VerticalMetrics.html
[`MetricsBuilder`]: https://docs.rs/font-model/latest/font_model/metrics/struct.MetricsBuilder.html
[`Font`]: https://docs.rs/font-model/latest/font_model/font/struct.Font.html
[`FontBuilder`]: https://docs.rs/font-model/latest/font_model/font/struct.FontBuilder.html
[`Kerning`]: https://docs.rs/font-model/latest/font_model/font/struct.Kerning.html
[`NameRecord`]: https://docs.rs/font-model/latest/font_model/name/struct.NameRecord.html
[`Os2Metrics`]: https://docs.rs/font-model/latest/font_model/os2/struct.Os2Metrics.html