//! Serde impls for the owned types, behind the optional `serde` feature.
//!
//! Hand-written in one place rather than `#[cfg_attr]` attributes scattered
//! across the modules, so the feature boundary is a single readable file and
//! the `no_std` build never sees a derive it does not need.
//!
//! The mapping is deliberately structural. Glyph ids serialise as their raw
//! `u16`; commands as `(opcode, …floats)`; a `cmap` as a list of
//! `(format, entries)`; a `Font` as its eight public parts. A model that
//! round-trips through JSON, toml, or a binary format comes back equal.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::cmap::{Cmap, CmapSubtable};
use crate::font::{Font, KernPair, Kerning, TableTag};
use crate::glyph::{Glyph, GlyphId};
use crate::metrics::{FontMetrics, Metrics, VerticalMetrics};
use crate::name::NameRecord;
use crate::os2::Os2Metrics;
use crate::outline::{Bbox, Command, Outline, Path, Winding};

/// The opcodes a [`Command`] serialises as: 0 move, 1 line, 2 quad, 3 cubic,
/// 4 close.
const OP_MOVE: u8 = 0;
const OP_LINE: u8 = 1;
const OP_QUAD: u8 = 2;
const OP_CUBIC: u8 = 3;
const OP_CLOSE: u8 = 4;

/// How many `f32`s each opcode carries, indexed by opcode.
const FLOATS_PER_OP: [usize; 5] = [2, 2, 4, 6, 0];

impl Serialize for GlyphId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u16(self.to_u16())
    }
}

impl<'de> Deserialize<'de> for GlyphId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(GlyphId::new(u16::deserialize(d)?))
    }
}

impl Serialize for Command {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Always six floats on the wire, so every variant has the same shape
        // and a self-describing format (JSON, toml) can read it back without
        // knowing which opcode it is looking at. The unused slots are zero.
        //
        // The wire form is `[opcode, [f0 … f5]]`: a heterogeneous pair, which a
        // self-describing format reads positionally — the opcode first, then
        // the fixed six floats.
        let (op, f) = match *self {
            Command::MoveTo(x, y) => (OP_MOVE, [x, y, 0.0, 0.0, 0.0, 0.0]),
            Command::LineTo(x, y) => (OP_LINE, [x, y, 0.0, 0.0, 0.0, 0.0]),
            Command::QuadTo(cx, cy, x, y) => (OP_QUAD, [cx, cy, x, y, 0.0, 0.0]),
            Command::CubicTo(c1x, c1y, c2x, c2y, x, y) => (OP_CUBIC, [c1x, c1y, c2x, c2y, x, y]),
            Command::Close => (OP_CLOSE, [0.0; 6]),
        };
        (op, f).serialize(s)
    }
}

impl<'de> Deserialize<'de> for Command {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (op, mut f) = <(u8, [f32; 6])>::deserialize(d)?;
        // The float count is a property of the opcode; the array is fixed at six
        // so a variant with fewer floats is padded rather than rejected.
        let need = match op {
            OP_MOVE | OP_LINE | OP_QUAD | OP_CUBIC | OP_CLOSE => {
                FLOATS_PER_OP.get(usize::from(op)).copied().unwrap_or(0)
            }
            other => return Err(serde::de::Error::custom(format!("unknown opcode {other}"))),
        };
        // Zero the padding beyond what the opcode uses, so a hand-written
        // payload cannot smuggle extra data into a command.
        for slot in f.iter_mut().skip(need) {
            *slot = 0.0;
        }
        let v = |i: usize| f.get(i).copied().unwrap_or(0.0);
        Ok(match op {
            OP_MOVE => Command::MoveTo(v(0), v(1)),
            OP_LINE => Command::LineTo(v(0), v(1)),
            OP_QUAD => Command::QuadTo(v(0), v(1), v(2), v(3)),
            OP_CUBIC => Command::CubicTo(v(0), v(1), v(2), v(3), v(4), v(5)),
            _ => Command::Close,
        })
    }
}

impl Serialize for Bbox {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (self.min_x(), self.min_y(), self.max_x(), self.max_y()).serialize(s)
    }
}

