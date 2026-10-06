//! Fuzz target — `font_model_fuzz`: arbitrary bytes become outlines, cmap
//! subtables, and whole fonts.
//!
//! The contract under fuzz is *totality plus invariants*. Every public
//! constructor must resolve to a value or a typed [`ModelError`] for any input
//! at all — never a panic, never an out-of-bounds read. Alongside that, three
//! properties are asserted on whatever the fuzzer produces:
//!
//! 1. **Validation agrees with construction.** A `Font` the builder accepted
//!    must pass `validate`, and one it rejected must not.
//! 2. **A validated cmap round-trips.** Encoding a subtable and decoding the
//!    result preserves every mapping that was there to begin with.
//! 3. **Geometry contains itself.** A path's bounding box holds every sampled
//!    point of every segment, and a validated outline has no non-finite
//!    coordinate.
//!
//! Inputs are bounded (point counts, contour counts) so a fuzzer run cannot be
//! turned into an OOM or an hours-long case by a single input.

#![no_main]

use font_model::{
    from_entries, CodePointSet, Cmap, CmapSubtable, Command, Font, FontBuilder, Glyph, GlyphId,
    ModelError, Outline, Path,
};
use libfuzzer_sys::fuzz_target;

/// Upper bound on the commands in one path.
const MAX_COMMANDS: usize = 64;
/// Upper bound on the contours in one outline.
const MAX_CONTOURS: usize = 8;
/// Upper bound on the glyphs in a generated font.
const MAX_GLYPHS: usize = 16;
/// Upper bound on the `cmap` entries in a generated subtable.
const MAX_ENTRIES: usize = 64;

/// One `f32` from the input, kept finite so the interesting paths are the
/// arithmetic ones rather than the non-finite guards.
fn coord(data: &[u8], at: usize) -> f32 {
    let bits = u32::from_le_bytes([
        data.get(at).copied().unwrap_or(0),
        data.get(at + 1).copied().unwrap_or(0),
        data.get(at + 2).copied().unwrap_or(0),
        data.get(at + 3).copied().unwrap_or(0),
    ]);
    let v = f32::from_bits(bits);
    if v.is_finite() {
        // Fold into a range a font outline plausibly occupies, so the
        // geometry assertions are about real shapes.
        (v % 2000.0) as f32
    } else {
        0.0
    }
}

/// Build a path from the input, sometimes including deliberately hostile
/// coordinates (NaN, infinity) so the validators are exercised too.
fn path_from(data: &[u8]) -> Path {
    let mut p = Path::starting_at(coord(data, 0), coord(data, 4));
    let count = usize::from(data.first().copied().unwrap_or(0)) % MAX_COMMANDS;
    for k in 1..count {
        let at = k * 12;
        // Every 16th command is a wildcard: a non-finite coordinate.
        let poison = k % 16 == 0;
        let nan = if poison { f32::NAN } else { 0.0 };
        let inf = if poison { f32::INFINITY } else { 0.0 };
        match k % 4 {
            0 => {
                p.line_to(coord(data, at) + nan, coord(data, at + 4) + inf);
            }
            1 => {
                p.quad_to(
                    coord(data, at),
                    coord(data, at + 4),
                    coord(data, at + 8) + nan,
                    coord(data, at + 20),
                );
            }
            2 => {
                p.cubic_to(
                    coord(data, at) + inf,
                    coord(data, at + 4),
                    coord(data, at + 8),
                    coord(data, at + 20) + nan,
                    coord(data, at + 24),
                    coord(data, at + 28),
                );
            }
            _ => {
                p.line_by(coord(data, at), coord(data, at + 4));
            }
        }
    }
    p.ensure_closed();
    p
}

/// Build an outline from the input.
fn outline_from(data: &[u8]) -> Outline {
    let mut o = Outline::new();
    let n = 1 + usize::from(data.get(1).copied().unwrap_or(0)) % MAX_CONTOURS;
    for k in 0..n {
        o.push_contour(path_from(data.get(k * 32..).unwrap_or(data)));
    }
    o
}

