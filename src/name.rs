//! The `name` table's records, owned.

use alloc::string::String;

/// Well-known name ids (the subset a writer and an editor care about).
pub mod name_id {
    /// Copyright notice.
    pub const COPYRIGHT: u16 = 0;
    /// Font family name.
    pub const FAMILY: u16 = 1;
    /// Font subfamily (style) name.
    pub const SUBFAMILY: u16 = 2;
    /// Unique font identifier.
    pub const UNIQUE_ID: u16 = 3;
    /// Full font name.
    pub const FULL_NAME: u16 = 4;
    /// Version string.
    pub const VERSION: u16 = 5;
    /// PostScript name.
    pub const POSTSCRIPT: u16 = 6;
    /// Trademark.
    pub const TRADEMARK: u16 = 7;
    /// Manufacturer.
    pub const MANUFACTURER: u16 = 8;
    /// Designer.
    pub const DESIGNER: u16 = 9;
    /// Description.
    pub const DESCRIPTION: u16 = 10;
    /// Vendor URL.
    pub const VENDOR_URL: u16 = 11;
    /// Designer URL.
    pub const DESIGNER_URL: u16 = 12;
    /// License description.
    pub const LICENSE: u16 = 13;
    /// License URL.
    pub const LICENSE_URL: u16 = 14;
    /// Preferred family.
    pub const PREFERRED_FAMILY: u16 = 16;
    /// Preferred subfamily.
    pub const PREFERRED_SUBFAMILY: u16 = 17;
}

/// Platform ids.
pub mod platform {
    /// Unicode.
    pub const UNICODE: u16 = 0;
    /// Macintosh.
    pub const MACINTOSH: u16 = 1;
    /// Windows.
    pub const WINDOWS: u16 = 3;
}

/// One owned naming record: what it names, who says so, and the string.
///
/// The model stores the *decoded* string, not raw bytes, so an editor can
/// show it without a `name`-table decode step; a writer re-encodes on the way
/// out.
///
/// ```
/// use font_model::name::{name_id, platform, preferred_record, NameRecord};
///
/// let r = NameRecord::new(name_id::FAMILY, "Demo".into());
/// assert_eq!(r.name_id, name_id::FAMILY);
/// assert_eq!(r.value, "Demo");
/// assert!(r.is_windows() && r.is_english());
/// // The preference helper picks it out of a mixed record set.
/// let records = vec![
///     NameRecord::with_locale(name_id::FAMILY, platform::MACINTOSH, 0, 0, "Mac".into()),
///     r.clone(),
/// ];
/// assert_eq!(
///     preferred_record(&records, name_id::FAMILY).map(|r| r.value.as_str()),
///     Some("Demo")
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameRecord {
    /// What the string names (see [`name_id`]; raw ids pass through).
    pub name_id: u16,
    /// Platform id (see [`platform`]).
    pub platform_id: u16,
    /// Platform-specific encoding id.
    pub encoding_id: u16,
    /// Language id.
    pub language_id: u16,
    /// The decoded string.
    pub value: String,
}

impl NameRecord {
    /// A Windows/Unicode English record — the most common case.
    #[must_use]
    pub fn new(name_id: u16, value: String) -> Self {
        NameRecord {
            name_id,
            platform_id: platform::WINDOWS,
            encoding_id: 1,
            language_id: 0x0409,
            value,
        }
    }

    /// A record with explicit platform, encoding, and language.
    #[must_use]
    pub const fn with_locale(
        name_id: u16,
        platform_id: u16,
        encoding_id: u16,
        language_id: u16,
        value: String,
    ) -> Self {
        NameRecord {
            name_id,
            platform_id,
            encoding_id,
            language_id,
            value,
        }
    }

    /// True for a Windows (platform 3) record.
    #[must_use]
    pub const fn is_windows(&self) -> bool {
        self.platform_id == platform::WINDOWS
    }

    /// True for an English (0x0409) record.
    #[must_use]
    pub const fn is_english(&self) -> bool {
        self.language_id == 0x0409
    }

    /// True for a Unicode (platform 0) record.
    #[must_use]
    pub const fn is_unicode(&self) -> bool {
        self.platform_id == platform::UNICODE
    }
}

