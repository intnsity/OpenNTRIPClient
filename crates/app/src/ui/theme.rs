//! Butter Paper design system: the single source of visual truth.
//!
//! Authoritative reference: source\YellowBGs.md. The load-bearing rules:
//!
//! - Depth is communicated by paper tint, never by heavy shadow. The stack,
//!   recessed to raised: PAPER_0 wells, PAPER_1 window base, PAPER_2 cards,
//!   PAPER_3 (pure white) editable inputs. PAPER_TINT marks hover/selection.
//! - Ink is a warm brown ramp, never pure black - #000 reads harsh and cold
//!   on yellow paper.
//! - Neutrals stay warm, accents stay cool: cobalt means interactive
//!   (buttons, links, focus, active tab, the plot line), and the semantic
//!   colors are cool and saturated. Warning is signal orange, NEVER yellow -
//!   yellow vanishes on this paper.
//! - White means editable: PAPER_3 appears only on surfaces the user can
//!   type into.
//! - Contrast is checked against PAPER_1 (#FFF7E0), not white.
//!
//! Every window styles itself exclusively through these tokens and the
//! helpers below; ad-hoc `Color32` literals in render code are a defect.

use egui::{Color32, CornerRadius, Stroke};

// ---------------------------------------------------------------------
// Paper (backgrounds)
// ---------------------------------------------------------------------

/// Recessed wells: plot background, code/log beds, table header bands.
pub const PAPER_0: Color32 = Color32::from_rgb(0xFA, 0xF3, 0xD9);
/// The window base every panel sits on.
pub const PAPER_1: Color32 = Color32::from_rgb(0xFF, 0xF7, 0xE0);
/// Cards and group boxes floating on the base; also dialogs and popups.
pub const PAPER_2: Color32 = Color32::from_rgb(0xFF, 0xFB, 0xEB);
/// Editable inputs only - the single pure white in the system.
pub const PAPER_3: Color32 = Color32::WHITE;
/// Hover and selection highlight.
pub const PAPER_TINT: Color32 = Color32::from_rgb(0xFF, 0xF4, 0xC9);

// ---------------------------------------------------------------------
// Ink (text) - warm browns, never pure black
// ---------------------------------------------------------------------

/// Primary text; ~13.5:1 on PAPER_1 (AAA).
pub const INK_PRIMARY: Color32 = Color32::from_rgb(0x2B, 0x24, 0x17);
/// Secondary text: log body, captions, idle status lines, and every
/// placeholder or hint the user is meant to READ - at ~5.6:1 on PAPER_1 it
/// is the darkest ink that still recedes, and the floor for body text.
pub const INK_SECONDARY: Color32 = Color32::from_rgb(0x6B, 0x62, 0x50);
/// Disabled-control ink, interactive-widget outlines, and non-text gauge
/// boundaries (the status strip's activity well) - never body text.
///
/// Deliberate deviation from YellowBGs.md: the guide's #9C927C measures
/// 2.88:1 on PAPER_1, under even the 3:1 large-text/non-text floor, so the
/// token is darkened one step to clear 3:1 on every paper (pinned by
/// `muted_clears_non_text_floor_everywhere`). That makes it legal for the
/// two roles it actually serves - text inside disabled controls (WCAG-exempt,
/// and egui dims those further via opacity) and the 1.4.11 non-text outline
/// on unfocused inputs/buttons. Readable text always uses INK_SECONDARY or
/// darker; `muted_ink_stays_out_of_render_code` enforces the reservation.
pub const INK_MUTED: Color32 = Color32::from_rgb(0x8F, 0x85, 0x70);

// ---------------------------------------------------------------------
// Accent - cobalt is the one interactive color
// ---------------------------------------------------------------------

