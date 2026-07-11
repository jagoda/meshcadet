// SPDX-License-Identifier: GPL-3.0-only
//! Host-side glyph-coverage verification harness.
//!
//! Hard Constraint: "every icon renders, never
//! blank". The firmware crate's `#[cfg(test)]` blocks are type-checked but
//! never EXECUTED on host (`firmware/` cross-compiles for the Xtensa/esp-idf
//! target — see `firmware/Cargo.toml`'s doc comment), so a glyph-coverage
//! check cannot live there as a firmware `#[test]`. This crate is the
//! host-runnable equivalent, in the same spirit as `gen_emoji_font.c` itself:
//! plain text scanning, no esp toolchain required, runnable by `cargo test`
//! at the repo root and by every downstream change to the UI screens.
//!
//! # What this catches
//!
//! `firmware/gen_emoji_font.c` rasterises a curated set of codepoints
//! (`BMP_SYMBOLS` at every `PIXEL_SIZES` entry; `EMOJI_CPS`/`UI_EXTRA_CPS`
//! only at the `EMOJI_SIZES` subset) and already fails ITS OWN build if any
//! registered codepoint has no glyph in EITHER font face
//! (`g_missing_glyph_count`). What it can NOT catch — because it has no
//! visibility into `firmware/src/ui/screens/*.rs` — is:
//!
//! 1. **Level 2 — unregistered codepoint.** A codepoint used in a screen's
//!    Slint literal that was never added to `BMP_SYMBOLS`/`EMOJI_CPS`/
//!    `UI_EXTRA_CPS` at all. This is the exact bug class the SYNC INVARIANT
//!    comments in `gen_emoji_font.c` document as having already bitten this
//!    codebase repeatedly (admin_menu.rs's 🔔/🔊/💤, gps_status.rs's —/…,
//!    and — caught by this harness's very first run —
//!    admin_menu.rs's 🔋).
//! 2. **Level 3 — wrong size.** An EMOJI-class codepoint (one only rasterised
//!    at the curated `EMOJI_SIZES` subset, unlike BMP symbols + ASCII which
//!    are rasterised at every `PIXEL_SIZES` entry) shown at a font-size
//!    outside `EMOJI_SIZES`. Also caught by this harness's first run (see
//!    `KNOWN_GAPS` below for the one pre-existing, deliberately-deferred
//!    instance of this).
//!
//! # Scope: Slint literals only, not dynamic runtime content
//!
//! This scans STATIC string literals written directly in each screen's
//! `slint::slint!{}` macro block — the UI-chrome glyphs (button labels,
//! icons, titles). It deliberately does NOT trace codepoints that only exist
//! at runtime inside a Rust-formatted `String` bound to a Slint property
//! (e.g. a contact's nickname, an incoming message body, or the 📍 telemetry
//! line assembled in a formatter elsewhere) — that content is unbounded
//! Unicode and is not what "freeze the icon inventory" can promise to cover;
//! `protocol::emoji::EMOJI_TABLE`'s own SYNC INVARIANT with `EMOJI_CPS`
//! covers the one bounded, enumerable case of dynamic content (the 40-entry
//! curated picker set).
//!
//! # Design: simple, auditable, fails loud on ambiguity
//!
//! Per this design's own risk note: "keep it a simple, auditable scan
//! and treat any parse gap as a NO-GO". Concretely:
//! - A tiny hand-rolled tokenizer separates Slint/Rust code from comments
//!   (`//`, `/* */`) and string-literal bodies, so a glyph mentioned only in
//!   a doc-comment diagram (e.g. the box-drawing chars in `compose.rs`'s
//!   ASCII-art layout sketch) is never mistaken for a rendered glyph.
//! - Regexes over the comment/string-blanked text locate `component Foo { }`
//!   definitions, their `in property <string>`/`in property <length>`
//!   declarations (with defaults), and every `Text { text: ...; font-size:
//!   ...; }` occurrence — resolving the handful of reusable row components
//!   (`ToggleRow`, `InfoRow`, `NavRow`, `StatusRow`, `HeaderIconButton`) back
//!   to the font-size their `label`/`icon`/`value` property actually renders
//!   at, wherever they're instantiated with a literal string.
//! - Anything the resolver can't pin down for an EMOJI-class codepoint
//!   (identifier-driven font-size with no discoverable default/override) is
//!   reported as an **unresolved** finding — a hard failure, not a silent
//!   skip — per the "parse gap = NO-GO" doctrine. BMP-class codepoints don't
//!   need size resolution at all (rasterised at every `PIXEL_SIZES` entry
//!   once registered), so an unresolved size there is not reported.

