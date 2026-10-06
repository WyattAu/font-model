//! Font metrics: [`Metrics`], [`MetricsBuilder`], and the vertical set.

use alloc::vec::Vec;

use crate::error::ModelError;
use crate::outline::Outline;
use crate::{Glyph, GlyphId};

/// Horizontal metrics for one glyph: the `hmtx` pair.
///
/// This is the small, per-glyph view. The *header* view — ascender,
/// descender, line gap, and the min/max extents — is [`FontMetrics`].
///
/// ```
/// use font_model::{Metrics, Outline, Path};
///
/// // A box from x = 100 to x = 500: lsb 100, advance 400.
/// let mut o = Outline::new();
/// let mut p = Path::starting_at(100.0, 0.0);
/// p.line_to(500.0, 0.0);
/// p.line_to(500.0, 400.0);
/// p.close();
/// o.push_contour(p);
///
/// let (advance, lsb) = Metrics::advance_for(&o, font_model::GlyphId::new(1)).unwrap();
/// assert_eq!((advance, lsb), (400, 100));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Metrics {
    /// The advance width in font units (`hmtx.advanceWidth`).
    pub advance_width: u16,
    /// The left side bearing in font units (`hmtx.lsb`).
    pub left_side_bearing: i16,
}

impl Metrics {
    /// A metrics pair.
    #[must_use]
    pub const fn new(advance_width: u16, left_side_bearing: i16) -> Self {
        Metrics {
            advance_width,
            left_side_bearing,
        }
    }

    /// The advance width.
    #[must_use]
    pub const fn advance_width(&self) -> u16 {
        self.advance_width
    }

    /// The left side bearing.
    #[must_use]
    pub const fn left_side_bearing(&self) -> i16 {
        self.left_side_bearing
    }

    /// The right side bearing, given the outline width: the advance minus the
    /// ink width. `None` for an empty glyph.
    #[must_use]
    pub fn right_side_bearing(&self, outline: &Outline) -> Option<i16> {
        let b = outline.bbox()?;
        let ink = crate::num::round(b.width()) as i32;
        let advance = i32::from(self.advance_width);
        i16::try_from(advance - ink).ok()
    }

    /// Compute `(advance, lsb)` for an outline.
    ///
    /// The left side bearing is the outline's minimum x. The advance is the
    /// outline's width — `ceil`ed, so a fractional width always gets its full
    /// ink — or `0` for an empty outline.
    ///
    /// # Errors
    /// [`ModelError::AdvanceOverflow`] when the outline is wider than
    /// `u16::MAX` font units.
    pub fn advance_for(outline: &Outline, glyph: GlyphId) -> Result<(u16, i16), ModelError> {
        let Some(bbox) = outline.bbox() else {
            return Ok((0, 0));
        };
        // The advance comes from the ink's width, so it is computed before the
        // bearing: an outline far to the left of the origin has a representable
        // width and an unrepresentable `min_x`.
        let advance = crate::num::ceil(f64::from(bbox.width()));
        if advance > f64::from(u16::MAX) {
            return Err(ModelError::AdvanceOverflow {
                glyph,
                advance: advance as i32,
            });
        }
        // A `min_x` outside `i16` saturates rather than failing: the width is
        // still the honest answer, and the alternative is rejecting a glyph
        // whose advance is fine.
        let lsb = crate::num::to_i32(crate::num::round(bbox.min_x()))
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        Ok((advance as u16, lsb))
    }
}

/// A running maximum/minimum over a glyph store's horizontal extents.
///
/// Every minimum is seeded from the *first* value rather than from zero: a
/// font whose bearings are all positive must report a positive minimum, and a
/// zero seed would silently clamp it. A `max` of zero is correct (no glyph
/// cannot have a negative advance) and is the identity for that accumulator.
#[derive(Debug, Clone, Copy, Default)]
struct Extents {
    advance_width_max: u16,
    min_left_side_bearing: Option<i16>,
    min_right_side_bearing: Option<i16>,
    x_max_extent: i16,
}

