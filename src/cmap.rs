//! Character maps: [`Cmap`], [`CmapSubtable`], and [`CodePointSet`].
//!
//! This module carries **format 4** (the BMP segment/delta form every TrueType
//! font has) and **format 12** (segmented coverage above the BMP, plus
//! everything below it). Both directions are modelled: decode a subtable into
//! the owned model and encode it back to bytes. That symmetry is what lets an
//! editor round-trip a font's `cmap` through a config format without losing
//! the mapping.
//!
//! # Invariants
//!
//! A [`CmapSubtable`] is **sorted and non-overlapping**: codepoints ascend
//! strictly across segments. [`Cmap::validate`] is where that is checked and
//! [`ModelError::NonMonotonicCmap`] is what names the offending pair.
//!
//! ```
//! use font_model::{Cmap, CodePointSet, GlyphId};
//!
//! // Build a format-12 subtable over an astral plane plus a BMP run, then
//! // encode and decode it back.
//! let sub = font_model::from_entries(
//!     &[(0x41, GlyphId::new(1)), (0x1F600, GlyphId::new(2))],
//!     12,
//! );
//! let bytes = sub.encode();
//! let back = Cmap::decode_subtable(&bytes).expect("decodes");
//! assert_eq!(back.lookup(0x41), Some(GlyphId::new(1)));
//! assert_eq!(back.lookup(0x1F600), Some(GlyphId::new(2)));
//! assert_eq!(back.lookup(0x42), None);
//!
//! // The codepoint view is the sorted, mergeable set of mapped codepoints.
//! let set = sub.codepoints();
//! assert_eq!(set.len(), 2);
//! assert!(set.contains(0x1F600));
//! ```

use alloc::vec;
use alloc::vec::Vec;

use crate::error::ModelError;
use crate::GlyphId;

/// A sorted, mergeable set of codepoints.
///
/// The set is the currency of every subsetting operation: it is what
/// [`Cmap::codepoints`] returns and what [`crate::Font::subset`] consumes.
/// Insertion keeps it sorted and duplicate-free.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodePointSet {
    cps: Vec<u32>,
}

impl CodePointSet {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        CodePointSet { cps: Vec::new() }
    }

    /// A set from any iterator of codepoints: sorted and de-duplicated.
    #[must_use]
    pub fn from_iter_raw(iter: impl IntoIterator<Item = u32>) -> Self {
        let mut cps: Vec<u32> = iter.into_iter().collect();
        cps.sort_unstable();
        cps.dedup();
        CodePointSet { cps }
    }

    /// The codepoints, ascending.
    #[must_use]
    pub fn as_slice(&self) -> &[u32] {
        &self.cps
    }

    /// Number of codepoints.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cps.len()
    }

    /// True when the set holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cps.is_empty()
    }

    /// Insert a codepoint, keeping the set sorted and unique.
    pub fn insert(&mut self, cp: u32) {
        match self.cps.binary_search(&cp) {
            Ok(_) => {}
            Err(at) => self.cps.insert(at, cp),
        }
    }

    /// True when `cp` is in the set.
    #[must_use]
    pub fn contains(&self, cp: u32) -> bool {
        self.cps.binary_search(&cp).is_ok()
    }

    /// Iterate the codepoints, ascending.
    pub fn iter(&self) -> core::slice::Iter<'_, u32> {
        self.cps.iter()
    }

    /// The union of two sets.
    #[must_use]
    pub fn union(&self, other: &CodePointSet) -> CodePointSet {
        let mut cps = self.cps.clone();
        cps.extend_from_slice(&other.cps);
        cps.sort_unstable();
        cps.dedup();
        CodePointSet { cps }
    }

    /// The intersection of two sets.
    #[must_use]
    pub fn intersection(&self, other: &CodePointSet) -> CodePointSet {
        let cps = self
            .cps
            .iter()
            .copied()
            .filter(|cp| other.contains(*cp))
            .collect();
        CodePointSet { cps }
    }

    /// `self` minus `other`.
    #[must_use]
    pub fn difference(&self, other: &CodePointSet) -> CodePointSet {
        let cps = self
            .cps
            .iter()
            .copied()
            .filter(|cp| !other.contains(*cp))
            .collect();
        CodePointSet { cps }
    }

    /// True when every codepoint of `self` is also in `other`.
    #[must_use]
    pub fn is_subset_of(&self, other: &CodePointSet) -> bool {
        self.cps.iter().all(|cp| other.contains(*cp))
    }
}

/// One segment of a modelled subtable: an inclusive codepoint run.
///
/// Ascending and non-overlapping across the subtable. The glyph at
/// `first_glyph` covers `start`, and — unless the owning subtable supplies an
/// explicit glyph array for this segment — each subsequent codepoint takes the
/// next glyph id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// First codepoint of the run, as a `u32` (format 12 goes above the BMP).
    pub start: u32,
    /// Last codepoint of the run, inclusive.
    pub end: u32,
    /// The glyph id at `start`. When the subtable carries an explicit glyph
    /// array for this segment, that array is authoritative and this is only
    /// its first entry.
    pub first_glyph: u32,
}

/// The decoded, owned form of one `cmap` subtable.
///
/// Both modelled formats are *segmented coverage*: an ascending list of
/// `[start, end]` ranges whose glyph ids either run consecutively
/// (`first_glyph + (cp − start)`, format 12 and format 4's `idDelta` path) or
/// come from an explicit array (format 4's `glyphIndexArray`).
///
/// Format 4's glyph-index array is normalised away on decode: a segment that
/// used the array becomes a segment whose glyphs are consecutive only if they
/// genuinely are; otherwise the array values are kept as a per-codepoint
/// table. Both encode back to a spec-valid subtable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmapSubtable {
    /// Format 4: the BMP segment/delta form.
    Format4 {
        /// Segments, ascending and non-overlapping.
        segments: Vec<Segment>,
        /// Explicit glyph ids for format-4 segments that used the
        /// `glyphIndexArray`; entry `k` belongs to segment `k` and has
        /// `end − start + 1` entries (a `0` entry means "not mapped").
        glyph_ids: Vec<Vec<u16>>,
    },
    /// Format 12: segmented coverage, `u32` codepoints.
    Format12 {
        /// Segments, ascending and non-overlapping.
        segments: Vec<Segment>,
    },
}

impl CmapSubtable {
    /// An empty format-4 subtable.
    #[must_use]
    pub fn empty_format4() -> Self {
        CmapSubtable::Format4 {
            segments: Vec::new(),
            glyph_ids: Vec::new(),
        }
    }

    /// An empty format-12 subtable.
    #[must_use]
    pub fn empty_format12() -> Self {
        CmapSubtable::Format12 {
            segments: Vec::new(),
        }
    }

    /// The format number this subtable encodes as (4 or 12).
    #[must_use]
    pub const fn format(&self) -> u16 {
        match self {
            CmapSubtable::Format4 { .. } => 4,
            CmapSubtable::Format12 { .. } => 12,
        }
    }