use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Provisioning-codec golden-vector generator — see `golden`'s module doc.
pub mod golden;

/// One known, already-documented, deliberately-deferred gap — see
/// `firmware/gen_emoji_font.c`'s `EMOJI_SIZES` doc comment ("KNOWN, DEFERRED
/// GAP") for the full rationale: `unprovisioned.rs` shows 📻 at 28px (pre-
/// existing, predates this harness), and the design plan explicitly scopes
/// the fix to a later theming pass rather than
/// Phase 1 either bloating `EMOJI_SIZES` for every curated emoji or reaching
/// into a screen file this phase doesn't own. Kept as an explicit, narrow,
/// commented allowlist — NOT a silent skip — so it cannot mask any other gap.
const KNOWN_GAPS: &[(&str, u32, u32)] = &[("unprovisioned.rs", 0x1F4FB, 28)];

/// One coverage violation found by [`check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// A codepoint appears in a screen's Slint literal but is not present in
    /// ANY of `gen_emoji_font.c`'s registered tables (ASCII range excepted).
    Unregistered {
        file: String,
        codepoint: u32,
        ch: char,
    },
    /// An EMOJI-class codepoint (registered, but only rasterised at the
    /// curated `EMOJI_SIZES` subset) is used at a font-size outside that
    /// subset.
    WrongSize {
        file: String,
        codepoint: u32,
        ch: char,
        size_px: u32,
    },
    /// An EMOJI-class codepoint's rendering font-size could not be
    /// statically determined (see module doc's "fails loud on ambiguity").
    UnresolvedSize {
        file: String,
        codepoint: u32,
        ch: char,
        detail: String,
    },
    /// A `font-size: Npx` literal (regardless of what text it applies to) is
    /// not a member of `PIXEL_SIZES` — the Slint software renderer snaps it
    /// to the nearest registered size and rescales glyph metrics, garbling
    /// text (the type-scale reconciliation half of this harness's
    /// acceptance bar, independent of glyph coverage).
    SizeNotInScale { file: String, size_px: u32 },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Unregistered {
                file,
                codepoint,
                ch,
            } => write!(
                f,
                "{file}: U+{codepoint:04X} ({ch:?}) is used in a Slint literal but is not \
                 registered in ANY of gen_emoji_font.c's BMP_SYMBOLS/EMOJI_CPS/UI_EXTRA_CPS \
                 tables — it renders BLANK on real hardware."
            ),
            Violation::WrongSize {
                file,
                codepoint,
                ch,
                size_px,
            } => write!(
                f,
                "{file}: U+{codepoint:04X} ({ch:?}) is shown at {size_px}px, which is not in \
                 gen_emoji_font.c's EMOJI_SIZES — it rasterises as an empty (blank) glyph at \
                 that size."
            ),
            Violation::UnresolvedSize {
                file,
                codepoint,
                ch,
                detail,
            } => write!(
                f,
                "{file}: could not statically determine the font-size U+{codepoint:04X} \
                 ({ch:?}) renders at ({detail}) — treating as a NO-GO per the harness's \
                 \"parse gap = fail loud\" design; extend the scanner or simplify the markup."
            ),
            Violation::SizeNotInScale { file, size_px } => write!(
                f,
                "{file}: font-size: {size_px}px is not a member of gen_emoji_font.c's \
                 PIXEL_SIZES — the renderer will snap it to the nearest registered size and \
                 rescale glyph metrics (garbled text)."
            ),
        }
    }
}

/// The frozen icon inventory, parsed straight out of `gen_emoji_font.c`.
#[derive(Debug, Default)]
struct FontTables {
    bmp_symbols: HashSet<u32>,
    emoji_cps: HashSet<u32>,
    ui_extra_cps: HashSet<u32>,
    pixel_sizes: HashSet<u32>,
    emoji_sizes: HashSet<u32>,
}