impl Extents {
    /// Fold one glyph into the running values.
    fn add(&mut self, g: &Glyph) {
        self.advance_width_max = self.advance_width_max.max(g.advance_width());
        let lsb = g.left_side_bearing();
        self.min_left_side_bearing = Some(match self.min_left_side_bearing {
            Some(m) => m.min(lsb),
            None => lsb,
        });
        let Some(bbox) = g.bbox() else {
            return;
        };
        let ink = crate::num::round(bbox.width()) as i32;
        let rsb = i32::from(g.advance_width()) - ink;
        if let Ok(v) = i16::try_from(rsb) {
            self.min_right_side_bearing = Some(match self.min_right_side_bearing {
                Some(m) => m.min(v),
                None => v,
            });
        }
        let extent = i32::from(lsb) + crate::num::round(bbox.max_x()) as i32;
        if let Ok(e) = i16::try_from(extent) {
            self.x_max_extent = self.x_max_extent.max(e);
        }
    }

    /// The header these extents produce.
    fn into_header(
        self,
        ascender: i16,
        descender: i16,
        vertical: Option<VerticalMetrics>,
    ) -> FontMetrics {
        FontMetrics {
            ascender,
            descender,
            line_gap: 0,
            advance_width_max: self.advance_width_max,
            // An empty store reports the OpenType-recommended zeros rather
            // than inventing a minimum.
            min_left_side_bearing: self.min_left_side_bearing.unwrap_or(0),
            min_right_side_bearing: self.min_right_side_bearing.unwrap_or(0),
            x_max_extent: self.x_max_extent,
            vertical,
        }
    }
}

/// The `hhea` header: global vertical metrics plus the horizontal extents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontMetrics {
    /// Typographic ascender, in font units (positive).
    pub ascender: i16,
    /// Typographic descender, in font units (negative).
    pub descender: i16,
    /// Typographic line gap, in font units.
    pub line_gap: i16,
    /// The largest advance width across the font.
    pub advance_width_max: u16,
    /// The smallest left side bearing across the font.
    pub min_left_side_bearing: i16,
    /// The smallest right side bearing across the font.
    pub min_right_side_bearing: i16,
    /// The largest `lsb + xMax` across the font (`xMaxExtent`).
    pub x_max_extent: i16,
    /// Vertical metrics, when the font carries a `vmtx`.
    pub vertical: Option<VerticalMetrics>,
}

impl Default for FontMetrics {
    fn default() -> Self {
        // The OpenType-recommended defaults for a Latin face at 1000 upem.
        FontMetrics {
            ascender: 800,
            descender: -200,
            line_gap: 0,
            advance_width_max: 0,
            min_left_side_bearing: 0,
            min_right_side_bearing: 0,
            x_max_extent: 0,
            vertical: None,
        }
    }
}

impl FontMetrics {
    /// Vertical metrics with the given vertical line box.
    #[must_use]
    pub const fn with_vertical(self, vertical: VerticalMetrics) -> Self {
        FontMetrics {
            vertical: Some(vertical),
            ..self
        }
    }

    /// The typographic line height: ascender − descender + line gap.
    #[must_use]
    pub fn line_height(&self) -> i32 {
        i32::from(self.ascender) - i32::from(self.descender) + i32::from(self.line_gap)
    }

    /// The `vhea`-equivalent line height when vertical metrics exist.
    #[must_use]
    pub fn vertical_line_height(&self) -> Option<i32> {
        self.vertical
            .map(|v| i32::from(v.ascender) - i32::from(v.descender) + i32::from(v.line_gap))
    }

