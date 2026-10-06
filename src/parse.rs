//! The bridge from [`font-parse`]'s borrowed views to this owned model.
//!
//! Everything else in the crate takes owned data; [`from_sfnt`] is the one
//! function that takes foreign bytes. It runs the L0 parser (which is total —
//! malformed input yields a typed [`ParseError`]) and *resolves* what it finds
//! into the model: outlines become [`Command`] streams, `hmtx` pairs become
//! per-glyph [`Metrics`], `cmap` becomes owned subtables, `name` becomes owned
//! strings.
//!
//! What the bridge does **not** do:
//!
//! - **CFF outlines.** `font-parse` exposes the `CFF` table raw and does not
//!   decode charstrings; an `'OTTO'` font loads with empty outlines and a
//!   `CFF ` opaque table attached.
//! - **Layout tables.** `GSUB`/`GPOS`/`GDEF` are carried through opaquely so a
//!   read-modify-write round trip stays lossless. `font-shape` reorders runs by
//!   codepoint class rather than interpreting `GSUB`.
//! - **Variation.** `gvar` deltas are not applied; the default instance's
//!   outlines are what load.
//! - **Instructions.** Hinting bytecode is not interpreted; outlines load
//!   unhinted.

use alloc::string::String;
use alloc::vec::Vec;
use font_parse::{FontRef, GlyphOutline, OutlinePoint};

use crate::cmap::{self, Cmap, CmapSubtable};
use crate::error::ModelError;
use crate::font::{Font, Kerning, TableTag};
use crate::glyph::{Glyph, GlyphId};
use crate::metrics::{FontMetrics, Metrics};
use crate::name::NameRecord;
use crate::os2::Os2Metrics;
use crate::outline::{Outline, Path};

/// Tables carried through the model opaquely, in load order.
///
/// Layout tables first (they are the ones a layout engine needs and this crate
/// must not lose), then everything else the parser knows, then any unknown tag
/// found in the directory — so a read-modify-write round trip loses nothing.
const OPAQUE_TAGS: [TableTag; 3] = [crate::TAG_GSUB, crate::TAG_GPOS, *b"GDEF"];

/// Resolve a parsed font into the owned model.
///
/// # Errors
/// [`ModelError::Parse`] when the parser rejects the bytes, or the model
/// rejects what the parser found — a `cmap` referencing a glyph past
/// `numGlyphs`, or an `hmtx` inconsistency the model catches.
///
/// ```
/// # fn main() -> Result<(), font_model::ModelError> {
/// // A font with no bytes at all is a parse error, not a panic.
/// assert!(font_model::from_sfnt(&[]).is_err());
/// # Ok(())
/// # }
/// ```
#[allow(clippy::result_large_err)]
pub fn from_sfnt(bytes: &[u8]) -> Result<Font, ModelError> {
    let parsed = FontRef::new(bytes)?;
    from_font_ref(&parsed)
}