impl FontTables {
    fn is_registered(&self, cp: u32) -> bool {
        (0x20..=0x7E).contains(&cp)
            || self.bmp_symbols.contains(&cp)
            || self.emoji_cps.contains(&cp)
            || self.ui_extra_cps.contains(&cp)
    }

    /// EMOJI-class = drawn from the emoji face, gated by `EMOJI_SIZES` — as
    /// opposed to ASCII/BMP-symbol chars, which are rasterised at every
    /// `PIXEL_SIZES` entry once registered (see `gen_emoji_font.c`'s
    /// `EMOJI_SIZES` doc comment).
    fn is_emoji_class(&self, cp: u32) -> bool {
        self.emoji_cps.contains(&cp) || self.ui_extra_cps.contains(&cp)
    }
}

fn extract_array(src: &str, array_name: &str) -> String {
    let needle = format!("{array_name}[] = {{");
    let start = match src.find(&needle) {
        Some(i) => i + needle.len(),
        None => return String::new(),
    };
    let end = src[start..]
        .find("};")
        .map(|i| start + i)
        .unwrap_or(src.len());
    src[start..end].to_string()
}

fn extract_hex_numbers(body: &str) -> Vec<u32> {
    let re = Regex::new(r"0[xX][0-9A-Fa-f]+").unwrap();
    re.find_iter(body)
        .filter_map(|m| u32::from_str_radix(&m.as_str()[2..], 16).ok())
        .collect()
}

fn extract_dec_numbers(body: &str) -> Vec<u32> {
    let re = Regex::new(r"\d+").unwrap();
    re.find_iter(body)
        .filter_map(|m| m.as_str().parse().ok())
        .collect()
}

fn parse_font_tables(gen_emoji_font_c: &Path) -> FontTables {
    let src = fs::read_to_string(gen_emoji_font_c)
        .unwrap_or_else(|e| panic!("reading {}: {e}", gen_emoji_font_c.display()));
    FontTables {
        bmp_symbols: extract_hex_numbers(&extract_array(&src, "BMP_SYMBOLS"))
            .into_iter()
            .collect(),
        emoji_cps: extract_hex_numbers(&extract_array(&src, "EMOJI_CPS"))
            .into_iter()
            .collect(),
        ui_extra_cps: extract_hex_numbers(&extract_array(&src, "UI_EXTRA_CPS"))
            .into_iter()
            .collect(),
        pixel_sizes: extract_dec_numbers(&extract_array(&src, "PIXEL_SIZES"))
            .into_iter()
            .collect(),
        emoji_sizes: extract_dec_numbers(&extract_array(&src, "EMOJI_SIZES"))
            .into_iter()
            .collect(),
    }
}

// ── Tokenizer: separate code from comments/string bodies ───────────────────

struct Literal {
    /// Char index of the opening `"`.
    quote_start: usize,
    decoded: String,
}

struct Tokenized {
    /// Same char-length as the source; comment bodies and string-literal
    /// bodies are blanked to spaces (quotes/braces/identifiers in real code
    /// are untouched) so brace-matching and regex scans never see comment
    /// or string CONTENT — only real markup structure. Guaranteed pure ASCII
    /// (every non-ASCII source char lives inside a string or comment, both
    /// blanked), so byte offsets == char offsets throughout.
    masked: String,
    literals: Vec<Literal>,
}