/// Links, primary buttons, focus rings, active tab, the plot line.
pub const ACCENT: Color32 = Color32::from_rgb(0x1E, 0x40, 0xAF);
/// Hover/pressed shade of ACCENT.
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x1B, 0x36, 0x91);
/// Cool tint behind accent callouts and under selected TEXT: the one
/// background that reads as a band against every warm paper step AND the
/// white of an input, because it shifts hue (cool) rather than lightness.
pub const ACCENT_TINT: Color32 = Color32::from_rgb(0xE4, 0xEB, 0xFA);
/// Secondary editorial accent (eyebrows, category labels); never on
/// anything interactive.
#[allow(dead_code)]
pub const GRAPHITE: Color32 = Color32::from_rgb(0x3F, 0x3F, 0x46);

// ---------------------------------------------------------------------
// Semantics - clean and cool; warning skips yellow entirely
// ---------------------------------------------------------------------

/// Success / RTK Fixed: emerald.
pub const SUCCESS: Color32 = Color32::from_rgb(0x04, 0x78, 0x57);
// Badge/callout tints: unconsumed until the M3 dialogs land.
#[allow(dead_code)]
pub const SUCCESS_TINT: Color32 = Color32::from_rgb(0xDC, 0xF2, 0xE7);
/// Warning / RTK Float: signal orange (~4.9:1 on PAPER_1).
pub const WARNING: Color32 = Color32::from_rgb(0xC2, 0x41, 0x0C);
#[allow(dead_code)]
pub const WARNING_TINT: Color32 = Color32::from_rgb(0xFC, 0xE9, 0xDE);
/// Danger / Invalid fix / TLS-insecure: crimson.
pub const DANGER: Color32 = Color32::from_rgb(0xBE, 0x12, 0x3C);
pub const DANGER_TINT: Color32 = Color32::from_rgb(0xFC, 0xE5, 0xEA);

// ---------------------------------------------------------------------
// Hairlines - warm ink at low alpha, so they sit right on every paper step
// ---------------------------------------------------------------------

/// rgba(43,36,23,0.14) premultiplied: standard hairline border.
pub const HAIRLINE: Color32 = Color32::from_rgba_premultiplied(6, 5, 3, 36);
/// rgba(43,36,23,0.28) premultiplied: window outlines. (Input/widget
/// outlines use opaque INK_MUTED instead - see `visuals()` - because a
/// translucent hairline cannot reach the 3:1 non-text floor.)
pub const HAIRLINE_STRONG: Color32 = Color32::from_rgba_premultiplied(12, 10, 6, 71);
/// rgba(43,36,23,0.10) premultiplied: the only shadow tint in the system,
/// reserved for true overlays (dialogs, popups).
const SHADOW: Color32 = Color32::from_rgba_premultiplied(4, 4, 2, 26);

/// Shared corner radius: 6 px on widgets, 8 px on cards and windows.
const RADIUS_WIDGET: CornerRadius = CornerRadius::same(6);
const RADIUS_CARD: CornerRadius = CornerRadius::same(8);

/// Install the Butter Paper visuals on the context. Call once at startup;
/// everything egui paints afterwards derives from these tokens. Both theme
/// slots get the same visuals: the paper is the app's identity, so an OS
/// light/dark flip must not restyle it mid-session.
/// Install IBM Plex as the interface typeface: Plex Sans (Text weight - the
/// weight IBM tunes for UI body copy) for proportional text, Plex Mono for the
/// monospace log / hex / table beds. Both are prepended to egui's default
/// families, so the bundled fallback fonts still cover glyphs Plex lacks
/// (emoji, non-Latin scripts). Fonts are OFL-1.1: see assets/fonts/OFL.txt and
/// CREDITS.md.
fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontFamily};
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "PlexSans".to_owned(),
        std::sync::Arc::new(FontData::from_static(include_bytes!(
            "../../../../assets/fonts/IBMPlexSans-Text.ttf"
        ))),
    );
    fonts.font_data.insert(
        "PlexMono".to_owned(),
        std::sync::Arc::new(FontData::from_static(include_bytes!(
            "../../../../assets/fonts/IBMPlexMono-Regular.ttf"
        ))),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "PlexSans".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "PlexMono".to_owned());
    ctx.set_fonts(fonts);
}