/// Resolve an already-parsed [`FontRef`] into the owned model.
///
/// Takes the parser's view by reference so a caller who already has one does
/// not re-parse.
///
/// # Errors
/// See [`from_sfnt`].
#[allow(clippy::result_large_err)]
pub fn from_font_ref(parsed: &FontRef<'_>) -> Result<Font, ModelError> {
    let units_per_em = parsed.head()?.units_per_em()?;
    let num_glyphs = parsed.num_glyphs();
    if num_glyphs == 0 {
        return Err(ModelError::UnknownGlyph {
            glyph: GlyphId::NOTDEF,
            glyph_count: 0,
        });
    }
    // -- glyphs ------------------------------------------------------------
    let hmtx = parsed.hmtx().ok();
    let mut glyphs = Vec::with_capacity(usize::from(num_glyphs));
    for index in 0..num_glyphs {
        let id = GlyphId::new(index);
        let outline = parsed
            .outline(font_parse::GlyphId::new(u32::from(index)))
            .unwrap_or(None);
        let outline = outline.map(to_outline).unwrap_or_default();
        let (advance, lsb) = match &hmtx {
            Some(h) => (
                h.advance(index).unwrap_or(0),
                h.left_side_bearing(index).unwrap_or(0),
            ),
            None => (0, 0),
        };
        glyphs.push(Glyph::new(id, advance, lsb, outline));
    }
    // -- cmap --------------------------------------------------------------
    let cmap = load_cmap(parsed)?;
    // -- headers -----------------------------------------------------------
    let mut metrics = FontMetrics::derive(&glyphs);
    if let Ok(hhea) = parsed.hhea() {
        if let (Ok(a), Ok(d), Ok(g)) = (hhea.ascender(), hhea.descender(), hhea.line_gap()) {
            metrics.ascender = a;
            metrics.descender = d;
            metrics.line_gap = g;
        }
    }
    if let Some(v) = load_vertical(parsed) {
        metrics.vertical = Some(v);
    }
    // -- name --------------------------------------------------------------
    let mut names = Vec::new();
    if let Ok(table) = parsed.name() {
        if let Ok(records) = table.records() {
            for r in records {
                names.push(NameRecord::with_locale(
                    r.name_id,
                    r.platform_id,
                    r.encoding_id,
                    r.language_id,
                    decode_name(&r),
                ));
            }
        }
    }
    // -- OS/2 --------------------------------------------------------------
    let os2 = parsed.os2().ok().map(|t| load_os2(&t));
    // -- kerning -----------------------------------------------------------
    let kerning = parsed
        .table_by_tag(font_parse::Tag::new(b"kern"))
        .map_or_else(Kerning::new, Kerning::from_kern_bytes);
    // -- opaque tables -----------------------------------------------------
    let mut tables: Vec<(TableTag, Vec<u8>)> = Vec::new();
    for tag in OPAQUE_TAGS {
        if let Some(data) = raw_table(parsed, tag) {
            tables.push((tag, data));
        }
    }
    for (raw_tag, _checksum, _offset, _length) in parsed.table_directory() {
        if tables.iter().any(|(t, _)| *t == raw_tag) {
            continue;
        }
        if is_modelled(raw_tag) {
            continue;
        }
        if let Some(data) = parsed.table_by_tag(font_parse::Tag::new(&raw_tag)) {
            tables.push((raw_tag, data.to_vec()));
        }
    }
    // A font whose `cmap` or kerning points past `numGlyphs` is malformed as
    // a *model*; report it precisely rather than dropping the entry silently.
    validate_references(&glyphs, &cmap, &kerning)?;
    Ok(Font::from_parts(
        units_per_em,
        metrics,
        glyphs,
        cmap,
        names,
        os2,
        kerning,
        tables,
    ))
}

/// Raw bytes for one four-byte tag.
fn raw_table(parsed: &FontRef<'_>, tag: TableTag) -> Option<Vec<u8>> {
    parsed
        .table_by_tag(font_parse::Tag::new(&tag))
        .map(<[u8]>::to_vec)
}

/// Tags the model interprets directly and therefore does not carry opaquely.
fn is_modelled(tag: TableTag) -> bool {
    const MODELLED: [TableTag; 10] = [
        *b"cmap", *b"head", *b"hhea", *b"hmtx", *b"maxp", *b"name", *b"OS/2", *b"glyf", *b"loca",
        *b"kern",
    ];
    MODELLED.contains(&tag)
}