impl<'de> Deserialize<'de> for Bbox {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let t = <(f32, f32, f32, f32)>::deserialize(d)?;
        Ok(Bbox::from_corners(t.0, t.1, t.2, t.3))
    }
}

impl Serialize for Winding {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(match self {
            Winding::Degenerate => 0,
            Winding::CounterClockwise => 1,
            Winding::Clockwise => 2,
        })
    }
}

impl<'de> Deserialize<'de> for Winding {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match u8::deserialize(d)? {
            1 => Winding::CounterClockwise,
            2 => Winding::Clockwise,
            _ => Winding::Degenerate,
        })
    }
}

impl Serialize for Path {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.commands().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Path {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let cmds = Vec::<Command>::deserialize(d)?;
        let mut p = Path::new();
        for c in cmds {
            p.push(c);
        }
        Ok(p)
    }
}

impl Serialize for Outline {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.contours().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Outline {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let contours = Vec::<Path>::deserialize(d)?;
        let mut o = Outline::new();
        for c in contours {
            o.contours_mut().push(c);
        }
        Ok(o)
    }
}

impl Serialize for Glyph {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Glyph", 4)?;
        st.serialize_field("id", &self.id().to_u16())?;
        st.serialize_field("advance_width", &self.advance_width())?;
        st.serialize_field("left_side_bearing", &self.left_side_bearing())?;
        st.serialize_field("outline", self.outline())?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for Glyph {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            id: u16,
            advance_width: u16,
            left_side_bearing: i16,
            outline: Outline,
        }
        let r = Repr::deserialize(d)?;
        Ok(Glyph::new(
            GlyphId::new(r.id),
            r.advance_width,
            r.left_side_bearing,
            r.outline,
        ))
    }
}

impl Serialize for Metrics {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (self.advance_width, self.left_side_bearing).serialize(s)
    }
}

impl<'de> Deserialize<'de> for Metrics {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let t = <(u16, i16)>::deserialize(d)?;
        Ok(Metrics::new(t.0, t.1))
    }
}

impl Serialize for VerticalMetrics {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (
            self.ascender,
            self.descender,
            self.line_gap,
            self.advance_height_max,
            self.min_top_side_bearing,
            self.min_bottom_side_bearing,
            self.y_max_extent,
        )
            .serialize(s)
    }
}

impl<'de> Deserialize<'de> for VerticalMetrics {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            ascender: i16,
            descender: i16,
            line_gap: i16,
            advance_height_max: u16,
            min_top_side_bearing: i16,
            min_bottom_side_bearing: i16,
            y_max_extent: i16,
        }
        let r = Repr::deserialize(d)?;
        Ok(VerticalMetrics {
            ascender: r.ascender,
            descender: r.descender,
            line_gap: r.line_gap,
            advance_height_max: r.advance_height_max,
            min_top_side_bearing: r.min_top_side_bearing,
            min_bottom_side_bearing: r.min_bottom_side_bearing,
            y_max_extent: r.y_max_extent,
        })
    }
}

impl Serialize for FontMetrics {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("FontMetrics", 8)?;
        st.serialize_field("ascender", &self.ascender)?;
        st.serialize_field("descender", &self.descender)?;
        st.serialize_field("line_gap", &self.line_gap)?;
        st.serialize_field("advance_width_max", &self.advance_width_max)?;
        st.serialize_field("min_left_side_bearing", &self.min_left_side_bearing)?;
        st.serialize_field("min_right_side_bearing", &self.min_right_side_bearing)?;
        st.serialize_field("x_max_extent", &self.x_max_extent)?;
        st.serialize_field("vertical", &self.vertical)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for FontMetrics {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            ascender: i16,
            descender: i16,
            line_gap: i16,
            advance_width_max: u16,
            min_left_side_bearing: i16,
            min_right_side_bearing: i16,
            x_max_extent: i16,
            #[serde(default)]
            vertical: Option<VerticalMetrics>,
        }
        let r = Repr::deserialize(d)?;
        Ok(FontMetrics {
            ascender: r.ascender,
            descender: r.descender,
            line_gap: r.line_gap,
            advance_width_max: r.advance_width_max,
            min_left_side_bearing: r.min_left_side_bearing,
            min_right_side_bearing: r.min_right_side_bearing,
            x_max_extent: r.x_max_extent,
            vertical: r.vertical,
        })
    }
}