    /// Map a codepoint to a glyph. `None` for "not mapped": outside the
    /// subtable's plane, in a hole of an explicit glyph array, mapped to
    /// glyph 0, or resolving to a glyph id beyond the `u16` glyph space.
    #[must_use]
    pub fn lookup(&self, cp: u32) -> Option<GlyphId> {
        let (segments, glyph_ids) = match self {
            CmapSubtable::Format4 {
                segments,
                glyph_ids,
            } => {
                if cp > 0xFFFF {
                    return None;
                }
                (segments, glyph_ids.as_slice())
            }
            CmapSubtable::Format12 { segments } => (segments, &[][..]),
        };
        let idx = find_segment(segments, cp)?;
        let seg = segments.get(idx)?;
        let explicit = glyph_ids.get(idx).filter(|arr| !arr.is_empty());
        let gid: u32 = match explicit {
            // Explicit array: entry `k` of segment `idx` covers
            // `seg.start + k`, and a zero entry is a hole.
            Some(arr) => u32::from(
                arr.get(usize::try_from(cp - seg.start).unwrap_or(usize::MAX))
                    .copied()
                    .unwrap_or(0),
            ),
            // An absent *or empty* array means the run is a delta segment.
            None => {
                // Consecutive run: glyph = first_glyph + (cp - start). A run
                // that would step past the glyph space clamps rather than
                // wrapping — a wrapped id would alias an unrelated glyph.
                seg.first_glyph
                    .wrapping_add(cp.wrapping_sub(seg.start))
                    .min(u32::from(u16::MAX))
            }
        };
        let gid = u16::try_from(gid).ok().filter(|&g| g != 0)?;
        Some(GlyphId::new(gid))
    }

    /// The mapped codepoints, ascending.
    #[must_use]
    pub fn codepoints(&self) -> CodePointSet {
        let (segments, glyph_ids) = match self {
            CmapSubtable::Format4 {
                segments,
                glyph_ids,
            } => (segments, glyph_ids.as_slice()),
            CmapSubtable::Format12 { segments } => (segments, &[][..]),
        };
        let mut set = CodePointSet::new();
        for (idx, seg) in segments.iter().enumerate() {
            for cp in seg.start..=seg.end {
                // An explicit array may leave holes, so test each codepoint.
                if self.lookup(cp).is_some() {
                    set.insert(cp);
                }
                let _ = glyph_ids.get(idx);
            }
        }
        set
    }

    /// Every `(codepoint, glyph)` pair, ascending by codepoint. The inverse of
    /// the encode path and what a writer iterates.
    pub fn entries(&self) -> Vec<(u32, GlyphId)> {
        let mut out = Vec::new();
        for cp in self.codepoints().iter() {
            if let Some(gid) = self.lookup(*cp) {
                out.push((*cp, gid));
            }
        }
        out
    }