/// Type scale over the Plex families. Body steps up half a point from egui's
/// default to suit Plex Sans's UI weight; the monospace beds stay compact so
/// the RTCM and sourcetable tables keep their column density.
fn install_text_styles(ctx: &egui::Context) {
    use egui::{FontFamily, FontId, TextStyle};
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(11.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(18.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.5, FontFamily::Monospace),
            ),
        ]
        .into();
    });
}

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    install_text_styles(ctx);
    ctx.set_visuals_of(egui::Theme::Light, visuals());
    ctx.set_visuals_of(egui::Theme::Dark, visuals());
}

/// The full egui mapping of the token set.
///
/// egui shares one widget style between button text, checkbox/radio labels
/// and combo text, so the global interactive style is the guide's
/// "secondary" button: white surface (an interactive widget is a thing you
/// can act on, like an input), strong warm border, ink text - with cobalt
/// arriving on hover, focus and selection. Solid-cobalt primary actions are
/// opted into per-widget via [`accent_button`].
pub fn visuals() -> egui::Visuals {
    use egui::style::{Selection, TextCursorStyle, WidgetVisuals};

    let mut v = egui::Visuals::light();
    v.dark_mode = false;
    v.override_text_color = None;

    v.widgets.noninteractive = WidgetVisuals {
        // Windows fall back to this fill; hairline strokes also draw
        // separators and default frame outlines.
        bg_fill: PAPER_2,
        weak_bg_fill: PAPER_2,
        bg_stroke: Stroke::new(1.0, HAIRLINE),
        corner_radius: RADIUS_WIDGET,
        fg_stroke: Stroke::new(1.0, INK_PRIMARY),
        expansion: 0.0,
    };
    v.widgets.inactive = WidgetVisuals {
        // bg_fill paints checkbox/radio interiors - editable, so white.
        bg_fill: PAPER_3,
        weak_bg_fill: PAPER_3,
        // Opaque muted-ink outline, not a hairline: an unfocused input's
        // white plate is only ~1.07:1 against the paper, so the border is
        // the sole "this is editable/clickable" boundary and must clear the
        // WCAG 1.4.11 non-text 3:1 floor on white and on every paper.
        // Decorative hairlines (separators, card edges) stay low-alpha.
        bg_stroke: Stroke::new(1.0, INK_MUTED),
        corner_radius: RADIUS_WIDGET,
        fg_stroke: Stroke::new(1.0, INK_PRIMARY),
        expansion: 0.0,
    };
    v.widgets.hovered = WidgetVisuals {
        // Hover = paper tint fill plus the cobalt "this is interactive" ring.
        bg_fill: PAPER_TINT,
        weak_bg_fill: PAPER_TINT,
        bg_stroke: Stroke::new(1.0, ACCENT),
        corner_radius: RADIUS_WIDGET,
        fg_stroke: Stroke::new(1.5, INK_PRIMARY),
        expansion: 1.0,
    };
    v.widgets.active = WidgetVisuals {
        // Pressed and focused: the cobalt ring thickens into a focus ring.
        // fg_stroke doubles as `strong_text_color`, so it stays ink.
        bg_fill: PAPER_TINT,
        weak_bg_fill: PAPER_TINT,
        bg_stroke: Stroke::new(1.5, ACCENT_HOVER),
        corner_radius: RADIUS_WIDGET,
        fg_stroke: Stroke::new(2.0, INK_PRIMARY),
        expansion: 1.0,
    };
    v.widgets.open = WidgetVisuals {
        bg_fill: PAPER_TINT,
        weak_bg_fill: PAPER_TINT,
        bg_stroke: Stroke::new(1.0, ACCENT),
        corner_radius: RADIUS_WIDGET,
        fg_stroke: Stroke::new(1.0, INK_PRIMARY),
        expansion: 0.0,
    };

    // Text selection: the cool accent tint, NOT the paper tint. egui uses
    // `selection` for dragged-over text in inputs and labels, where the
    // warm PAPER_TINT band is invisible over white (1.10:1) and the only
    // remaining cue would be a dark-brown-to-dark-navy glyph shift. The
    // guide's paper-tint rule targets row hover/selection, which egui
    // styles separately via widgets.hovered above.
    v.selection = Selection {
        bg_fill: ACCENT_TINT,
        stroke: Stroke::new(1.0, ACCENT),
    };
    v.hyperlink_color = ACCENT;

    v.panel_fill = PAPER_1;
    v.window_fill = PAPER_2;
    v.window_stroke = Stroke::new(1.0, HAIRLINE_STRONG);
    v.window_corner_radius = RADIUS_CARD;
    // Dialogs and popups are the only true overlays, so they keep a shadow -
    // warm-tinted, per the guide, never a gray one.
    v.window_shadow.color = SHADOW;
    v.popup_shadow.color = SHADOW;
    v.menu_corner_radius = RADIUS_WIDGET;

    v.faint_bg_color = PAPER_0; // striped rows
    v.extreme_bg_color = PAPER_0; // wells: plot bed, scrollbar gutter
    v.text_edit_bg_color = Some(PAPER_3); // white = editable
    v.code_bg_color = PAPER_0;

    v.warn_fg_color = WARNING;
    v.error_fg_color = DANGER;

    // Left unset, egui synthesizes weak text (RichText::weak, TextEdit hint
    // text) by fading INK_PRIMARY to ~4.0:1 on the papers - under the 4.5:1
    // AA floor. Weak text is still text the user reads (placeholders, row
    // counts, footers), so it takes INK_SECONDARY, the documented body-text
    // floor. Pinned by `visuals_honor_the_paper_rules`.
    v.weak_text_color = Some(INK_SECONDARY);

    v.text_cursor = TextCursorStyle {
        stroke: Stroke::new(2.0, ACCENT),
        ..v.text_cursor
    };
    // Cobalt says clickable; the pointer cursor repeats it.
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    v
}