impl Serialize for NameRecord {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("NameRecord", 5)?;
        st.serialize_field("name_id", &self.name_id)?;
        st.serialize_field("platform_id", &self.platform_id)?;
        st.serialize_field("encoding_id", &self.encoding_id)?;
        st.serialize_field("language_id", &self.language_id)?;
        st.serialize_field("value", &self.value)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for NameRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            name_id: u16,
            platform_id: u16,
            encoding_id: u16,
            language_id: u16,
            value: String,
        }
        let r = Repr::deserialize(d)?;
        Ok(NameRecord::with_locale(
            r.name_id,
            r.platform_id,
            r.encoding_id,
            r.language_id,
            r.value,
        ))
    }
}

/// The 24 `OS/2` fields, flattened through a helper so the struct name in
/// the derived repr matches the field order.
macro_rules! os2_fields {
    ($m:ident) => {
        impl Serialize for $m {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let mut st = s.serialize_struct(stringify!($m), 24)?;
                st.serialize_field("version", &self.version)?;
                st.serialize_field("avg_char_width", &self.avg_char_width)?;
                st.serialize_field("weight_class", &self.weight_class)?;
                st.serialize_field("width_class", &self.width_class)?;
                st.serialize_field("fs_type", &self.fs_type)?;
                st.serialize_field("panose", &self.panose)?;
                st.serialize_field("unicode_ranges", &self.unicode_ranges)?;
                st.serialize_field("vendor_id", &self.vendor_id)?;
                st.serialize_field("fs_selection", &self.fs_selection)?;
                st.serialize_field("first_char_index", &self.first_char_index)?;
                st.serialize_field("last_char_index", &self.last_char_index)?;
                st.serialize_field("typo_ascender", &self.typo_ascender)?;
                st.serialize_field("typo_descender", &self.typo_descender)?;
                st.serialize_field("typo_line_gap", &self.typo_line_gap)?;
                st.serialize_field("win_ascent", &self.win_ascent)?;
                st.serialize_field("win_descent", &self.win_descent)?;
                st.serialize_field("code_page_ranges", &self.code_page_ranges)?;
                st.serialize_field("x_height", &self.x_height)?;
                st.serialize_field("cap_height", &self.cap_height)?;
                st.serialize_field("default_char", &self.default_char)?;
                st.serialize_field("break_char", &self.break_char)?;
                st.serialize_field("max_context", &self.max_context)?;
                st.end()
            }
        }

        impl<'de> Deserialize<'de> for $m {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                #[derive(Deserialize)]
                struct Repr {
                    version: u16,
                    avg_char_width: i16,
                    weight_class: u16,
                    width_class: u16,
                    fs_type: u16,
                    #[serde(default)]
                    panose: [u8; 10],
                    #[serde(default)]
                    unicode_ranges: [u32; 4],
                    #[serde(default)]
                    vendor_id: [u8; 4],
                    #[serde(default)]
                    fs_selection: u16,
                    #[serde(default)]
                    first_char_index: u16,
                    #[serde(default)]
                    last_char_index: u16,
                    #[serde(default)]
                    typo_ascender: i16,
                    #[serde(default)]
                    typo_descender: i16,
                    #[serde(default)]
                    typo_line_gap: i16,
                    #[serde(default)]
                    win_ascent: u16,
                    #[serde(default)]
                    win_descent: u16,
                    #[serde(default)]
                    code_page_ranges: [u32; 2],
                    #[serde(default)]
                    x_height: i16,
                    #[serde(default)]
                    cap_height: i16,
                    #[serde(default)]
                    default_char: u16,
                    #[serde(default)]
                    break_char: u16,
                    #[serde(default)]
                    max_context: u16,
                }
                let r = Repr::deserialize(d)?;
                Ok($m {
                    version: r.version,
                    avg_char_width: r.avg_char_width,
                    weight_class: r.weight_class,
                    width_class: r.width_class,
                    fs_type: r.fs_type,
                    panose: r.panose,
                    unicode_ranges: r.unicode_ranges,
                    vendor_id: r.vendor_id,
                    fs_selection: r.fs_selection,
                    first_char_index: r.first_char_index,
                    last_char_index: r.last_char_index,
                    typo_ascender: r.typo_ascender,
                    typo_descender: r.typo_descender,
                    typo_line_gap: r.typo_line_gap,
                    win_ascent: r.win_ascent,
                    win_descent: r.win_descent,
                    code_page_ranges: r.code_page_ranges,
                    x_height: r.x_height,
                    cap_height: r.cap_height,
                    default_char: r.default_char,
                    break_char: r.break_char,
                    max_context: r.max_context,
                })
            }
        }
    };
}