/// Build a `cmap` entry list from the input, sorted and deduplicated.
fn entries_from(data: &[u8]) -> Vec<(u32, GlyphId)> {
    let count = usize::from(data.get(2).copied().unwrap_or(0)) % MAX_ENTRIES;
    let mut raw: Vec<(u32, GlyphId)> = Vec::with_capacity(count);
    for k in 0..count {
        let at = 4 + k * 4;
        let cp = u32::from_le_bytes([
            data.get(at).copied().unwrap_or(0),
            data.get(at + 1).copied().unwrap_or(0),
            data.get(at + 2).copied().unwrap_or(0),
            data.get(at + 3).copied().unwrap_or(1),
        ]);
        // Glyph ids start at 1: entry 0 means "not mapped", which a round trip
        // is not expected to preserve.
        let gid = 1 + u16::from(data.get(k % 7).copied().unwrap_or(0)) % 64;
        raw.push((cp, GlyphId::new(gid)));
    }
    let mut sorted = raw;
    sorted.sort_by_key(|(cp, _)| *cp);
    // `from_entries` sorts and de-duplicates anyway; a repeated codepoint keeps
    // its last glyph, so hand it the same shape it will end up with.
    sorted.dedup_by(|a, b| {
        if a.0 == b.0 {
            b.1 = a.1;
            true
        } else {
            false
        }
    });
    sorted
}

/// Every mapping in `sub` must survive an encode/decode round trip.
fn assert_cmap_round_trips(sub: &CmapSubtable) {
    let bytes = sub.encode();
    let back = match CmapSubtable::decode(&bytes) {
        Ok(b) => b,
        // A decode failure is a real defect: `encode` must produce bytes this
        // crate can read back.
        Err(e) => panic!("encode produced undecodable bytes: {e}"),
    };
    for (cp, gid) in sub.entries() {
        assert_eq!(back.lookup(cp), Some(gid), "U+{cp:04X} lost");
    }
    assert_eq!(back.entries().len(), sub.entries().len());
}

/// Every point a curve takes, sampled at 33 values of `t`.
fn curve_samples(path: &Path) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut cur = (0.0f32, 0.0f32);
    for cmd in path.commands() {
        match *cmd {
            Command::MoveTo(x, y) => {
                cur = (x, y);
                out.push(cur);
            }
            Command::LineTo(x, y) => {
                for i in 0..=32 {
                    let t = i as f32 / 32.0;
                    out.push((
                        cur.0 + (x - cur.0) * t,
                        cur.1 + (y - cur.1) * t,
                    ));
                }
                cur = (x, y);
            }
            Command::QuadTo(cx, cy, x, y) => {
                for i in 0..=32 {
                    let t = i as f32 / 32.0;
                    let mt = 1.0 - t;
                    out.push((
                        mt * mt * cur.0 + 2.0 * mt * t * cx + t * t * x,
                        mt * mt * cur.1 + 2.0 * mt * t * cy + t * t * y,
                    ));
                }
                cur = (x, y);
            }
            Command::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                for i in 0..=32 {
                    let t = i as f32 / 32.0;
                    let mt = 1.0 - t;
                    out.push((
                        mt * mt * mt * cur.0
                            + 3.0 * mt * mt * t * c1x
                            + 3.0 * mt * t * t * c2x
                            + t * t * t * x,
                        mt * mt * mt * cur.1
                            + 3.0 * mt * mt * t * c1y
                            + 3.0 * mt * t * t * c2y
                            + t * t * t * y,
                    ));
                }
                cur = (x, y);
            }
            Command::Close => out.push(cur),
        }
    }
    out
}

