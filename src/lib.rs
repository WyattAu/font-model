//! Typed, owned, editable font model — outlines, character maps, metrics,
//! validation, and subsetting.
//!
//! `font-model` is the estate's **L1 font substrate**: the resolved, owned,
//! mutable form of a typeface that sits directly on [`font-parse`]'s L0
//! byte-level views. A parser gives you borrowed table views over `&[u8]`; an
//! editor needs owned glyphs it can reshape, a writer needs metrics it can
//! serialise, and a subsetter needs a codepoint set it can intersect against.
//! That is what this crate is.
//!
//! ```
//! use font_model::{Font, Glyph, GlyphId, Outline, Path};
//!
//! // Build a font from parts.
//! let mut outline = Outline::new();
//! let mut p = Path::starting_at(80.0, 0.0);
//! p.line_to(520.0, 0.0);
//! p.line_to(520.0, 700.0);
//! p.line_to(80.0, 700.0);
//! p.close();
//! outline.push_contour(p);
//!
//! let mut cmap = font_model::Cmap::format4();
//! cmap.subtables_mut()[0] =
//!     font_model::cmap::insert_entry(&cmap.subtables()[0], 'A' as u32, GlyphId::new(1));
//!
//! let font = Font::builder()
//!     .units_per_em(1000)
//!     .glyph(Glyph::new(GlyphId::NOTDEF, 600, 80, outline.clone()))
//!     .glyph(Glyph::new(GlyphId::new(1), 600, 80, outline))
//!     .cmap(cmap)
//!     .build()?;
//!
//! assert_eq!(font.glyph_for('A' as u32), Some(GlyphId::new(1)));
//!
//! // Subset to 'A' plus .notdef.
//! let mut keep = font_model::CodePointSet::new();
//! keep.insert('A' as u32);
//! let sub = font.subset(&keep)?;
//! assert_eq!(sub.glyph_count(), 2);
//! assert!(sub.validate().is_ok());
//! # Ok::<(), font_model::ModelError>(())
//! ```
//!
//! # Model, not parser
//!
//! [`font-parse`] reads bytes in place: zero-copy, allocation-free, total.
//! This crate owns its data. [`Font::from_sfnt`] is the bridge — it runs the
//! L0 parser and resolves its borrowed views into this model — and it is the
//! only function here that takes foreign bytes.
//!
//! # Invariants
//!
//! [`Font::validate`] is the single gate, and every error variant names the
//! offending glyph and field:
//!
//! | Invariant | Error |
//! |---|---|
//! | `units_per_em` in `16..=16384` | [`ModelError::InvalidUnitsPerEm`] |
//! | ink implies non-zero advance | [`ModelError::ZeroAdvance`] |
//! | contours closed | [`ModelError::UnclosedContour`] |
//! | coordinates finite | [`ModelError::NonFiniteCoordinate`] |
//! | `cmap` strictly increasing | [`ModelError::NonMonotonicCmap`] |
//! | every glyph reference resolves | [`ModelError::UnknownGlyph`] |
//!
//! # Layer
//!
//! L1 — substrate. Its only estate-internal dependency is `font-parse` (L0);
//! `thiserror` carries the error taxonomy, `serde` is optional and off by
//! default so a model can round-trip through a config format without forcing
//! the dependency on everyone.
//!
//! [`font-parse`]: https://docs.rs/font-parse

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod cmap;
mod error;
pub mod font;
pub mod glyph;
pub mod metrics;
pub mod name;
mod num;
pub mod os2;
pub mod outline;
pub mod parse;

#[cfg(feature = "serde")]
mod serde_impls;

pub use cmap::{from_entries, insert_entry, Cmap, CmapSubtable, CodePointSet, Segment};
pub use error::ModelError;
pub use font::{
    build_stub_font, name_value, Font, FontBuilder, KernPair, Kerning, TableTag, MAX_UNITS_PER_EM,
    MIN_UNITS_PER_EM, TAG_CMAP, TAG_GPOS, TAG_GSUB, TAG_KERN,
};
pub use glyph::{Glyph, GlyphId};
pub use metrics::{FontMetrics, Metrics, MetricsBuilder, VerticalMetrics};
pub use name::{name_id, platform, preferred_record, NameRecord};
pub use os2::{fs_type, weight_class, width_class, Os2Metrics};
pub use outline::{Bbox, Command, FlatSegment, Outline, Path, Subpath, Winding};
pub use parse::{from_font_ref, from_sfnt, hmtx_rows, to_outline};
