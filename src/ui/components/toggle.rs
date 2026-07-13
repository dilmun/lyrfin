//! The reusable toggle-switch widget shared by every boolean settings row.
//!
//! A wide sliding switch: ON `(──●)` in the theme's `toggle_on` colour (knob
//! pushed right), OFF `(●──)` in `toggle_off` (knob left). It replaces the old
//! `●  on / ○  off` text so booleans read as switches and the right-aligned
//! value column stays put regardless of state.

use super::*;
use crate::ui::theme::Theme;
use ratatui::style::Style;
use ratatui::text::Span;

/// Display width (cells) of a toggle switch. Both states are this wide so value
/// columns line up whether a row is on or off.
pub const TOGGLE_W: usize = 5;

/// A toggle-switch span: ON `(──●)` (accent, `toggle_on`), OFF `(●──)` (dim,
/// `toggle_off`). One colour for the whole glyph — the knob position plus the
/// colour carry the state.
pub fn toggle_span(th: &Theme, on: bool) -> Span<'static> {
    let (glyph, role) = if on {
        ("(──●)", th.toggle_on())
    } else {
        ("(●──)", th.toggle_off())
    };
    Span::styled(glyph, Style::default().fg(col(role)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::Theme;

    #[test]
    fn toggle_span_is_constant_width() {
        let th = Theme::aurora();
        assert_eq!(toggle_span(&th, true).content.chars().count(), TOGGLE_W);
        assert_eq!(toggle_span(&th, false).content.chars().count(), TOGGLE_W);
    }

    #[test]
    fn toggle_span_uses_toggle_roles() {
        let th = Theme::aurora();
        assert_eq!(toggle_span(&th, true).style.fg, Some(col(th.toggle_on())));
        assert_eq!(toggle_span(&th, false).style.fg, Some(col(th.toggle_off())));
    }
}