/// Load the `cmap` into owned subtables.
///
/// Every Unicode-capable subtable is enumerated and decoded — the model keeps
/// both format 4 and format 12, so a font with a UCS-4 table keeps its astral
/// coverage rather than collapsing to the BMP.
fn load_cmap(parsed: &FontRef<'_>) -> Result<Cmap, ModelError> {
    let Ok(table) = parsed.cmap() else {
        return Ok(Cmap::format4());
    };
    let mut out = Cmap::new();
    let records = table.records();
    for (i, record) in records.iter().enumerate() {
        if !record.is_unicode() {
            continue;
        }
        let Ok(sub) = table.subtable(i) else { continue };
        let format = match sub {
            font_parse::CmapSubtable::Format4(_) => 4,
            font_parse::CmapSubtable::Format12(_) => 12,
            // Formats 0 and 6 are read by the parser but are not modelled for
            // encoding; fold them into the format-4 representation.
            font_parse::CmapSubtable::Format0(_) | font_parse::CmapSubtable::Format6(_) => 4,
        };
        let entries = enumerate_subtable(&sub);
        if entries.is_empty() {
            continue;
        }
        out.subtables_mut()
            .push(cmap::from_entries(&entries, format));
    }
    if out.subtables().is_empty() {
        out.subtables_mut().push(CmapSubtable::empty_format4());
    }
    Ok(out)
}

/// Enumerate a parsed subtable's mapped codepoints.
///
/// The parser's view offers point lookups, not enumeration, so the range is
/// walked: surrogates are skipped (they are not scalar values), and a
/// subtable that reaches above the BMP costs 4× the walk of a BMP one.
fn enumerate_subtable(sub: &font_parse::CmapSubtable<'_>) -> Vec<(u32, GlyphId)> {
    const MAX: u32 = 0x10_FFFF;
    let mut out = Vec::new();
    let mut cp = 0u32;
    while cp <= MAX {
        if !(0xD800..=0xDFFF).contains(&cp) {
            if let Some(gid) = sub.lookup(cp) {
                out.push((cp, GlyphId::new(gid)));
            }
        }
        cp += 1;
    }
    out
}

/// Decode a `name` record's bytes to an owned string.
fn decode_name(record: &font_parse::NameRecord<'_>) -> String {
    record.decode()
}

/// Load `OS/2` fields, using the model defaults for anything absent.
fn load_os2(t: &font_parse::Os2Table<'_>) -> Os2Metrics {
    let mut o = Os2Metrics::default();
    if let Ok(v) = t.version() {
        o.version = v;
    }
    if let Ok(v) = t.avg_char_width() {
        o.avg_char_width = v;
    }
    if let Ok(v) = t.weight_class() {
        o.weight_class = v;
    }
    if let Ok(v) = t.width_class() {
        o.width_class = v;
    }
    if let Ok(v) = t.fs_type() {
        o.fs_type = v;
    }
    if let Ok(p) = t.panose() {
        for (slot, byte) in o.panose.iter_mut().zip(p.iter().take(10)) {
            *slot = *byte;
        }
    }
    if let Ok(r) = t.unicode_ranges() {
        o.unicode_ranges = r;
    }
    if let Ok(v) = t.vendor_id() {
        for (slot, byte) in o.vendor_id.iter_mut().zip(v.iter().take(4)) {
            *slot = *byte;
        }
    }
    if let Ok(v) = t.fs_selection() {
        o.fs_selection = v;
    }
    if let Ok(v) = t.typo_ascender() {
        o.typo_ascender = v;
    }
    if let Ok(v) = t.typo_descender() {
        o.typo_descender = v;
    }
    if let Ok(v) = t.typo_line_gap() {
        o.typo_line_gap = v;
    }
    if let Ok(v) = t.win_ascent() {
        o.win_ascent = v;
    }
    if let Ok(v) = t.win_descent() {
        o.win_descent = v;
    }
    if let Ok(v) = t.code_page_ranges() {
        o.code_page_ranges = v;
    }
    if let Ok(v) = t.x_height() {
        o.x_height = v;
    }
    if let Ok(v) = t.cap_height() {
        o.cap_height = v;
    }
    o
}