/// A PAPER_2 card with a hairline border: the styled replacement for
/// `ui.group(..)` on group boxes and floating panels.
pub fn card(style: &egui::Style) -> egui::Frame {
    egui::Frame::group(style)
        .fill(PAPER_2)
        .stroke(Stroke::new(1.0, HAIRLINE))
        .corner_radius(RADIUS_CARD)
        .inner_margin(8)
}

/// The recessed bed under monospace log/code content: read-only diagnostic
/// text steps one elevation below its panel onto the theme's extreme
/// background (PAPER_0 here, the same bed the plot uses via
/// `extreme_bg_color`) inside a hairline border. Every monospace log surface
/// (event log, connection log, hex dump, unparsed sourcetable lines) must
/// sit in this well; YellowBGs.md files code/log beds under PAPER_0, and a
/// bed that floats flat on card paper breaks the depth model. Colors come
/// from the active `Visuals` so the well tracks whatever theme is installed.
pub fn well_frame(visuals: &egui::Visuals) -> egui::Frame {
    egui::Frame::new()
        .fill(visuals.extreme_bg_color)
        .stroke(visuals.widgets.noninteractive.bg_stroke)
        .inner_margin(4)
}

/// Paint the current table-header cell's background as the recessed PAPER_0
/// band. YellowBGs.md files table headers under paper-0, one step BELOW the
/// striped body rows, so the elevation model never inverts at the top of a
/// table (pinned by `header_band_is_recessed_below_window_and_stripes`).
/// Each cell expands by half the item spacing so adjacent cells tile into
/// one continuous band with no gaps at the column boundaries. Call first
/// inside a `header.col(..)` closure, before laying out the label.
pub fn header_band(ui: &egui::Ui) {
    let sp = ui.spacing().item_spacing;
    let rect = ui.max_rect().expand2(egui::vec2(sp.x / 2.0, sp.y / 2.0));
    ui.painter().rect_filled(rect, CornerRadius::ZERO, PAPER_0);
}