    /// The segments, ascending. Empty for a subtable that maps nothing.
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        match self {
            CmapSubtable::Format4 { segments, .. } | CmapSubtable::Format12 { segments } => {
                segments
            }
        }
    }

    /// The explicit glyph-id arrays, one per format-4 segment; an empty inner
    /// `Vec` means the segment is a delta (consecutive-glyph) segment.
    #[must_use]
    pub fn glyph_arrays(&self) -> &[Vec<u16>] {
        match self {
            CmapSubtable::Format4 { glyph_ids, .. } => glyph_ids,
            CmapSubtable::Format12 { .. } => &[],
        }
    }

    /// Number of codepoints that map to a glyph.
    #[must_use]
    pub fn len(&self) -> usize {
        self.codepoints().len()
    }

    /// True when nothing maps.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Encode to `cmap` subtable bytes (no table header, just the subtable).
    ///
    /// The output is spec-valid: format 4 ends with the mandatory
    /// `0xFFFF` terminator segment, and every segment is ascending.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        match self {
            CmapSubtable::Format4 { .. } => self.encode_format4(),
            CmapSubtable::Format12 { .. } => self.encode_format12(),
        }
    }

    /// Encode as a format-4 subtable.
    #[must_use]
    pub fn encode_format4(&self) -> Vec<u8> {
        // One encoded segment per modelled segment. The model already knows
        // which segments carry explicit glyph ids, so this is a straight
        // transcription rather than a re-derivation.
        let (segments, glyph_ids) = match self {
            CmapSubtable::Format4 {
                segments,
                glyph_ids,
            } => (segments.as_slice(), glyph_ids.as_slice()),
            CmapSubtable::Format12 { segments } => (segments.as_slice(), &[][..]),
        };
        struct Plan {
            start: u32,
            end: u32,
            delta: i16,
            /// Explicit glyph ids; empty means "use the delta".
            array: Vec<u16>,
        }
        let mut plans: Vec<Plan> = Vec::new();
        for (idx, seg) in segments.iter().enumerate() {
            if seg.start > 0xFFFE {
                continue;
            }
            let end = seg.end.min(0xFFFE);
            if seg.start > end {
                continue;
            }
            let array: Vec<u16> = glyph_ids
                .get(idx)
                .filter(|a| !a.is_empty())
                .map(|a| {
                    a.iter()
                        .copied()
                        .take(usize::try_from(end - seg.start + 1).unwrap_or(0))
                        .collect()
                })
                .unwrap_or_default();
            plans.push(Plan {
                start: seg.start,
                end,
                // A delta segment stores `glyph = (cp + idDelta) mod 2^16`, and
                // `first_glyph = start + idDelta`, so `idDelta` inverts exactly.
                delta: (seg.first_glyph as i32 - seg.start as i32) as i16,
                array,
            });
        }
        // The mandatory terminator segment: start = end = 0xFFFF, with
        // idDelta = 1 so that 0xFFFF + 1 == 0 (glyph 0, "not mapped").
        plans.push(Plan {
            start: 0xFFFF,
            end: 0xFFFF,
            delta: 1,
            array: Vec::new(),
        });
        let final_plans = plans;
        let seg_count = final_plans.len();
        let seg_x2 = 2 * seg_count as u16;
        let entry_selector = {
            let mut e = 0u16;
            while (1usize << (e + 1)) <= seg_count {
                e += 1;
            }
            e
        };
        let search_range = 2u16.wrapping_mul(1u16 << entry_selector);
        let range_shift = seg_x2.wrapping_sub(search_range);

        // `glyphIndexArray` sits after `idRangeOffset`. An `idRangeOffset` is a
        // u16 *element* offset measured from its own slot:
        // (ro / 2) + (cp − start) + seg − segCount == the array index, so the
        // needed value is (array index at cp == start) + segCount − seg.
        let mut glyph_array: Vec<u16> = Vec::new();
        let mut range_offsets: Vec<u16> = Vec::with_capacity(seg_count);
        for (seg, plan) in final_plans.iter().enumerate() {
            if plan.array.is_empty() {
                range_offsets.push(0);
            } else {
                let ro = 2 * (glyph_array.len() + seg_count - seg);
                range_offsets.push(u16::try_from(ro).unwrap_or(0));
                glyph_array.extend_from_slice(&plan.array);
            }
        }
        let length = 16 + 8 * seg_count + 2 * glyph_array.len();
        let mut out = Vec::with_capacity(length);
        out.extend_from_slice(&4u16.to_be_bytes());
        out.extend_from_slice(&(length as u16).to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // language
        out.extend_from_slice(&seg_x2.to_be_bytes());
        out.extend_from_slice(&search_range.to_be_bytes());
        out.extend_from_slice(&entry_selector.to_be_bytes());
        out.extend_from_slice(&range_shift.to_be_bytes());
        for plan in &final_plans {
            out.extend_from_slice(&u16::try_from(plan.end).unwrap_or(0xFFFF).to_be_bytes());
        }
        out.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
        for plan in &final_plans {
            out.extend_from_slice(&u16::try_from(plan.start).unwrap_or(0xFFFF).to_be_bytes());
        }
        for plan in &final_plans {
            out.extend_from_slice(&plan.delta.to_be_bytes());
        }
        for ro in &range_offsets {
            out.extend_from_slice(&ro.to_be_bytes());
        }
        for g in &glyph_array {
            out.extend_from_slice(&g.to_be_bytes());
        }
        out
    }

    /// Encode as a format-12 subtable.
    #[must_use]
    pub fn encode_format12(&self) -> Vec<u8> {
        let groups = self.format12_groups();
        let length = 16 + 12 * groups.len();
        let mut out = Vec::with_capacity(length);
        out.extend_from_slice(&12u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // reserved
        out.extend_from_slice(&(length as u32).to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes()); // language
        out.extend_from_slice(&(groups.len() as u32).to_be_bytes());
        for (start, end, gid) in &groups {
            out.extend_from_slice(&start.to_be_bytes());
            out.extend_from_slice(&end.to_be_bytes());
            out.extend_from_slice(&gid.to_be_bytes());
        }
        out
    }

    /// Decode a subtable from its bytes.
    ///
    /// # Errors
    /// [`ModelError::UnsupportedCmapFormat`] for anything other than formats 4
    /// and 12; [`ModelError::Parse`] when the byte layout is inconsistent.
    #[allow(clippy::result_large_err)]
    pub fn decode(data: &[u8]) -> Result<Self, ModelError> {
        let format = read_u16(data, 0).ok_or_else(truncated)?;
        match format {
            4 => Self::decode_format4(data),
            12 => Self::decode_format12(data),
            other => Err(ModelError::UnsupportedCmapFormat { format: other }),
        }
    }

    /// Decode a format-4 subtable.
    ///
    /// # Errors
    /// [`ModelError::Parse`] when the segment arrays are inconsistent.
    #[allow(clippy::result_large_err)]
    pub fn decode_format4(data: &[u8]) -> Result<Self, ModelError> {
        let seg_x2 = read_u16(data, 6).ok_or_else(truncated)?;
        if seg_x2 == 0 || seg_x2 % 2 != 0 {
            return Err(ModelError::Parse {
                message: alloc::format!(
                    "format 4 segCountX2 {seg_x2} is not a positive even count"
                ),
            });
        }
        let seg_count = usize::from(seg_x2 / 2);
        let ends_at = 14usize;
        let starts_at = ends_at + 2 * seg_count + 2; // + reservedPad
        let deltas_at = starts_at + 2 * seg_count;
        let ranges_at = deltas_at + 2 * seg_count;
        // The glyphIndexArray begins after the four segment arrays.
        let mut segments = Vec::with_capacity(seg_count);
        let mut glyph_ids = Vec::with_capacity(seg_count);
        for i in 0..seg_count {
            let end = read_u16(data, ends_at + 2 * i).ok_or_else(truncated)?;
            let start = read_u16(data, starts_at + 2 * i).ok_or_else(truncated)?;
            let delta = read_i16(data, deltas_at + 2 * i).ok_or_else(truncated)?;
            let range_offset = read_u16(data, ranges_at + 2 * i).ok_or_else(truncated)?;
            // A segment whose start is past its end describes nothing; skip it
            // rather than materialising a zero-width run.
            if start > end {
                segments.push(Segment {
                    start: u32::from(start),
                    end: u32::from(start),
                    first_glyph: 0,
                });
                glyph_ids.push(vec![0]);
                continue;
            }
            let width = usize::from(end - start) + 1;
            if range_offset == 0 {
                // idDelta path: glyph = (cp + delta) mod 65536, so the run's
                // first glyph is `start + delta`.
                segments.push(Segment {
                    start: u32::from(start),
                    end: u32::from(end),
                    first_glyph: u32::from(u16::wrapping_add(start, delta as u16)),
                });
                glyph_ids.push(Vec::new());
            } else {
                // idRangeOffset is a u16 element offset from its own slot:
                // (ro / 2) + (cp − start) + seg − segCount == array index.
                // The spec: `idRangeOffset` is a *byte* offset from the address of its own
                // slot (the `idRangeOffset` entry for this segment), and the
                // value there is the glyph for `start`.
                let base = ranges_at + 2 * i + usize::from(range_offset);
                let mut arr = Vec::with_capacity(width);
                for k in 0..width {
                    arr.push(read_u16(data, base + 2 * k).unwrap_or(0));
                }
                // Normalise: an array that happens to be an arithmetic run
                // collapses to a delta segment.
                let linear = arr
                    .first()
                    .copied()
                    .map(|first| {
                        arr.iter().enumerate().all(|(k, &g)| {
                            g == u16::wrapping_add(first, u16::try_from(k).unwrap_or(0))
                        })
                    })
                    .unwrap_or(false);
                let first_glyph = u32::from(arr.first().copied().unwrap_or(0));
                segments.push(Segment {
                    start: u32::from(start),
                    end: u32::from(end),
                    first_glyph,
                });
                glyph_ids.push(if linear { Vec::new() } else { arr });
            }
        }
        Ok(CmapSubtable::Format4 {
            segments,
            glyph_ids,
        })
    }

    /// Decode a format-12 subtable.
    ///
    /// # Errors
    /// [`ModelError::Parse`] when the group array is inconsistent.
    #[allow(clippy::result_large_err)]
    pub fn decode_format12(data: &[u8]) -> Result<Self, ModelError> {
        let n_groups = read_u32(data, 12).ok_or_else(truncated)?;
        let n = usize::try_from(n_groups).map_err(|_| ModelError::Parse {
            message: alloc::format!("format 12 declares {n_groups} groups"),
        })?;
        let mut segments = Vec::with_capacity(n);
        for i in 0..n {
            let at = 16 + 12 * i;
            let start = read_u32(data, at).ok_or_else(truncated)?;
            let end = read_u32(data, at + 4).ok_or_else(truncated)?;
            let start_gid = read_u32(data, at + 8).ok_or_else(truncated)?;
            if end < start {
                return Err(ModelError::Parse {
                    message: alloc::format!(
                        "format 12 group {i} ends (U+{end:X}) before it starts (U+{start:X})"
                    ),
                });
            }
            segments.push(Segment {
                start,
                end,
                first_glyph: start_gid,
            });
        }
        Ok(CmapSubtable::Format12 { segments })
    }

    /// The BMP-only entries, ascending — the working set for a format-4
    /// encode, and what a caller checks to decide whether a subtable needs a
    /// format-12 companion.
    ///
    /// ```
    /// use font_model::{from_entries, GlyphId};
    ///
    /// let sub = from_entries(&[(0x41, GlyphId::new(1)), (0x1F600, GlyphId::new(2))], 12);
    /// assert_eq!(sub.bmp_entries(), vec![(0x41, 1)]);
    /// ```
    #[must_use]
    pub fn bmp_entries(&self) -> Vec<(u32, u32)> {
        self.entries()
            .into_iter()
            .filter(|(cp, _)| *cp <= 0xFFFF)
            .map(|(cp, gid)| (cp, gid.to_u32()))
            .collect()
    }

    /// Format-12 groups for the whole range, ascending.
    fn format12_groups(&self) -> Vec<(u32, u32, u32)> {
        let entries = self.entries();
        let mut groups: Vec<(u32, u32, u32)> = Vec::new();
        let mut i = 0usize;
        while i < entries.len() {
            let Some(&(start, gid)) = entries.get(i) else {
                break;
            };
            let mut end = start;
            let mut j = i + 1;
            while let Some(&(cp, g)) = entries.get(j) {
                if cp != end + 1 || g.to_u32() != gid.to_u32().wrapping_add(end + 1 - start) {
                    break;
                }
                end = cp;
                j += 1;
            }
            groups.push((start, end, gid.to_u32()));
            i = j;
        }
        groups
    }
}

/// A font's character map: one or more modelled subtables.
///
/// Lookup tries the subtables in order and returns the first hit, mirroring
/// the parser's spec-preference scan while staying in the owned model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cmap {
    subtables: Vec<CmapSubtable>,
}