/// Load `vhea` + `vmtx` into the vertical header, when both are present.
fn load_vertical(parsed: &FontRef<'_>) -> Option<crate::VerticalMetrics> {
    // Both tables must be present: `vhea` alone carries no per-glyph advances,
    // and `vmtx` alone has no line box to interpret it against.
    let vhea = parsed.table_by_tag(font_parse::Tag::new(b"vhea"))?;
    parsed.table_by_tag(font_parse::Tag::new(b"vmtx"))?;
    let be16 = |at: usize| -> i16 {
        match (vhea.get(at), vhea.get(at + 1)) {
            (Some(&a), Some(&b)) => i16::from_be_bytes([a, b]),
            _ => 0,
        }
    };
    Some(crate::VerticalMetrics {
        ascender: be16(4),
        descender: be16(6),
        line_gap: be16(8),
        advance_height_max: be16(10).max(0) as u16,
        min_top_side_bearing: be16(12),
        min_bottom_side_bearing: be16(14),
        y_max_extent: be16(16),
    })
}

/// Reject a `cmap` or kerning set that points at a glyph the store lacks.
fn validate_references(glyphs: &[Glyph], cmap: &Cmap, kerning: &Kerning) -> Result<(), ModelError> {
    for cp in cmap.codepoints().iter() {
        if let Some(gid) = cmap.lookup(*cp) {
            if glyphs.get(gid.index()).is_none() {
                return Err(ModelError::UnknownGlyph {
                    glyph: gid,
                    glyph_count: glyphs.len(),
                });
            }
        }
    }
    for pair in kerning.iter() {
        for gid in [pair.left, pair.right] {
            if glyphs.get(gid.index()).is_none() {
                return Err(ModelError::UnknownGlyph {
                    glyph: gid,
                    glyph_count: glyphs.len(),
                });
            }
        }
    }
    Ok(())
}

/// Convert a parser contour list into a resolved [`Outline`].
///
/// TrueType contours carry on-curve and off-curve (quadratic control) points.
/// Two off-curve points in a row imply an on-curve point at their midpoint,
/// and a contour that starts off-curve is started at the midpoint of its last
/// and first points — both handled here, so the resulting [`Path`] is a
/// proper segment stream rather than a raw point dump.
#[must_use]
pub fn to_outline(outline: GlyphOutline) -> Outline {
    let mut out = Outline::new();
    for contour in &outline.contours {
        if let Some(path) = contour_to_path(&contour.points) {
            out.push_contour(path);
        }
    }
    out
}

/// Convert one contour's points into a closed [`Path`].
fn contour_to_path(points: &[OutlinePoint]) -> Option<Path> {
    let n = points.len();
    if n == 0 {
        return None;
    }
    if n == 1 {
        let p = points.first()?;
        let mut path = Path::starting_at(p.x, p.y);
        path.close();
        return Some(path);
    }
    // Start at an on-curve point; if there is none, start at the implied
    // midpoint between the last and first off-curve points.
    let start_index = points.iter().position(|p| p.on_curve).unwrap_or(0);
    // The modulus keeps both lookups in range: `start_index < n` and `k < n`.
    let first = points.get(start_index)?;
    let last = points.get((start_index + n - 1) % n)?;
    let last_xy = (last.x, last.y);
    let (sx, sy) = if first.on_curve {
        (first.x, first.y)
    } else if last.on_curve {
        last_xy
    } else {
        midpoint(last_xy, (first.x, first.y))
    };
    let mut path = Path::starting_at(sx, sy);
    let mut pending_control: Option<(f32, f32)> = None;
    // Walk the contour from *after* the chosen start point, wrapping around.
    // The wrap-around segment back to the start is deliberately not emitted:
    // `close` accounts for it implicitly, and drawing it twice would leave a
    // redundant line in the segment stream.
    for k in 1..n {
        let Some(p) = points.get((start_index + k) % n) else {
            break;
        };
        if p.on_curve {
            match pending_control.take() {
                Some(c) => {
                    path.quad_to(c.0, c.1, p.x, p.y);
                }
                None => {
                    path.line_to(p.x, p.y);
                }
            }
        } else {
            if let Some(c) = pending_control {
                // Two controls in a row: the implied on-curve point between
                // them is the segment's endpoint.
                let m = midpoint(c, (p.x, p.y));
                path.quad_to(c.0, c.1, m.0, m.1);
            }
            pending_control = Some((p.x, p.y));
        }
    }
    // Close back to the start, finishing any dangling control.
    if let Some(c) = pending_control {
        path.quad_to(c.0, c.1, sx, sy);
    }
    path.close();
    Some(path)
}

