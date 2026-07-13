//! The Settings overlay forms: the global tabbed overlay, the per-view `;` quick
//! popup, the shared group-row builder, and the row renderer. Split out of
//! `views` to keep settings rendering self-contained.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{AppState, MouseTarget, ScrollBox};
use crate::ui::components;
use crate::ui::theme::Theme;

use super::settings_rows::{SettingValue, setting_label_value};

/// Settings as a centered, tabbed overlay over the current view (command-palette
/// only — Settings is not a view). Every settings group is a horizontal tab
/// ([`AppState::settings_tabs`] — the current view's non-empty groups); `group` is
/// the active-tab index into that list. Tab/⇧Tab switch tabs, j/k move, h/l adjust,
/// ⏎ toggles, esc closes. Each change writes straight back to `config.toml`.
pub fn settings_overlay(f: &mut Frame, area: Rect, app: &AppState) {
    let (w, h) = components::overlay_dims(app, area);
    let tabs = app.settings_tabs();
    let active = app
        .settings
        .group
        .unwrap_or(0)
        .min(tabs.len().saturating_sub(1));
    let inner = components::overlay_frame(
        f,
        area,
        app,
        w,
        h,
        &components::FrameSpec {
            title: "Settings",
            tabs: &tabs,
            active_tab: active,
            footer: &[
                ("⇥", "tab"),
                ("j/k", "move"),
                ("⏎", "toggle"),
                ("f", "size"),
                ("esc", "close"),
            ],
        },
    );
    if inner.height == 0 {
        return;
    }
    let (lines, sel_line, item_lines) = settings_group_lines(app, inner.width as usize);
    render_settings_list(f, app, inner, lines, sel_line, &item_lines);
}

/// Build the rendered rows for the active settings group (window detail or the
/// popup overlay). Returns `(lines, selected_line, (row, flat_line) map)`.
fn settings_group_lines(
    app: &AppState,
    iw: usize,
) -> (Vec<Line<'static>>, usize, Vec<(usize, usize)>) {
    use crate::app::{NameTarget, Setting};
    let th = &app.theme;
    let items = app.settings_group_items();
    let sel = app.settings.sel.min(items.len().saturating_sub(1));
    let mut lines: Vec<Line> = Vec::new();
    let mut sel_line = 0usize;
    let mut item_lines: Vec<(usize, usize)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let (label, value) = setting_label_value(app, item);
        if i == sel {
            sel_line = lines.len();
        }
        item_lines.push((i, lines.len()));
        lines.push(setting_line(app, th, i == sel, &label, &value, iw));

        // inline text-entry shown under "Add music directory…"
        if matches!(item, Setting::AddDir)
            && matches!(app.input.naming, Some(NameTarget::AddMusicDir))
        {
            lines.push(Line::from(vec![
                Span::styled("    › ", Style::default().fg(th.accent[0].into())),
                Span::styled(
                    app.input.buffer.clone(),
                    Style::default().fg(th.text.into()),
                ),
                Span::styled("▌", Style::default().fg(th.accent[0].into())),
            ]));
        }
    }
    (lines, sel_line, item_lines)
}

/// The `;` popup's size. It shares the global overlay's width at every `f` step
/// (so growing is monotonic — never a shrink-then-grow), and grows to the same
/// roomy centred card at the higher steps, so resize behaves identically
/// everywhere. At the compact step (`overlay_size == 0`) its *height* is capped to
/// the view's *tallest* tab — not the active one, so switching tabs never resizes
/// or re-centers the card — avoiding a big empty card for a few rows while still
/// letting a tall tab scroll inside the compact frame.
pub(crate) fn popup_dims(app: &AppState, area: Rect) -> (u16, u16) {
    let (w, h) = components::overlay_dims(app, area);
    if app.config.overlay_size == 0 {
        // rows + tab bar + footer + borders + a little breathing room, but never
        // taller than the compact overlay itself (tall tabs then scroll)
        let content = app.popup_max_rows() as u16 + 6;
        return (w, content.clamp(10, h));
    }
    (w, h)
}