fn decode_rust_escapes(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('u') => {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    let mut hex = String::new();
                    for h in chars.by_ref() {
                        if h == '}' {
                            break;
                        }
                        hex.push(h);
                    }
                    if let Ok(cp) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(cp) {
                            out.push(ch);
                        }
                    }
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn tokenize(src: &str) -> Tokenized {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut masked: Vec<char> = chars.clone();
    let mut literals = Vec::new();
    let mut i = 0usize;
    while i < n {
        let c = chars[i];
        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            while i < n && chars[i] != '\n' {
                masked[i] = ' ';
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            masked[i] = ' ';
            masked[i + 1] = ' ';
            i += 2;
            while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                masked[i] = ' ';
                i += 1;
            }
            if i + 1 < n {
                masked[i] = ' ';
                masked[i + 1] = ' ';
                i += 2;
            } else {
                i = n;
            }
            continue;
        }
        if c == '"' {
            let quote_start = i;
            i += 1;
            let mut raw = String::new();
            while i < n && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < n {
                    raw.push(chars[i]);
                    raw.push(chars[i + 1]);
                    masked[i] = ' ';
                    masked[i + 1] = ' ';
                    i += 2;
                    continue;
                }
                raw.push(chars[i]);
                masked[i] = ' ';
                i += 1;
            }
            if i < n {
                i += 1; // consume closing quote
            }
            literals.push(Literal {
                quote_start,
                decoded: decode_rust_escapes(&raw),
            });
            continue;
        }
        i += 1;
    }
    Tokenized {
        masked: masked.into_iter().collect(),
        literals,
    }
}

fn brace_spans(masked: &str) -> Vec<(usize, usize)> {
    let mut stack = Vec::new();
    let mut spans = Vec::new();
    for (idx, c) in masked.chars().enumerate() {
        match c {
            '{' => stack.push(idx),
            '}' => {
                if let Some(open) = stack.pop() {
                    spans.push((open, idx));
                }
            }
            _ => {}
        }
    }
    spans
}

fn innermost_span(spans: &[(usize, usize)], pos: usize) -> Option<(usize, usize)> {
    spans
        .iter()
        .filter(|(o, c)| *o < pos && pos < *c)
        .min_by_key(|(o, c)| c - o)
        .copied()
}