os2_fields!(Os2Metrics);

impl Serialize for KernPair {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (self.left.to_u16(), self.right.to_u16(), self.value).serialize(s)
    }
}

impl<'de> Deserialize<'de> for KernPair {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let t = <(u16, u16, i16)>::deserialize(d)?;
        Ok(KernPair {
            left: GlyphId::new(t.0),
            right: GlyphId::new(t.1),
            value: t.2,
        })
    }
}

impl Serialize for Kerning {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let pairs: Vec<KernPair> = self.iter().copied().collect();
        pairs.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Kerning {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let pairs = Vec::<KernPair>::deserialize(d)?;
        let mut k = Kerning::new();
        for p in pairs {
            k.insert(p.left, p.right, p.value);
        }
        Ok(k)
    }
}

impl Serialize for crate::cmap::CodePointSet {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.as_slice().serialize(s)
    }
}

impl<'de> Deserialize<'de> for crate::cmap::CodePointSet {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(crate::cmap::CodePointSet::from_iter_raw(
            Vec::<u32>::deserialize(d)?,
        ))
    }
}

/// The wire form of a `CmapSubtable`: a format number plus its entries.
///
/// Structurally lossless (an empty subtable stays empty and keeps its format)
/// and format-agnostic, so a model round-trips without depending on the
/// segment representation.
#[derive(Serialize, Deserialize)]
pub struct CmapRepr {
    /// The subtable format (4 or 12).
    pub format: u16,
    /// The `(codepoint, glyph id)` entries, ascending.
    pub entries: Vec<(u32, u16)>,
}

impl CmapRepr {
    /// Rebuild the modelled subtable.
    #[must_use]
    pub fn into_subtable(self) -> CmapSubtable {
        let entries: Vec<(u32, GlyphId)> = self
            .entries
            .into_iter()
            .map(|(cp, gid)| (cp, GlyphId::new(gid)))
            .collect();
        crate::cmap::from_entries(&entries, self.format)
    }

    /// The wire form of a modelled subtable.
    #[must_use]
    pub fn from_subtable(sub: &CmapSubtable) -> Self {
        CmapRepr {
            format: sub.format(),
            entries: sub
                .entries()
                .into_iter()
                .map(|(cp, gid)| (cp, gid.to_u16()))
                .collect(),
        }
    }
}

