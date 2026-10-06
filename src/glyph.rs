//! Glyphs: [`GlyphId`] and [`Glyph`].

use alloc::vec::Vec;
use core::fmt;

use crate::cmap::Cmap;
use crate::error::ModelError;
use crate::outline::{Bbox, Outline};
use crate::Metrics;

/// A glyph index into the font's glyph store.
///
/// The index is `u16`, matching `maxp.numGlyphs` and every on-disk glyph
/// reference. `GlyphId(0)` is `.notdef` by universal convention.
///
/// ```
/// use font_model::GlyphId;
///
/// assert!(GlyphId::new(0) < GlyphId::new(1));
/// assert_eq!(GlyphId::new(0xFFFF).to_u32(), 65535);
/// // Construction is checked: a u32 beyond the glyph space is rejected.
/// assert!(GlyphId::try_from(70_000u32).is_err());
/// ```
///
/// A glyph index. Ordering is the numeric ordering of the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct GlyphId(u16);

impl GlyphId {
    /// `.notdef`, glyph index 0.
    pub const NOTDEF: GlyphId = GlyphId(0);
    /// The highest addressable glyph index.
    pub const MAX: GlyphId = GlyphId(u16::MAX);

    /// A glyph id from a `u16` index. Always valid — the type *is* `u16`.
    #[must_use]
    pub const fn new(index: u16) -> Self {
        GlyphId(index)
    }

    /// A glyph id from a `u32`, checked against the glyph space.
    ///
    /// # Errors
    /// [`ModelError::UnknownGlyph`] when the value exceeds `u16::MAX`.
    #[allow(clippy::result_large_err)]
    pub fn try_from_u32(value: u32) -> Result<Self, ModelError> {
        u32::try_into(value)
            .map(GlyphId)
            .map_err(|_| ModelError::UnknownGlyph {
                glyph: GlyphId(u16::MAX),
                glyph_count: 0,
            })
    }

    /// The raw `u16` index.
    #[must_use]
    pub const fn to_u16(self) -> u16 {
        self.0
    }

    /// The index as a `u32`, for arithmetic with codepoints and offsets.
    #[must_use]
    pub const fn to_u32(self) -> u32 {
        self.0 as u32
    }

    /// The index as a `usize` slot.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// True for glyph index 0.
    #[must_use]
    pub const fn is_notdef(self) -> bool {
        self.0 == 0
    }
}

impl From<u16> for GlyphId {
    fn from(v: u16) -> Self {
        GlyphId(v)
    }
}

impl TryFrom<u32> for GlyphId {
    type Error = ModelError;

    fn try_from(v: u32) -> Result<Self, Self::Error> {
        GlyphId::try_from_u32(v)
    }
}

impl TryFrom<usize> for GlyphId {
    type Error = ModelError;

    fn try_from(v: usize) -> Result<Self, Self::Error> {
        u32::try_from(v)
            .map_err(|_| ModelError::UnknownGlyph {
                glyph: GlyphId(u16::MAX),
                glyph_count: 0,
            })
            .and_then(GlyphId::try_from)
    }
}

impl fmt::Display for GlyphId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gid{}", self.0)
    }
}

/// One glyph: an index, its horizontal metrics, and its outline.
///
/// Metrics live on the glyph rather than in a side table because that is what
/// an editor mutates and what a writer serialises; [`Metrics`] is the *header*
/// view derived from them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Glyph {
    id: GlyphId,
    advance_width: u16,
    left_side_bearing: i16,
    outline: Outline,
}

impl Glyph {
    /// A glyph from its parts.
    #[must_use]
    pub const fn new(
        id: GlyphId,
        advance_width: u16,
        left_side_bearing: i16,
        outline: Outline,
    ) -> Self {
        Glyph {
            id,
            advance_width,
            left_side_bearing,
            outline,
        }
    }