    /// Derive the header extents from a glyph store.
    ///
    /// The horizontal extents are **exact** functions of the glyphs: the
    /// maxima start at zero and the minima are the true smallest values, so a
    /// font whose bearings are all positive reports a positive
    /// `min_left_side_bearing` rather than being clamped to the field's zero.
    /// [`FontMetrics::reset_extents`] is that starting state.
    ///
    /// The ascender and descender are the exception: they *grow* from
    /// [`FontMetrics::default`]'s recommended 800/−200, so a glyph store that
    /// never descends below the baseline still gets a line box with room for
    /// descenders.
    #[must_use]
    pub fn derive(glyphs: &[Glyph]) -> FontMetrics {
        let mut acc = Extents::default();
        let mut ascender = Self::default().ascender;
        let mut descender = Self::default().descender;
        for g in glyphs {
            acc.add(g);
            if let Some(bbox) = g.bbox() {
                if let Ok(y) = i16::try_from(crate::num::round(bbox.max_y()) as i32) {
                    ascender = ascender.max(y);
                }
                if let Ok(y) = i16::try_from(crate::num::round(bbox.min_y()) as i32) {
                    descender = descender.min(y);
                }
            }
        }
        acc.into_header(ascender, descender, None)
    }

    /// Zero the horizontal extents, leaving the vertical line box alone.
    pub fn reset_extents(&mut self) {
        let ascender = self.ascender;
        let descender = self.descender;
        let line_gap = self.line_gap;
        let vertical = self.vertical;
        *self = Extents::default().into_header(ascender, descender, vertical);
        self.line_gap = line_gap;
    }
}

/// The `vhea`/`vmtx` vertical header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VerticalMetrics {
    /// Vertical ascender, in font units.
    pub ascender: i16,
    /// Vertical descender, in font units.
    pub descender: i16,
    /// Vertical line gap, in font units.
    pub line_gap: i16,
    /// The largest vertical advance across the font.
    pub advance_height_max: u16,
    /// The smallest top side bearing across the font.
    pub min_top_side_bearing: i16,
    /// The smallest bottom side bearing across the font.
    pub min_bottom_side_bearing: i16,
    /// The largest `tsb + yMax` across the font.
    pub y_max_extent: i16,
}

impl VerticalMetrics {
    /// The vertical line height: ascender − descender + line gap.
    #[must_use]
    pub fn line_height(&self) -> i32 {
        i32::from(self.ascender) - i32::from(self.descender) + i32::from(self.line_gap)
    }
}

/// Computes a glyph store's metrics from its outlines.
///
/// The builder is the round trip: `build()` produces the header, and
/// [`MetricsBuilder::advance_for`] is the per-glyph function
/// [`Glyph::recompute_metrics`] uses, so a store built through the builder
/// and a store whose glyphs recompute themselves always agree.
///
/// ```
/// use font_model::{FontMetrics, Glyph, GlyphId, MetricsBuilder, Outline, Path};
///
/// let mut o = Outline::new();
/// let mut p = Path::starting_at(100.0, 0.0);
/// p.line_to(500.0, 0.0);
/// p.line_to(500.0, 400.0);
/// p.close();
/// o.push_contour(p);
///
/// let mut g = Glyph::new(GlyphId::new(1), 0, 0, o);
/// g.recompute_metrics().expect("recomputes");
/// assert_eq!(g.advance_width(), 400);
///
/// // The builder reads the glyph's *stored* metrics, so recompute before
/// // pushing — otherwise the header describes the pre-recompute glyph.
/// let mut b = MetricsBuilder::new();
/// b.push(&g);
/// let derived = b.build();
/// assert_eq!(derived.advance_width_max, 400);
/// assert_eq!(derived.min_left_side_bearing, 100);
/// assert_eq!(derived.x_max_extent, 600, "lsb + xMax");
///
/// // And the header derived straight from the store agrees.
/// assert_eq!(FontMetrics::derive(std::slice::from_ref(&g)), derived);
/// ```
#[derive(Debug, Clone)]
pub struct MetricsBuilder {
    ascender: i16,
    descender: i16,
    line_gap: i16,
    extents: Extents,
    seen: Vec<GlyphId>,
    vertical: Option<VerticalMetrics>,
}

impl Default for MetricsBuilder {
    fn default() -> Self {
        MetricsBuilder::new()
    }
}

impl MetricsBuilder {
    /// A builder with no glyphs and the OpenType-recommended vertical
    /// defaults.
    #[must_use]
    pub fn new() -> Self {
        MetricsBuilder {
            ascender: 800,
            descender: -200,
            line_gap: 0,
            extents: Extents::default(),
            seen: Vec::new(),
            vertical: None,
        }
    }

