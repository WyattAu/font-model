//! Typed, exhaustive error taxonomy for the model.
//!
//! Every failure the model can produce names **the glyph and the field** it
//! came from, so an editor can highlight the offending cell and a writer can
//! emit a diagnostic without re-deriving context. The enum is deliberately
//! exhaustive (no `#[non_exhaustive]`): callers are expected to match every
//! variant, and a new variant is a semver-minor event by design.

use alloc::format;
use alloc::string::String;
use core::fmt;

use crate::GlyphId;

/// The single crate-level error for model construction, validation, and
/// subsetting.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelError {
    /// `unitsPerEm` is outside the OpenType-legal range `16..=16384`.
    InvalidUnitsPerEm {
        /// The rejected value.
        units_per_em: u16,
    },
    /// A glyph with at least one outline point has a zero advance width.
    /// An empty (space-like) glyph may legitimately have a zero advance.
    ZeroAdvance {
        /// The glyph whose `advance_width` is zero.
        glyph: GlyphId,
    },
    /// A glyph's outline contour is not closed with a trailing
    /// [`Close`](crate::Command::Close) command.
    UnclosedContour {
        /// The glyph with the unclosed contour.
        glyph: GlyphId,
        /// Index of the offending contour within the outline.
        contour: usize,
    },
    /// An outline coordinate is NaN or infinite. Named per field so the
    /// message points at the exact command slot that is corrupt.
    NonFiniteCoordinate {
        /// The glyph with the corrupt coordinate.
        glyph: GlyphId,
        /// Index of the contour within the outline.
        contour: usize,
        /// Index of the command within the contour.
        command: usize,
        /// The field name (`"x"`, `"y"`, `"control.x"`, …).
        field: &'static str,
    },
    /// `cmap` entries are not strictly increasing in codepoint order — either
    /// unsorted segments or two segments that overlap.
    NonMonotonicCmap {
        /// Index of the offending subtable within the `cmap`.
        subtable: usize,
        /// The previously seen codepoint.
        previous: u32,
        /// The codepoint that broke monotonicity.
        current: u32,
    },
    /// A glyph id referenced by the model (from `cmap` or kerning) does not
    /// exist in the font's glyph store.
    UnknownGlyph {
        /// The dangling reference.
        glyph: GlyphId,
        /// How many glyphs the font actually holds.
        glyph_count: usize,
    },
    /// A computed advance width does not fit the `u16` that `hmtx` stores.
    AdvanceOverflow {
        /// The glyph whose advance could not be represented.
        glyph: GlyphId,
        /// The unclamped advance in font units.
        advance: i32,
    },
    /// A `cmap` subtable format outside the modelled set (4 and 12) was
    /// encountered while reading a font.
    UnsupportedCmapFormat {
        /// The rejected format number.
        format: u16,
    },
    /// Reading a font failed: the byte-level parser rejected the input.
    Parse {
        /// The parser's own message, verbatim.
        message: String,
    },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::InvalidUnitsPerEm { units_per_em } => write!(
                f,
                "unitsPerEm {units_per_em} out of range (must be 16..=16384)"
            ),
            ModelError::ZeroAdvance { glyph } => {
                write!(f, "glyph {glyph} has outline but zero advance width")
            }
            ModelError::UnclosedContour { glyph, contour } => write!(
                f,
                "glyph {glyph} contour {contour} is not closed (no trailing Close)"
            ),
            ModelError::NonFiniteCoordinate {
                glyph,
                contour,
                command,
                field,
            } => write!(
                f,
                "glyph {glyph} contour {contour} command {command} field {field} is not finite"
            ),
            ModelError::NonMonotonicCmap {
                subtable,
                previous,
                current,
            } => write!(
                f,
                "cmap subtable {subtable} is not strictly increasing: \
                 U+{previous:04X} followed by U+{current:04X}"
            ),
            ModelError::UnknownGlyph { glyph, glyph_count } => write!(
                f,
                "glyph {} referenced but the font holds only {glyph_count}",
                glyph.to_u32()
            ),
            ModelError::AdvanceOverflow { glyph, advance } => write!(
                f,
                "glyph {glyph} advance {advance} exceeds the u16 hmtx range"
            ),
            ModelError::UnsupportedCmapFormat { format } => write!(
                f,
                "unsupported cmap subtable format {format} (this model carries formats 4 and 12)"
            ),
            ModelError::Parse { message } => write!(f, "font parse failed: {message}"),
        }
    }
}

impl From<font_parse::ParseError> for ModelError {
    fn from(e: font_parse::ParseError) -> Self {
        ModelError::Parse {
            message: format!("{e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::ModelError;
    use crate::GlyphId;
    use alloc::string::{String, ToString};

    // Test harness: assertions legitimately panic; the lib target stays
    // lint-clean.

    /// Every variant, rendered once — `Display` coverage for the taxonomy.
    fn all_variants() -> [ModelError; 10] {
        [
            ModelError::InvalidUnitsPerEm { units_per_em: 0 },
            ModelError::ZeroAdvance {
                glyph: GlyphId::new(1),
            },
            ModelError::UnclosedContour {
                glyph: GlyphId::new(2),
                contour: 3,
            },
            ModelError::NonFiniteCoordinate {
                glyph: GlyphId::new(3),
                contour: 1,
                command: 4,
                field: "control.y",
            },
            ModelError::NonMonotonicCmap {
                subtable: 0,
                previous: 0x41,
                current: 0x40,
            },
            ModelError::UnknownGlyph {
                glyph: GlyphId::new(9),
                glyph_count: 2,
            },
            ModelError::AdvanceOverflow {
                glyph: GlyphId::new(4),
                advance: 70_000,
            },
            ModelError::UnsupportedCmapFormat { format: 6 },
            ModelError::Parse {
                message: String::from("truncated table 'head'"),
            },
            ModelError::InvalidUnitsPerEm {
                units_per_em: 20_000,
            },
        ]
    }

    #[test]
    fn display_names_the_glyph_and_field() {
        for e in all_variants() {
            let s = e.to_string();
            assert!(!s.is_empty(), "{e:?} rendered empty");
        }
        let zero = ModelError::ZeroAdvance {
            glyph: GlyphId::new(7),
        }
        .to_string();
        assert!(
            zero.contains('7'),
            "zero-advance message omits the glyph: {zero}"
        );
        let nfc = ModelError::NonFiniteCoordinate {
            glyph: GlyphId::new(2),
            contour: 0,
            command: 1,
            field: "control.x",
        }
        .to_string();
        assert!(
            nfc.contains("control.x"),
            "coordinate message omits the field"
        );
        let mono = ModelError::NonMonotonicCmap {
            subtable: 1,
            previous: 0x100,
            current: 0x0F,
        }
        .to_string();
        assert!(mono.contains("U+0100"), "cmap message omits the codepoints");
    }

    #[test]
    fn parse_error_converts() {
        let e: ModelError = font_parse::ParseError::InvalidSfnt.into();
        assert!(matches!(e, ModelError::Parse { .. }));
        assert!(e.to_string().starts_with("font parse failed:"));
    }
}