    /// An empty (space-like) glyph with no ink.
    #[must_use]
    pub fn empty(id: GlyphId, advance_width: u16) -> Self {
        Glyph {
            id,
            advance_width,
            left_side_bearing: 0,
            outline: Outline::new(),
        }
    }

    /// The glyph index.
    #[must_use]
    pub const fn id(&self) -> GlyphId {
        self.id
    }

    /// The advance width in font units.
    #[must_use]
    pub const fn advance_width(&self) -> u16 {
        self.advance_width
    }

    /// Set the advance width.
    pub fn set_advance_width(&mut self, advance: u16) {
        self.advance_width = advance;
    }

    /// The left side bearing in font units.
    #[must_use]
    pub const fn left_side_bearing(&self) -> i16 {
        self.left_side_bearing
    }

    /// Set the left side bearing.
    pub fn set_left_side_bearing(&mut self, lsb: i16) {
        self.left_side_bearing = lsb;
    }

    /// The outline.
    #[must_use]
    pub const fn outline(&self) -> &Outline {
        &self.outline
    }

    /// The outline, mutably.
    pub fn outline_mut(&mut self) -> &mut Outline {
        &mut self.outline
    }

    /// True when the glyph has no ink.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.outline.is_empty()
    }

    /// The glyph's exact bounding box, `None` when it has no ink.
    #[must_use]
    pub fn bbox(&self) -> Option<Bbox> {
        self.outline.bbox()
    }

    /// The glyph's metrics as a [`Metrics`] pair.
    ///
    /// # Errors
    /// Never in the current model; the signature keeps the door open for
    /// metrics that can fail validation.
    #[allow(clippy::result_large_err)]
    pub fn metrics(&self) -> Result<Metrics, ModelError> {
        Ok(Metrics::new(self.advance_width, self.left_side_bearing))
    }

    /// Recompute the metrics from the outline: the left side bearing becomes
    /// the outline's minimum x, and the advance becomes the outline's width
    /// (rounded up), or 0 for an empty glyph.
    ///
    /// # Errors
    /// [`ModelError::AdvanceOverflow`] when the outline is wider than
    /// `u16::MAX` font units.
    pub fn recompute_metrics(&mut self) -> Result<(), ModelError> {
        let (advance, lsb) = Metrics::advance_for(&self.outline, self.id)?;
        self.advance_width = advance;
        self.left_side_bearing = lsb;
        Ok(())
    }

    /// True when the outline has ink but no advance.
    #[must_use]
    pub fn has_ink_without_advance(&self) -> bool {
        self.advance_width == 0 && !self.outline.is_empty()
    }

    /// Validate this glyph's own invariants: finite coordinates, closed
    /// contours, non-zero advance when there is ink.
    ///
    /// # Errors
    /// Whatever [`Outline::validate`] reports, plus
    /// [`ModelError::ZeroAdvance`].
    pub fn validate(&self) -> Result<(), ModelError> {
        self.outline.validate(self.id)?;
        if self.has_ink_without_advance() {
            return Err(ModelError::ZeroAdvance { glyph: self.id });
        }
        Ok(())
    }
}

/// Validate a whole glyph store: the ids are dense and ascending, and every
/// glyph passes its own invariants.
///
/// # Errors
/// [`ModelError::UnknownGlyph`] when glyph `n` carries id `n' != n`;
/// otherwise the first per-glyph violation.
pub fn validate_glyphs(glyphs: &[Glyph]) -> Result<(), ModelError> {
    for (n, g) in glyphs.iter().enumerate() {
        if g.id().index() != n {
            return Err(ModelError::UnknownGlyph {
                glyph: g.id(),
                glyph_count: n,
            });
        }
        g.validate()?;
    }
    Ok(())
}

/// Build a minimal font-shaped glyph set: `.notdef`, plus one empty glyph per
/// requested codepoint. The starting point for a generated font.
#[must_use]
pub fn stub_glyphs(codepoints: &[u32]) -> Vec<Glyph> {
    let mut out = Vec::with_capacity(codepoints.len() + 1);
    out.push(Glyph::new(GlyphId::NOTDEF, 600, 0, notdef_outline()));
    for (i, cp) in codepoints.iter().enumerate() {
        let gid = GlyphId::new(u16::try_from(i + 1).unwrap_or(u16::MAX));
        let _ = cp;
        out.push(Glyph::empty(gid, 600));
    }
    out
}