impl Cmap {
    /// An empty `cmap` (nothing maps).
    #[must_use]
    pub fn new() -> Self {
        Cmap {
            subtables: Vec::new(),
        }
    }

    /// A `cmap` with one empty format-4 subtable — the minimum every font
    /// carries.
    #[must_use]
    pub fn format4() -> Self {
        Cmap {
            subtables: vec![CmapSubtable::empty_format4()],
        }
    }

    /// A `cmap` with one empty format-12 subtable.
    #[must_use]
    pub fn format12() -> Self {
        Cmap {
            subtables: vec![CmapSubtable::empty_format12()],
        }
    }

    /// The subtables, in lookup order.
    #[must_use]
    pub fn subtables(&self) -> &[CmapSubtable] {
        &self.subtables
    }

    /// The subtables, mutably.
    pub fn subtables_mut(&mut self) -> &mut Vec<CmapSubtable> {
        &mut self.subtables
    }

    /// The subtable a writer should emit first: format 12 when the font has
    /// one (it covers every codepoint), otherwise the first format-4 subtable.
    /// `None` for an empty `cmap`.
    #[must_use]
    pub fn preferred(&self) -> Option<&CmapSubtable> {
        self.subtables
            .iter()
            .find(|s| s.format() == 12)
            .or_else(|| self.subtables.first())
    }

    /// Map a codepoint to a glyph across every subtable.
    #[must_use]
    pub fn lookup(&self, cp: u32) -> Option<GlyphId> {
        self.subtables.iter().find_map(|s| s.lookup(cp))
    }

    /// The union of every subtable's mapped codepoints.
    #[must_use]
    pub fn codepoints(&self) -> CodePointSet {
        let mut set = CodePointSet::new();
        for s in &self.subtables {
            for cp in s.codepoints().iter() {
                set.insert(*cp);
            }
        }
        set
    }

    /// Number of subtables.
    #[must_use]
    pub fn len(&self) -> usize {
        self.subtables.len()
    }

    /// True when the `cmap` carries no subtable.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.subtables.is_empty()
    }

    /// Encode the whole `cmap` table (header + encoding records + subtables),
    /// writing one encoding record per subtable: `(3, 1)` for format 4 and
    /// `(3, 10)` for format 12.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let n = self.subtables.len();
        let header = 4 + 8 * n;
        let mut offsets = Vec::with_capacity(n);
        let mut bodies = Vec::with_capacity(n);
        let mut at = header;
        for sub in &self.subtables {
            offsets.push(at);
            let body = sub.encode();
            // Subtables are 4-byte aligned within the table.
            at += body.len();
            at = (at + 3) & !3;
            bodies.push(body);
        }
        let mut out = Vec::with_capacity(at);
        out.extend_from_slice(&0u16.to_be_bytes()); // version
        out.extend_from_slice(&(n as u16).to_be_bytes());
        for (i, sub) in self.subtables.iter().enumerate() {
            let (platform, encoding) = if sub.format() == 12 {
                (3u16, 10u16)
            } else {
                (3, 1)
            };
            out.extend_from_slice(&platform.to_be_bytes());
            out.extend_from_slice(&encoding.to_be_bytes());
            out.extend_from_slice(
                &u32::try_from(offsets.get(i).copied().unwrap_or(0))
                    .unwrap_or(0)
                    .to_be_bytes(),
            );
        }
        for body in &bodies {
            out.extend_from_slice(body);
            while out.len() % 4 != 0 {
                out.push(0);
            }
        }
        out
    }

    /// Decode a whole `cmap` table: header, encoding records, and every
    /// subtable they point at.
    ///
    /// # Errors
    /// [`ModelError::Parse`] when the header or records are inconsistent, or
    /// [`ModelError::UnsupportedCmapFormat`] for a subtable format outside the
    /// modelled set.
    #[allow(clippy::result_large_err)]
    pub fn decode(data: &[u8]) -> Result<Self, ModelError> {
        let num_tables = read_u16(data, 2).ok_or_else(truncated)?;
        let mut out = Vec::new();
        for i in 0..usize::from(num_tables) {
            let at = 4 + 8 * i;
            let offset = read_u32(data, at + 4).ok_or_else(truncated)?;
            let start = usize::try_from(offset).map_err(|_| truncated())?;
            let body = data.get(start..).ok_or_else(truncated)?;
            out.push(CmapSubtable::decode(body)?);
        }
        Ok(Cmap { subtables: out })
    }

    /// Decode a standalone subtable (no `cmap` header) — what
    /// [`CmapSubtable::encode`] produced.
    ///
    /// # Errors
    /// See [`CmapSubtable::decode`].
    #[allow(clippy::result_large_err)]
    pub fn decode_subtable(data: &[u8]) -> Result<CmapSubtable, ModelError> {
        CmapSubtable::decode(data)
    }

    /// An empty format-4 subtable, for building.
    #[must_use]
    pub fn subtable_format4() -> CmapSubtable {
        CmapSubtable::empty_format4()
    }

    /// An empty format-12 subtable, for building.
    #[must_use]
    pub fn subtable_format12() -> CmapSubtable {
        CmapSubtable::empty_format12()
    }

    /// Check the monotonicity invariant across every subtable.
    ///
    /// # Errors
    /// [`ModelError::NonMonotonicCmap`] naming the subtable index and the two
    /// codepoints that broke the ordering.
    pub fn validate(&self) -> Result<(), ModelError> {
        for (si, sub) in self.subtables.iter().enumerate() {
            // Checked at the *segment* level rather than the entry level: two
            // overlapping segments de-duplicate into a single entry, so an
            // entry walk would not see the overlap at all. The segments must
            // ascend, and each must start strictly after the previous one ends.
            let mut previous_end: Option<u32> = None;
            for seg in sub.segments() {
                if let Some(end) = previous_end {
                    if seg.start <= end {
                        return Err(ModelError::NonMonotonicCmap {
                            subtable: si,
                            previous: end,
                            current: seg.start,
                        });
                    }
                }
                previous_end = Some(seg.end);
            }
        }
        Ok(())
    }
}