    /// Override the typographic ascender.
    #[must_use]
    pub const fn with_ascender(mut self, ascender: i16) -> Self {
        self.ascender = ascender;
        self
    }

    /// Override the typographic descender.
    #[must_use]
    pub const fn with_descender(mut self, descender: i16) -> Self {
        self.descender = descender;
        self
    }

    /// Override the typographic line gap.
    #[must_use]
    pub const fn with_line_gap(mut self, line_gap: i16) -> Self {
        self.line_gap = line_gap;
        self
    }

    /// Attach vertical metrics.
    #[must_use]
    pub fn with_vertical(mut self, vertical: VerticalMetrics) -> Self {
        self.vertical = Some(vertical);
        self
    }

    /// Feed a glyph into the running maxima. The glyph's *stored* metrics are
    /// used; call [`Glyph::recompute_metrics`] first if the outline changed.
    pub fn push(&mut self, glyph: &Glyph) -> &mut Self {
        // The same accumulation as [`FontMetrics::derive`], so a header built
        // here and one derived from the store are the same function of the
        // glyphs.
        self.extents.add(glyph);
        self.seen.push(glyph.id());
        self
    }

    /// Number of glyphs fed in.
    #[must_use]
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// True when no glyph has been fed in.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    /// The `(advance, lsb)` an outline would get — the same function
    /// [`Glyph::recompute_metrics`] uses.
    ///
    /// # Errors
    /// [`ModelError::AdvanceOverflow`] when the outline is too wide for
    /// `hmtx`.
    pub fn advance_for(outline: &Outline, glyph: GlyphId) -> Result<(u16, i16), ModelError> {
        Metrics::advance_for(outline, glyph)
    }