impl Serialize for Font {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Font", 8)?;
        st.serialize_field("units_per_em", &self.units_per_em())?;
        st.serialize_field("metrics", self.metrics())?;
        st.serialize_field("glyphs", self.glyphs())?;
        let cmap: Vec<CmapRepr> = self
            .cmap()
            .subtables()
            .iter()
            .map(CmapRepr::from_subtable)
            .collect();
        st.serialize_field("cmap", &cmap)?;
        st.serialize_field("names", self.names())?;
        // Cloned rather than borrowed: `Serialize` for `Font` hands out an
        // owned snapshot, which keeps the field order independent of the
        // borrow of `self`.
        st.serialize_field("os2", &self.os2().cloned())?;
        st.serialize_field("kerning", self.kerning())?;
        let tables: Vec<(TableTag, Vec<u8>)> = self.tables().to_vec();
        st.serialize_field("tables", &tables)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for Font {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            units_per_em: u16,
            #[serde(default)]
            metrics: FontMetrics,
            glyphs: Vec<Glyph>,
            #[serde(default)]
            cmap: Vec<CmapRepr>,
            #[serde(default)]
            names: Vec<NameRecord>,
            #[serde(default)]
            os2: Option<Os2Metrics>,
            #[serde(default)]
            kerning: Kerning,
            #[serde(default)]
            tables: Vec<(TableTag, Vec<u8>)>,
        }
        let r = Repr::deserialize(d)?;
        let mut cmap = Cmap::new();
        for sub in r.cmap {
            cmap.subtables_mut().push(sub.into_subtable());
        }
        if cmap.subtables().is_empty() {
            cmap = Cmap::format4();
        }
        Ok(Font::from_parts(
            r.units_per_em,
            r.metrics,
            r.glyphs,
            cmap,
            r.names,
            r.os2,
            r.kerning,
            r.tables,
        ))
    }
}