fn slice_chars(s: &str, start: usize, end: usize) -> String {
    s.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// How a reusable component's text property resolves to a font-size.
#[derive(Debug, Clone)]
enum SizeSource {
    Literal(u32),
    /// Driven by another property (e.g. `font-size: icon_size;`); resolve at
    /// the instantiation site (override) or fall back to the named
    /// property's own declared default within the component.
    ViaProperty(String),
}

/// A custom component definition's resolved (text-property -> font-size) map.
#[derive(Debug, Default)]
struct ComponentInfo {
    span: (usize, usize),
    /// text-property-name -> how its Text's font-size resolves
    text_props: HashMap<String, SizeSource>,
    /// length-property-name -> literal px default declared in this component
    length_defaults: HashMap<String, u32>,
}

/// One resolved (codepoint, size) usage site found in a screen file.
struct Usage {
    ch: char,
    size_px: Option<u32>,
    unresolved_detail: Option<String>,
}

fn find_component_defs(masked: &str, spans: &[(usize, usize)]) -> HashMap<String, ComponentInfo> {
    let comp_re = Regex::new(r"(?:export\s+)?component\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    let text_lit_re = Regex::new(r#"text\s*:\s*"#).unwrap();
    let text_ident_re = Regex::new(r"text\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*;").unwrap();
    let fs_px_re = Regex::new(r"font-size\s*:\s*(\d+)px").unwrap();
    let fs_ident_re = Regex::new(r"font-size\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*;").unwrap();
    let prop_re =
        Regex::new(r"in(?:-out)?\s+property\s*<\s*(string|length)\s*>\s*([A-Za-z_][A-Za-z0-9_-]*)\s*(?::\s*(\d+)px)?\s*;")
            .unwrap();

    let mut out = HashMap::new();

    for cap in comp_re.captures_iter(masked) {
        let name = cap.get(1).unwrap().as_str().to_string();
        let after = cap.get(0).unwrap().end();
        // Find the component's own opening brace: the first '{' at/after `after`.
        let open = match masked[after..].find('{') {
            Some(off) => after + off,
            None => continue,
        };
        let span = match spans.iter().find(|(o, _)| *o == open) {
            Some(s) => *s,
            None => continue,
        };
        let body = slice_chars(masked, span.0, span.1 + 1);

        let mut info = ComponentInfo {
            span,
            ..Default::default()
        };

        // Property declarations (string text props + length defaults).
        for pc in prop_re.captures_iter(&body) {
            let kind = pc.get(1).unwrap().as_str();
            let pname = pc.get(2).unwrap().as_str().to_string();
            if kind == "length" {
                if let Some(d) = pc.get(3) {
                    info.length_defaults
                        .insert(pname, d.as_str().parse().unwrap());
                }
            }
        }

        // Every `Text { ... }`-shaped occurrence inside this component: pair
        // `text: <ident>` with the nearest `font-size:` in the SAME
        // innermost brace span.
        for tm in text_ident_re.captures_iter(&body) {
            let ident = tm.get(1).unwrap().as_str().to_string();
            let pos_in_body = tm.get(0).unwrap().start();
            // Re-derive this position in the ORIGINAL `masked` coordinate
            // space to reuse the global `spans`.
            let global_pos = span.0 + body[..pos_in_body].chars().count();
            let inner = innermost_span(spans, global_pos);
            let Some((io, ic)) = inner else { continue };
            let inner_text = slice_chars(masked, io, ic + 1);
            if let Some(m) = fs_px_re.captures(&inner_text) {
                let px: u32 = m.get(1).unwrap().as_str().parse().unwrap();
                info.text_props.insert(ident, SizeSource::Literal(px));
            } else if let Some(m) = fs_ident_re.captures(&inner_text) {
                let via = m.get(1).unwrap().as_str().to_string();
                info.text_props.insert(ident, SizeSource::ViaProperty(via));
            }
        }
        let _ = text_lit_re; // literal `text: "..."` inside a component def is handled generically below via direct-usage scanning

        out.insert(name, info);
    }
    out
}

/// Every `font-size: Npx` literal found anywhere in the macro block,
/// regardless of what text (if any) it applies to — the raw material for
/// the `PIXEL_SIZES`-membership check (independent of glyph coverage).
fn scan_font_sizes(path: &Path) -> Vec<u32> {
    let src =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let tok = tokenize(&src);
    let masked = &tok.masked;
    let Some((mo, mc)) = macro_block_span(masked) else {
        return Vec::new();
    };
    let body = slice_chars(masked, mo, mc + 1);
    let fs_px_re = Regex::new(r"font-size\s*:\s*(\d+)px").unwrap();
    fs_px_re
        .captures_iter(&body)
        .map(|c| c.get(1).unwrap().as_str().parse().unwrap())
        .collect()
}

/// Locate the `slint::slint!{ ... }` macro block's outer brace span.
fn macro_block_span(masked: &str) -> Option<(usize, usize)> {
    let open = masked
        .find("slint::slint!")
        .or_else(|| masked.find("slint !"))
        .and_then(|idx| masked[idx..].find('{').map(|off| idx + off))?;
    brace_spans(masked).into_iter().find(|(o, _)| *o == open)
}

/// Scan one screen file's `slint::slint!{}` macro block for every literal's
/// (codepoint, resolved-size) usage.
fn scan_file(path: &Path) -> Vec<Usage> {
    let src =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let tok = tokenize(&src);
    let masked = &tok.masked;

    let spans = brace_spans(masked);
    let Some((mo, mc)) = macro_block_span(masked) else {
        return Vec::new();
    };

    let literals_by_pos: HashMap<usize, &Literal> = tok
        .literals
        .iter()
        .filter(|l| l.quote_start > mo && l.quote_start < mc)
        .map(|l| (l.quote_start, l))
        .collect();

    let components = find_component_defs(masked, &spans);

    let fs_px_re = Regex::new(r"font-size\s*:\s*(\d+)px").unwrap();
    let text_lit_quote_re = Regex::new(r#"text\s*:\s*""#).unwrap();
    let prop_lit_re = Regex::new(r#"([A-Za-z_][A-Za-z0-9_-]*)\s*:\s*""#).unwrap();

    let mut usages: Vec<Usage> = Vec::new();
    let mut resolved_positions: HashSet<usize> = HashSet::new();

    // 1) Direct `Text { text: "..."; ... font-size: Npx; ... }` occurrences
    //    (also covers property-default literals like
    //    `in property <string> x: "\u{2014}";`, which simply won't find a
    //    font-size in their enclosing span — fine, since those are always
    //    BMP-class in this codebase and don't need one).
    for m in text_lit_quote_re.find_iter(masked) {
        let quote_pos = m.end() - 1; // index of the opening `"`
        let Some(lit) = literals_by_pos.get(&quote_pos) else {
            continue;
        };
        resolved_positions.insert(quote_pos);
        let inner = innermost_span(&spans, quote_pos);
        let size_px = inner.and_then(|(io, ic)| {
            let inner_text = slice_chars(masked, io, ic + 1);
            fs_px_re
                .captures(&inner_text)
                .map(|c| c.get(1).unwrap().as_str().parse().unwrap())
        });
        for ch in lit.decoded.chars() {
            usages.push(Usage {
                ch,
                size_px,
                unresolved_detail: None,
            });
        }
    }

    // 2) Literal string properties passed to a KNOWN reusable component
    //    instantiation (e.g. `ToggleRow { label: "🔔 ..."; }`), resolved
    //    through that component's own `text:`/`font-size:` wiring.
    for (cname, cinfo) in &components {
        // Every occurrence of `<cname> {` OTHER than the definition itself
        // is an instantiation site.
        let inst_re = Regex::new(&format!(r"\b{}\s*\{{", regex::escape(cname))).unwrap();
        for im in inst_re.find_iter(masked) {
            let open = im.end() - 1;
            if open == cinfo.span.0 {
                continue; // the component's own definition, not an instantiation
            }
            let Some(ispan) = spans.iter().find(|(o, _)| *o == open).copied() else {
                continue;
            };
            let inst_text = slice_chars(masked, ispan.0, ispan.1 + 1);
            for pm in prop_lit_re.captures_iter(&inst_text) {
                let pname = pm.get(1).unwrap().as_str();
                let Some(size_source) = cinfo.text_props.get(pname) else {
                    continue;
                };
                let local_quote_pos = pm.get(0).unwrap().end() - 1;
                let global_quote_pos = ispan.0 + inst_text[..local_quote_pos].chars().count();
                let Some(lit) = literals_by_pos.get(&global_quote_pos) else {
                    continue;
                };
                resolved_positions.insert(global_quote_pos);

                let (size_px, unresolved_detail) = match size_source {
                    SizeSource::Literal(px) => (Some(*px), None),
                    SizeSource::ViaProperty(varname) => {
                        // Override at THIS instantiation site?
                        let override_re =
                            Regex::new(&format!(r"{}\s*:\s*(\d+)px", regex::escape(varname)))
                                .unwrap();
                        if let Some(om) = override_re.captures(&inst_text) {
                            (Some(om.get(1).unwrap().as_str().parse().unwrap()), None)
                        } else if let Some(default_px) = cinfo.length_defaults.get(varname) {
                            (Some(*default_px), None)
                        } else {
                            (
                                None,
                                Some(format!(
                                    "{cname}.{pname} renders via font-size: {varname}, whose value \
                                     could not be resolved at this instantiation or as a component default"
                                )),
                            )
                        }
                    }
                };
                for ch in lit.decoded.chars() {
                    usages.push(Usage {
                        ch,
                        size_px,
                        unresolved_detail: unresolved_detail.clone(),
                    });
                }
            }
        }
    }

    // 3) Any remaining literal inside the macro block that wasn't matched
    //    above (e.g. a bare property default not shaped like `text: "...";`)
    //    still needs Level-2 (registration) coverage, with no size claim.
    for lit in tok
        .literals
        .iter()
        .filter(|l| l.quote_start > mo && l.quote_start < mc)
    {
        if resolved_positions.contains(&lit.quote_start) {
            continue;
        }
        for ch in lit.decoded.chars() {
            usages.push(Usage {
                ch,
                size_px: None,
                unresolved_detail: None,
            });
        }
    }

    usages
}

/// Run the full glyph-coverage check.
///
/// `repo_root`: path to the MeshCadet repository root (containing
/// `firmware/`).
pub fn check(repo_root: &Path) -> Vec<Violation> {
    let tables = parse_font_tables(&repo_root.join("firmware/gen_emoji_font.c"));
    let screens_dir = repo_root.join("firmware/src/ui/screens");
    let mut files: Vec<PathBuf> = fs::read_dir(&screens_dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", screens_dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "rs"))
        .filter(|p| p.file_name().and_then(|n| n.to_str()) != Some("mod.rs"))
        .collect();
    files.sort();

    let mut violations = Vec::new();
    for path in &files {
        let file_name = path.file_name().unwrap().to_string_lossy().to_string();
        for size_px in scan_font_sizes(path) {
            if !tables.pixel_sizes.contains(&size_px) {
                violations.push(Violation::SizeNotInScale {
                    file: file_name.clone(),
                    size_px,
                });
            }
        }
    }
    for path in files {
        let file_name = path.file_name().unwrap().to_string_lossy().to_string();
        for usage in scan_file(&path) {
            let cp = usage.ch as u32;
            if cp <= 0x7E {
                continue; // plain ASCII: always covered
            }
            if !tables.is_registered(cp) {
                violations.push(Violation::Unregistered {
                    file: file_name.clone(),
                    codepoint: cp,
                    ch: usage.ch,
                });
                continue;
            }
            if !tables.is_emoji_class(cp) {
                continue; // BMP symbol: rasterised at every PIXEL_SIZES entry, no size check needed
            }
            if KNOWN_GAPS.iter().any(|(f, kcp, ksize)| {
                *f == file_name && *kcp == cp && usage.size_px == Some(*ksize)
            }) {
                continue;
            }
            match (usage.size_px, &usage.unresolved_detail) {
                (Some(px), _) if !tables.emoji_sizes.contains(&px) => {
                    violations.push(Violation::WrongSize {
                        file: file_name.clone(),
                        codepoint: cp,
                        ch: usage.ch,
                        size_px: px,
                    });
                }
                (None, Some(detail)) => {
                    violations.push(Violation::UnresolvedSize {
                        file: file_name.clone(),
                        codepoint: cp,
                        ch: usage.ch,
                        detail: detail.clone(),
                    });
                }
                _ => {}
            }
        }
    }
    violations
}

/// Locate the repo root from any working directory inside it, by walking up
/// from `CARGO_MANIFEST_DIR` (this crate lives at `<repo_root>/xtask`).
pub fn repo_root_from_manifest_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask crate must live directly under the repo root")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_coverage_is_complete() {
        let violations = check(&repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "\nglyph-coverage harness found {} violation(s):\n{}\n",
            violations.len(),
            violations
                .iter()
                .map(|v| format!("  - {v}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn font_tables_parse_known_entries() {
        let tables =
            parse_font_tables(&repo_root_from_manifest_dir().join("firmware/gen_emoji_font.c"));
        assert!(
            tables.is_registered(0x1F4FB),
            "📻 (radio) must be registered"
        );
        assert!(
            tables.is_registered(0x1F50B),
            "🔋 (battery) must be registered"
        );
        assert!(tables.is_emoji_class(0x1F50B));
        assert!(
            !tables.is_emoji_class(0x2014),
            "em dash is a BMP symbol, not emoji-class"
        );
        assert!(tables.emoji_sizes.contains(&14));
        assert!(!tables.emoji_sizes.contains(&28));
    }

    #[test]
    fn decode_rust_escapes_handles_unicode_escape() {
        assert_eq!(decode_rust_escapes("\\u{2014}"), "\u{2014}");
        assert_eq!(decode_rust_escapes("plain"), "plain");
    }

    // ── Synthetic-fixture tests ─────────────────────────────────────────────
    //
    // `glyph_coverage_is_complete` above only ever exercises the "no
    // violations" path against the LIVE repo — once clean, it can never
    // again exercise the detector's positive (violation-found) branches. A
    // future refactor of the scanner could silently break detection and
    // that test would keep passing. These build a throwaway repo-shaped
    // fixture per case so each violation kind (and the clean/allowlist
    // paths) has a standing, always-exercised regression test independent
    // of the live repo's current state.

    const FIXTURE_C: &str = r#"
static const unsigned long EMOJI_CPS[] = {
    0x1F600,
};
static const unsigned long UI_EXTRA_CPS[] = {
    0x1F4E4,
};
static const unsigned long BMP_SYMBOLS[] = {
    0x2039,
};
static const int PIXEL_SIZES[] = {8, 9, 10, 11, 13, 14, 15, 16, 18, 20, 22, 28};
static const int EMOJI_SIZES[] = {11, 13, 14, 16, 18, 20};
"#;

    /// Build a throwaway `<tmp>/firmware/{gen_emoji_font.c, src/ui/screens/test_screen.rs}`
    /// tree and return its root. Callers own cleanup via [`cleanup_fixture`].
    fn write_fixture(label: &str, screen_rs: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "xtask-fixture-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let screens_dir = root.join("firmware/src/ui/screens");
        fs::create_dir_all(&screens_dir).unwrap();
        fs::write(root.join("firmware/gen_emoji_font.c"), FIXTURE_C).unwrap();
        fs::write(screens_dir.join("test_screen.rs"), screen_rs).unwrap();
        root
    }

    fn cleanup_fixture(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fixture_clean_screen_has_no_violations() {
        let root = write_fixture(
            "clean",
            r#"slint::slint! {
    export component TestUi inherits Window {
        Text { text: "😀"; font-size: 18px; }
        Text { text: "‹"; font-size: 9px; }
    }
}"#,
        );
        let violations = check(&root);
        cleanup_fixture(&root);
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn fixture_detects_unregistered_codepoint() {
        let root = write_fixture(
            "unregistered",
            r#"slint::slint! {
    export component TestUi inherits Window {
        Text { text: "🔋"; font-size: 14px; }
    }
}"#,
        );
        let violations = check(&root);
        cleanup_fixture(&root);
        assert_eq!(
            violations,
            vec![Violation::Unregistered {
                file: "test_screen.rs".into(),
                codepoint: 0x1F50B,
                ch: '🔋'
            }]
        );
    }

    #[test]
    fn fixture_detects_wrong_emoji_size() {
        let root = write_fixture(
            "wrong-emoji-size",
            r#"slint::slint! {
    export component TestUi inherits Window {
        Text { text: "😀"; font-size: 28px; }
    }
}"#,
        );
        let violations = check(&root);
        cleanup_fixture(&root);
        assert_eq!(
            violations,
            vec![Violation::WrongSize {
                file: "test_screen.rs".into(),
                codepoint: 0x1F600,
                ch: '😀',
                size_px: 28
            }]
        );
    }

    #[test]
    fn fixture_detects_size_not_in_scale() {
        let root = write_fixture(
            "size-not-in-scale",
            r#"slint::slint! {
    export component TestUi inherits Window {
        Text { text: "hello"; font-size: 12px; }
    }
}"#,
        );
        let violations = check(&root);
        cleanup_fixture(&root);
        assert_eq!(
            violations,
            vec![Violation::SizeNotInScale {
                file: "test_screen.rs".into(),
                size_px: 12
            }]
        );
    }

    #[test]
    fn fixture_resolves_reusable_row_component_indirection() {
        // Mirrors the real ToggleRow/InfoRow pattern: a literal string passed
        // via a named property to a reusable component, rendered through
        // that component's OWN internal `Text { text: label; font-size: N; }`.
        let root = write_fixture(
            "row-indirection",
            r#"slint::slint! {
    component Row {
        in property <string> label;
        Text { text: label; font-size: 14px; }
    }
    export component TestUi inherits Window {
        Row { label: "🔋  Battery"; }
    }
}"#,
        );
        let violations = check(&root);
        cleanup_fixture(&root);
        assert_eq!(
            violations,
            vec![Violation::Unregistered {
                file: "test_screen.rs".into(),
                codepoint: 0x1F50B,
                ch: '🔋'
            }],
            "🔋 is unregistered in this fixture's tables — the point is that the row-\
             component indirection itself must not swallow the finding"
        );
    }

    #[test]
    fn fixture_comment_only_mention_is_not_a_violation() {
        // A glyph mentioned only in a `//` comment (e.g. a doc-comment ASCII-art
        // diagram) must never be mistaken for a rendered glyph.
        let root = write_fixture(
            "comment-only",
            r#"slint::slint! {
    export component TestUi inherits Window {
        // 🔋 mentioned here only in a comment, never rendered
        Text { text: "hello"; font-size: 13px; }
    }
}"#,
        );
        let violations = check(&root);
        cleanup_fixture(&root);
        assert!(violations.is_empty(), "{violations:?}");
    }
}