/// A rectangular `.notdef` box: the conventional hollow rectangle.
fn notdef_outline() -> Outline {
    let mut o = Outline::new();
    let mut p = crate::Path::starting_at(50.0, 0.0);
    p.line_to(550.0, 0.0);
    p.line_to(550.0, 700.0);
    p.line_to(50.0, 700.0);
    p.close();
    o.push_contour(p);
    let mut h = crate::Path::starting_at(100.0, 50.0);
    h.line_to(100.0, 650.0);
    h.line_to(500.0, 650.0);
    h.line_to(500.0, 50.0);
    h.close();
    o.push_contour(h);
    o
}

/// A `cmap` for a glyph store: identity over `codepoints`, i.e. the `i`-th
/// requested codepoint maps to glyph `i + 1` and nothing maps to `.notdef`.
///
/// Codepoints are split by plane — format 4 below the BMP, format 12 above it
/// — because format 4 cannot express an astral codepoint at all.
#[must_use]
pub fn identity_cmap(codepoints: &[u32]) -> Cmap {
    let entries: Vec<(u32, GlyphId)> = codepoints
        .iter()
        .enumerate()
        .map(|(i, cp)| (*cp, GlyphId::new(u16::try_from(i + 1).unwrap_or(u16::MAX))))
        .collect();
    let mut bmp: Vec<(u32, GlyphId)> = entries
        .iter()
        .copied()
        .filter(|(cp, _)| *cp <= 0xFFFF)
        .collect();
    let astral: Vec<(u32, GlyphId)> = entries
        .iter()
        .copied()
        .filter(|(cp, _)| *cp > 0xFFFF)
        .collect();
    bmp.sort_by_key(|(cp, _)| *cp);
    let mut c = Cmap::new();
    if !bmp.is_empty() {
        c.subtables_mut().push(crate::cmap::from_entries(&bmp, 4));
    }
    if !astral.is_empty() {
        c.subtables_mut()
            .push(crate::cmap::from_entries(&astral, 12));
    }
    if c.subtables().is_empty() {
        c.subtables_mut().push(crate::cmap::from_entries(&[], 4));
    }
    c
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::{identity_cmap, stub_glyphs, validate_glyphs, Glyph, GlyphId};
    use crate::{ModelError, Outline, Path};
    use alloc::string::ToString;
    use alloc::vec;
    use alloc::vec::Vec;

    // Test harness: assertions legitimately panic; the lib target stays
    // lint-clean.

    fn box_outline() -> Outline {
        let mut o = Outline::new();
        let mut p = Path::starting_at(100.0, 0.0);
        p.line_to(500.0, 0.0);
        p.line_to(500.0, 400.0);
        p.line_to(100.0, 400.0);
        p.close();
        o.push_contour(p);
        o
    }

    #[test]
    fn glyph_id_construction_and_bounds() {
        assert_eq!(GlyphId::new(0), GlyphId::NOTDEF);
        assert!(GlyphId::new(0).is_notdef());
        assert!(!GlyphId::new(1).is_notdef());
        assert_eq!(GlyphId::new(0xFFFF), GlyphId::MAX);
        assert_eq!(GlyphId::MAX.to_u16(), u16::MAX);
        assert_eq!(GlyphId::new(7).to_u32(), 7);
        assert_eq!(GlyphId::new(7).index(), 7);
        assert_eq!(GlyphId::new(0xFFFF).index(), 65535);
    }

    #[test]
    fn glyph_id_ordering_is_numeric() {
        assert!(GlyphId::new(0) < GlyphId::new(1));
        assert!(GlyphId::new(9) < GlyphId::new(10));
        assert!(GlyphId::new(100) > GlyphId::new(99));
        let mut v = vec![
            GlyphId::new(300),
            GlyphId::new(1),
            GlyphId::new(42),
            GlyphId::new(0),
        ];
        v.sort();
        assert_eq!(
            v,
            vec![
                GlyphId::new(0),
                GlyphId::new(1),
                GlyphId::new(42),
                GlyphId::new(300)
            ]
        );
    }

    #[test]
    fn glyph_id_try_from_is_checked() {
        assert_eq!(GlyphId::try_from(5u32).expect("fits"), GlyphId::new(5));
        assert_eq!(GlyphId::from(5u16), GlyphId::new(5));
        assert_eq!(GlyphId::try_from(0xFFFFu32).expect("fits"), GlyphId::MAX);
        assert!(GlyphId::try_from(70_000u32).is_err());
        assert!(GlyphId::try_from(usize::MAX).is_err());
        assert_eq!(GlyphId::try_from(3usize).expect("fits"), GlyphId::new(3));
        assert!(GlyphId::try_from_u32(0).is_ok());
        assert!(GlyphId::try_from_u32(u32::MAX).is_err());
    }

    #[test]
    fn glyph_id_display_and_hash() {
        assert_eq!(GlyphId::new(12).to_string(), "gid12");
        use core::hash::{Hash, Hasher};
        // `DefaultHasher` lives in `std`, so hash through a fixed-value
        // `Hasher` instead — the point is that equal ids hash equal.
        struct Zero(u64);
        impl Hasher for Zero {
            fn finish(&self) -> u64 {
                self.0
            }
            fn write(&mut self, bytes: &[u8]) {
                for &b in bytes {
                    self.0 = self.0.wrapping_mul(31).wrapping_add(u64::from(b));
                }
            }
        }
        let mut h1 = Zero(0);
        let mut h2 = Zero(0);
        GlyphId::new(3).hash(&mut h1);
        GlyphId::new(3).hash(&mut h2);
        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn glyph_accessors_and_setters() {
        let mut g = Glyph::new(GlyphId::new(4), 500, 100, box_outline());
        assert_eq!(g.id(), GlyphId::new(4));
        assert_eq!(g.advance_width(), 500);
        assert_eq!(g.left_side_bearing(), 100);
        assert_eq!(g.outline().len(), 1);
        assert!(!g.is_empty());
        g.set_advance_width(600);
        g.set_left_side_bearing(50);
        assert_eq!(g.advance_width(), 600);
        assert_eq!(g.left_side_bearing(), 50);
        g.outline_mut().contours_mut().clear();
        assert!(g.is_empty());
        assert!(g.bbox().is_none());
        assert_eq!(g.metrics().expect("metrics").advance_width(), 600);
    }

    #[test]
    fn recompute_metrics_from_outline() {
        let mut g = Glyph::new(GlyphId::new(1), 0, 0, box_outline());
        g.recompute_metrics().expect("recomputes");
        assert_eq!(g.left_side_bearing(), 100, "lsb is the outline's min x");
        assert_eq!(g.advance_width(), 400, "advance is the outline's width");
        // An empty glyph recomputes to a zero advance.
        let mut e = Glyph::empty(GlyphId::new(2), 300);
        e.recompute_metrics().expect("recomputes");
        assert_eq!(e.advance_width(), 0);
        assert_eq!(e.left_side_bearing(), 0);
    }

    #[test]
    fn recompute_reports_overflow() {
        let mut o = Outline::new();
        let mut p = Path::starting_at(0.0, 0.0);
        p.line_to(70_000.0, 0.0);
        p.line_to(70_000.0, 10.0);
        p.close();
        o.push_contour(p);
        let mut g = Glyph::new(GlyphId::new(1), 0, 0, o);
        match g.recompute_metrics().unwrap_err() {
            ModelError::AdvanceOverflow { glyph, advance } => {
                assert_eq!(glyph, GlyphId::new(1));
                assert!(advance > 65_535);
            }
            other => panic!("expected AdvanceOverflow, got {other:?}"),
        }
    }

    #[test]
    fn glyph_validate_rejects_zero_advance_with_ink() {
        let g = Glyph::new(GlyphId::new(8), 0, 0, box_outline());
        assert!(g.has_ink_without_advance());
        match g.validate().unwrap_err() {
            ModelError::ZeroAdvance { glyph } => assert_eq!(glyph, GlyphId::new(8)),
            other => panic!("expected ZeroAdvance, got {other:?}"),
        }
        // Empty glyph with zero advance is legal.
        let e = Glyph::empty(GlyphId::new(8), 0);
        assert!(!e.has_ink_without_advance());
        assert!(e.validate().is_ok());
    }

    #[test]
    fn glyph_validate_accepts_well_formed() {
        let g = Glyph::new(GlyphId::new(8), 400, 100, box_outline());
        assert!(g.validate().is_ok());
    }

    #[test]
    fn validate_glyphs_requires_dense_ids() {
        let ok = stub_glyphs(&[0x41, 0x42]);
        assert!(validate_glyphs(&ok).is_ok());
        // A store whose second glyph claims a different id.
        let mut broken = ok.clone();
        broken[1] = Glyph::empty(GlyphId::new(9), 100);
        match validate_glyphs(&broken).unwrap_err() {
            ModelError::UnknownGlyph { glyph, glyph_count } => {
                assert_eq!(glyph, GlyphId::new(9));
                assert_eq!(glyph_count, 1);
            }
            other => panic!("expected UnknownGlyph, got {other:?}"),
        }
        // Propagates a per-glyph violation.
        let mut bad = stub_glyphs(&[0x41]);
        bad[1] = Glyph::new(GlyphId::new(1), 0, 0, box_outline());
        assert!(matches!(
            validate_glyphs(&bad).unwrap_err(),
            ModelError::ZeroAdvance { .. }
        ));
        assert!(validate_glyphs(&[]).is_ok());
    }

    #[test]
    fn stub_glyphs_are_well_formed() {
        let gs = stub_glyphs(&[0x41, 0x42, 0x43]);
        assert_eq!(gs.len(), 4);
        assert_eq!(gs[0].id(), GlyphId::NOTDEF);
        assert_eq!(gs[3].id(), GlyphId::new(3));
        assert!(!gs[0].is_empty(), ".notdef has a box");
        assert!(gs[1].is_empty());
        assert!(validate_glyphs(&gs).is_ok());
        // Saturates rather than panicking at the top of the glyph space.
        let many: Vec<u32> = (0..70_000u32).collect();
        assert_eq!(stub_glyphs(&many).len(), 70_001);
    }

    #[test]
    fn identity_cmap_matches_the_glyph_store() {
        let cps = [0x41u32, 0x42, 0x1F600];
        let gs = stub_glyphs(&cps);
        let cmap = identity_cmap(&cps);
        assert_eq!(cmap.lookup(0x41), Some(GlyphId::new(1)));
        assert_eq!(cmap.lookup(0x42), Some(GlyphId::new(2)));
        assert_eq!(cmap.lookup(0x1F600), Some(GlyphId::new(3)));
        assert_eq!(cmap.lookup(0x43), None);
        // Every mapped codepoint points at a glyph that exists.
        for cp in cmap.codepoints().iter() {
            let gid = cmap.lookup(*cp).expect("mapped");
            assert!(gs.get(gid.index()).is_some(), "dangling {gid}");
        }
        // A large codepoint list saturates the glyph id without panicking.
        let many: Vec<u32> = (0..70_000u32).collect();
        let cmap = identity_cmap(&many);
        assert!(cmap.lookup(65_536).is_some());
    }

    #[test]
    fn glyph_default_is_notdef() {
        let g = Glyph::default();
        assert_eq!(g.id(), GlyphId::NOTDEF);
        assert!(g.is_empty());
        assert_eq!(g.advance_width(), 0);
    }
}