/// Persistent danger banner (e.g. TLS verification disabled): crimson rule
/// over the danger tint. Square corners - it is a full-width strip, not a
/// floating card.
pub fn danger_banner() -> egui::Frame {
    egui::Frame::new()
        .fill(DANGER_TINT)
        .stroke(Stroke::new(1.0, DANGER))
        .inner_margin(egui::Margin::symmetric(8, 4))
}

/// Solid-cobalt primary action button (Connect, Save...): the one place a
/// widget opts out of the white "secondary" surface. Disabled rendering is
/// egui's own opacity pass, so the fill dims correctly.
pub fn accent_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(text.into())
            .color(Color32::WHITE)
            .strong(),
    )
    .fill(ACCENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative luminance per WCAG, from sRGB bytes.
    fn luminance(c: Color32) -> f64 {
        let ch = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (l1, l2) = (luminance(a), luminance(b));
        (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
    }

    /// Rule 5 of the paper: contrast is checked against PAPER_1, not white.
    /// Ink and every semantic/interactive foreground must clear WCAG AA
    /// (4.5:1) on the base paper.
    #[test]
    fn foregrounds_clear_aa_on_base_paper() {
        for (name, c) in [
            ("ink-primary", INK_PRIMARY),
            ("ink-secondary", INK_SECONDARY),
            ("accent", ACCENT),
            ("accent-hover", ACCENT_HOVER),
            ("success", SUCCESS),
            ("warning", WARNING),
            ("danger", DANGER),
        ] {
            let ratio = contrast(c, PAPER_1);
            assert!(ratio >= 4.5, "{name} is {ratio:.2}:1 on paper-1");
        }
    }

    /// The paper stack must strictly lighten toward the viewer - that
    /// ordering IS the elevation model.
    #[test]
    fn paper_stack_lightens_upward() {
        let steps = [PAPER_0, PAPER_1, PAPER_2, PAPER_3];
        for pair in steps.windows(2) {
            assert!(luminance(pair[0]) < luminance(pair[1]));
        }
    }

    /// White is reserved for editable surfaces; the widget mapping must not
    /// leak it anywhere else, and text on paper must never be pure black.
    #[test]
    fn visuals_honor_the_paper_rules() {
        let v = visuals();
        assert_eq!(v.text_edit_bg_color, Some(PAPER_3));
        assert_ne!(v.panel_fill, PAPER_3);
        assert_ne!(v.window_fill, PAPER_3);
        assert_ne!(v.extreme_bg_color, PAPER_3);
        assert_ne!(v.widgets.noninteractive.fg_stroke.color, Color32::BLACK);
        assert_eq!(v.hyperlink_color, ACCENT);
        // Text selection is the cool accent tint (visible on white inputs);
        // row hover/selection keeps the warm paper tint.
        assert_eq!(v.selection.bg_fill, ACCENT_TINT);
        assert_eq!(v.widgets.hovered.bg_fill, PAPER_TINT);
        // Unfocused interactive outlines are the opaque non-text border.
        assert_eq!(v.widgets.inactive.bg_stroke.color, INK_MUTED);
        // Weak text (hints, placeholders, counters) must not fall back to
        // egui's synthesized fade (~4.0:1, sub-AA): it takes the secondary
        // ink, the documented floor for text the user is meant to read.
        assert_eq!(v.weak_text_color, Some(INK_SECONDARY));
    }

    /// The well must step down to the same bed the plot uses
    /// (extreme_bg_color) with the theme's hairline stroke, in both theme
    /// polarities - this is what makes log content read as a recessed
    /// surface instead of floating flat on the panel or card.
    #[test]
    fn well_uses_extreme_bg_and_hairline_in_both_themes() {
        for visuals in [egui::Visuals::light(), egui::Visuals::dark()] {
            let frame = well_frame(&visuals);
            assert_eq!(frame.fill, visuals.extreme_bg_color);
            assert_eq!(frame.stroke, visuals.widgets.noninteractive.bg_stroke);
            assert_ne!(
                frame.fill, visuals.panel_fill,
                "the well must visually step off the panel"
            );
        }
    }

    /// The guide's table contract: the header band is a RECESSED paper-0
    /// fill, darker than the window fill and no lighter than the body
    /// stripes, so the elevation model never inverts at the top of a table.
    /// (`header_band` paints PAPER_0 directly; this pins the ordering that
    /// makes that the correct token.)
    #[test]
    fn header_band_is_recessed_below_window_and_stripes() {
        assert!(luminance(PAPER_0) < luminance(PAPER_2));
        // Never lighter than the striped body rows egui paints below it.
        let stripes = visuals().faint_bg_color;
        assert!(luminance(PAPER_0) <= luminance(stripes));
    }

    /// Every monospace log/code bed sits in the shared well: the depth model
    /// (YellowBGs.md section 3) puts this content class on PAPER_0, and the
    /// pre-theme windows shipped without it. Source-scan like the INK_MUTED
    /// guard, since frame styling leaves no other testable trace.
    #[test]
    fn monospace_log_beds_sit_in_wells() {
        let ui_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        for file in ["log_pane.rs", "connlog_window.rs", "sourcetable_browser.rs"] {
            let text = std::fs::read_to_string(ui_dir.join(file)).expect("source is UTF-8");
            assert!(
                text.contains("well_frame"),
                "{file} renders a monospace log bed without theme::well_frame"
            );
        }
    }

    /// INK_MUTED's two legal roles - disabled-control ink and widget
    /// outlines - are large-text/non-text contexts, so the token must clear
    /// the WCAG 3:1 floor on every surface it can border or sit on. The
    /// guide's original #9C927C failed this everywhere (2.77-3.08:1),
    /// which is why the token deviates from YellowBGs.md.
    #[test]
    fn muted_clears_non_text_floor_everywhere() {
        for (name, paper) in [
            ("paper-0", PAPER_0),
            ("paper-1", PAPER_1),
            ("paper-2", PAPER_2),
            ("paper-3", PAPER_3),
            ("paper-tint", PAPER_TINT),
        ] {
            let ratio = contrast(INK_MUTED, paper);
            assert!(ratio >= 3.0, "ink-muted is {ratio:.2}:1 on {name}");
        }
    }

    /// INK_MUTED is sub-AA by design, so no render code may paint readable
    /// text with it: outside this file the token must not appear at all.
    /// (Disabled text goes through egui's own disabled pass; anything a
    /// user must read takes INK_SECONDARY or darker.) A future legitimate
    /// use extends the allowlist here, deliberately.
    ///
    /// Allowlisted: status_strip.rs strokes the activity gauge's well with
    /// INK_MUTED - the token's non-text-boundary role (the same one the
    /// widget outlines use via `visuals()`), never text.
    #[test]
    fn muted_ink_stays_out_of_render_code() {
        let ui_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let allowed = ["theme.rs", "status_strip.rs"];
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&ui_dir).expect("src/ui exists") {
            let path = entry.expect("dir entry").path();
            if path.extension().is_none_or(|e| e != "rs")
                || path
                    .file_name()
                    .is_some_and(|n| allowed.iter().any(|a| n == *a))
            {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("source is UTF-8");
            if text.contains("INK_MUTED") {
                offenders.push(path);
            }
        }
        assert!(
            offenders.is_empty(),
            "INK_MUTED referenced outside theme.rs: {offenders:?}"
        );
    }

    /// The text-selection band must be COOL: warm papers all have R >= B,
    /// so a blue-leaning tint stays distinguishable by hue on every step
    /// (and on white, where the warm PAPER_TINT washes out at 1.10:1).
    #[test]
    fn selection_band_is_cool_on_warm_paper() {
        assert!(ACCENT_TINT.b() > ACCENT_TINT.r());
        for paper in [PAPER_0, PAPER_1, PAPER_2, PAPER_TINT] {
            assert!(paper.r() >= paper.b());
        }
        // And it still darkens white more than the old paper tint did.
        assert!(contrast(ACCENT_TINT, PAPER_3) > contrast(PAPER_TINT, PAPER_3));
    }
}
