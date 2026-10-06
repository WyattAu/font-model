//! Model inventory — a printable summary of a font model.
//!
//! The example builds a small font in memory (no fixture file needed, so it
//! runs anywhere with `cargo run --example inventory`), then reports what the
//! model holds: glyph count, codepoint coverage, per-glyph metrics with
//! validation verdicts, the `cmap` subtable split, and the opaque tables
//! carried through.
//!
//! ```sh
//! cargo run --example inventory
//! ```
//!
//! Pass a path to read a real font through the `font-parse` bridge instead:
//!
//! ```sh
//! cargo run --example inventory -- tests/fixtures/minimal.ttf
//! ```

// Test harness: assertions legitimately panic; the lib target holds the
// deny-level lints.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use std::process::ExitCode;

use font_model::{
    build_stub_font, from_entries, insert_entry, Cmap, CmapSubtable, Font, Glyph, GlyphId,
    MetricsBuilder, ModelError, NameRecord, Outline, Path,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("inventory: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A closed box outline — three of them make a distinctive glyph.
fn box_outline(x0: f32, y0: f32, x1: f32, y1: f32) -> Outline {
    let mut o = Outline::new();
    let mut p = Path::starting_at(x0, y0);
    p.line_to(x1, y0);
    p.line_to(x1, y1);
    p.line_to(x0, y1);
    p.close();
    o.push_contour(p);
    o
}

/// The in-memory demo font: `.notdef`, `space`, `A`, and an astral glyph.
fn demo_font() -> Result<Font, ModelError> {
    let mut sub = CmapSubtable::empty_format4();
    sub = insert_entry(&sub, u32::from(' '), GlyphId::new(1));
    sub = insert_entry(&sub, u32::from('A'), GlyphId::new(2));
    let mut cmap = Cmap::new();
    cmap.subtables_mut().push(sub);
    cmap.subtables_mut()
        .push(from_entries(&[(0x1F600, GlyphId::new(3))], 12));

    let mut metrics = MetricsBuilder::new()
        .with_ascender(800)
        .with_descender(-200)
        .with_line_gap(0);
    let glyphs = vec![
        Glyph::new(
            GlyphId::NOTDEF,
            600,
            50,
            box_outline(50.0, 0.0, 550.0, 700.0),
        ),
        Glyph::empty(GlyphId::new(1), 300),
        Glyph::new(
            GlyphId::new(2),
            600,
            60,
            box_outline(60.0, 0.0, 540.0, 700.0),
        ),
        Glyph::new(
            GlyphId::new(3),
            700,
            80,
            box_outline(80.0, 0.0, 620.0, 700.0),
        ),
    ];
    for g in &glyphs {
        metrics.push(g);
    }

    Font::builder()
        .units_per_em(1000)
        .metrics(metrics.build())
        .glyphs(glyphs)
        .cmap(cmap)
        .name(NameRecord::new(
            font_model::name_id::FAMILY,
            "Inventory".into(),
        ))
        .name(NameRecord::new(
            font_model::name_id::SUBFAMILY,
            "Regular".into(),
        ))
        .table(*b"GSUB", vec![0x00, 0x01, 0x00, 0x00])
        .build()
}

fn run() -> Result<(), ModelError> {
    // A path argument reads a real font through the bridge; otherwise the
    // in-memory demo font is used.
    let font = match std::env::args().nth(1) {
        Some(path) => {
            let bytes = std::fs::read(&path).unwrap_or_else(|e| {
                eprintln!("inventory: could not read {path}: {e}");
                std::process::exit(1);
            });
            font_model::from_sfnt(&bytes)?
        }
        None => {
            println!("(no font path given; using the in-memory demo font)\n");
            demo_font()?
        }
    };

    let cps = font.codepoints();
    println!("font inventory");
    println!("==============");
    println!("units per em    : {}", font.units_per_em());
    println!("glyph count     : {}", font.glyph_count());
    println!("codepoint count : {}", cps.len());
    println!("ascender        : {}", font.metrics().ascender);
    println!("descender       : {}", font.metrics().descender);
    println!("line gap        : {}", font.metrics().line_gap);
    println!("line height     : {}", font.metrics().line_height());
    println!("advance max     : {}", font.metrics().advance_width_max);
    println!("min lsb         : {}", font.metrics().min_left_side_bearing);
    println!("x max extent    : {}", font.metrics().x_max_extent);
    if let Some(v) = font.metrics().vertical {
        println!(
            "vertical line   : {} (ascent {} / descent {} / gap {})",
            v.line_height(),
            v.ascender,
            v.descender,
            v.line_gap
        );
    }
    if let Some(os2) = font.os2() {
        println!(
            "os/2            : version {}, weight {}, width {}, typo line {}",
            os2.version,
            os2.weight_class,
            os2.width_class,
            os2.typo_line_height()
        );
    }

    println!("\nnames");
    for r in font.names() {
        println!("  id {:>3}: {}", r.name_id, r.value);
    }

    println!("\ncmap subtables");
    for (i, sub) in font.cmap().subtables().iter().enumerate() {
        let mut mapped = 0u32;
        for cp in cps.iter() {
            if sub.lookup(*cp).is_some() {
                mapped += 1;
            }
        }
        println!(
            "  [{i}] format {format:>2}, {segments} segment(s), {mapped} mapped codepoint(s), \
             {bytes} byte(s) encoded",
            format = sub.format(),
            segments = sub.segments().len(),
            bytes = sub.encode().len()
        );
        for (cp, gid) in sub.entries() {
            let ch = char::from_u32(cp);
            match ch {
                Some(c) if !c.is_control() => {
                    println!("        U+{cp:04X} '{c}' -> {gid}")
                }
                _ => println!("        U+{cp:04X} -> {gid}"),
            }
        }
    }

    println!("\nglyphs");
    println!(
        "  {:>4}  {:>7}  {:>6}  {:>6}  {:>10}  {:>12}  verdict",
        "gid", "advance", "lsb", "ink", "bbox", "area"
    );
    for g in font.glyphs() {
        let bbox = g.bbox();
        let ink = bbox.map_or(0.0, |b| b.width());
        let ink_text = format!("{ink:.0}");
        let bbox_text = bbox.map_or_else(
            || "-".to_string(),
            |b| {
                format!(
                    "{:.0},{:.0}..{:.0},{:.0}",
                    b.min_x(),
                    b.min_y(),
                    b.max_x(),
                    b.max_y()
                )
            },
        );
        let verdict = match g.validate() {
            Ok(()) => "ok".to_string(),
            Err(e) => {
                // Keep the column one field wide; the message is long.
                e.to_string().chars().take(40).collect::<String>()
            }
        };
        println!(
            "  {:>4}  {:>7}  {:>6}  {:>6}  {:>10}  {:>12}  {}",
            g.id(),
            g.advance_width(),
            g.left_side_bearing(),
            ink_text,
            bbox_text,
            format!("{:.0}", g.outline().area()),
            verdict
        );
    }

    let pairs = font.kerning().iter().count();
    if pairs > 0 {
        println!("\nkerning ({pairs} pair(s))");
        for p in font.kerning().iter() {
            println!("  {} {} -> {}", p.left, p.right, p.value);
        }
    }

    let tables = font.tables();
    if !tables.is_empty() {
        println!("\nopaque tables (carried through unchanged)");
        for (tag, bytes) in tables {
            let printable: String = tag.iter().map(|&b| b as char).collect();
            println!("  '{printable}' {} byte(s)", bytes.len());
        }
    }

    println!("\nvalidation");
    match font.validate() {
        Ok(()) => println!("  the model validates"),
        Err(e) => println!("  INVALID: {e}"),
    }

    // A subset round trip, so the report shows the write path working.
    let mut keep = CodePointSetAlias::new();
    for cp in cps.iter().take(1) {
        keep.insert(*cp);
    }
    match font.subset(&keep) {
        Ok(sub) => println!(
            "  subset to 1 codepoint: {} glyph(s), valid: {}",
            sub.glyph_count(),
            sub.validate().is_ok()
        ),
        Err(e) => println!("  subset failed: {e}"),
    }

    let stub = build_stub_font(&[0x41])?;
    println!(
        "  stub font sanity: {} glyph(s), valid: {}",
        stub.glyph_count(),
        stub.validate().is_ok()
    );
    Ok(())
}

/// A local alias so the example reads clearly without a long import.
type CodePointSetAlias = font_model::CodePointSet;
