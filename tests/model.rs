//! Integration tests for the public model surface: the operations a subsetter,
//! an editor, and a writer actually call.
//!
//! The unit suites live next to the code they test; these exercise the crate
//! the way a downstream consumer does — through `font_model::`'s re-exports
//! only, with no crate-internal shortcuts.

// Test harness: assertions legitimately panic; the lib target holds the
// deny-level lints.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use font_model::{
    build_stub_font, from_entries, insert_entry, Cmap, CmapSubtable, CodePointSet, Font,
    FontMetrics, Glyph, GlyphId, Metrics, MetricsBuilder, ModelError, NameRecord, Outline, Path,
    TAG_GPOS, TAG_GSUB,
};

/// A closed 500×700 box at (50, 0) — the crate's standard test glyph.
fn box_outline() -> Outline {
    let mut o = Outline::new();
    let mut p = Path::starting_at(50.0, 0.0);
    p.line_to(550.0, 0.0);
    p.line_to(550.0, 700.0);
    p.line_to(50.0, 700.0);
    p.close();
    o.push_contour(p);
    o
}

/// A font with `.notdef`, 'A' (U+0041), and 'B' (U+0042).
fn demo_font() -> Font {
    let mut sub = CmapSubtable::empty_format4();
    sub = insert_entry(&sub, 0x41, GlyphId::new(1));
    sub = insert_entry(&sub, 0x42, GlyphId::new(2));
    let mut cmap = Cmap::new();
    cmap.subtables_mut().push(sub);
    Font::builder()
        .units_per_em(1000)
        .ascender(800)
        .descender(-200)
        .glyph(Glyph::new(GlyphId::NOTDEF, 600, 50, box_outline()))
        .glyph(Glyph::new(GlyphId::new(1), 600, 50, box_outline()))
        .glyph(Glyph::new(GlyphId::new(2), 600, 50, box_outline()))
        .cmap(cmap)
        .name(NameRecord::new(1, "Demo".into()))
        .kern(GlyphId::new(1), GlyphId::new(2), -40)
        .table(TAG_GSUB, vec![1, 2, 3])
        .build()
        .expect("valid")
}

#[test]
fn builder_to_validate_to_subset_pipeline() {
    let font = demo_font();
    assert_eq!(font.units_per_em(), 1000);
    assert_eq!(font.glyph_count(), 3);
    assert!(font.validate().is_ok());

    // 'A' and 'B' map; nothing else does.
    assert_eq!(font.glyph_for(0x41), Some(GlyphId::new(1)));
    assert_eq!(font.glyph_for(0x42), Some(GlyphId::new(2)));
    assert_eq!(font.glyph_for(0x43), None);
    assert_eq!(font.codepoints().as_slice(), &[0x41, 0x42]);

    // Kerning is applied between the mapped pair.
    assert_eq!(font.kern(GlyphId::new(1), GlyphId::new(2)), -40);
    assert_eq!(font.kern(GlyphId::new(2), GlyphId::new(1)), 0);

    // Subset to 'A' only: `.notdef` and 'A' survive, 'B' and its kern pair go.
    let mut keep = CodePointSet::new();
    keep.insert(0x41);
    let sub = font.subset(&keep).expect("subsets");
    assert_eq!(sub.glyph_count(), 2);
    assert_eq!(sub.glyph_for(0x41), Some(GlyphId::new(1)));
    assert_eq!(sub.glyph_for(0x42), None);
    assert_eq!(sub.kerning().len(), 0);
    assert!(sub.validate().is_ok());
    // Metadata is carried through, not stripped.
    assert_eq!(sub.names().len(), 1);
    assert_eq!(sub.table(TAG_GSUB), Some([1u8, 2, 3].as_slice()));
}

