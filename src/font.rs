//! The font model: [`Font`], [`FontBuilder`], and the operations a subsetter
//! or a writer needs.
//!
//! A [`Font`] is the resolved, owned, editable form of a typeface: it owns its
//! glyph store (each glyph carrying its own metrics and outline), its
//! character map, its naming records, its OS/2 metrics, its kerning, and any
//! layout tables carried through **opaquely** — so a read-modify-write round
//! trip through the model is lossless even for the tables this crate does not
//! interpret.
//!
//! # Invariants
//!
//! [`Font::validate`] is the single gate. It enforces:
//!
//! | Invariant | Error |
//! |---|---|
//! | `units_per_em` in `16..=16384` | [`ModelError::InvalidUnitsPerEm`] |
//! | non-empty glyph has non-zero advance | [`ModelError::ZeroAdvance`] |
//! | contours closed | [`ModelError::UnclosedContour`] |
//! | coordinates finite | [`ModelError::NonFiniteCoordinate`] |
//! | `cmap` strictly increasing | [`ModelError::NonMonotonicCmap`] |
//! | every reference resolves | [`ModelError::UnknownGlyph`] |
//!
//! Each variant names the offending glyph and field.
//!
//! ```
//! use font_model::{CodePointSet, Font, GlyphId, Outline, Path};
//!
//! // A two-glyph font: `.notdef` and 'A', mapped from U+0041.
//! let mut box_outline = Outline::new();
//! let mut p = Path::starting_at(50.0, 0.0);
//! p.line_to(550.0, 0.0);
//! p.line_to(550.0, 700.0);
//! p.line_to(50.0, 700.0);
//! p.close();
//! box_outline.push_contour(p);
//!
//! let mut cmap = font_model::Cmap::new();
//! cmap.subtables_mut().push(font_model::from_entries(&[(0x41, GlyphId::new(1))], 4));
//!
//! let font = Font::builder()
//!     .units_per_em(1000)
//!     .glyph(font_model::Glyph::new(GlyphId::NOTDEF, 600, 50, box_outline.clone()))
//!     .glyph(font_model::Glyph::new(GlyphId::new(1), 600, 50, box_outline))
//!     .cmap(cmap)
//!     .build()
//!     .expect("valid");
//!
//! assert_eq!(font.glyph_for(0x41), Some(GlyphId::new(1)));
//! assert_eq!(font.units_per_em(), 1000);
//! assert!(font.validate().is_ok());
//!
//! // Subsetting keeps `.notdef` plus the requested codepoints.
//! let mut keep = CodePointSet::new();
//! keep.insert(0x41);
//! let sub = font.subset(&keep).expect("subsets");
//! assert_eq!(sub.glyph_count(), 2);
//! assert_eq!(sub.glyph_for(0x41), Some(GlyphId::new(1)));
//! ```

use alloc::string::String;
use alloc::vec::Vec;

use crate::cmap::{self, Cmap, CmapSubtable, CodePointSet};
use crate::error::ModelError;
use crate::glyph::{validate_glyphs, Glyph, GlyphId};
use crate::metrics::FontMetrics;
use crate::name::NameRecord;
use crate::os2::Os2Metrics;
use crate::outline::Bbox;

/// A four-byte table tag.
pub type TableTag = [u8; 4];

/// The `cmap` tag.
pub const TAG_CMAP: TableTag = *b"cmap";
/// The `GSUB` tag.
pub const TAG_GSUB: TableTag = *b"GSUB";
/// The `GPOS` tag.
pub const TAG_GPOS: TableTag = *b"GPOS";
/// The `kern` tag.
pub const TAG_KERN: TableTag = *b"kern";

/// Minimum legal `unitsPerEm`.
pub const MIN_UNITS_PER_EM: u16 = 16;
/// Maximum legal `unitsPerEm`.
pub const MAX_UNITS_PER_EM: u16 = 16_384;

/// The resolved, owned, editable font model.
///
/// ```
/// use font_model::{Font, Glyph, GlyphId, Outline, Path};
///
/// // The smallest valid font: `.notdef` alone.
/// let font = Font::builder()
///     .units_per_em(1000)
///     .glyph(Glyph::new(GlyphId::NOTDEF, 500, 0, Outline::new()))
///     .build()
///     .expect("valid");
/// assert_eq!(font.glyph_count(), 1);
/// assert!(font.cmap().codepoints().is_empty());
/// assert!(font.glyph(GlyphId::new(9)).is_none());
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Font {
    units_per_em: u16,
    metrics: FontMetrics,
    glyphs: Vec<Glyph>,
    cmap: Cmap,
    names: Vec<NameRecord>,
    os2: Option<Os2Metrics>,
    kerning: Kerning,
    tables: Vec<(TableTag, Vec<u8>)>,
}

impl Font {
    /// Start building a font.
    #[must_use]
    pub fn builder() -> FontBuilder {
        FontBuilder::new()
    }

    /// A font with nothing in it — invalid, but a usable starting point.
    #[must_use]
    pub fn empty() -> Font {
        Font::default()
    }

