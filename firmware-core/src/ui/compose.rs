// SPDX-License-Identifier: GPL-3.0-only
//! Compose screen — pure shortcode-prefix scan for the `:shortcode:` emoji
//! autocomplete.
//!
//! The `slint::slint!{}` view and the `ComposeScreen` Rust wrapper stay in
//! `firmware/src/ui/screens/compose.rs` (they depend on Slint); only the
//! plain-data helper below moves here so its tests execute under `cargo
//! test --workspace` (this crate is a detached, cross-compiled workspace —
//! see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block written there
//! would type-check but never run). See
//! `docs/adr/0005-firmware-core-extraction.md`.

/// Return the in-progress `:shortcode` token: the text after the last `:` in
/// `text`, provided that tail contains only shortcode characters (ASCII
/// alphanumerics and `_`).  An empty tail (a just-typed trailing `:`) is a
/// valid prefix and matches every shortcode.  Returns `None` when there is no
/// `:` or when the tail has already been terminated (e.g. by whitespace or a
/// closing `:`), so the autocomplete bar is hidden outside shortcode entry.
pub fn current_shortcode_prefix(text: &str) -> Option<&str> {
    let colon = text.rfind(':')?;
    let tail = &text[colon + 1..];
    if tail.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(tail)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::current_shortcode_prefix;

    #[test]
    fn bare_colon_opens_with_empty_prefix() {
        assert_eq!(current_shortcode_prefix("hello :"), Some(""));
    }

    #[test]
    fn partial_shortcode_is_the_prefix() {
        assert_eq!(current_shortcode_prefix("hi :sm"), Some("sm"));
        assert_eq!(current_shortcode_prefix(":rock"), Some("rock"));
    }

    #[test]
    fn closed_or_absent_shortcode_yields_none() {
        // Closed by whitespace after the token.
        assert_eq!(current_shortcode_prefix(":smile "), None);
        // No colon at all.
        assert_eq!(current_shortcode_prefix("plain text"), None);
        // A completed :shortcode: -> tail after last ':' is empty => Some(""),
        // which is acceptable (re-opens for a new shortcode after the colon).
    }
}