/// Per-view quick-settings popup (the `;` shortcut). A compact, tabbed sibling of
/// the global Settings overlay: the view's options grouped into Panes / a content
/// tab / Playback (Tab/⇧Tab switch); same rounded frame + footer + toggle
/// switches. Esc closes.
pub fn settings_popup(f: &mut Frame, area: Rect, app: &AppState) {
    let Some(active) = app.settings.popup else {
        return;
    };
    let tabs = app.popup_tab_names();
    let title = format!("{} options", app.layout.title());
    let (w, h) = popup_dims(app, area);
    let inner = components::overlay_frame(
        f,
        area,
        app,
        w,
        h,
        &components::FrameSpec {
            title: &title,
            tabs: &tabs,
            active_tab: active.min(tabs.len().saturating_sub(1)),
            footer: &[
                ("⇥", "tab"),
                ("j/k", "move"),
                ("⏎", "toggle"),
                ("f", "size"),
                ("esc", "close"),
            ],
        },
    );
    if inner.height == 0 {
        return;
    }
    let (lines, sel_line, item_lines) = settings_group_lines(app, inner.width as usize);
    render_settings_list(f, app, inner, lines, sel_line, &item_lines);
}

/// Shared scroll/click/render for both settings modes (the global tabbed overlay
/// and the `;` popup). Key hints live in each overlay's footer bar (see
/// `components::overlay_frame`); the status bar mirrors them for discoverability.
fn render_settings_list(
    f: &mut Frame,
    app: &AppState,
    list_area: Rect,
    lines: Vec<Line>,
    sel_line: usize,
    item_lines: &[(usize, usize)],
) {
    let h = list_area.height as usize;
    let total = lines.len();
    let offset = components::sticky_off(&app.settings.off, sel_line, total, h);
    app.register_click(list_area, MouseTarget::Scroll(ScrollBox::Settings));
    for (idx, fl) in item_lines {
        if *fl >= offset && *fl < offset + h {
            let sy = list_area.y + (*fl - offset) as u16;
            app.register_click(
                Rect::new(list_area.x, sy, list_area.width, 1),
                MouseTarget::SettingRow(*idx),
            );
        }
    }
    f.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), list_area);
}

/// One settings row: `▸ label …… value`, value aligned to the right edge. The
/// value is either a toggle switch (booleans) or accent text (everything else).
fn setting_line(
    app: &AppState,
    th: &Theme,
    on: bool,
    label: &str,
    value: &SettingValue,
    w: usize,
) -> Line<'static> {
    // the selected row is a rounded capsule (bright — the settings list is always
    // the focused surface); its interior lays out inside `w - 2` (the caps take a
    // col each side), and `pill_line` fills the interior + rounds the corners.
    let bg = components::sel_fill(th, on, true);
    let iw = w.saturating_sub(2);
    let marker = if on { "▸ " } else { "  " };
    // the value span(s) and their display width (toggles are a fixed width so the
    // value column stays aligned whether a row is on or off)
    // On the selected row the pill fills with `th.selection`; the value's normal
    // accent colour (`toggle_on`/accent text) is often the *same* accent family,
    // so it collides with the fill and disappears. Mirror the label: switch the
    // value to the high-contrast `th.text` when selected. A toggle's knob position
    // ((──●)/(●──)) still carries on/off without the colour.
    let (value_span, vlen) = match value {
        SettingValue::Toggle(b) => {
            let sw = components::toggle_span(th, *b);
            let style = if on {
                Style::default().fg(th.text.into())
            } else {
                sw.style
            };
            (Span::styled(sw.content, style), components::TOGGLE_W)
        }
        SettingValue::Text(s) => {
            let fg: Color = if on {
                th.text.into()
            } else {
                th.accent[1].into()
            };
            (
                Span::styled(s.clone(), Style::default().fg(fg)),
                s.chars().count(),
            )
        }
    };
    let budget = iw.saturating_sub(2 + vlen + 2).max(1);
    let mut nm: String = label.chars().take(budget).collect();
    if label.chars().count() > budget {
        nm.pop();
        nm.push('…');
    }
    let fill = iw.saturating_sub(2 + nm.chars().count() + vlen);
    let lbl_fg: Color = if on {
        th.text.into()
    } else {
        th.text_dim.into()
    };
    let content = Line::from(vec![
        Span::styled(marker, Style::default().fg(th.accent[0].into())),
        Span::styled(nm, Style::default().fg(lbl_fg)),
        Span::raw(" ".repeat(fill)),
        value_span,
    ]);
    components::pill_line(app, w, content, bg)
}