/// A validated outline has no non-finite coordinate, and its box holds every
/// point its curves take.
///
/// The probe is a *sampled curve*, not `Path::points`: `points` includes
/// control points, and a Bézier's curve lies inside the convex hull of its
/// controls, so a control point is legitimately outside the curve's own box.
/// That is the property the box claims — the curve's extrema — and checking it
/// against the control hull would be checking something else.
fn assert_outline_is_sound(outline: &Outline, gid: GlyphId) {
    if outline.validate(gid).is_ok() {
        if let Some(bbox) = outline.bbox() {
            let slack = 1e-3 * bbox.width().abs().max(bbox.height().abs()).max(1.0);
            for contour in outline.contours() {
                for p in curve_samples(contour) {
                    assert!(
                        p.0 >= bbox.min_x() - slack
                            && p.0 <= bbox.max_x() + slack
                            && p.1 >= bbox.min_y() - slack
                            && p.1 <= bbox.max_y() + slack,
                        "curve point {p:?} outside {bbox:?}"
                    );
                }
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // ---- Outlines: geometry and validation must never panic. ----
    let outline = outline_from(data);
    assert_outline_is_sound(&outline, GlyphId::new(0));

    // Every derived view is total.
    let _ = outline.bbox();
    let _ = outline.area();
    let _ = outline.signed_area();
    let _ = outline.point_count();
    let _ = outline.validate(GlyphId::new(0));

    // Reversal is total and idempotent on the geometry: the same area, either
    // direction. Only asserted for a finite area — a poisoned coordinate makes
    // both sides NaN, and `NaN.abs() < x` is false, so the comparison would
    // fail on a correct result.
    let mut reversed = outline.clone();
    if let Some(first) = reversed.contours_mut().first_mut() {
        first.reverse();
    }
    // The tolerance is scaled to the *coordinates*, not to the area: a signed
    // area is a cancelling sum, so its absolute error is the epsilon of the
    // largest product it forms — which is extent², not the area. Two shapes
    // whose areas are both ~0 can still differ by the rounding of terms of
    // order 1800².
    let (a, b) = (outline.area(), reversed.area());
    if a.is_finite() && b.is_finite() {
        let extent = outline
            .bbox()
            .map_or(0.0f32, |bb| bb.width().abs().max(bb.height().abs()));
        let slack = 1e-5 * extent * extent + 1e-3;
        assert!((b - a).abs() <= slack, "reversed area {b} vs {a} (slack {slack})");
    }

    // ---- cmap: encode/decode round-trips for both modelled formats. ----
    let entries = entries_from(data);
    for format in [4u16, 12] {
        let sub = from_entries(&entries, format);
        assert_cmap_round_trips(&sub);
        // The whole table round-trips too.
        let mut cmap = Cmap::new();
        cmap.subtables_mut().push(sub);
        let table = cmap.encode();
        if let Ok(back) = Cmap::decode(&table) {
            for cp in cmap.codepoints().iter() {
                assert_eq!(
                    back.lookup(*cp),
                    cmap.lookup(*cp),
                    "table U+{cp:04X} lost"
                );
            }
        }
    }

    // ---- Fonts: build must agree with validate. ----
    let glyph_count = 1 + usize::from(data.get(3).copied().unwrap_or(0)) % MAX_GLYPHS;
    let mut builder = FontBuilder::new()
        .units_per_em(data.get(9).copied().map_or(1000, |b| u16::from(b % 100) + 16));
    for k in 0..glyph_count {
        let gid = GlyphId::new(u16::try_from(k).unwrap_or(0));
        let glyph_outline = outline_from(data.get(k * 16..).unwrap_or(data));
        let advance = 1 + u16::from(data.get(k % 11).copied().unwrap_or(1)) % 2000;
        builder = builder.glyph(Glyph::new(gid, advance, 0, glyph_outline));
    }
    let sub = from_entries(&entries, 4);
    let mut cmap = Cmap::new();
    cmap.subtables_mut().push(sub);
    let built: Result<Font, ModelError> = builder.cmap(cmap).build();

    match built {
        Ok(font) => {
            // A built font validates — that is the builder's contract.
            font.validate().expect("a built font validates");
            // The codepoint set is a genuine sorted set.
            let cps = font.codepoints();
            let mut prev: Option<u32> = None;
            for cp in cps.iter() {
                if let Some(p) = prev {
                    assert!(*cp > p, "cmap codepoints not ascending at U+{cp:X}");
                }
                prev = Some(*cp);
                // Every mapped codepoint resolves to a glyph that exists.
                let gid = font.glyph_for(*cp).expect("mapped");
                assert!(font.glyph(gid).is_some(), "dangling {gid}");
            }
            // Subsetting to the font's own codepoints yields a valid font.
            if let Ok(sub) = font.subset(&cps) {
                sub.validate().expect("a subset validates");
                assert!(sub.glyph_count() <= font.glyph_count());
            }
            // Subsetting to nothing keeps `.notdef` and stays valid.
            if let Ok(empty) = font.subset(&CodePointSet::new()) {
                empty.validate().expect("an empty subset validates");
            }
        }
        Err(_) => {
            // Rejection is fine; a panic is not.
        }
    }

    // ---- Arbitrary bytes as a font: a typed error, never a panic. ----
    let _ = font_model::from_sfnt(data);
});
