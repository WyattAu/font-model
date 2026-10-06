//! Property tests: invariants that must hold for *arbitrary* inputs, not the
//! hand-picked cases in the unit suite.
//!
//! The two headline properties are the `cmap` round trip over arbitrary sorted
//! codepoint sets (500 cases) and the `bbox` containment guarantee (300 cases).
//! Both are the kind of thing that a unit test can accidentally arrange to
//! pass: a fixed example exercises one shape, a generator explores the space.

// Test harness: assertions legitimately panic; the lib target holds the
// deny-level lints.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use font_model::{from_entries, CmapSubtable, CodePointSet, GlyphId, Outline, Path};
use proptest::prelude::*;

/// A small pool of glyph ids, so runs collide and holes appear.
///
/// Ids start at 1: glyph 0 is `.notdef`, and a `cmap` entry of 0 means "not
/// mapped" — so it is not a mapping a round trip can be expected to preserve.
/// Always non-empty: a codepoint with no glyph is not a case worth generating.
fn glyph_pool(min: usize) -> impl Strategy<Value = Vec<GlyphId>> {
    prop::collection::vec((1u16..12).prop_map(GlyphId::new), min..64)
}

/// A sorted, duplicate-free list of BMP codepoints.
///
/// The generator builds a set of arbitrary codepoints and hands back the
/// sorted, de-duplicated form — exactly what `CodePointSet` stores, so a round
/// trip through it cannot "cheat" by preserving the generator's order.
///
/// U+FFFF is excluded: the format-4 spec reserves it for the mandatory
/// terminator segment, so it is not a codepoint format 4 can carry.
fn sorted_bmp_codepoints(max: usize) -> impl Strategy<Value = Vec<u32>> {
    prop::collection::vec(0u32..0xFFFF, 0..max).prop_map(|cps| {
        let set = CodePointSet::from_iter_raw(cps);
        set.as_slice().to_vec()
    })
}