/// Insert a `(codepoint, glyph)` pair into a subtable, extending the run it
/// belongs to when the glyph ids stay consecutive.
///
/// Returns the rebuilt subtable; the caller owns it (the model stores owned
/// subtables, and this keeps `CmapSubtable` a plain data type).
#[must_use]
pub fn insert_entry(sub: &CmapSubtable, cp: u32, gid: GlyphId) -> CmapSubtable {
    // Rebuild from the full entry list: correctness over cleverness, and the
    // entry counts involved are small.
    let mut entries = sub.entries();
    entries.retain(|(c, _)| *c != cp);
    entries.push((cp, gid));
    entries.sort_by_key(|(c, _)| *c);
    from_entries(&entries, sub.format())
}

/// Build a subtable of the given format from a sorted `(codepoint, glyph)`
/// entry list, collapsing consecutive runs.
#[must_use]
pub fn from_entries(entries: &[(u32, GlyphId)], format: u16) -> CmapSubtable {
    // Sorting here rather than trusting the caller: the invariant every
    // downstream consumer relies on (ascending, de-duplicated) is worth more
    // than the O(n log n) on a list that is usually already sorted. A repeated
    // codepoint keeps its *last* glyph, matching an overwrite.
    let mut sorted: Vec<(u32, GlyphId)> = entries.to_vec();
    sorted.sort_by_key(|(cp, _)| *cp);
    sorted.dedup_by(|a, b| {
        if a.0 == b.0 {
            b.1 = a.1;
            true
        } else {
            false
        }
    });
    let entries = sorted.as_slice();
    // Group the entries into maximal runs of *consecutive codepoints*,
    // recording whether each run's glyph ids are also consecutive. Format 12
    // can only express the latter, so it splits the runs further; format 4
    // uses the explicit `glyphIndexArray` for a run that is not consecutive.
    struct Run {
        start: u32,
        end: u32,
        first_glyph: u32,
        consecutive: bool,
    }
    let mut runs: Vec<Run> = Vec::new();
    let mut i = 0usize;
    while i < entries.len() {
        let Some(&(start, gid)) = entries.get(i) else {
            break;
        };
        let first_glyph = gid.to_u32();
        let mut end = start;
        let mut j = i + 1;
        while let Some(&(cp, _)) = entries.get(j) {
            if cp != end + 1 {
                break;
            }
            end = cp;
            j += 1;
        }
        let consecutive = entries
            .get(i..j)
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .all(|(k, (_, g))| g.to_u32() == first_glyph.wrapping_add(k as u32));
        runs.push(Run {
            start,
            end,
            first_glyph,
            consecutive,
        });
        i = j;
    }
    if format == 12 {
        // A format-12 group stores only its first glyph id, so a run whose
        // glyph ids are *not* consecutive has to be split back into maximal
        // sub-runs that are.
        let mut segments: Vec<Segment> = Vec::new();
        for run in runs {
            if run.consecutive {
                segments.push(Segment {
                    start: run.start,
                    end: run.end,
                    first_glyph: run.first_glyph,
                });
                continue;
            }
            let width = usize::try_from(run.end - run.start + 1).unwrap_or(0);
            let slice = match (i_of(entries, run.start), width) {
                (Some(at), width) => entries.get(at..at.saturating_add(width)),
                _ => None,
            };
            let Some(slice) = slice else { continue };
            let mut k = 0usize;
            while k < slice.len() {
                let Some(&(s, g)) = slice.get(k) else {
                    break;
                };
                let mut e = s;
                let mut m = k + 1;
                while let Some(&(ncp, ng)) = slice.get(m) {
                    if ncp != e + 1 || ng.to_u32() != g.to_u32().wrapping_add(e + 1 - s) {
                        break;
                    }
                    e = ncp;
                    m += 1;
                }
                segments.push(Segment {
                    start: s,
                    end: e,
                    first_glyph: g.to_u32(),
                });
                k = m;
            }
        }
        return CmapSubtable::Format12 { segments };
    }
    // Format 4 is BMP-only, and U+FFFF is reserved for the terminator
    // segment, so the highest mappable codepoint is U+FFFE.
    let mut segments = Vec::new();
    let mut glyph_ids: Vec<Vec<u16>> = Vec::new();
    for run in runs {
        if run.start > 0xFFFE {
            continue;
        }
        let start = run.start;
        let end = run.end.min(0xFFFE);
        segments.push(Segment {
            start,
            end,
            first_glyph: run.first_glyph,
        });
        if run.consecutive {
            // Expressible as a delta (mod 2^16).
            glyph_ids.push(Vec::new());
        } else {
            // Anything else needs the explicit array, holes included.
            let arr = (start..=end)
                .map(|cp| {
                    entries
                        .iter()
                        .find(|(c, _)| *c == cp)
                        .and_then(|(_, g)| u16::try_from(g.to_u32()).ok())
                        .unwrap_or(0)
                })
                .collect();
            glyph_ids.push(arr);
        }
    }
    CmapSubtable::Format4 {
        segments,
        glyph_ids,
    }
}

/// The index of `cp` in a sorted entry list, `None` when absent.
fn i_of(entries: &[(u32, GlyphId)], cp: u32) -> Option<usize> {
    entries
        .iter()
        .position(|(c, _)| *c == cp)
        .or_else(|| entries.iter().position(|(c, _)| *c >= cp))
}

/// Find the segment covering `cp` in an ascending, non-overlapping list.
///
/// Binary search: the last segment whose `start` is `<= cp` wins, and the
/// result must also satisfy `cp <= end`.
fn find_segment(segments: &[Segment], cp: u32) -> Option<usize> {
    // The comparator never returns `Equal`, so `binary_search_by` always
    // returns `Err(insertion_point)` — the index of the first segment starting
    // after `cp`. Either arm of the match names that index.
    let idx = match segments.binary_search_by(|s| {
        if s.start > cp {
            core::cmp::Ordering::Greater
        } else {
            core::cmp::Ordering::Less
        }
    }) {
        Ok(i) | Err(i) => i,
    };
    let idx = idx.checked_sub(1)?;
    let seg = segments.get(idx)?;
    (cp <= seg.end).then_some(idx)
}