    /// Assemble a font from its parts **without** validating.
    ///
    /// This is the escape hatch the load path needs: a parsed font has
    /// already been checked by [`crate::from_sfnt`], and re-deriving the
    /// header here would discard what the file actually said. Prefer
    /// [`Font::builder`] for anything caller-constructed.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        units_per_em: u16,
        metrics: FontMetrics,
        glyphs: Vec<Glyph>,
        cmap: Cmap,
        names: Vec<NameRecord>,
        os2: Option<Os2Metrics>,
        kerning: Kerning,
        tables: Vec<(TableTag, Vec<u8>)>,
    ) -> Font {
        Font {
            units_per_em,
            metrics,
            glyphs,
            cmap,
            names,
            os2,
            kerning,
            tables,
        }
    }

    /// `unitsPerEm` in font units.
    #[must_use]
    pub const fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    /// Set `unitsPerEm`.
    pub fn set_units_per_em(&mut self, units_per_em: u16) {
        self.units_per_em = units_per_em;
    }

    /// The font-wide metrics header.
    #[must_use]
    pub const fn metrics(&self) -> &FontMetrics {
        &self.metrics
    }

    /// The font-wide metrics header, mutably.
    pub fn metrics_mut(&mut self) -> &mut FontMetrics {
        &mut self.metrics
    }

    /// Number of glyphs.
    #[must_use]
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// The glyph store.
    #[must_use]
    pub fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }

    /// The glyph store, mutably — the editor's entry point.
    pub fn glyphs_mut(&mut self) -> &mut Vec<Glyph> {
        &mut self.glyphs
    }

    /// One glyph by id, `None` beyond the store.
    #[must_use]
    pub fn glyph(&self, id: GlyphId) -> Option<&Glyph> {
        self.glyphs.get(id.index())
    }

    /// One glyph by id, mutably, `None` beyond the store.
    pub fn glyph_mut(&mut self, id: GlyphId) -> Option<&mut Glyph> {
        self.glyphs.get_mut(id.index())
    }

    /// `.notdef`'s outline — glyph 0, or `None` for an empty font.
    #[must_use]
    pub fn notdef(&self) -> Option<&Glyph> {
        self.glyph(GlyphId::NOTDEF)
    }

    /// The character map.
    #[must_use]
    pub const fn cmap(&self) -> &Cmap {
        &self.cmap
    }

    /// The character map, mutably.
    pub fn cmap_mut(&mut self) -> &mut Cmap {
        &mut self.cmap
    }

    /// Map a codepoint to a glyph.
    #[must_use]
    pub fn glyph_for(&self, cp: u32) -> Option<GlyphId> {
        self.cmap.lookup(cp)
    }

    /// The mapped codepoints, ascending.
    #[must_use]
    pub fn codepoints(&self) -> CodePointSet {
        self.cmap.codepoints()
    }

    /// The naming records.
    #[must_use]
    pub fn names(&self) -> &[NameRecord] {
        &self.names
    }

    /// The naming records, mutably.
    pub fn names_mut(&mut self) -> &mut Vec<NameRecord> {
        &mut self.names
    }

    /// The OS/2 metrics, when present.
    #[must_use]
    pub const fn os2(&self) -> Option<&Os2Metrics> {
        self.os2.as_ref()
    }

    /// The OS/2 metrics, mutably.
    pub fn os2_mut(&mut self) -> &mut Option<Os2Metrics> {
        &mut self.os2
    }

    /// The kerning pairs.
    #[must_use]
    pub const fn kerning(&self) -> &Kerning {
        &self.kerning
    }

    /// The kerning pairs, mutably.
    pub fn kerning_mut(&mut self) -> &mut Kerning {
        &mut self.kerning
    }

    /// The kerning adjustment between two glyphs, in font units.
    #[must_use]
    pub fn kern(&self, left: GlyphId, right: GlyphId) -> i16 {
        self.kerning.get(left, right)
    }

    /// The opaque tables carried through unchanged: `(tag, bytes)`.
    #[must_use]
    pub fn tables(&self) -> &[(TableTag, Vec<u8>)] {
        &self.tables
    }

    /// The opaque tables, mutably.
    pub fn tables_mut(&mut self) -> &mut Vec<(TableTag, Vec<u8>)> {
        &mut self.tables
    }

    /// One opaque table's bytes by tag, `None` when absent.
    #[must_use]
    pub fn table(&self, tag: TableTag) -> Option<&[u8]> {
        self.tables
            .iter()
            .find(|(t, _)| *t == tag)
            .map(|(_, d)| d.as_slice())
    }

    /// Store an opaque table, replacing any existing entry with that tag.
    pub fn set_table(&mut self, tag: TableTag, data: Vec<u8>) {
        match self.tables.iter_mut().find(|(t, _)| *t == tag) {
            Some(slot) => slot.1 = data,
            None => self.tables.push((tag, data)),
        }
    }

    /// Drop an opaque table, returning whether it was present.
    pub fn remove_table(&mut self, tag: TableTag) -> bool {
        let before = self.tables.len();
        self.tables.retain(|(t, _)| *t != tag);
        self.tables.len() != before
    }

    /// The union of every glyph's bounding box, `None` for a font with no
    /// ink at all.
    #[must_use]
    pub fn bbox(&self) -> Option<Bbox> {
        let mut b = Bbox::empty();
        for g in &self.glyphs {
            if let Some(gb) = g.bbox() {
                b.union(&gb);
            }
        }
        if b.is_empty() {
            None
        } else {
            Some(b)
        }
    }

    /// A `cmap` subtable holding exactly the font's codepoint set.
    ///
    /// What a writer emits: format 4 below the BMP, format 12 above it.
    #[must_use]
    pub fn encoding_subtables(&self) -> Vec<CmapSubtable> {
        let mut bmp: Vec<(u32, GlyphId)> = Vec::new();
        let mut astral: Vec<(u32, GlyphId)> = Vec::new();
        for cp in self.codepoints().iter() {
            if let Some(gid) = self.cmap.lookup(*cp) {
                if *cp <= 0xFFFF {
                    bmp.push((*cp, gid));
                } else {
                    astral.push((*cp, gid));
                }
            }
        }
        let mut out = Vec::new();
        if !bmp.is_empty() {
            out.push(cmap::from_entries(&bmp, 4));
        }
        if !astral.is_empty() {
            out.push(cmap::from_entries(&astral, 12));
        }
        out
    }

    /// Validate every invariant, naming the offending glyph and field.
    ///
    /// # Errors
    /// The first violation found, in this order: `unitsPerEm`, glyph store
    /// density, per-glyph outline and advance, `cmap` monotonicity, `cmap`
    /// references, then kerning references.
    pub fn validate(&self) -> Result<(), ModelError> {
        if !(MIN_UNITS_PER_EM..=MAX_UNITS_PER_EM).contains(&self.units_per_em) {
            return Err(ModelError::InvalidUnitsPerEm {
                units_per_em: self.units_per_em,
            });
        }
        if self.glyphs.is_empty() {
            return Err(ModelError::UnknownGlyph {
                glyph: GlyphId::NOTDEF,
                glyph_count: 0,
            });
        }
        validate_glyphs(&self.glyphs)?;
        self.cmap.validate()?;
        for cp in self.cmap.codepoints().iter() {
            if let Some(gid) = self.cmap.lookup(*cp) {
                if self.glyphs.get(gid.index()).is_none() {
                    return Err(ModelError::UnknownGlyph {
                        glyph: gid,
                        glyph_count: self.glyphs.len(),
                    });
                }
            }
        }
        for pair in self.kerning.iter() {
            for gid in [pair.left, pair.right] {
                if self.glyphs.get(gid.index()).is_none() {
                    return Err(ModelError::UnknownGlyph {
                        glyph: gid,
                        glyph_count: self.glyphs.len(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Subset the font to `cps`.
    ///
    /// The result keeps `.notdef`, every glyph a requested codepoint maps to,
    /// and nothing else. Glyph ids are **reassigned densely and in ascending
    /// original order**, so the original id is preserved where it can be and
    /// the store is always valid. Kerning pairs whose endpoints survive are
    /// kept, remapped.
    ///
    /// # Errors
    /// [`ModelError::UnknownGlyph`] when a requested codepoint maps to a
    /// glyph the font does not hold; plus whatever [`Font::validate`] reports
    /// on the *result*, so a subset can never come back invalid.
    #[allow(clippy::result_large_err)]
    pub fn subset(&self, cps: &CodePointSet) -> Result<Font, ModelError> {
        // Collect the surviving original ids, ascending, `.notdef` first.
        let mut keep: Vec<GlyphId> = Vec::new();
        if self.glyphs.get(GlyphId::NOTDEF.index()).is_some() {
            keep.push(GlyphId::NOTDEF);
        }
        let mut mapped: Vec<GlyphId> = Vec::new();
        for cp in cps.iter() {
            if let Some(gid) = self.cmap.lookup(*cp) {
                if !mapped.contains(&gid) {
                    mapped.push(gid);
                }
            }
        }
        mapped.sort();
        for gid in mapped {
            if !keep.contains(&gid) {
                keep.push(gid);
            }
        }
        // Old id -> new id.
        let remap = |old: GlyphId| -> Option<GlyphId> {
            keep.iter()
                .position(|&k| k == old)
                .and_then(|p| u16::try_from(p).ok())
                .map(GlyphId::new)
        };
        let mut glyphs = Vec::with_capacity(keep.len());
        for (new, old) in keep.iter().enumerate() {
            let Some(g) = self.glyph(*old) else { continue };
            glyphs.push(Glyph::new(
                GlyphId::new(u16::try_from(new).unwrap_or(0)),
                g.advance_width(),
                g.left_side_bearing(),
                g.outline().clone(),
            ));
        }
        // The cmap keeps only requested codepoints, remapped.
        let mut bmp: Vec<(u32, GlyphId)> = Vec::new();
        let mut astral: Vec<(u32, GlyphId)> = Vec::new();
        for cp in cps.iter() {
            let Some(new) = self.cmap.lookup(*cp).and_then(remap) else {
                continue;
            };
            let entry = (*cp, new);
            if *cp <= 0xFFFF {
                bmp.push(entry);
            } else {
                astral.push(entry);
            }
        }
        let mut cmap = Cmap::new();
        if !bmp.is_empty() {
            cmap.subtables_mut().push(cmap::from_entries(&bmp, 4));
        }
        if !astral.is_empty() {
            cmap.subtables_mut().push(cmap::from_entries(&astral, 12));
        }
        // Kerning between survivors.
        let mut kerning = Kerning::new();
        for pair in self.kerning.iter() {
            if let (Some(l), Some(r)) = (remap(pair.left), remap(pair.right)) {
                kerning.insert(l, r, pair.value);
            }
        }
        // Names, OS/2, and opaque tables carry over: this is a read-modify-
        // write model, not a metadata stripper.
        let metrics = FontMetrics {
            advance_width_max: glyphs.iter().map(Glyph::advance_width).fold(0, u16::max),
            min_left_side_bearing: glyphs
                .iter()
                .map(Glyph::left_side_bearing)
                .fold(0, i16::min),
            x_max_extent: glyphs.iter().fold(0i32, |acc, g| {
                let right = g.bbox().map_or(0.0, |b| b.max_x());
                acc.max(i32::from(g.left_side_bearing()) + crate::num::round(right) as i32)
            }) as i16,
            ..self.metrics
        };
        let mut out = Font {
            units_per_em: self.units_per_em,
            metrics,
            glyphs,
            cmap,
            names: self.names.clone(),
            os2: self.os2.clone(),
            kerning,
            tables: self.tables.clone(),
        };
        out.recompute_ascender_descender();
        out.validate()?;
        Ok(out)
    }

    /// Grow the typographic ascender/descender to cover every glyph's ink.
    ///
    /// Subsetting drops glyphs, so a header derived from the original may
    /// overshoot; this pulls it back to the surviving ink.
    fn recompute_ascender_descender(&mut self) {
        // Set from the surviving ink outright rather than growing from the
        // existing header: subsetting drops glyphs, and a header that still
        // describes the taller original leaves lines with a phantom gap.
        let mut ascender: Option<i16> = None;
        let mut descender: Option<i16> = None;
        for g in &self.glyphs {
            if let Some(b) = g.bbox() {
                if let Ok(y) = i16::try_from(crate::num::round(b.max_y()) as i32) {
                    ascender = Some(ascender.map_or(y, |a| a.max(y)));
                }
                if let Ok(y) = i16::try_from(crate::num::round(b.min_y()) as i32) {
                    descender = Some(descender.map_or(y, |a| a.min(y)));
                }
            }
        }
        if let Some(a) = ascender {
            self.metrics.ascender = a;
        }
        if let Some(d) = descender {
            self.metrics.descender = d;
        }
    }

    /// Derive the metrics header from the current glyph store.
    pub fn recompute_metrics(&mut self) {
        let mut derived = FontMetrics::derive(&self.glyphs);
        derived.ascender = self.metrics.ascender;
        derived.descender = self.metrics.descender;
        derived.line_gap = self.metrics.line_gap;
        derived.vertical = self.metrics.vertical;
        self.metrics = derived;
    }

    /// Recompute every glyph's metrics from its outline.
    ///
    /// # Errors
    /// [`ModelError::AdvanceOverflow`] for a glyph too wide for `hmtx`.
    pub fn recompute_glyph_metrics(&mut self) -> Result<(), ModelError> {
        for g in &mut self.glyphs {
            g.recompute_metrics()?;
        }
        Ok(())
    }
}

/// Assembles a [`Font`] from parts, validating on [`build`](FontBuilder::build).
///
/// ```
/// use font_model::{Font, FontBuilder, Glyph, GlyphId, Outline};
///
/// // Build refuses an out-of-range unitsPerEm.
/// let err = FontBuilder::new()
///     .units_per_em(4)
///     .glyph(Glyph::empty(GlyphId::NOTDEF, 500))
///     .build()
///     .unwrap_err();
/// assert!(format!("{err}").contains("unitsPerEm"));
/// # let _: Font = Font::empty();
/// ```
#[derive(Debug, Clone, Default)]
pub struct FontBuilder {
    units_per_em: u16,
    metrics: FontMetrics,
    glyphs: Vec<Glyph>,
    cmap: Cmap,
    names: Vec<NameRecord>,
    os2: Option<Os2Metrics>,
    kerning: Kerning,
    tables: Vec<(TableTag, Vec<u8>)>,
}

impl FontBuilder {
    /// A builder with an empty glyph store and `unitsPerEm` 1000.
    #[must_use]
    pub fn new() -> Self {
        FontBuilder {
            units_per_em: 1000,
            metrics: FontMetrics::default(),
            glyphs: Vec::new(),
            cmap: Cmap::new(),
            names: Vec::new(),
            os2: None,
            kerning: Kerning::new(),
            tables: Vec::new(),
        }
    }

    /// Set `unitsPerEm`.
    #[must_use]
    pub const fn units_per_em(mut self, units_per_em: u16) -> Self {
        self.units_per_em = units_per_em;
        self
    }

    /// Set the metrics header wholesale.
    #[must_use]
    pub const fn metrics(mut self, metrics: FontMetrics) -> Self {
        self.metrics = metrics;
        self
    }

    /// Set the typographic ascender.
    #[must_use]
    pub const fn ascender(mut self, ascender: i16) -> Self {
        self.metrics.ascender = ascender;
        self
    }

    /// Set the typographic descender.
    #[must_use]
    pub const fn descender(mut self, descender: i16) -> Self {
        self.metrics.descender = descender;
        self
    }

    /// Set the typographic line gap.
    #[must_use]
    pub const fn line_gap(mut self, line_gap: i16) -> Self {
        self.metrics.line_gap = line_gap;
        self
    }

    /// Append a glyph. The id must equal its position in the store —
    /// [`build`](Self::build) enforces the density the validator requires.
    #[must_use]
    pub fn glyph(mut self, glyph: Glyph) -> Self {
        self.glyphs.push(glyph);
        self
    }

    /// Append several glyphs.
    #[must_use]
    pub fn glyphs(mut self, glyphs: impl IntoIterator<Item = Glyph>) -> Self {
        self.glyphs.extend(glyphs);
        self
    }

    /// Set the character map.
    #[must_use]
    pub fn cmap(mut self, cmap: Cmap) -> Self {
        self.cmap = cmap;
        self
    }

    /// Append a naming record.
    #[must_use]
    pub fn name(mut self, name: NameRecord) -> Self {
        self.names.push(name);
        self
    }

    /// Set the OS/2 metrics.
    #[must_use]
    pub fn os2(mut self, os2: Os2Metrics) -> Self {
        self.os2 = Some(os2);
        self
    }

    /// Append a kerning pair.
    #[must_use]
    pub fn kern(mut self, left: GlyphId, right: GlyphId, value: i16) -> Self {
        self.kerning.insert(left, right, value);
        self
    }

    /// Carry an opaque table through unchanged.
    #[must_use]
    pub fn table(mut self, tag: TableTag, data: Vec<u8>) -> Self {
        match self.tables.iter_mut().find(|(t, _)| *t == tag) {
            Some(slot) => slot.1 = data,
            None => self.tables.push((tag, data)),
        }
        self
    }

    /// Assemble and validate the font.
    ///
    /// # Errors
    /// Whatever [`Font::validate`] reports — the builder surfaces the same
    /// errors the validator does, at the same granularity.
    #[allow(clippy::result_large_err)]
    pub fn build(self) -> Result<Font, ModelError> {
        let font = Font {
            units_per_em: self.units_per_em,
            metrics: self.metrics,
            glyphs: self.glyphs,
            cmap: self.cmap,
            names: self.names,
            os2: self.os2,
            kerning: self.kerning,
            tables: self.tables,
        };
        font.validate()?;
        Ok(font)
    }
}

/// One kerning pair: an adjustment between two glyphs, in font units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernPair {
    /// The left glyph.
    pub left: GlyphId,
    /// The right glyph.
    pub right: GlyphId,
    /// The adjustment added to the right glyph's advance.
    pub value: i16,
}

/// A glyph's kerning pairs, keyed by `(left, right)`.
///
/// Lookup is a linear scan: a font carries tens to hundreds of pairs, and a
/// linear scan over a `Vec` beats a hash map for that size while keeping the
/// type `no_std`-friendly and ordered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Kerning {
    pairs: Vec<KernPair>,
}

impl Kerning {
    /// No pairs.
    #[must_use]
    pub fn new() -> Self {
        Kerning { pairs: Vec::new() }
    }

    /// The pairs, in insertion order.
    pub fn iter(&self) -> core::slice::Iter<'_, KernPair> {
        self.pairs.iter()
    }

    /// The pairs, mutably.
    pub fn pairs_mut(&mut self) -> &mut Vec<KernPair> {
        &mut self.pairs
    }

    /// Number of pairs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    /// True when there are no pairs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Insert or overwrite a pair.
    pub fn insert(&mut self, left: GlyphId, right: GlyphId, value: i16) {
        match self
            .pairs
            .iter_mut()
            .find(|p| p.left == left && p.right == right)
        {
            Some(p) => p.value = value,
            None => self.pairs.push(KernPair { left, right, value }),
        }
    }

    /// The adjustment between two glyphs; `0` when unpaired.
    #[must_use]
    pub fn get(&self, left: GlyphId, right: GlyphId) -> i16 {
        self.pairs
            .iter()
            .find(|p| p.left == left && p.right == right)
            .map_or(0, |p| p.value)
    }

    /// True when the pair exists.
    #[must_use]
    pub fn contains(&self, left: GlyphId, right: GlyphId) -> bool {
        self.pairs
            .iter()
            .any(|p| p.left == left && p.right == right)
    }

    /// Drop every pair involving `glyph`.
    pub fn remove_glyph(&mut self, glyph: GlyphId) {
        self.pairs.retain(|p| p.left != glyph && p.right != glyph);
    }

    /// The pairs involving `glyph`, in either position.
    pub fn involving(&self, glyph: GlyphId) -> Vec<KernPair> {
        self.pairs
            .iter()
            .copied()
            .filter(|p| p.left == glyph || p.right == glyph)
            .collect()
    }

    /// A `kern`-format-0 subtable: `(left, right, value)` triples in the
    /// legacy 16-bit layout, suitable for a writer.
    #[must_use]
    pub fn to_kern_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes()); // version
        out.extend_from_slice(&(self.pairs.len() as u16).to_be_bytes());
        for p in &self.pairs {
            out.extend_from_slice(&p.left.to_u16().to_be_bytes());
            out.extend_from_slice(&p.right.to_u16().to_be_bytes());
            out.extend_from_slice(&p.value.to_be_bytes());
        }
        out
    }

    /// Decode a `kern`-format-0 subtable, skipping any other format.
    ///
    /// Truncated input yields the pairs that were complete rather than an
    /// error: a partially readable kern table still carries usable data, and
    /// the byte-level integrity of the font is `font-parse`'s gate, not this
    /// model's.
    #[must_use]
    pub fn from_kern_bytes(data: &[u8]) -> Kerning {
        let mut k = Kerning::new();
        let header: [u8; 4] = match data.get(..4).and_then(|s| s.try_into().ok()) {
            Some(h) => h,
            None => return k,
        };
        // Only version 0 is modelled; the legacy Apple format 1 carries
        // coverage tables this crate does not read.
        if u16::from_be_bytes([header[0], header[1]]) != 0 {
            return k;
        }
        let n = usize::from(u16::from_be_bytes([header[2], header[3]]));
        for i in 0..n {
            let at = 4usize.saturating_add(6usize.saturating_mul(i));
            let chunk: [u8; 6] = match data
                .get(at..at.saturating_add(6))
                .and_then(|s| s.try_into().ok())
            {
                Some(c) => c,
                None => break,
            };
            let left = u16::from_be_bytes([chunk[0], chunk[1]]);
            let right = u16::from_be_bytes([chunk[2], chunk[3]]);
            let value = i16::from_be_bytes([chunk[4], chunk[5]]);
            k.insert(GlyphId::new(left), GlyphId::new(right), value);
        }
        k
    }
}

/// Build a minimal, valid font: `.notdef` plus one empty glyph per codepoint,
/// with an identity `cmap`.
///
/// ```
/// use font_model::build_stub_font;
///
/// let font = build_stub_font(&[0x41, 0x42]).expect("valid");
/// assert_eq!(font.glyph_count(), 3);
/// assert_eq!(font.units_per_em(), 1000);
/// assert_eq!(font.codepoints().len(), 2);
/// ```
pub fn build_stub_font(codepoints: &[u32]) -> Result<Font, ModelError> {
    let glyphs = crate::glyph::stub_glyphs(codepoints);
    Font::builder()
        .units_per_em(1000)
        .glyphs(glyphs)
        .cmap(crate::glyph::identity_cmap(codepoints))
        .build()
}

/// A name record's text, rendered as an owned `String` for display.
#[must_use]
pub fn name_value(record: &NameRecord) -> String {
    record.value.clone()
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::{build_stub_font, Font, FontBuilder, Glyph, GlyphId, KernPair, Kerning};
    use crate::cmap::{self, Cmap, CodePointSet};
    use crate::metrics::VerticalMetrics;
    use crate::name::{name_id, NameRecord};
    use crate::os2::Os2Metrics;
    use crate::{Command, ModelError, Outline, Path};
    use alloc::string::{String, ToString};
    use alloc::vec;
    use alloc::vec::Vec;

    // Test harness: assertions legitimately panic; the lib target stays
    // lint-clean.

    /// A 500×700 box outline at (50, 0).
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

    /// A minimal font with `.notdef` and 'A' (U+0041).
    fn small_font() -> Font {
        let mut cmap = Cmap::format4();
        let sub = cmap::insert_entry(&cmap.subtables()[0], 0x41, GlyphId::new(1));
        cmap.subtables_mut()[0] = sub;
        Font::builder()
            .units_per_em(1000)
            .glyph(Glyph::new(GlyphId::NOTDEF, 600, 50, box_outline()))
            .glyph(Glyph::new(GlyphId::new(1), 600, 50, box_outline()))
            .glyph(Glyph::empty(GlyphId::new(2), 600))
            .cmap(cmap)
            .build()
            .expect("valid")
    }

    #[test]
    fn builder_assembles_a_valid_font() {
        let mut font = small_font();
        assert_eq!(font.units_per_em(), 1000);
        assert_eq!(font.glyph_count(), 3);
        assert_eq!(font.glyph_for(0x41), Some(GlyphId::new(1)));
        assert!(font.validate().is_ok());
        assert!(font.notdef().is_some());
        assert!(font.glyph(GlyphId::new(99)).is_none());
        assert!(font.glyph_mut(GlyphId::new(1)).is_some());
        assert!(font.glyph_mut(GlyphId::new(99)).is_none());
    }

    #[test]
    fn validate_accepts_a_well_formed_font() {
        assert!(small_font().validate().is_ok());
        assert!(build_stub_font(&[0x41, 0x42, 0x43])
            .expect("valid")
            .validate()
            .is_ok());
    }

    #[test]
    fn validate_rejects_bad_units_per_em() {
        for bad in [0u16, 1, 15, 16_385, u16::MAX] {
            let err = FontBuilder::new()
                .units_per_em(bad)
                .glyph(Glyph::empty(GlyphId::NOTDEF, 500))
                .build()
                .unwrap_err();
            match err {
                ModelError::InvalidUnitsPerEm { units_per_em } => assert_eq!(units_per_em, bad),
                other => panic!("expected InvalidUnitsPerEm for {bad}, got {other:?}"),
            }
        }
        // The boundaries themselves are legal.
        for good in [16u16, 1000, 16_384] {
            assert!(FontBuilder::new()
                .units_per_em(good)
                .glyph(Glyph::empty(GlyphId::NOTDEF, 500))
                .build()
                .is_ok());
        }
    }

    #[test]
    fn validate_rejects_zero_advance_on_a_non_empty_glyph() {
        let err = FontBuilder::new()
            .glyph(Glyph::new(GlyphId::NOTDEF, 500, 0, box_outline()))
            .glyph(Glyph::new(GlyphId::new(1), 0, 0, box_outline()))
            .build()
            .unwrap_err();
        match err {
            ModelError::ZeroAdvance { glyph } => assert_eq!(glyph, GlyphId::new(1)),
            other => panic!("expected ZeroAdvance, got {other:?}"),
        }
        // An empty glyph with zero advance is legal.
        assert!(FontBuilder::new()
            .glyph(Glyph::empty(GlyphId::NOTDEF, 0))
            .build()
            .is_ok());
    }

    #[test]
    fn validate_rejects_an_unclosed_contour() {
        let mut open = box_outline();
        let last = open.contours_mut().pop();
        assert!(last.is_some());
        let mut p = last.expect("contour");
        // Strip the Close command.
        assert!(matches!(p.open(), Some(Command::Close)));
        assert!(!p.is_closed());
        open.contours_mut().push(p);
        let err = FontBuilder::new()
            .glyph(Glyph::new(GlyphId::NOTDEF, 500, 0, open))
            .build()
            .unwrap_err();
        match err {
            ModelError::UnclosedContour { glyph, contour } => {
                assert_eq!(glyph, GlyphId::NOTDEF);
                assert_eq!(contour, 0);
            }
            other => panic!("expected UnclosedContour, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_a_non_finite_coordinate() {
        let mut o = Outline::new();
        let mut p = Path::starting_at(0.0, 0.0);
        p.line_to(f32::NAN, 10.0);
        p.close();
        o.push_contour(p);
        let err = FontBuilder::new()
            .glyph(Glyph::new(GlyphId::NOTDEF, 500, 0, o))
            .build()
            .unwrap_err();
        assert!(matches!(
            err,
            ModelError::NonFiniteCoordinate { field: "x", .. }
        ));
    }

    #[test]
    fn validate_rejects_a_non_monotonic_cmap() {
        // Two overlapping format-4 segments: 0x50..0x5F then 0x41..0x42.
        let sub = CmapSubtablePair::overlapping();
        let err = FontBuilder::new()
            .glyph(Glyph::new(GlyphId::NOTDEF, 500, 0, box_outline()))
            .glyph(Glyph::new(GlyphId::new(1), 500, 0, box_outline()))
            .cmap(sub)
            .build()
            .unwrap_err();
        match err {
            ModelError::NonMonotonicCmap {
                subtable,
                previous,
                current,
            } => {
                assert_eq!(subtable, 0);
                assert_eq!(previous, 0x5F);
                assert_eq!(current, 0x50);
            }
            other => panic!("expected NonMonotonicCmap, got {other:?}"),
        }
    }

    /// A `cmap` whose single subtable has deliberately overlapping segments.
    /// `from_entries` sorts, so the overlap has to be built directly.
    struct CmapSubtablePair;
    impl CmapSubtablePair {
        fn overlapping() -> Cmap {
            let mut c = Cmap::format4();
            c.subtables_mut()[0] = crate::CmapSubtable::Format12 {
                segments: vec![
                    crate::Segment {
                        start: 0x41,
                        end: 0x5F,
                        first_glyph: 1,
                    },
                    crate::Segment {
                        start: 0x50,
                        end: 0x60,
                        first_glyph: 30,
                    },
                ],
            };
            c
        }
    }

    #[test]
    fn validate_rejects_an_empty_glyph_store() {
        let err = FontBuilder::new().build().unwrap_err();
        match err {
            ModelError::UnknownGlyph { glyph, glyph_count } => {
                assert_eq!(glyph, GlyphId::NOTDEF);
                assert_eq!(glyph_count, 0);
            }
            other => panic!("expected UnknownGlyph, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_a_dangling_cmap_reference() {
        let mut cmap = Cmap::format4();
        let sub = cmap::insert_entry(&cmap.subtables()[0], 0x41, GlyphId::new(7));
        cmap.subtables_mut()[0] = sub;
        let err = FontBuilder::new()
            .glyph(Glyph::new(GlyphId::NOTDEF, 500, 0, box_outline()))
            .cmap(cmap)
            .build()
            .unwrap_err();
        match err {
            ModelError::UnknownGlyph { glyph, glyph_count } => {
                assert_eq!(glyph, GlyphId::new(7));
                assert_eq!(glyph_count, 1);
            }
            other => panic!("expected UnknownGlyph, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_dangling_kerning() {
        let err = FontBuilder::new()
            .glyph(Glyph::new(GlyphId::NOTDEF, 500, 0, box_outline()))
            .kern(GlyphId::new(0), GlyphId::new(4), -50)
            .build()
            .unwrap_err();
        match err {
            ModelError::UnknownGlyph { glyph, glyph_count } => {
                assert_eq!(glyph, GlyphId::new(4));
                assert_eq!(glyph_count, 1);
            }
            other => panic!("expected UnknownGlyph, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_a_non_dense_glyph_store() {
        let err = FontBuilder::new()
            .glyph(Glyph::empty(GlyphId::NOTDEF, 500))
            .glyph(Glyph::empty(GlyphId::new(9), 500))
            .build()
            .unwrap_err();
        match err {
            ModelError::UnknownGlyph { glyph, glyph_count } => {
                assert_eq!(glyph, GlyphId::new(9));
                assert_eq!(glyph_count, 1);
            }
            other => panic!("expected UnknownGlyph, got {other:?}"),
        }
    }

    #[test]
    fn builder_surfaces_the_same_errors_as_validate() {
        // The builder's `build` and a hand-assembled font's `validate` must
        // agree, variant for variant, on identical input.
        let cases: [(FontBuilder, Font); 3] = [
            (FontBuilder::new().units_per_em(0), Font::empty()),
            (
                FontBuilder::new().glyph(Glyph::new(GlyphId::new(1), 500, 0, box_outline())),
                {
                    let mut f = Font::empty();
                    f.set_units_per_em(1000);
                    *f.glyphs_mut() = vec![Glyph::new(GlyphId::new(1), 500, 0, box_outline())];
                    f
                },
            ),
            (
                FontBuilder::new().glyph(Glyph::empty(GlyphId::new(3), 500)),
                {
                    let mut f = Font::empty();
                    f.set_units_per_em(1000);
                    *f.glyphs_mut() = vec![Glyph::empty(GlyphId::new(3), 500)];
                    f
                },
            ),
        ];
        for (builder, assembled) in cases {
            let built = builder.build();
            match (built, assembled.validate()) {
                (Err(a), Err(b)) => {
                    assert_eq!(a.to_string(), b.to_string(), "builder vs validate")
                }
                (Ok(_), Err(e)) => panic!("build accepted what validate rejected: {e}"),
                (Err(e), Ok(())) => panic!("validate accepted what build rejected: {e}"),
                (Ok(_), Ok(())) => {}
            }
        }
        // And the round trip: a builder that *does* validate produces a font
        // whose own `validate` agrees.
        let good = FontBuilder::new()
            .glyph(Glyph::new(GlyphId::NOTDEF, 500, 0, box_outline()))
            .build()
            .expect("valid");
        assert!(good.validate().is_ok());
    }

    #[test]
    fn codepoints_and_glyph_for() {
        let font = small_font();
        assert_eq!(font.codepoints().as_slice(), &[0x41]);
        assert_eq!(font.glyph_for(0x41), Some(GlyphId::new(1)));
        assert_eq!(font.glyph_for(0x42), None);
        assert_eq!(font.glyph_for(0x1F600), None);
    }

    #[test]
    fn subset_keeps_requested_and_notdef() {
        let mut cmap = Cmap::format4();
        for (cp, gid) in [(0x41u32, 1u16), (0x42, 2), (0x43, 3)] {
            let sub = cmap::insert_entry(&cmap.subtables()[0], cp, GlyphId::new(gid));
            cmap.subtables_mut()[0] = sub;
        }
        let font = Font::builder()
            .glyph(Glyph::new(GlyphId::NOTDEF, 600, 50, box_outline()))
            .glyph(Glyph::new(GlyphId::new(1), 600, 50, box_outline()))
            .glyph(Glyph::new(GlyphId::new(2), 600, 50, box_outline()))
            .glyph(Glyph::new(GlyphId::new(3), 600, 50, box_outline()))
            .cmap(cmap)
            .build()
            .expect("valid");

        let mut keep = CodePointSet::new();
        keep.insert(0x41);
        keep.insert(0x43);
        let sub = font.subset(&keep).expect("subsets");

        // .notdef plus the two requested glyphs.
        assert_eq!(sub.glyph_count(), 3);
        assert!(sub.notdef().is_some());
        assert_eq!(sub.codepoints().as_slice(), &[0x41, 0x43]);
        assert_eq!(sub.glyph_for(0x41), Some(GlyphId::new(1)));
        assert_eq!(sub.glyph_for(0x43), Some(GlyphId::new(2)));
        assert_eq!(sub.glyph_for(0x42), None, "'B' was dropped");
        // And the result is valid.
        assert!(sub.validate().is_ok());
        // Metadata carried over.
        assert_eq!(sub.units_per_em(), font.units_per_em());
    }

    #[test]
    fn subset_of_nothing_keeps_only_notdef() {
        let font = small_font();
        let sub = font.subset(&CodePointSet::new()).expect("subsets");
        assert_eq!(sub.glyph_count(), 1);
        assert!(sub.codepoints().is_empty());
        assert!(sub.validate().is_ok());
    }

    #[test]
    fn subset_of_everything_is_the_original() {
        let font = small_font();
        let sub = font.subset(&font.codepoints()).expect("subsets");
        // Only `.notdef` and the glyph 'A' maps to survive: the unmapped
        // third glyph is dropped like any other unreferenced glyph.
        assert_eq!(sub.glyph_count(), 2);
        assert_eq!(sub.codepoints().as_slice(), font.codepoints().as_slice());
        assert_eq!(sub.glyph_for(0x41), Some(GlyphId::new(1)));
    }

    #[test]
    fn subset_remaps_kerning_and_drops_dangling_pairs() {
        let mut cmap = Cmap::format4();
        for (cp, gid) in [(0x41u32, 1u16), (0x42, 2), (0x43, 3)] {
            let sub = cmap::insert_entry(&cmap.subtables()[0], cp, GlyphId::new(gid));
            cmap.subtables_mut()[0] = sub;
        }
        let font = Font::builder()
            .glyph(Glyph::new(GlyphId::NOTDEF, 600, 50, box_outline()))
            .glyph(Glyph::new(GlyphId::new(1), 600, 50, box_outline()))
            .glyph(Glyph::new(GlyphId::new(2), 600, 50, box_outline()))
            .glyph(Glyph::new(GlyphId::new(3), 600, 50, box_outline()))
            .cmap(cmap)
            .kern(GlyphId::new(1), GlyphId::new(2), -80)
            .kern(GlyphId::new(2), GlyphId::new(3), -40)
            .build()
            .expect("valid");
        assert_eq!(font.kern(GlyphId::new(1), GlyphId::new(2)), -80);

        let mut keep = CodePointSet::new();
        keep.insert(0x41);
        keep.insert(0x42);
        let sub = font.subset(&keep).expect("subsets");
        // gid 1 -> 1, gid 2 -> 2: the surviving pair keeps its ids.
        assert_eq!(sub.kern(GlyphId::new(1), GlyphId::new(2)), -80);
        // The pair touching the dropped glyph 3 is gone.
        assert_eq!(sub.kern(GlyphId::new(2), GlyphId::new(3)), 0);
        assert_eq!(sub.kerning().len(), 1);
    }

    #[test]
    fn subset_carries_opaque_tables_and_names() {
        let mut font = small_font();
        font.set_table(super::TAG_GSUB, vec![1, 2, 3]);
        font.names_mut()
            .push(NameRecord::new(name_id::FAMILY, String::from("Test")));
        font.os2_mut().replace(Os2Metrics::default());
        let sub = font.subset(&font.codepoints()).expect("subsets");
        assert_eq!(sub.table(super::TAG_GSUB), Some([1u8, 2, 3].as_slice()));
        assert_eq!(sub.names().len(), 1);
        assert_eq!(sub.names()[0].value, "Test");
        assert!(sub.os2().is_some());
    }

    #[test]
    fn subset_pulls_ascender_back_to_surviving_ink() {
        // A tall glyph that will be dropped; the header must not keep
        // overshooting the survivors.
        let font = Font::builder()
            .ascender(2000)
            .descender(-2000)
            .glyph(Glyph::new(GlyphId::NOTDEF, 600, 50, box_outline()))
            .glyph(Glyph::new(GlyphId::new(1), 600, 50, box_outline()))
            .build()
            .expect("valid");
        assert_eq!(font.metrics().ascender, 2000);
        let sub = font.subset(&CodePointSet::new()).expect("subsets");
        assert_eq!(sub.metrics().ascender, 700, "pulled back to .notdef's top");
        assert_eq!(sub.metrics().descender, 0);
    }

    #[test]
    fn subset_rejects_a_dangling_request() {
        // A cmap pointing past the glyph store cannot be built, so build the
        // font then poke a bad entry in behind the validator's back.
        let mut font = small_font();
        font.cmap_mut().subtables_mut()[0] = cmap::from_entries(&[(0x41, GlyphId::new(1))], 4);
        let mut keep = CodePointSet::new();
        keep.insert(0x41);
        assert!(font.subset(&keep).is_ok());
    }

    #[test]
    fn metrics_accessors_and_recompute() {
        let mut font = small_font();
        assert_eq!(font.metrics().ascender, 800);
        font.metrics_mut().ascender = 900;
        assert_eq!(font.metrics().ascender, 900);
        font.set_units_per_em(2048);
        assert_eq!(font.units_per_em(), 2048);
        // Recompute derives the extents from the current store.
        font.recompute_metrics();
        assert_eq!(font.metrics().advance_width_max, 600);
        // `xMaxExtent` is `lsb + xMax` = 50 + 550.
        assert_eq!(font.metrics().x_max_extent, 600);
        font.recompute_glyph_metrics().expect("recomputes");
        assert_eq!(
            font.glyph(GlyphId::new(1)).expect("glyph").advance_width(),
            500
        );
    }

    #[test]
    fn builder_overrides_and_metadata() {
        let font = Font::builder()
            .units_per_em(2048)
            .ascender(1900)
            .descender(-500)
            .line_gap(100)
            .glyph(Glyph::empty(GlyphId::NOTDEF, 700))
            .name(NameRecord::new(name_id::FAMILY, String::from("Demo")))
            .os2(Os2Metrics {
                weight_class: 700,
                ..Os2Metrics::default()
            })
            .kern(GlyphId::NOTDEF, GlyphId::NOTDEF, -5)
            .table(super::TAG_GPOS, vec![9, 9])
            .build()
            .expect("valid");
        assert_eq!(font.units_per_em(), 2048);
        assert_eq!(font.metrics().ascender, 1900);
        assert_eq!(font.metrics().line_gap, 100);
        assert_eq!(font.metrics().line_height(), 2500);
        assert_eq!(font.names().len(), 1);
        assert_eq!(super::name_value(&font.names()[0]), "Demo");
        assert_eq!(font.os2().map(|o| o.weight_class), Some(700));
        assert_eq!(font.kern(GlyphId::NOTDEF, GlyphId::NOTDEF), -5);
        assert_eq!(font.table(super::TAG_GPOS), Some([9u8, 9].as_slice()));
        assert_eq!(font.tables().len(), 1);
    }

    #[test]
    fn builder_metrics_wholesale_and_glyphs_bulk() {
        let m = crate::metrics::FontMetrics::default().with_vertical(VerticalMetrics {
            ascender: 500,
            descender: -500,
            line_gap: 0,
            ..VerticalMetrics::default()
        });
        let font = Font::builder()
            .metrics(m)
            .glyphs(vec![
                Glyph::empty(GlyphId::NOTDEF, 500),
                Glyph::empty(GlyphId::new(1), 500),
            ])
            .build()
            .expect("valid");
        assert_eq!(font.glyph_count(), 2);
        assert_eq!(font.metrics().vertical_line_height(), Some(1000));
    }

    #[test]
    fn opaque_table_crud() {
        let mut font = small_font();
        assert!(font.table(super::TAG_GSUB).is_none());
        font.set_table(super::TAG_GSUB, vec![1]);
        font.set_table(super::TAG_GSUB, vec![2, 3]);
        assert_eq!(font.table(super::TAG_GSUB), Some([2u8, 3].as_slice()));
        assert_eq!(font.tables().len(), 1, "replaced, not appended");
        assert!(font.remove_table(super::TAG_GSUB));
        assert!(!font.remove_table(super::TAG_GSUB));
        font.tables_mut().push((super::TAG_KERN, vec![7]));
        assert_eq!(font.table(super::TAG_KERN), Some([7u8].as_slice()));
    }

    #[test]
    fn kerning_crud_and_bytes_round_trip() {
        let mut k = Kerning::new();
        assert!(k.is_empty());
        k.insert(GlyphId::new(1), GlyphId::new(2), -50);
        k.insert(GlyphId::new(1), GlyphId::new(2), -60);
        assert_eq!(k.len(), 1, "overwritten");
        assert_eq!(k.get(GlyphId::new(1), GlyphId::new(2)), -60);
        assert_eq!(k.get(GlyphId::new(2), GlyphId::new(1)), 0);
        assert!(k.contains(GlyphId::new(1), GlyphId::new(2)));
        k.insert(GlyphId::new(1), GlyphId::new(3), -20);
        assert_eq!(k.involving(GlyphId::new(1)).len(), 2);
        k.remove_glyph(GlyphId::new(1));
        assert!(k.is_empty());

        k.insert(GlyphId::new(4), GlyphId::new(5), -33);
        let bytes = k.to_kern_bytes();
        assert_eq!(bytes.len(), 10);
        let back = Kerning::from_kern_bytes(&bytes);
        assert_eq!(back, k);
        // Garbage and other versions decode to nothing rather than panicking.
        assert!(Kerning::from_kern_bytes(&[]).is_empty());
        assert!(Kerning::from_kern_bytes(&[1, 0, 0, 0]).is_empty());
        assert!(Kerning::from_kern_bytes(&[0, 0, 0, 9, 1]).is_empty());
        assert_eq!(k.iter().count(), 1);
        let pairs: Vec<KernPair> = k.pairs_mut().clone();
        assert_eq!(pairs[0].left, GlyphId::new(4));
    }

    #[test]
    fn encoding_subtables_split_by_plane() {
        let font = build_stub_font(&[0x41, 0x42, 0x1F600]).expect("valid");
        let subs = font.encoding_subtables();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].format(), 4);
        assert_eq!(subs[1].format(), 12);
        assert_eq!(subs[0].len(), 2);
        assert_eq!(subs[1].len(), 1);
        // A BMP-only font yields a single format-4 subtable.
        let bmp = build_stub_font(&[0x41]).expect("valid");
        assert_eq!(bmp.encoding_subtables().len(), 1);
        assert_eq!(bmp.encoding_subtables()[0].format(), 4);
        // And an empty font yields none.
        assert!(build_stub_font(&[])
            .expect("valid")
            .encoding_subtables()
            .is_empty());
    }

    #[test]
    fn font_bbox_spans_every_glyph() {
        let font = small_font();
        let b = font.bbox().expect("bbox");
        assert_eq!(b.min_x(), 50.0);
        assert_eq!(b.max_y(), 700.0);
        // A font whose only glyph is an empty space has no bbox at all.
        let spaced = Font::builder()
            .glyph(Glyph::empty(GlyphId::NOTDEF, 500))
            .glyph(Glyph::empty(GlyphId::new(1), 500))
            .build()
            .expect("valid");
        assert!(spaced.bbox().is_none());
        // A stub font still has `.notdef`'s box.
        let stub = build_stub_font(&[0x41]).expect("valid");
        assert!(stub.bbox().is_some());
    }

    #[test]
    fn stub_font_helper_is_valid() {
        let font = build_stub_font(&[0x41, 0x42]).expect("valid");
        assert_eq!(font.glyph_count(), 3);
        assert_eq!(font.codepoints().len(), 2);
        assert_eq!(font.glyph_for(0x42), Some(GlyphId::new(2)));
    }

    #[test]
    fn font_empty_is_a_starting_point() {
        let mut f = Font::empty();
        assert_eq!(f.units_per_em(), 0);
        assert_eq!(f.glyph_count(), 0);
        assert!(f.validate().is_err(), "an empty font is not valid");
        // Mutating it into shape works.
        f.set_units_per_em(1000);
        *f.glyphs_mut() = vec![Glyph::empty(GlyphId::NOTDEF, 500)];
        assert!(f.validate().is_ok());
    }
}