#[test]
fn every_invariant_is_reachable_and_names_its_subject() {
    // unitsPerEm
    let e = Font::builder()
        .units_per_em(8)
        .glyph(Glyph::empty(GlyphId::NOTDEF, 500))
        .build()
        .unwrap_err();
    assert!(matches!(
        e,
        ModelError::InvalidUnitsPerEm { units_per_em: 8 }
    ));
    assert!(format!("{e}").contains("unitsPerEm"));

    // Ink without an advance.
    let e = Font::builder()
        .glyph(Glyph::new(GlyphId::NOTDEF, 0, 0, box_outline()))
        .build()
        .unwrap_err();
    match e {
        ModelError::ZeroAdvance { glyph } => assert_eq!(glyph, GlyphId::NOTDEF),
        other => panic!("{other:?}"),
    }

    // An unclosed contour.
    let mut open = box_outline();
    let mut c = open.contours_mut().pop().expect("contour");
    c.open();
    open.contours_mut().push(c);
    let e = Font::builder()
        .glyph(Glyph::new(GlyphId::NOTDEF, 600, 0, open))
        .build()
        .unwrap_err();
    assert!(matches!(e, ModelError::UnclosedContour { contour: 0, .. }));

    // A non-finite coordinate.
    let mut o = Outline::new();
    let mut p = Path::starting_at(f32::NAN, 0.0);
    p.line_to(1.0, 1.0);
    p.close();
    o.push_contour(p);
    let e = Font::builder()
        .glyph(Glyph::new(GlyphId::NOTDEF, 600, 0, o))
        .build()
        .unwrap_err();
    assert!(matches!(
        e,
        ModelError::NonFiniteCoordinate { field: "x", .. }
    ));

    // Non-monotonic cmap.
    let mut c = Cmap::format4();
    c.subtables_mut()[0] = CmapSubtable::Format12 {
        segments: vec![
            font_model::Segment {
                start: 0x41,
                end: 0x50,
                first_glyph: 1,
            },
            font_model::Segment {
                start: 0x45,
                end: 0x60,
                first_glyph: 5,
            },
        ],
    };
    let e = Font::builder()
        .glyph(Glyph::new(GlyphId::NOTDEF, 600, 0, box_outline()))
        .cmap(c)
        .build()
        .unwrap_err();
    match e {
        ModelError::NonMonotonicCmap {
            subtable,
            previous,
            current,
        } => {
            assert_eq!(subtable, 0);
            assert_eq!(previous, 0x50);
            assert_eq!(current, 0x45);
        }
        other => panic!("{other:?}"),
    }

    // A dangling glyph reference from the cmap.
    let mut c = Cmap::format4();
    c.subtables_mut()[0] = from_entries(&[(0x41, GlyphId::new(9))], 4);
    let e = Font::builder()
        .glyph(Glyph::new(GlyphId::NOTDEF, 600, 0, box_outline()))
        .cmap(c)
        .build()
        .unwrap_err();
    match e {
        ModelError::UnknownGlyph { glyph, glyph_count } => {
            assert_eq!(glyph, GlyphId::new(9));
            assert_eq!(glyph_count, 1);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_error_variant_renders_informatively() {
    // One message per variant, checked for the detail the variant promises.
    let cases: Vec<(ModelError, &str)> = vec![
        (
            ModelError::InvalidUnitsPerEm { units_per_em: 3 },
            "unitsPerEm 3",
        ),
        (
            ModelError::ZeroAdvance {
                glyph: GlyphId::new(4),
            },
            "gid4",
        ),
        (
            ModelError::UnclosedContour {
                glyph: GlyphId::new(4),
                contour: 2,
            },
            "contour 2",
        ),
        (
            ModelError::NonFiniteCoordinate {
                glyph: GlyphId::new(4),
                contour: 1,
                command: 3,
                field: "control2.z",
            },
            "control2.z",
        ),
        (
            ModelError::NonMonotonicCmap {
                subtable: 1,
                previous: 0x20,
                current: 0x10,
            },
            "U+0020",
        ),
        (
            ModelError::UnknownGlyph {
                glyph: GlyphId::new(7),
                glyph_count: 2,
            },
            "glyph 7",
        ),
        (
            ModelError::AdvanceOverflow {
                glyph: GlyphId::new(5),
                advance: 70_000,
            },
            "70000",
        ),
        (ModelError::UnsupportedCmapFormat { format: 6 }, "format 6"),
        (
            ModelError::Parse {
                message: String::from("truncated"),
            },
            "truncated",
        ),
    ];
    for (e, needle) in cases {
        let msg = format!("{e}");
        assert!(msg.contains(needle), "{msg:?} should mention {needle:?}");
    }
}

#[test]
fn metrics_round_trip_through_the_builder() {
    let mut glyphs = vec![
        Glyph::new(GlyphId::NOTDEF, 0, 0, box_outline()),
        Glyph::empty(GlyphId::new(1), 600),
    ];
    let mut b = MetricsBuilder::new()
        .with_ascender(750)
        .with_descender(-250);
    for g in &mut glyphs {
        g.recompute_metrics().expect("recomputes");
        b.push(g);
    }
    let header = b.build();
    assert_eq!(header.ascender, 750);
    assert_eq!(header.descender, -250);
    assert_eq!(header.line_height(), 1000);
    // `.notdef`'s box is 500 wide at x = 50, so its recomputed metrics are
    // advance 500 / lsb 50, and `xMaxExtent` is lsb + xMax = 600.
    assert_eq!(glyphs[0].advance_width(), 500);
    assert_eq!(glyphs[0].left_side_bearing(), 50);
    assert_eq!(header.x_max_extent, 600);
    // The derived header agrees with the built one.
    assert_eq!(
        FontMetrics::derive(&glyphs).x_max_extent,
        header.x_max_extent
    );
    // And the per-glyph accessor agrees with the field.
    assert_eq!(glyphs[0].metrics().expect("metrics"), Metrics::new(500, 50));
}

#[test]
fn stub_font_covers_the_bmp_and_beyond() {
    let cps = [0x41u32, 0x42, 0x1F600, 0x10FFFF];
    let font = build_stub_font(&cps).expect("valid");
    assert_eq!(font.glyph_count(), 5);
    assert_eq!(font.codepoints().len(), 4);
    for cp in cps {
        let gid = font.glyph_for(cp).expect("mapped");
        assert!(font.glyph(gid).is_some());
    }
    // The writer-facing subtable split puts the astral codepoints in format 12.
    let subs = font.encoding_subtables();
    assert_eq!(subs.len(), 2);
    assert_eq!(subs[0].format(), 4);
    assert_eq!(subs[1].format(), 12);
    // And each subtable encodes to bytes that decode back.
    for sub in subs {
        let back = CmapSubtable::decode(&sub.encode()).expect("decodes");
        assert_eq!(back.entries().len(), sub.entries().len());
    }
}

#[test]
fn outline_geometry_helpers_agree_with_each_other() {
    let o = box_outline();
    let b = o.bbox().expect("bbox");
    assert_eq!((b.min_x(), b.min_y()), (50.0, 0.0));
    assert_eq!((b.max_x(), b.max_y()), (550.0, 700.0));
    assert_eq!(o.area(), 500.0 * 700.0);
    // `box_outline` runs counter-clockwise in the y-up coordinates the model
    // stores; a TrueType outer contour is the same loop with the opposite
    // sign, which is what flipping below produces.
    assert_eq!(
        o.contours()[0].winding(),
        font_model::Winding::CounterClockwise
    );
    let mut flipped = box_outline();
    flipped.contours_mut()[0].reverse();
    assert_eq!(
        flipped.contours()[0].winding(),
        font_model::Winding::Clockwise
    );
    assert_eq!(flipped.bbox().expect("bbox"), b);
    assert_eq!(flipped.area(), o.area());
}

#[test]
fn font_edits_are_visible_through_the_accessors() {
    let mut font = demo_font();
    // Reshape a glyph.
    {
        let g = font.glyph_mut(GlyphId::new(1)).expect("glyph");
        g.set_advance_width(700);
        g.set_left_side_bearing(120);
    }
    assert_eq!(
        font.glyph(GlyphId::new(1)).expect("glyph").advance_width(),
        700
    );
    font.recompute_metrics();
    assert_eq!(font.metrics().advance_width_max, 700);

    // Add a codepoint.
    font.cmap_mut().subtables_mut()[0] =
        insert_entry(&font.cmap().subtables()[0], 0x43, GlyphId::new(1));
    assert_eq!(font.glyph_for(0x43), Some(GlyphId::new(1)));
    assert!(font.cmap().validate().is_ok());

    // Replace an opaque table, then drop it.
    font.set_table(TAG_GPOS, vec![9]);
    font.set_table(TAG_GPOS, vec![9, 9]);
    assert_eq!(font.table(TAG_GPOS), Some([9u8, 9].as_slice()));
    assert!(font.remove_table(TAG_GPOS));
    assert!(font.table(TAG_GPOS).is_none());
    assert!(font.validate().is_ok());
}

#[test]
fn cmap_encode_decode_round_trips_through_the_public_api() {
    // Format 4 with a delta run, an array run, and a hole.
    let entries = vec![
        (0x20u32, GlyphId::new(0x48)),
        (0x41, GlyphId::new(3)),
        (0x42, GlyphId::new(0)),
        (0x43, GlyphId::new(5)),
    ];
    let sub = from_entries(&entries, 4);
    let bytes = sub.encode();
    let back = CmapSubtable::decode(&bytes).expect("decodes");
    assert_eq!(back.lookup(0x20), Some(GlyphId::new(0x48)));
    assert_eq!(back.lookup(0x41), Some(GlyphId::new(3)));
    assert_eq!(back.lookup(0x42), None, "the hole stayed a hole");
    assert_eq!(back.lookup(0x43), Some(GlyphId::new(5)));
    // U+FFFF is the terminator and never maps.
    assert_eq!(back.lookup(0xFFFF), None);

    // The whole table round-trips too.
    let mut c = Cmap::new();
    c.subtables_mut().push(sub);
    let table = c.encode();
    let back = Cmap::decode(&table).expect("decodes");
    assert_eq!(back.lookup(0x41), Some(GlyphId::new(3)));
    assert_eq!(back.codepoints().len(), 3);
}