/// A shared "truncated cmap" parse error.
fn truncated() -> ModelError {
    ModelError::Parse {
        message: alloc::string::String::from("truncated cmap subtable"),
    }
}

/// Big-endian `u16` read, `None` when the buffer is short.
fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    let bytes: [u8; 2] = data.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_be_bytes(bytes))
}

/// Big-endian `i16` read.
fn read_i16(data: &[u8], at: usize) -> Option<i16> {
    read_u16(data, at).map(|v| v as i16)
}

/// Big-endian `u32` read, `None` when the buffer is short.
fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    let bytes: [u8; 4] = data.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::float_cmp
    )]
    use super::{insert_entry, Cmap, CmapSubtable, CodePointSet};
    use crate::{GlyphId, ModelError};
    use alloc::vec;
    use alloc::vec::Vec;

    // Test harness: assertions legitimately panic; the lib target stays
    // lint-clean.

    /// The format-4 fixture from the OpenType spec's worked example:
    /// three segments plus the 0xFFFF terminator, exercising both the
    /// `idDelta` path and the `glyphIndexArray` path.
    fn spec_format4() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&4u16.to_be_bytes()); // format
        d.extend_from_slice(&40u16.to_be_bytes()); // length (patched below)
        d.extend_from_slice(&0u16.to_be_bytes()); // language
        d.extend_from_slice(&6u16.to_be_bytes()); // segCountX2 = 3 segments
        d.extend_from_slice(&4u16.to_be_bytes()); // searchRange
        d.extend_from_slice(&1u16.to_be_bytes()); // entrySelector
        d.extend_from_slice(&2u16.to_be_bytes()); // rangeShift
        d.extend_from_slice(&0x0020u16.to_be_bytes()); // end[0]
        d.extend_from_slice(&0x007Au16.to_be_bytes()); // end[1]
        d.extend_from_slice(&0xFFFFu16.to_be_bytes()); // end[2] terminator
        d.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
        d.extend_from_slice(&0x0020u16.to_be_bytes()); // start[0]
        d.extend_from_slice(&0x0061u16.to_be_bytes()); // start[1]
        d.extend_from_slice(&0xFFFFu16.to_be_bytes()); // start[2]
        d.extend_from_slice(&0x0028u16.to_be_bytes()); // idDelta[0] 0x20+0x28=0x48
        d.extend_from_slice(&0x0000u16.to_be_bytes()); // idDelta[1] (array)
        d.extend_from_slice(&0x0001u16.to_be_bytes()); // idDelta[2] -> 0
        d.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset[0]
                                                  // idRangeOffset is a byte offset from the address of its own slot;
                                                  // the array sits at offset 40 and the slot at 36, so 4.
        d.extend_from_slice(&4u16.to_be_bytes()); // idRangeOffset[1]
        d.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset[2]
                                                  // glyphIndexArray for segment 1: 'a'..'z' -> glyphs 3, 0, 5, 0, 7 ...
        for g in [3u16, 0, 5, 0, 7] {
            d.extend_from_slice(&g.to_be_bytes());
        }
        let len = u16::try_from(d.len()).unwrap_or(0);
        d[2..4].copy_from_slice(&len.to_be_bytes());
        d
    }

    #[test]
    fn format4_decodes_spec_example() {
        let bytes = spec_format4();
        let sub = CmapSubtable::decode(&bytes).expect("decodes");
        assert_eq!(sub.format(), 4);
        // idDelta path: 0x20 -> 0x48.
        assert_eq!(sub.lookup(0x20), Some(GlyphId::new(0x48)));
        // Array path with holes.
        assert_eq!(sub.lookup(0x61), Some(GlyphId::new(3)));
        assert_eq!(sub.lookup(0x62), None);
        assert_eq!(sub.lookup(0x63), Some(GlyphId::new(5)));
        assert_eq!(sub.lookup(0x65), Some(GlyphId::new(7)));
        // The terminator maps to glyph 0 == unmapped.
        assert_eq!(sub.lookup(0xFFFF), None);
        assert_eq!(sub.lookup(0x0000), None);
        // Above the BMP.
        assert_eq!(sub.lookup(0x1_0000), None);
    }

    #[test]
    fn format4_round_trips_the_spec_example() {
        let sub = CmapSubtable::decode(&spec_format4()).expect("decodes");
        let bytes = sub.encode();
        let back = CmapSubtable::decode(&bytes).expect("re-decodes");
        for cp in 0u32..=0xFFFF {
            assert_eq!(sub.lookup(cp), back.lookup(cp), "mismatch at U+{cp:04X}");
        }
        assert_eq!(sub.entries(), back.entries());
    }

    #[test]
    fn format4_round_trips_random_bmp_runs() {
        // Every interesting shape: single points, contiguous runs, gaps,
        // non-consecutive glyph ids inside a contiguous codepoint run, and the
        // U+FFFF sentinel.
        let cases: [Vec<(u32, u16)>; 6] = [
            vec![(0x41, 1)],
            vec![(0x41, 1), (0x42, 2), (0x43, 3)],
            vec![(0x41, 9), (0x50, 3)],
            vec![(0x41, 5), (0x42, 5), (0x43, 5)],
            vec![(0x00, 1), (0x7FFF, 2), (0xFFFE, 3)],
            vec![(0x30, 100), (0x39, 109), (0x3A, 200), (0x41, 300)],
        ];
        for case in cases {
            let entries: Vec<(u32, GlyphId)> =
                case.iter().map(|(c, g)| (*c, GlyphId::new(*g))).collect();
            let sub = super::from_entries(&entries, 4);
            let bytes = sub.encode();
            let back = CmapSubtable::decode(&bytes).expect("decodes");
            for (cp, gid) in &entries {
                assert_eq!(back.lookup(*cp), Some(*gid), "case {case:?} at U+{cp:X}");
            }
            // Unmapped neighbours stay unmapped.
            assert_eq!(back.lookup(0x10), None, "case {case:?}");
            assert_eq!(back.len(), entries.len(), "case {case:?}");
        }
    }

    #[test]
    fn format4_always_appends_the_ffff_terminator() {
        let sub = super::from_entries(&[(0x41, GlyphId::new(1))], 4);
        let bytes = sub.encode();
        let seg_x2 = u16::from_be_bytes([bytes[6], bytes[7]]);
        let seg_count = usize::from(seg_x2 / 2);
        let ends_at = 14;
        let last_end = u16::from_be_bytes([
            bytes[ends_at + 2 * (seg_count - 1)],
            bytes[ends_at + 2 * (seg_count - 1) + 1],
        ]);
        assert_eq!(last_end, 0xFFFF, "terminator segment missing");
        // `startCode` follows `endCode` plus the reservedPad u16.
        let starts_at = ends_at + 2 * seg_count + 2;
        let last_start = u16::from_be_bytes([
            bytes[starts_at + 2 * (seg_count - 1)],
            bytes[starts_at + 2 * (seg_count - 1) + 1],
        ]);
        assert_eq!(last_start, 0xFFFF, "terminator start missing");
    }

    #[test]
    fn format4_never_maps_the_terminator() {
        // Even when the caller tries to map U+FFFF, the terminator segment
        // resolves to glyph 0 == unmapped.
        let sub = super::from_entries(&[(0x41, GlyphId::new(1)), (0xFFFF, GlyphId::new(7))], 4);
        let back = CmapSubtable::decode(&sub.encode()).expect("decodes");
        assert_eq!(back.lookup(0xFFFF), None);
        assert_eq!(back.lookup(0x41), Some(GlyphId::new(1)));
    }

    #[test]
    fn format12_round_trips_above_the_bmp() {
        let entries = vec![
            (0x1F600u32, GlyphId::new(10)),
            (0x1F601, GlyphId::new(11)),
            (0x1F602, GlyphId::new(12)),
            (0x20000, GlyphId::new(200)),
            (0x41, GlyphId::new(1)),
            (0x10FFFF, GlyphId::new(65535)),
        ];
        let sub = super::from_entries(&entries, 12);
        let bytes = sub.encode();
        assert_eq!(u16::from_be_bytes([bytes[0], bytes[1]]), 12);
        let back = CmapSubtable::decode(&bytes).expect("decodes");
        for (cp, gid) in &entries {
            assert_eq!(back.lookup(*cp), Some(*gid), "at U+{cp:X}");
        }
        assert_eq!(back.lookup(0x1F603), None, "past the first group");
        assert_eq!(back.lookup(0x1F5FF), None);
        // `from_entries` sorts and de-duplicates, so the round trip preserves
        // the *set* of entries rather than the input order.
        let mut expected = entries.clone();
        expected.sort_by_key(|(cp, _)| *cp);
        assert_eq!(back.entries(), expected);
    }

    #[test]
    fn format12_handles_a_span_past_the_bmp() {
        // A single run crossing the BMP boundary (0xFFFE, 0xFFFF, 0x10000,
        // 0x10001) exercises the u32 codepoint path and the group encoder.
        let entries: Vec<(u32, GlyphId)> = (0xFFFEu32..=0x10001)
            .map(|cp| {
                (
                    cp,
                    GlyphId::new(100 + u16::try_from(cp - 0xFFFE).unwrap_or(0)),
                )
            })
            .collect();
        let sub = super::from_entries(&entries, 12);
        let back = CmapSubtable::decode(&sub.encode()).expect("decodes");
        for (cp, gid) in &entries {
            assert_eq!(back.lookup(*cp), Some(*gid));
        }
    }

    #[test]
    fn lookup_of_mapped_unmapped_and_surrogate() {
        let sub = super::from_entries(
            &[
                (0x41, GlyphId::new(1)),
                (0xD800, GlyphId::new(2)), // a surrogate codepoint, kept raw
                (0x1F600, GlyphId::new(3)),
            ],
            12,
        );
        assert_eq!(sub.lookup(0x41), Some(GlyphId::new(1)));
        assert_eq!(sub.lookup(0x42), None);
        assert_eq!(sub.lookup(0xD800), Some(GlyphId::new(2)));
        assert_eq!(sub.lookup(0xDFFF), None);
        assert_eq!(sub.lookup(0x1F600), Some(GlyphId::new(3)));
        assert_eq!(sub.lookup(0x10FFFF), None);
    }

    #[test]
    fn insert_entry_extends_runs() {
        let mut sub = Cmap::subtable_format12();
        sub = insert_entry(&sub, 0x41, GlyphId::new(1));
        sub = insert_entry(&sub, 0x42, GlyphId::new(2));
        sub = insert_entry(&sub, 0x44, GlyphId::new(4));
        assert_eq!(sub.lookup(0x41), Some(GlyphId::new(1)));
        assert_eq!(sub.lookup(0x42), Some(GlyphId::new(2)));
        assert_eq!(sub.lookup(0x43), None);
        assert_eq!(sub.lookup(0x44), Some(GlyphId::new(4)));
        // Overwriting an existing entry keeps the count stable.
        sub = insert_entry(&sub, 0x42, GlyphId::new(9));
        assert_eq!(sub.lookup(0x42), Some(GlyphId::new(9)));
        assert_eq!(sub.len(), 3);
        let back = CmapSubtable::decode(&sub.encode()).expect("decodes");
        assert_eq!(back.lookup(0x42), Some(GlyphId::new(9)));
    }

    #[test]
    fn codepoint_set_operations() {
        let a = CodePointSet::from_iter_raw(vec![3, 1, 2, 2]);
        assert_eq!(a.as_slice(), &[1, 2, 3]);
        assert_eq!(a.len(), 3);
        let mut b = CodePointSet::new();
        assert!(b.is_empty());
        b.insert(5);
        b.insert(1);
        b.insert(5);
        assert_eq!(b.as_slice(), &[1, 5]);
        assert!(b.contains(5));
        assert!(!b.contains(4));

        let u = a.union(&b);
        assert_eq!(u.as_slice(), &[1, 2, 3, 5]);
        let i = a.intersection(&b);
        assert_eq!(i.as_slice(), &[1]);
        let d = a.difference(&b);
        assert_eq!(d.as_slice(), &[2, 3]);
        assert!(CodePointSet::from_iter_raw(vec![1]).is_subset_of(&b));
        assert!(!a.is_subset_of(&b));
        assert_eq!(a.iter().copied().collect::<Vec<u32>>(), vec![1, 2, 3]);
        assert_eq!(CodePointSet::default(), CodePointSet::new());
    }

    #[test]
    fn subtable_codepoints_and_entries() {
        let sub = super::from_entries(
            &[
                (0x41, GlyphId::new(1)),
                (0x42, GlyphId::new(2)),
                (0x43, GlyphId::new(5)),
            ],
            4,
        );
        let cps = sub.codepoints();
        assert_eq!(cps.len(), 3);
        assert!(cps.contains(0x43));
        assert_eq!(sub.entries().len(), 3);
        assert!(!sub.is_empty());
        assert!(CmapSubtable::empty_format4().is_empty());
        assert!(CmapSubtable::empty_format12().is_empty());
    }

    #[test]
    fn cmap_table_round_trip() {
        let mut c = Cmap::new();
        c.subtables_mut()
            .push(super::from_entries(&[(0x41, GlyphId::new(1))], 4));
        c.subtables_mut()
            .push(super::from_entries(&[(0x1F600, GlyphId::new(2))], 12));
        let bytes = c.encode();
        let back = Cmap::decode(&bytes).expect("decodes");
        assert_eq!(back.len(), 2);
        assert_eq!(back.subtables()[0].format(), 4);
        assert_eq!(back.subtables()[1].format(), 12);
        assert_eq!(back.lookup(0x41), Some(GlyphId::new(1)));
        assert_eq!(back.lookup(0x1F600), Some(GlyphId::new(2)));
        let cps = back.codepoints();
        assert_eq!(cps.len(), 2);
    }

    #[test]
    fn cmap_lookups_fall_through_subtables() {
        let mut c = Cmap::new();
        c.subtables_mut()
            .push(super::from_entries(&[(0x41, GlyphId::new(1))], 4));
        c.subtables_mut()
            .push(super::from_entries(&[(0x42, GlyphId::new(2))], 12));
        assert_eq!(c.lookup(0x41), Some(GlyphId::new(1)));
        assert_eq!(c.lookup(0x42), Some(GlyphId::new(2)));
        assert_eq!(c.lookup(0x43), None);
        assert_eq!(c.len(), 2);
        assert!(!c.is_empty());
        assert!(Cmap::new().is_empty());
        assert_eq!(Cmap::format4().subtables().len(), 1);
        assert_eq!(Cmap::format12().subtables()[0].format(), 12);
        assert!(c.preferred().is_some());
        assert!(Cmap::new().preferred().is_none());
    }

    #[test]
    fn validate_rejects_non_monotonic() {
        // A format-12 subtable whose segments overlap: `validate` walks the
        // *entries*, so the second segment's first codepoint (0x41) is not
        // strictly greater than the first segment's last (0x5F).
        let bad = CmapSubtable::Format12 {
            segments: vec![
                super::Segment {
                    start: 0x41,
                    end: 0x5F,
                    first_glyph: 10,
                },
                super::Segment {
                    start: 0x50,
                    end: 0x5F,
                    first_glyph: 20,
                },
            ],
        };
        let c = Cmap {
            subtables: vec![bad],
        };
        match c.validate().unwrap_err() {
            ModelError::NonMonotonicCmap {
                subtable,
                previous,
                current,
            } => {
                assert_eq!(subtable, 0);
                assert_eq!(previous, 0x5F, "the first segment's end");
                assert_eq!(current, 0x50, "the overlapping start");
                let msg = alloc::format!(
                    "{}",
                    ModelError::NonMonotonicCmap {
                        subtable,
                        previous,
                        current
                    }
                );
                assert!(msg.contains("U+005F") && msg.contains("U+0050"), "{msg}");
            }
            other => panic!("expected NonMonotonicCmap, got {other:?}"),
        }
        // A repeated codepoint across two subtables is *legal* — a BMP table
        // inside a format-12 one is exactly that.
        let overlap_across = Cmap {
            subtables: vec![
                super::from_entries(&[(0x41, GlyphId::new(1))], 4),
                super::from_entries(&[(0x41, GlyphId::new(2))], 12),
            ],
        };
        assert!(overlap_across.validate().is_ok());
    }

    #[test]
    fn validate_accepts_well_formed() {
        let mut c = Cmap::new();
        c.subtables_mut()
            .push(super::from_entries(&[(0x41, GlyphId::new(1))], 4));
        c.subtables_mut().push(super::from_entries(
            &[(0x1F600, GlyphId::new(2)), (0x1F601, GlyphId::new(3))],
            12,
        ));
        assert!(c.validate().is_ok());
        assert!(Cmap::new().validate().is_ok());
    }

    #[test]
    fn decode_rejects_unsupported_and_truncated() {
        let mut f6 = vec![0u8; 16];
        f6[0..2].copy_from_slice(&6u16.to_be_bytes());
        match CmapSubtable::decode(&f6).unwrap_err() {
            ModelError::UnsupportedCmapFormat { format } => assert_eq!(format, 6),
            other => panic!("expected UnsupportedCmapFormat, got {other:?}"),
        }
        // A two-byte buffer naming format 4: the header is there, the segment
        // arrays are not.
        assert!(matches!(
            CmapSubtable::decode(&[0, 4]).unwrap_err(),
            ModelError::Parse { .. }
        ));
        assert!(matches!(
            Cmap::decode(&[0, 0, 1, 0]).unwrap_err(),
            ModelError::Parse { .. }
        ));
        // Format 12 with a group that ends before it starts.
        let mut bad = vec![0u8; 28];
        bad[0..2].copy_from_slice(&12u16.to_be_bytes());
        bad[12..16].copy_from_slice(&1u32.to_be_bytes());
        bad[16..20].copy_from_slice(&0x100u32.to_be_bytes());
        bad[20..24].copy_from_slice(&0x050u32.to_be_bytes());
        assert!(matches!(
            CmapSubtable::decode(&bad).unwrap_err(),
            ModelError::Parse { .. }
        ));
    }

    #[test]
    fn from_entries_format4_clips_astral() {
        let entries = vec![(0x1F600u32, GlyphId::new(5)), (0x41, GlyphId::new(1))];
        let sub = super::from_entries(&entries, 4);
        assert_eq!(sub.len(), 1);
        assert_eq!(sub.lookup(0x41), Some(GlyphId::new(1)));
        assert_eq!(sub.lookup(0x1F600), None);
    }

    #[test]
    fn format4_with_array_encodes_back_to_an_array() {
        // A contiguous codepoint run with non-consecutive glyphs needs the
        // glyphIndexArray; the decode must reproduce the holes.
        let entries: Vec<(u32, GlyphId)> = (0x41u32..0x46)
            .map(|cp| {
                (
                    cp,
                    GlyphId::new(10 + u16::try_from((cp - 0x41) * 2).unwrap_or(0)),
                )
            })
            .collect();
        let sub = super::from_entries(&entries, 4);
        let bytes = sub.encode();
        let back = CmapSubtable::decode(&bytes).expect("decodes");
        for (cp, gid) in &entries {
            assert_eq!(back.lookup(*cp), Some(*gid), "at U+{cp:X}");
        }
        assert_eq!(back.entries(), entries);
    }

    #[test]
    fn tiny_and_degenerate_subtables() {
        // Empty encode/decode round trip.
        let sub = CmapSubtable::empty_format4();
        let back = CmapSubtable::decode(&sub.encode()).expect("decodes");
        assert!(back.is_empty());
        let sub12 = CmapSubtable::empty_format12();
        let back12 = CmapSubtable::decode(&sub12.encode()).expect("decodes");
        assert!(back12.is_empty());
        assert_eq!(back12.lookup(0x41), None);
        // Single-entry format 12.
        let one = super::from_entries(&[(0x41, GlyphId::new(1))], 12);
        let back = CmapSubtable::decode(&one.encode()).expect("decodes");
        assert_eq!(back.lookup(0x42), None);
        // Glyph id 0 means "not mapped".
        let zero = super::from_entries(&[(0x41, GlyphId::new(0))], 12);
        assert_eq!(zero.lookup(0x41), None);
        assert!(zero.codepoints().is_empty());
    }

    #[test]
    fn insert_entry_on_empty_and_overwrite_glyph_zero() {
        let sub = Cmap::subtable_format4();
        let sub = insert_entry(&sub, 0x41, GlyphId::new(3));
        assert_eq!(sub.lookup(0x41), Some(GlyphId::new(3)));
        let sub = insert_entry(&sub, 0x41, GlyphId::new(0));
        assert_eq!(sub.lookup(0x41), None, "glyph 0 unmaps");
        let sub = insert_entry(&sub, 0x41, GlyphId::new(4));
        assert_eq!(sub.lookup(0x41), Some(GlyphId::new(4)));
    }
}