/// The midpoint of two points.
fn midpoint(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}

/// Per-glyph metrics, extracted from a glyph store — what a writer serialises
/// into `hmtx`.
///
/// # Errors
/// Never in the current model; the signature keeps the door open for metrics
/// that can fail validation.
#[allow(clippy::result_large_err)]
pub fn hmtx_rows(glyphs: &[Glyph]) -> Result<Vec<Metrics>, ModelError> {
    glyphs.iter().map(Glyph::metrics).collect()
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::{contour_to_path, from_sfnt, hmtx_rows, to_outline};
    use crate::cmap::Cmap;
    use crate::font::{TAG_GPOS, TAG_GSUB};
    use crate::glyph::GlyphId;
    use crate::outline::{Command, Outline, Winding};
    use alloc::vec;
    use alloc::vec::Vec;

    // Test harness: assertions legitimately panic; the lib target stays
    // lint-clean.

    fn pt(x: f32, y: f32, on: bool) -> font_parse::OutlinePoint {
        font_parse::OutlinePoint { x, y, on_curve: on }
    }

    #[test]
    fn rejects_non_font_bytes() {
        assert!(from_sfnt(&[]).is_err());
        assert!(
            from_sfnt(&[0u8; 64]).is_err(),
            "a zeroed buffer is not a font"
        );
    }

    #[test]
    fn contour_with_only_on_curve_points_becomes_lines() {
        let path = contour_to_path(&[
            pt(0.0, 0.0, true),
            pt(100.0, 0.0, true),
            pt(0.0, 100.0, true),
        ])
        .expect("contour");
        // MoveTo + two lines + Close.
        assert_eq!(path.commands().len(), 4);
        assert!(matches!(path.commands()[1], Command::LineTo(..)));
        assert!(path.is_closed());
        // A right triangle with legs of 100: half of 100 x 100.
        assert!((path.area() - 5000.0).abs() < 1e-2, "{}", path.area());
    }

    #[test]
    fn contour_with_one_control_becomes_a_quad() {
        // on, off, on, on -> one quad then one line.
        let path = contour_to_path(&[
            pt(0.0, 0.0, true),
            pt(50.0, 100.0, false),
            pt(100.0, 0.0, true),
            pt(100.0, -100.0, true),
        ])
        .expect("contour");
        assert!(matches!(path.commands()[1], Command::QuadTo(..)));
        assert!(matches!(path.commands()[2], Command::LineTo(..)));
        assert_eq!(path.commands().len(), 4, "MoveTo, quad, line, Close");
        // Exact: the quad contributes -10000/3 of signed area and the closing
        // edge -5000, for -25000/3 overall.
        assert!(
            (path.signed_area() + 8_333.333).abs() < 1e-2,
            "area {}",
            path.signed_area()
        );
    }

    #[test]
    fn two_controls_in_a_row_imply_an_on_curve_midpoint() {
        // on, off, off, on: the two controls' midpoint becomes the endpoint
        // of the first quad, so the contour has two quads.
        let path = contour_to_path(&[
            pt(0.0, 0.0, true),
            pt(50.0, 100.0, false),
            pt(50.0, -100.0, false),
            pt(100.0, 0.0, true),
        ])
        .expect("contour");
        let quads = path
            .commands()
            .iter()
            .filter(|c| matches!(c, Command::QuadTo(..)))
            .count();
        assert_eq!(quads, 2, "implied midpoint split the segment");
        // Midpoint of (50,100) and (50,-100) is (50,0).
        assert!(matches!(
            path.commands()[1],
            Command::QuadTo(50.0, 100.0, 50.0, 0.0)
        ));
    }

    #[test]
    fn contour_starting_off_curve_starts_at_the_midpoint() {
        // off, on, off: start index 1 (the on-curve point), so the contour
        // begins there.
        let path = contour_to_path(&[
            pt(50.0, 100.0, false),
            pt(0.0, 0.0, true),
            pt(-50.0, 100.0, false),
        ])
        .expect("contour");
        assert_eq!(path.start_point(), (0.0, 0.0));
        assert!(path.is_closed());
        // A contour with no on-curve point at all starts at the implied
        // midpoint of the last and first points — here (0, 0) and (0, 100).
        let all_off = contour_to_path(&[
            pt(0.0, 0.0, false),
            pt(100.0, 0.0, false),
            pt(0.0, 100.0, false),
        ])
        .expect("contour");
        assert_eq!(all_off.start_point(), (0.0, 50.0));
        assert!(all_off.is_closed());
    }

    #[test]
    fn degenerate_contours() {
        assert!(contour_to_path(&[]).is_none());
        let single = contour_to_path(&[pt(5.0, 5.0, true)]).expect("contour");
        assert_eq!(single.len(), 2);
        assert_eq!(single.winding(), Winding::Degenerate);
        // All points identical: zero area, no panic.
        let same = contour_to_path(&[pt(1.0, 1.0, true), pt(1.0, 1.0, true), pt(1.0, 1.0, false)])
            .expect("contour");
        assert_eq!(same.area(), 0.0);
    }

    #[test]
    fn to_outline_assembles_contours() {
        let outline = font_parse::GlyphOutline {
            contours: vec![
                font_parse::Contour {
                    points: vec![
                        pt(0.0, 0.0, true),
                        pt(10.0, 0.0, true),
                        pt(10.0, 10.0, true),
                    ],
                },
                font_parse::Contour {
                    points: vec![
                        pt(20.0, 0.0, true),
                        pt(30.0, 0.0, true),
                        pt(30.0, 10.0, true),
                    ],
                },
                font_parse::Contour { points: Vec::new() },
            ],
        };
        let o = to_outline(outline);
        assert_eq!(o.len(), 2, "the empty contour was dropped");
        assert!(o.validate(GlyphId::new(1)).is_ok());
        assert_eq!(o.bbox().expect("bbox").max_x(), 30.0);
    }

    #[test]
    fn hmtx_rows_follow_the_glyph_store() {
        let glyphs = crate::glyph::stub_glyphs(&[0x41]);
        let rows = hmtx_rows(&glyphs).expect("rows");
        assert_eq!(rows.len(), glyphs.len());
        assert_eq!(rows[1].advance_width(), 600);
    }

    #[test]
    fn opaque_tag_helpers() {
        assert_eq!(TAG_GSUB, *b"GSUB");
        assert_eq!(TAG_GPOS, *b"GPOS");
        // The model's own tables are not carried opaquely.
        assert!(super::is_modelled(*b"cmap"));
        assert!(super::is_modelled(*b"head"));
        assert!(!super::is_modelled(*b"GSUB"));
        assert!(!super::is_modelled(*b"CFF "));
    }

    #[test]
    fn empty_outline_default_is_blank() {
        let o = Outline::new();
        assert!(to_outline(font_parse::GlyphOutline {
            contours: Vec::new()
        })
        .is_empty());
        assert!(o.is_empty());
        // A cmap with no subtables is tolerated by identity_cmap's guard.
        assert!(Cmap::new().lookup(0x41).is_none());
    }
}
