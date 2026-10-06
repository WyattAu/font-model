//! The `OS/2` table's metrics, owned.

/// Embedding permission bits (`fsType`).
pub mod fs_type {
    /// No embedding restrictions.
    pub const INSTALLABLE: u16 = 0;
    /// Restricted-license embedding.
    pub const RESTRICTED: u16 = 0x0002;
    /// Preview-and-print embedding.
    pub const PREVIEW_AND_PRINT: u16 = 0x0004;
    /// Editable embedding.
    pub const EDITABLE: u16 = 0x0008;
}

/// Weight classes (1–1000; the conventional values are below).
pub mod weight_class {
    /// Thin.
    pub const THIN: u16 = 100;
    /// Light.
    pub const LIGHT: u16 = 300;
    /// Regular.
    pub const REGULAR: u16 = 400;
    /// Bold.
    pub const BOLD: u16 = 700;
    /// Black.
    pub const BLACK: u16 = 900;
}

/// Width classes (1 ultra-condensed … 9 ultra-expanded).
pub mod width_class {
    /// Ultra-condensed.
    pub const ULTRA_CONDENSED: u16 = 1;
    /// Medium / normal.
    pub const MEDIUM: u16 = 5;
    /// Ultra-expanded.
    pub const ULTRA_EXPANDED: u16 = 9;
}

/// The `OS/2` table's owned fields.
///
/// Every field the model round-trips through; version-dependent fields keep
/// the OpenType defaults when absent, so a version-0 table and a version-5
/// table both land in one struct without a second representation.
///
/// ```
/// use font_model::os2::{width_class, Os2Metrics, weight_class};
///
/// let o = Os2Metrics {
///     weight_class: weight_class::BOLD,
///     ..Os2Metrics::default()
/// };
/// assert_eq!(o.weight_class, 700);
/// assert_eq!(o.width_class, width_class::MEDIUM);
/// assert!(!o.is_bold_bit());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Os2Metrics {
    /// Table version, 0–5.
    pub version: u16,
    /// Average character width.
    pub avg_char_width: i16,
    /// `usWeightClass`.
    pub weight_class: u16,
    /// `usWidthClass`.
    pub width_class: u16,
    /// Embedding permissions (`fsType`).
    pub fs_type: u16,
    /// PANOSE classification, 10 bytes.
    pub panose: [u8; 10],
    /// Unicode codepoint ranges, four `u32` bitfields.
    pub unicode_ranges: [u32; 4],
    /// The four-character vendor id, as bytes.
    pub vendor_id: [u8; 4],
    /// `fsSelection` flags.
    pub fs_selection: u16,
    /// First and last character index of the mac-only usage subset.
    pub first_char_index: u16,
    /// See [`first_char_index`].
    pub last_char_index: u16,
    /// Typographic ascender (`sTypoAscender`).
    pub typo_ascender: i16,
    /// Typographic descender (`sTypoDescender`).
    pub typo_descender: i16,
    /// Typographic line gap (`sTypoLineGap`).
    pub typo_line_gap: i16,
    /// Windows ascent (`usWinAscent`).
    pub win_ascent: u16,
    /// Windows descent (`usWinDescent`).
    pub win_descent: u16,
    /// Code-page ranges, two `u32` bitfields.
    pub code_page_ranges: [u32; 2],
    /// `sxHeight`, version 2+.
    pub x_height: i16,
    /// `sCapHeight`, version 2+.
    pub cap_height: i16,
    /// Default character, version 5+.
    pub default_char: u16,
    /// Breaking character, version 5+.
    pub break_char: u16,
    /// Maximum context, version 5+.
    pub max_context: u16,
}

impl Default for Os2Metrics {
    fn default() -> Self {
        Os2Metrics {
            version: 4,
            avg_char_width: 0,
            weight_class: weight_class::REGULAR,
            width_class: width_class::MEDIUM,
            fs_type: fs_type::INSTALLABLE,
            panose: [0; 10],
            unicode_ranges: [0; 4],
            vendor_id: *b"NONE",
            fs_selection: 0x0040, // REGULAR
            first_char_index: 0x0020,
            last_char_index: 0xFFFF,
            typo_ascender: 800,
            typo_descender: -200,
            typo_line_gap: 0,
            win_ascent: 800,
            win_descent: 200,
            code_page_ranges: [0, 0],
            x_height: 500,
            cap_height: 700,
            default_char: 0,
            break_char: 32,
            max_context: 1,
        }
    }
}

impl Os2Metrics {
    /// The version-5 defaults.
    #[must_use]
    pub fn new() -> Self {
        Os2Metrics::default()
    }

    /// True when the `BOLD` bit (5) is set in `fsSelection`.
    #[must_use]
    pub const fn is_bold_bit(&self) -> bool {
        self.fs_selection & 0x0020 != 0
    }

    /// True when the `ITALIC` bit (0) is set in `fsSelection`.
    #[must_use]
    pub const fn is_italic_bit(&self) -> bool {
        self.fs_selection & 0x0001 != 0
    }