/// Assert every owned type carries both halves of the contract.
///
/// Used by the test module so a new type without an impl fails to compile
/// rather than silently vanishing from the wire format.
#[allow(dead_code)]
fn assert_bidirectional<T>()
where
    T: Serialize + for<'de> Deserialize<'de>,
{
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::{assert_bidirectional, CmapRepr};
    use crate::font::{build_stub_font, TAG_GPOS, TAG_GSUB};
    use crate::glyph::{Glyph, GlyphId};
    use crate::metrics::{FontMetrics, Metrics, VerticalMetrics};
    use crate::name::{name_id, NameRecord};
    use crate::os2::Os2Metrics;
    use crate::outline::{Bbox, Command, Outline, Path, Winding};
    use crate::{CodePointSet, Font, Kerning, MetricsBuilder, Os2Metrics as Os2};
    use alloc::string::String;
    use alloc::vec;

    // Test harness: assertions legitimately panic; the lib target stays
    // lint-clean.

    /// Every owned type carries both halves of the serde contract.
    #[test]
    fn every_type_is_bidirectional() {
        assert_bidirectional::<GlyphId>();
        assert_bidirectional::<Command>();
        assert_bidirectional::<Bbox>();
        assert_bidirectional::<Winding>();
        assert_bidirectional::<Path>();
        assert_bidirectional::<Outline>();
        assert_bidirectional::<Glyph>();
        assert_bidirectional::<Metrics>();
        assert_bidirectional::<VerticalMetrics>();
        assert_bidirectional::<FontMetrics>();
        assert_bidirectional::<NameRecord>();
        assert_bidirectional::<Os2Metrics>();
        assert_bidirectional::<Kerning>();
        assert_bidirectional::<CodePointSet>();
        assert_bidirectional::<Font>();
        assert_bidirectional::<CmapRepr>();
    }

    /// A font with every optional part populated round-trips through the
    /// structural representation without losing anything.
    #[test]
    fn font_round_trips_with_every_part() {
        let mut font = build_stub_font(&[0x41, 0x42, 0x1F600]).expect("valid");
        font.names_mut()
            .push(NameRecord::new(name_id::FAMILY, String::from("Serde")));
        font.os2_mut().replace(Os2Metrics {
            weight_class: 700,
            unicode_ranges: [1, 2, 3, 4],
            ..Os2Metrics::default()
        });
        font.kerning_mut()
            .insert(GlyphId::new(1), GlyphId::new(2), -30);
        font.set_table(TAG_GSUB, vec![1, 2, 3]);
        font.set_table(TAG_GPOS, vec![4, 5]);
        font.metrics_mut().vertical = Some(VerticalMetrics {
            ascender: 500,
            descender: -500,
            line_gap: 0,
            ..VerticalMetrics::default()
        });

        let cmap: Vec<CmapRepr> = font
            .cmap()
            .subtables()
            .iter()
            .map(CmapRepr::from_subtable)
            .collect();
        assert_eq!(cmap.len(), 2);
        assert_eq!(cmap[0].format, 4);
        assert_eq!(cmap[1].format, 12);
        let mut rebuilt = crate::cmap::Cmap::new();
        for sub in cmap {
            rebuilt.subtables_mut().push(sub.into_subtable());
        }
        assert_eq!(rebuilt.lookup(0x41), font.glyph_for(0x41));
        assert_eq!(rebuilt.lookup(0x1F600), font.glyph_for(0x1F600));
        assert_eq!(rebuilt.codepoints(), font.codepoints());
    }

    #[test]
    fn cmap_repr_of_an_empty_subtable_keeps_its_format() {
        let empty4 = CmapRepr::from_subtable(&crate::CmapSubtable::empty_format4());
        assert_eq!(empty4.format, 4);
        assert!(empty4.entries.is_empty());
        assert!(empty4.into_subtable().is_empty());
        let empty12 = CmapRepr::from_subtable(&crate::CmapSubtable::empty_format12());
        assert_eq!(empty12.format, 12);
        assert!(empty12.into_subtable().is_empty());
    }

    #[test]
    fn serde_json_round_trips_a_font() {
        // The real proof that a model survives a config format.
        let mut font = build_stub_font(&[0x41, 0x42]).expect("valid");
        font.names_mut()
            .push(NameRecord::new(name_id::FAMILY, String::from("Json")));
        font.set_table(TAG_GSUB, vec![7, 8]);
        font.kerning_mut()
            .insert(GlyphId::new(1), GlyphId::new(2), -42);
        let json = serde_json::to_string(&font).expect("serialises");
        let back: Font = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back.units_per_em(), font.units_per_em());
        assert_eq!(back.glyph_count(), font.glyph_count());
        assert_eq!(back.codepoints(), font.codepoints());
        assert_eq!(back.glyph_for(0x42), font.glyph_for(0x42));
        assert_eq!(back.kern(GlyphId::new(1), GlyphId::new(2)), -42);
        assert_eq!(back.table(TAG_GSUB), Some([7u8, 8].as_slice()));
        assert_eq!(back.names().len(), 1);
        assert!(back.validate().is_ok());
    }

    #[test]
    fn serde_json_round_trips_curves_and_commands() {
        let mut outline = Outline::new();
        let mut p = Path::starting_at(10.0, 20.0);
        p.line_to(30.0, 40.0);
        p.quad_to(50.0, 60.0, 70.0, 80.0);
        p.cubic_to(90.0, 100.0, 110.0, 120.0, 130.0, 140.0);
        p.close();
        outline.push_contour(p);
        let g = Glyph::new(GlyphId::new(7), 150, 16, outline);
        let json = serde_json::to_string(&g).expect("serialises");
        let back: Glyph = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, g);
        // And the outline's commands specifically.
        let cmds: Vec<Command> = back.outline().contours()[0].commands().to_vec();
        assert_eq!(cmds.len(), 5);
        assert!(matches!(cmds[2], Command::QuadTo(..)));
        assert!(matches!(cmds[3], Command::CubicTo(..)));
        assert!(matches!(cmds[4], Command::Close));
    }

    #[test]
    fn serde_json_round_trips_metrics_and_bbox() {
        let m = Metrics::new(500, -42);
        let json = serde_json::to_string(&m).expect("serialises");
        assert_eq!(
            serde_json::from_str::<Metrics>(&json).expect("deserialises"),
            m
        );

        let b = Bbox::from_corners(-1.0, 2.0, 3.0, -4.0);
        let json = serde_json::to_string(&b).expect("serialises");
        assert_eq!(
            serde_json::from_str::<Bbox>(&json).expect("deserialises"),
            b
        );
    }

    #[test]
    fn serde_json_rejects_an_unknown_opcode() {
        assert!(serde_json::from_str::<Command>("[9, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0]]").is_err());
        // The float array is fixed at six, so a short one is rejected rather
        // than misread.
        assert!(serde_json::from_str::<Command>("[2, [0.0, 0.0]]").is_err());
        // Padding beyond the opcode's own floats is dropped, not read.
        let padded: Command =
            serde_json::from_str("[2, [1.0, 2.0, 3.0, 4.0, 9.0, 9.0]]").expect("quad");
        assert_eq!(padded, Command::QuadTo(1.0, 2.0, 3.0, 4.0));
        // And a full tuple is accepted.
        let q: Command = serde_json::from_str("[2, [1.0, 2.0, 3.0, 4.0, 0.0, 0.0]]").expect("quad");
        assert_eq!(q, Command::QuadTo(1.0, 2.0, 3.0, 4.0));
        let c: Command =
            serde_json::from_str("[3, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]]").expect("cubic");
        assert_eq!(c, Command::CubicTo(1.0, 2.0, 3.0, 4.0, 5.0, 6.0));
        let cl: Command =
            serde_json::from_str("[4, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0]]").expect("close");
        assert_eq!(cl, Command::Close);
        let mv: Command =
            serde_json::from_str("[0, [1.0, 2.0, 0.0, 0.0, 0.0, 0.0]]").expect("move");
        assert_eq!(mv, Command::MoveTo(1.0, 2.0));
        // And a payload with no opcode at all.
        assert!(serde_json::from_str::<Command>("[]").is_err());
    }

    #[test]
    fn winding_and_codepoint_set_round_trip() {
        for w in [
            Winding::CounterClockwise,
            Winding::Clockwise,
            Winding::Degenerate,
        ] {
            let json = serde_json::to_string(&w).expect("serialises");
            assert_eq!(
                serde_json::from_str::<Winding>(&json).expect("deserialises"),
                w
            );
        }
        let s = CodePointSet::from_iter_raw(vec![5, 1, 3]);
        let json = serde_json::to_string(&s).expect("serialises");
        assert_eq!(
            serde_json::from_str::<CodePointSet>(&json).expect("deserialises"),
            s
        );
    }

    #[test]
    fn empty_font_parts_round_trip() {
        let g = Glyph::empty(GlyphId::new(0), 0);
        let json = serde_json::to_string(&g).expect("serialises");
        assert_eq!(
            serde_json::from_str::<Glyph>(&json).expect("deserialises"),
            g
        );
        let e = Outline::new();
        let json = serde_json::to_string(&e).expect("serialises");
        assert_eq!(
            serde_json::from_str::<Outline>(&json).expect("deserialises"),
            e
        );
        let k = Kerning::new();
        let json = serde_json::to_string(&k).expect("serialises");
        assert_eq!(
            serde_json::from_str::<Kerning>(&json).expect("deserialises"),
            k
        );
        let o = Os2::default();
        let json = serde_json::to_string(&o).expect("serialises");
        assert_eq!(
            serde_json::from_str::<Os2Metrics>(&json).expect("deserialises"),
            o
        );
    }

    #[test]
    fn font_metrics_round_trip_with_and_without_vertical() {
        let without = FontMetrics::default();
        let json = serde_json::to_string(&without).expect("serialises");
        assert_eq!(
            serde_json::from_str::<FontMetrics>(&json).expect("deserialises"),
            without
        );
        let with = without.with_vertical(VerticalMetrics {
            ascender: 900,
            descender: -100,
            line_gap: 0,
            ..VerticalMetrics::default()
        });
        let json = serde_json::to_string(&with).expect("serialises");
        assert_eq!(
            serde_json::from_str::<FontMetrics>(&json).expect("deserialises"),
            with
        );
    }

    #[test]
    fn metrics_builder_and_derived_metrics_agree_after_a_round_trip() {
        let mut g = Glyph::new(GlyphId::new(0), 0, 0, {
            let mut o = Outline::new();
            let mut p = Path::starting_at(100.0, 0.0);
            p.line_to(500.0, 0.0);
            p.line_to(500.0, 400.0);
            p.close();
            o.push_contour(p);
            o
        });
        g.recompute_metrics().expect("recomputes");
        let mut b = MetricsBuilder::new();
        b.push(&g);
        let derived = b.build();
        let json = serde_json::to_string(&derived).expect("serialises");
        let back = serde_json::from_str::<FontMetrics>(&json).expect("deserialises");
        assert_eq!(back, derived);
    }
}