/// Find the best record for a name id: a Windows English record first, then
/// any Windows record, then the first record of any platform.
#[must_use]
pub fn preferred_record(records: &[NameRecord], name_id: u16) -> Option<&NameRecord> {
    records
        .iter()
        .find(|r| r.name_id == name_id && r.is_windows() && r.is_english())
        .or_else(|| {
            records
                .iter()
                .find(|r| r.name_id == name_id && r.is_windows())
        })
        .or_else(|| records.iter().find(|r| r.name_id == name_id))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::{name_id, platform, preferred_record, NameRecord};
    use alloc::string::{String, ToString};
    use alloc::vec;

    // Test harness: assertions legitimately panic; the lib target stays
    // lint-clean.

    #[test]
    fn new_record_defaults_to_windows_english() {
        let r = NameRecord::new(name_id::FAMILY, String::from("Demo"));
        assert_eq!(r.platform_id, platform::WINDOWS);
        assert_eq!(r.encoding_id, 1);
        assert_eq!(r.language_id, 0x0409);
        assert!(r.is_windows());
        assert!(r.is_english());
        assert!(!r.is_unicode());
    }

    #[test]
    fn with_locale_is_explicit() {
        let r = NameRecord::with_locale(
            name_id::SUBFAMILY,
            platform::MACINTOSH,
            0,
            0,
            String::from("Italic"),
        );
        assert_eq!(r.platform_id, platform::MACINTOSH);
        assert!(!r.is_windows());
        assert!(!r.is_english());
        assert!(!r.is_unicode());
        assert_eq!(r.value, "Italic");
        let u =
            NameRecord::with_locale(name_id::FAMILY, platform::UNICODE, 3, 0, String::from("x"));
        assert!(u.is_unicode());
    }

    #[test]
    fn preferred_record_prefers_windows_english() {
        let mac = NameRecord::with_locale(
            name_id::FAMILY,
            platform::MACINTOSH,
            0,
            0,
            String::from("MacFamily"),
        );
        let win_fr = NameRecord::with_locale(
            name_id::FAMILY,
            platform::WINDOWS,
            1,
            0x040C,
            String::from("FrenchFamily"),
        );
        let win_en = NameRecord::new(name_id::FAMILY, String::from("EnglishFamily"));
        let all = vec![mac.clone(), win_fr.clone(), win_en.clone()];
        assert_eq!(
            preferred_record(&all, name_id::FAMILY).map(|r| r.value.clone()),
            Some(String::from("EnglishFamily"))
        );
        // Without an English record, any Windows record wins.
        let no_en = vec![mac.clone(), win_fr.clone()];
        assert_eq!(
            preferred_record(&no_en, name_id::FAMILY).map(|r| r.value.clone()),
            Some(String::from("FrenchFamily"))
        );
        // With no Windows record at all, the first match.
        assert_eq!(
            preferred_record(core::slice::from_ref(&mac), name_id::FAMILY).map(|r| r.value.clone()),
            Some(String::from("MacFamily"))
        );
        // An absent id is None.
        assert!(preferred_record(&all, name_id::POSTSCRIPT).is_none());
        assert!(preferred_record(&[], name_id::FAMILY).is_none());
    }

    #[test]
    fn name_id_constants_are_the_spec_values() {
        assert_eq!(name_id::COPYRIGHT, 0);
        assert_eq!(name_id::FAMILY, 1);
        assert_eq!(name_id::SUBFAMILY, 2);
        assert_eq!(name_id::UNIQUE_ID, 3);
        assert_eq!(name_id::FULL_NAME, 4);
        assert_eq!(name_id::VERSION, 5);
        assert_eq!(name_id::POSTSCRIPT, 6);
        assert_eq!(name_id::TRADEMARK, 7);
        assert_eq!(name_id::MANUFACTURER, 8);
        assert_eq!(name_id::DESIGNER, 9);
        assert_eq!(name_id::DESCRIPTION, 10);
        assert_eq!(name_id::VENDOR_URL, 11);
        assert_eq!(name_id::DESIGNER_URL, 12);
        assert_eq!(name_id::LICENSE, 13);
        assert_eq!(name_id::LICENSE_URL, 14);
        assert_eq!(name_id::PREFERRED_FAMILY, 16);
        assert_eq!(name_id::PREFERRED_SUBFAMILY, 17);
    }

    #[test]
    fn record_display_is_the_value() {
        let r = NameRecord::new(name_id::FULL_NAME, String::from("Demo Regular"));
        assert_eq!(r.value.to_string(), "Demo Regular");
        // Clone and equality.
        assert_eq!(r.clone(), r);
    }
}