    /// The Windows line box: `win_ascent + win_descent`.
    #[must_use]
    pub fn win_line_height(&self) -> u32 {
        u32::from(self.win_ascent) + u32::from(self.win_descent)
    }

    /// The typographic line box: ascender − descender + line gap.
    #[must_use]
    pub fn typo_line_height(&self) -> i32 {
        i32::from(self.typo_ascender) - i32::from(self.typo_descender)
            + i32::from(self.typo_line_gap)
    }

    /// True when the codepoint range bitfields declare any coverage.
    #[must_use]
    pub const fn declares_unicode_ranges(&self) -> bool {
        self.unicode_ranges[0] != 0
            || self.unicode_ranges[1] != 0
            || self.unicode_ranges[2] != 0
            || self.unicode_ranges[3] != 0
    }

    /// True when `fsType` forbids embedding entirely.
    #[must_use]
    pub const fn is_embedding_forbidden(&self) -> bool {
        self.fs_type & 0x0002 != 0 && !self.is_installable()
    }

    /// True when the font marks itself installable.
    #[must_use]
    pub const fn is_installable(&self) -> bool {
        self.fs_type == fs_type::INSTALLABLE
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::{fs_type, weight_class, width_class, Os2Metrics};

    // Test harness: assertions legitimately panic; the lib target stays
    // lint-clean.

    #[test]
    fn defaults_are_a_regular_latin_face() {
        let o = Os2Metrics::default();
        assert_eq!(o.version, 4);
        assert_eq!(o.weight_class, weight_class::REGULAR);
        assert_eq!(o.width_class, width_class::MEDIUM);
        assert_eq!(o.fs_type, fs_type::INSTALLABLE);
        assert_eq!(o.typo_ascender, 800);
        assert_eq!(o.typo_descender, -200);
        assert_eq!(o.win_ascent, 800);
        assert_eq!(o.win_descent, 200);
        assert_eq!(o.vendor_id, *b"NONE");
        assert_eq!(o.panose, [0; 10]);
        assert_eq!(o.code_page_ranges, [0, 0]);
        assert_eq!(o.x_height, 500);
        assert_eq!(o.cap_height, 700);
        assert_eq!(o.break_char, 32);
        assert_eq!(Os2Metrics::new(), Os2Metrics::default());
    }

    #[test]
    fn line_heights() {
        let mut o = Os2Metrics::default();
        assert_eq!(o.win_line_height(), 1000);
        assert_eq!(o.typo_line_height(), 1000);
        o.typo_line_gap = 100;
        assert_eq!(o.typo_line_height(), 1100);
        o.win_ascent = 900;
        o.win_descent = 300;
        assert_eq!(o.win_line_height(), 1200);
    }

    #[test]
    fn fs_selection_bits() {
        let mut o = Os2Metrics::default();
        assert!(!o.is_bold_bit());
        assert!(!o.is_italic_bit());
        o.fs_selection |= 0x0021;
        assert!(o.is_bold_bit());
        assert!(o.is_italic_bit());
    }

    #[test]
    fn unicode_range_bits() {
        let mut o = Os2Metrics::default();
        assert!(!o.declares_unicode_ranges());
        o.unicode_ranges[2] = 1;
        assert!(o.declares_unicode_ranges());
        o.unicode_ranges[2] = 0;
        o.unicode_ranges[3] = 1 << 5;
        assert!(o.declares_unicode_ranges());
    }

    #[test]
    fn embedding_permissions() {
        let mut o = Os2Metrics::default();
        assert!(o.is_installable());
        assert!(!o.is_embedding_forbidden());
        o.fs_type = fs_type::RESTRICTED;
        assert!(!o.is_installable());
        assert!(o.is_embedding_forbidden());
        o.fs_type = fs_type::EDITABLE;
        assert!(!o.is_embedding_forbidden(), "editable is not forbidden");
        o.fs_type = fs_type::PREVIEW_AND_PRINT;
        assert!(!o.is_embedding_forbidden());
    }

    #[test]
    fn class_constants() {
        assert_eq!(weight_class::THIN, 100);
        assert_eq!(weight_class::LIGHT, 300);
        assert_eq!(weight_class::REGULAR, 400);
        assert_eq!(weight_class::BOLD, 700);
        assert_eq!(weight_class::BLACK, 900);
        assert_eq!(width_class::ULTRA_CONDENSED, 1);
        assert_eq!(width_class::MEDIUM, 5);
        assert_eq!(width_class::ULTRA_EXPANDED, 9);
        assert_eq!(fs_type::INSTALLABLE, 0);
        assert_eq!(fs_type::RESTRICTED, 2);
        assert_eq!(fs_type::PREVIEW_AND_PRINT, 4);
        assert_eq!(fs_type::EDITABLE, 8);
    }

    #[test]
    fn struct_update_and_clone() {
        let o = Os2Metrics {
            weight_class: weight_class::BLACK,
            unicode_ranges: [1, 2, 3, 4],
            ..Os2Metrics::default()
        };
        assert_eq!(o.weight_class, 900);
        assert_eq!(o.unicode_ranges, [1, 2, 3, 4]);
        assert_eq!(o.clone(), o);
        assert!(o != Os2Metrics::default());
    }
}