    /// The header.
    #[must_use]
    pub fn build(&self) -> FontMetrics {
        let mut header = self
            .extents
            .into_header(self.ascender, self.descender, self.vertical);
        header.line_gap = self.line_gap;
        header
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::{FontMetrics, Metrics, MetricsBuilder, VerticalMetrics};
    use crate::{Glyph, GlyphId, ModelError, Outline, Path};
    use alloc::vec::Vec;

    // Test harness: assertions legitimately panic; the lib target stays
    // lint-clean.

    /// A box outline from `x0` to `x1`, `y` from 0 to `h`.
    fn box_at(x0: f32, y0: f32, x1: f32, y1: f32) -> Outline {
        let mut o = Outline::new();
        let mut p = Path::starting_at(x0, y0);
        p.line_to(x1, y0);
        p.line_to(x1, y1);
        p.line_to(x0, y1);
        p.close();
        o.push_contour(p);
        o
    }

    #[test]
    fn metrics_pair_accessors() {
        let m = Metrics::new(500, 42);
        assert_eq!(m.advance_width(), 500);
        assert_eq!(m.left_side_bearing(), 42);
        assert_eq!(
            m,
            Metrics {
                advance_width: 500,
                left_side_bearing: 42
            }
        );
        assert_eq!(Metrics::default(), Metrics::new(0, 0));
    }

    #[test]
    fn right_side_bearing_from_outline() {
        let m = Metrics::new(400, 100);
        assert_eq!(
            m.right_side_bearing(&box_at(100.0, 0.0, 500.0, 400.0)),
            Some(0)
        );
        // An advance wider than the ink leaves a positive RSB.
        assert_eq!(
            m.right_side_bearing(&box_at(100.0, 0.0, 300.0, 400.0)),
            Some(200)
        );
        // Ink wider than the advance gives a negative RSB, which is legal.
        assert_eq!(
            m.right_side_bearing(&box_at(0.0, 0.0, 500.0, 10.0)),
            Some(-100)
        );
        // A right side bearing outside i16 is reported as absent rather than
        // wrapping to an unrelated value.
        let narrow = Metrics::new(0, 0);
        assert_eq!(
            narrow.right_side_bearing(&box_at(0.0, 0.0, 40_000.0, 10.0)),
            None
        );
        // No ink, no right side bearing.
        assert_eq!(m.right_side_bearing(&Outline::new()), None);
    }

    #[test]
    fn advance_for_computes_from_the_outline() {
        let (advance, lsb) =
            Metrics::advance_for(&box_at(100.0, 0.0, 500.0, 400.0), GlyphId::new(1))
                .expect("computes");
        assert_eq!(advance, 400);
        assert_eq!(lsb, 100);
        // Empty outline: no ink, no advance.
        assert_eq!(
            Metrics::advance_for(&Outline::new(), GlyphId::new(1)).expect("computes"),
            (0, 0)
        );
        // A negative minimum x becomes a negative bearing.
        assert_eq!(
            Metrics::advance_for(&box_at(-50.0, 0.0, 50.0, 10.0), GlyphId::new(1))
                .expect("computes"),
            (100, -50)
        );
    }

    #[test]
    fn advance_for_rounds_a_fractional_width_up() {
        let (advance, _) = Metrics::advance_for(&box_at(0.0, 0.0, 100.4, 10.0), GlyphId::new(1))
            .expect("computes");
        assert_eq!(advance, 101, "a fractional ink width gets its full ink");
    }

    #[test]
    fn advance_for_reports_overflow() {
        match Metrics::advance_for(&box_at(0.0, 0.0, 70_000.0, 10.0), GlyphId::new(3)).unwrap_err()
        {
            ModelError::AdvanceOverflow { glyph, .. } => assert_eq!(glyph, GlyphId::new(3)),
            other => panic!("expected AdvanceOverflow, got {other:?}"),
        }
    }

    #[test]
    fn advance_for_saturates_an_unrepresentable_bearing() {
        // min x = -40000 is outside i16, so the bearing saturates at i16::MIN
        // while the advance — the ink's width — is still the honest 1000.
        let (advance, lsb) =
            Metrics::advance_for(&box_at(-40_000.0, 0.0, -39_000.0, 10.0), GlyphId::new(1))
                .expect("computes");
        assert_eq!(advance, 1000);
        assert_eq!(lsb, i16::MIN);
        // And the other direction.
        let (_, lsb) =
            Metrics::advance_for(&box_at(40_000.0, 0.0, 41_000.0, 10.0), GlyphId::new(1))
                .expect("computes");
        assert_eq!(lsb, i16::MAX);
    }

    #[test]
    fn font_metrics_defaults_and_line_height() {
        let m = FontMetrics::default();
        assert_eq!(m.ascender, 800);
        assert_eq!(m.descender, -200);
        assert_eq!(m.line_gap, 0);
        assert_eq!(m.line_height(), 1000);
        assert!(m.vertical.is_none());
        assert!(m.vertical_line_height().is_none());
    }

    #[test]
    fn font_metrics_vertical() {
        let v = VerticalMetrics {
            ascender: 500,
            descender: -500,
            line_gap: 100,
            ..VerticalMetrics::default()
        };
        assert_eq!(v.line_height(), 1100);
        let m = FontMetrics::default().with_vertical(v);
        assert_eq!(m.vertical, Some(v));
        assert_eq!(m.vertical_line_height(), Some(1100));
    }

    #[test]
    fn derive_computes_running_extents() {
        let a = Glyph::new(GlyphId::new(0), 500, 100, box_at(100.0, 0.0, 500.0, 400.0));
        let b = Glyph::new(
            GlyphId::new(1),
            700,
            -20,
            box_at(-20.0, -50.0, 200.0, 900.0),
        );
        let m = FontMetrics::derive(&[a, b]);
        assert_eq!(m.advance_width_max, 700);
        assert_eq!(m.min_left_side_bearing, -20);
        // Glyph 0's ink is 400 wide against a 500 advance (+100); glyph 1's is
        // 220 against 700 (+480). The true minimum is +100: the extents start
        // at zero as the identity for a *maximum*, and a bearing's floor is
        // the smallest value it can take, not zero.
        assert_eq!(m.min_right_side_bearing, 100, "min of +100, +480");
        // And a negative RSB does lower it: make glyph 0 overhang.
        let overhang = Glyph::new(GlyphId::new(0), 100, 0, box_at(0.0, 0.0, 200.0, 10.0));
        assert_eq!(
            FontMetrics::derive(&[overhang]).min_right_side_bearing,
            -100
        );
        // `xMaxExtent` is `lsb + xMax`: 100 + 500 = 600, then -20 + 200 = 180.
        assert_eq!(m.x_max_extent, 600, "max of lsb + xMax");
        // The ascender grows to cover the tallest ink; the descender keeps
        // the default's -200 floor, since the OpenType-recommended default is
        // already below any glyph's y-min here.
        assert_eq!(m.ascender, 900);
        assert_eq!(m.descender, -200);
        // Empty store keeps the defaults.
        let e = FontMetrics::derive(&[]);
        assert_eq!(e, FontMetrics::default());
    }

    #[test]
    fn builder_accumulates_and_matches_derive() {
        let mut g = Glyph::new(GlyphId::new(0), 0, 0, box_at(100.0, 0.0, 500.0, 400.0));
        g.recompute_metrics().expect("recomputes");
        let mut b = MetricsBuilder::new();
        assert!(b.is_empty());
        b.push(&g);
        b.push(&Glyph::empty(GlyphId::new(1), 600));
        assert_eq!(b.len(), 2);
        let built = b.build();
        assert_eq!(built.advance_width_max, 600);
        assert_eq!(built.min_left_side_bearing, 0, "the empty glyph's lsb");
        // `lsb + xMax` = 100 + 500.
        assert_eq!(built.x_max_extent, 600);
        // Round trip: deriving from the same store agrees.
        let derived = FontMetrics::derive(&[g, Glyph::empty(GlyphId::new(1), 600)]);
        assert_eq!(built, derived);
    }

    #[test]
    fn builder_overrides_survive() {
        let m = MetricsBuilder::new()
            .with_ascender(1000)
            .with_descender(-300)
            .with_line_gap(90)
            .with_vertical(VerticalMetrics {
                ascender: 1,
                descender: -1,
                line_gap: 0,
                ..VerticalMetrics::default()
            })
            .build();
        assert_eq!(m.ascender, 1000);
        assert_eq!(m.descender, -300);
        assert_eq!(m.line_gap, 90);
        assert_eq!(m.line_height(), 1390);
        assert_eq!(m.vertical.map(|v| v.ascender), Some(1));
    }

    #[test]
    fn builder_advance_for_matches_glyph_recompute() {
        let outline = box_at(120.0, 0.0, 480.0, 400.0);
        let (advance, lsb) =
            MetricsBuilder::advance_for(&outline, GlyphId::new(2)).expect("computes");
        let mut g = Glyph::new(GlyphId::new(2), 0, 0, outline);
        g.recompute_metrics().expect("recomputes");
        assert_eq!(advance, g.advance_width());
        assert_eq!(lsb, g.left_side_bearing());
        assert_eq!(advance, 360);
        assert_eq!(lsb, 120);
    }

    #[test]
    fn builder_round_trips_a_whole_store() {
        // Build glyphs from outlines, recompute, feed the builder, and check
        // the header agrees with a fresh derive.
        let outlines: Vec<Outline> = (0..4)
            .map(|i| {
                let x = i as f32 * 10.0;
                box_at(x, 0.0, x + 300.0, 500.0)
            })
            .collect();
        let mut glyphs: Vec<Glyph> = Vec::new();
        let mut b = MetricsBuilder::new();
        for (i, o) in outlines.into_iter().enumerate() {
            let mut g = Glyph::new(GlyphId::new(u16::try_from(i).unwrap_or(0)), 0, 0, o);
            g.recompute_metrics().expect("recomputes");
            b.push(&g);
            glyphs.push(g);
        }
        assert_eq!(b.build(), FontMetrics::derive(&glyphs));
        // Every box is 300 wide, so the widest advance is 300.
        assert_eq!(b.build().advance_width_max, 300);
    }
}