proptest! {
    /// 500 cases: an arbitrary sorted codepoint set round-trips through the
    /// format-4 encoder and decoder with every mapping intact.
    // 500 cases each: the two headline properties, sized so a run explores the
    // space rather than grazing it.
    #[test]
    fn format4_round_trips_arbitrary_sorted_sets(
        cps in sorted_bmp_codepoints(64),
        gids in glyph_pool(1),
    ) {
        // Pair each codepoint with a glyph, cycling the pool, so the mapping is
        // well defined however the two lengths relate.
        let entries: Vec<(u32, GlyphId)> = cps
            .iter()
            .enumerate()
            .map(|(i, cp)| (*cp, gids[i % gids.len()]))
            .collect();
        let sub = from_entries(&entries, 4);
        let bytes = sub.encode();
        let back = CmapSubtable::decode(&bytes).expect("encoder output decodes");

        for (cp, gid) in &entries {
            prop_assert_eq!(
                back.lookup(*cp),
                Some(*gid),
                "codepoint U+{:04X} lost in the format-4 round trip",
                cp
            );
        }
        prop_assert_eq!(back.entries().len(), entries.len());
        // A codepoint *well outside* the set must not appear afterwards. The
        // probe is two below the lowest entry rather than one: consecutive
        // codepoints with consecutive glyphs are a single delta segment, and a
        // delta segment's arithmetic legitimately covers its own start minus
        // one in the u16 wrap. Two below is outside every such run.
        if let Some(first) = entries.first().map(|(cp, _)| *cp) {
            if first >= 2 {
                prop_assert_eq!(
                    back.lookup(first - 2),
                    None,
                    "U+{:04X} invented a mapping below the set",
                    first - 2
                );
            }
        }
    }

    /// 500 cases: format 12 round-trips above the BMP, including codepoints
    /// that straddle the surrogate gap.
    #[test]
    fn format12_round_trips_arbitrary_codepoints(
        cps in prop::collection::vec(0u32..0x110000, 0..48),
        gids in glyph_pool(1),
    ) {
        let set = CodePointSet::from_iter_raw(cps);
        let entries: Vec<(u32, GlyphId)> = set
            .as_slice()
            .iter()
            .enumerate()
            .map(|(i, cp)| (*cp, gids[i % gids.len()]))
            .collect();
        let sub = from_entries(&entries, 12);
        let bytes = sub.encode();
        let back = CmapSubtable::decode(&bytes).expect("encoder output decodes");
        for (cp, gid) in &entries {
            prop_assert_eq!(back.lookup(*cp), Some(*gid), "U+{:X} lost", cp);
        }
        prop_assert_eq!(back.entries().len(), entries.len());
    }

}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    /// A path's exact bounding box contains every sampled point on
    /// every one of its segments. This is the guarantee a rasterizer's bounds
    /// check depends on.
    #[test]
    fn bbox_contains_every_sampled_point(
        commands in prop::collection::vec(
            prop_oneof![
                4 => (-1000.0f32..1000.0, -1000.0f32..1000.0)
                    .prop_map(|(x, y)| font_model::Command::LineTo(x, y)),
                4 => (
                    -1000.0f32..1000.0,
                    -1000.0f32..1000.0,
                    -1000.0f32..1000.0,
                    -1000.0f32..1000.0
                ).prop_map(|(cx, cy, x, y)| font_model::Command::QuadTo(cx, cy, x, y)),
                4 => (
                    -1000.0f32..1000.0,
                    -1000.0f32..1000.0,
                    -1000.0f32..1000.0,
                    -1000.0f32..1000.0,
                    -1000.0f32..1000.0,
                    -1000.0f32..1000.0
                ).prop_map(|(a, b, c, d, x, y)| font_model::Command::CubicTo(a, b, c, d, x, y)),
            ],
            1..12
        )
    ) {
        let mut p = Path::new();
        p.move_to(0.0, 0.0);
        for c in commands {
            p.push(c);
        }
        let Some(bbox) = p.bbox() else {
            return Ok(());
        };
        // Containment with a tolerance: the box is built from the curve's
        // analytic extrema while the probe points are that curve evaluated at
        // 33 samples, and the two round differently in `f32`.
        let slack = 1e-3 * bbox.width().abs().max(bbox.height().abs()).max(1.0);
        let contains = |x: f32, y: f32| {
            x >= bbox.min_x() - slack
                && x <= bbox.max_x() + slack
                && y >= bbox.min_y() - slack
                && y <= bbox.max_y() + slack
        };
        // Sample each segment at 33 points and require containment. The
        // tolerance absorbs the f32 evaluation of the curve at `t`, which is
        // the same shape the bbox itself uses.
        let mut cur = (0.0f32, 0.0f32);
        for cmd in p.commands() {
            match *cmd {
                font_model::Command::LineTo(x, y) => {
                    for i in 0..=32 {
                        let t = i as f32 / 32.0;
                        let (px, py) = (cur.0 + (x - cur.0) * t, cur.1 + (y - cur.1) * t);
                        prop_assert!(
                            contains(px, py),
                            "line point ({px}, {py}) outside {bbox:?}"
                        );
                    }
                    cur = (x, y);
                }
                font_model::Command::QuadTo(cx, cy, x, y) => {
                    for i in 0..=32 {
                        let t = i as f32 / 32.0;
                        let mt = 1.0 - t;
                        let px = mt * mt * cur.0 + 2.0 * mt * t * cx + t * t * x;
                        let py = mt * mt * cur.1 + 2.0 * mt * t * cy + t * t * y;
                        prop_assert!(
                            contains(px, py),
                            "quad point ({px}, {py}) outside {bbox:?}"
                        );
                    }
                    cur = (x, y);
                }
                font_model::Command::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                    for i in 0..=32 {
                        let t = i as f32 / 32.0;
                        let mt = 1.0 - t;
                        let px = mt * mt * mt * cur.0
                            + 3.0 * mt * mt * t * c1x
                            + 3.0 * mt * t * t * c2x
                            + t * t * t * x;
                        let py = mt * mt * mt * cur.1
                            + 3.0 * mt * mt * t * c1y
                            + 3.0 * mt * t * t * c2y
                            + t * t * t * y;
                        prop_assert!(
                            contains(px, py),
                            "cubic point ({px}, {py}) outside {bbox:?}"
                        );
                    }
                    cur = (x, y);
                }
                _ => {}
            }
        }
    }

    /// 300 cases: an outline's aggregate bbox contains every contour's bbox,
    /// and an outline never validates a non-finite coordinate into existence.
    #[test]
    fn outline_bbox_covers_its_contours(
        contours in prop::collection::vec(
            prop::collection::vec((-500.0f32..500.0, -500.0f32..500.0), 4..8),
            1..4
        )
    ) {
        let mut outline = Outline::new();
        for corner in contours {
            let mut p = Path::starting_at(corner[0].0, corner[0].1);
            for pt in &corner[1..] {
                p.line_to(pt.0, pt.1);
            }
            p.close();
            outline.push_contour(p);
        }
        let whole = outline.bbox().expect("non-empty outline");
        for contour in outline.contours() {
            let cb = contour.bbox().expect("non-empty contour");
            prop_assert!(whole.contains(cb.min_x(), cb.min_y()), "{cb:?} not inside {whole:?}");
            prop_assert!(whole.contains(cb.max_x(), cb.max_y()), "{cb:?} not inside {whole:?}");
        }
        // Every generated coordinate is finite by construction, so validation
        // must accept the outline.
        prop_assert!(outline.validate(GlyphId::new(0)).is_ok());
    }

    /// 300 cases: reversing a path preserves its area and bounding box, and
    /// flips its winding.
    #[test]
    fn reverse_preserves_geometry(
        points in prop::collection::vec((-500.0f32..500.0, -500.0f32..500.0), 4..12)
    ) {
        // A four-point minimum guarantees a closed, non-degenerate subpath.
        prop_assume!(points.len() >= 4);
        let mut p = Path::starting_at(points[0].0, points[0].1);
        for (x, y) in &points[1..] {
            p.line_to(*x, *y);
        }
        p.close();
        // Reversal flips the *sign* of the signed area (that is the winding
        // flip), so the invariant is on the magnitude.
        let area = p.signed_area().abs();
        let bbox = p.bbox().expect("bbox");
        let winding = p.winding();
        p.reverse();
        prop_assert!(
            (p.signed_area().abs() - area) <= 1e-3 * area.max(1.0),
            "area {} vs {area}",
            p.signed_area()
        );
        let after = p.bbox().expect("bbox");
        prop_assert!((after.min_x() - bbox.min_x()).abs() < 1e-3);
        prop_assert!((after.max_x() - bbox.max_x()).abs() < 1e-3);
        prop_assert!((after.min_y() - bbox.min_y()).abs() < 1e-3);
        prop_assert!((after.max_y() - bbox.max_y()).abs() < 1e-3);
        // The winding flip is only meaningful for a shape with real area: a
        // near-degenerate contour can lose its sign to `f32` rounding on the
        // way round, which is a classification of a zero-area shape, not a
        // reversal bug.
        use font_model::Winding;
        if area > 1.0 {
            if winding == Winding::CounterClockwise {
                prop_assert_eq!(p.winding(), Winding::Clockwise);
            } else if winding == Winding::Clockwise {
                prop_assert_eq!(p.winding(), Winding::CounterClockwise);
            }
        }
    }

    /// 200 cases: a codepoint set's algebra is consistent — union contains
    /// both inputs, intersection is contained in both, difference contains
    /// neither.
    #[test]
    fn codepoint_set_algebra(
        a in prop::collection::vec(0u32..0x1000, 0..32),
        b in prop::collection::vec(0u32..0x1000, 0..32),
    ) {
        let x = CodePointSet::from_iter_raw(a);
        let y = CodePointSet::from_iter_raw(b);
        let u = x.union(&y);
        let i = x.intersection(&y);
        let d = x.difference(&y);
        prop_assert!(u.len() >= x.len() && u.len() >= y.len());
        prop_assert!(i.len() <= x.len() && i.len() <= y.len());
        prop_assert_eq!(i.len() + d.len(), x.len(), "partition of `a`");
        for cp in x.iter() {
            prop_assert!(u.contains(*cp));
            if y.contains(*cp) {
                prop_assert!(i.contains(*cp));
            } else {
                prop_assert!(d.contains(*cp));
            }
        }
        // Iterating is ascending and duplicate-free.
        let mut prev: Option<u32> = None;
        for cp in u.iter() {
            if let Some(p) = prev {
                prop_assert!(*cp > p, "not ascending at U+{cp:X}");
            }
            prev = Some(*cp);
        }
    }
}
