//! The ButterPaper design system: the single source of visual truth.
//!
//! Authoritative reference: the ButterPaper v2 token set
//! (github.com/intnsity/ButterPaper, `butter-paper-v2.css`), three colorways:
//! Butter Paper (light, the default), Southwest Night and Nordic Night
//! (dark). The load-bearing rules, true in every colorway:
//!
//! - Depth is communicated by surface tint, never by heavy shadow. The stack,
//!   recessed to raised: `paper_0` wells, `paper_1` window base, `paper_2`
//!   cards, `paper_3` editable inputs. `paper_tint` marks hover/selection.
//! - Ink is a warm ramp, never pure black (or pure white on the nights).
//! - The accent means interactive (buttons, links, focus, active tab, the
//!   plot line): cobalt on paper, bloom/ember orange on the nights.
//! - `paper_3` means editable: only surfaces the user can type into. Buttons
//!   and pickers wear the `accent_tint` face instead, so a control you click
//!   never reads as a field you type into.
//! - Every status carries a word, not only a color.
//! - Paper only: warning is signal orange, NEVER yellow - yellow vanishes on
//!   paper. The nights use caution yellow, which is their correct warning.
//!
//! Render code reads colors only through the token functions below
//! (`theme::ink_secondary()`, ...), which answer for the ACTIVE colorway;
//! ad-hoc `Color32` literals outside this file are a defect.

use std::sync::atomic::{AtomicU8, Ordering};

use egui::{Color32, CornerRadius, Stroke};

use crate::settings::Colorway;

/// One colorway's full token set. Field docs give each token's ROLE; the
/// values per colorway come from ButterPaper v2.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub name: &'static str,
    /// egui's own dark-mode switch (scrollbars, default fallbacks).
    pub dark: bool,
    /// Recessed wells: plot background, code/log beds, table header bands.
    pub paper_0: Color32,
    /// The window base every panel sits on. Contrast is checked against it.
    pub paper_1: Color32,
    /// Cards and group boxes floating on the base; also dialogs and popups.
    pub paper_2: Color32,
    /// Editable inputs only.
    pub paper_3: Color32,
    /// Hover and selection highlight.
    pub paper_tint: Color32,
    /// Primary text.
    pub ink_primary: Color32,
    /// Secondary text: captions, idle status lines, every placeholder or
    /// hint the user is meant to READ - the floor for body text.
    pub ink_secondary: Color32,
    /// ButterPaper's `control-border`: disabled-control ink, interactive
    /// outlines, non-text gauge boundaries - never body text
    /// (`muted_ink_stays_out_of_render_code` enforces the reservation).
    pub ink_muted: Color32,
    /// Links, primary buttons, focus rings, active tab, the plot line.
    pub accent: Color32,
    pub accent_hover: Color32,
    /// Text on a solid accent fill.
    pub on_accent: Color32,
    /// Face of secondary buttons and pickers, text-selection band, callouts.
    pub accent_tint: Color32,
    pub success: Color32,
    pub success_tint: Color32,
    pub warning: Color32,
    pub warning_tint: Color32,
    /// Danger ink for small labels.
    pub danger: Color32,
    pub danger_tint: Color32,
    /// Solid danger fill (the delete confirm button) and the text on it.
    pub danger_strong: Color32,
    pub on_danger: Color32,
    /// Quiet dividers, separators, card edges.
    pub hairline: Color32,
    /// Window and list outlines.
    pub hairline_strong: Color32,
    /// The only shadow tint, reserved for true overlays (dialogs, popups).
    pub shadow: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Butter Paper: warm cream paper, brown ink, cobalt interaction. The app's
/// identity and the default.
pub const BUTTER_PAPER: Palette = Palette {
    name: "Butter Paper",
    dark: false,
    paper_0: rgb(0xFAF3D9),
    paper_1: rgb(0xFFF7E0),
    paper_2: rgb(0xFFFBEB),
    paper_3: Color32::WHITE,
    paper_tint: rgb(0xFFF4C9),
    ink_primary: rgb(0x2B2417),
    ink_secondary: rgb(0x6B6250),
    // ButterPaper v2 `control-border` (3.7:1 on the base); clears the 3:1
    // non-text floor on every paper step, as the old #8F8570 did.
    ink_muted: rgb(0x897F6B),
    accent: rgb(0x1E40AF),
    accent_hover: rgb(0x1B3691),
    on_accent: Color32::WHITE,
    accent_tint: rgb(0xE4EBFA),
    success: rgb(0x047857),
    success_tint: rgb(0xDCF2E7),
    warning: rgb(0xC2410C),
    warning_tint: rgb(0xFCE9DE),
    danger: rgb(0xBE123C),
    danger_tint: rgb(0xFCE5EA),
    danger_strong: rgb(0xBE123C),
    on_danger: Color32::WHITE,
    // Warm ink at 14% / 28% alpha (premultiplied), so they sit right on
    // every paper step.
    hairline: Color32::from_rgba_premultiplied(6, 5, 3, 36),
    hairline_strong: Color32::from_rgba_premultiplied(12, 10, 6, 71),
    shadow: Color32::from_rgba_premultiplied(4, 4, 2, 26),
};

/// Southwest Night: desert dusk browns, bloom-orange interaction, caution
/// yellow warnings.
pub const SOUTHWEST_NIGHT: Palette = Palette {
    name: "Southwest Night",
    dark: true,
    paper_0: rgb(0x1E201C),
    paper_1: rgb(0x252520),
    paper_2: rgb(0x322F2A),
    paper_3: rgb(0x403A33),
    paper_tint: rgb(0x4A3C33),
    ink_primary: rgb(0xF5EFE4),
    ink_secondary: rgb(0xD7CCBB),
    ink_muted: rgb(0xA49280),
    accent: rgb(0xF0A27A),
    accent_hover: rgb(0xFFB78D),
    on_accent: rgb(0x2C1D15),
    accent_tint: rgb(0x4D342C),
    success: rgb(0xACD09F),
    success_tint: rgb(0x2E392B),
    warning: rgb(0xFFD447),
    warning_tint: rgb(0x41351C),
    danger: rgb(0xFF878E),
    danger_tint: rgb(0x44232B),
    danger_strong: rgb(0xFF5364),
    on_danger: rgb(0x260E15),
    hairline: rgb(0x4A4239),
    hairline_strong: rgb(0x65584D),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 110),
};

/// Nordic Night: cabin greys, ember-orange interaction - no navy, teal or
/// cobalt surfaces.
pub const NORDIC_NIGHT: Palette = Palette {
    name: "Nordic Night",
    dark: true,
    paper_0: rgb(0x1F1E20),
    paper_1: rgb(0x252426),
    paper_2: rgb(0x2F2D30),
    paper_3: rgb(0x39363A),
    paper_tint: rgb(0x443E40),
    ink_primary: rgb(0xF1EDE8),
    ink_secondary: rgb(0xCFC7C3),
    ink_muted: rgb(0x92888C),
    accent: rgb(0xFF914D),
    accent_hover: rgb(0xFFAB73),
    on_accent: rgb(0x241810),
    accent_tint: rgb(0x4B3026),
    success: rgb(0xAEC5AD),
    success_tint: rgb(0x2D382F),
    warning: rgb(0xFFD447),
    warning_tint: rgb(0x41351C),
    danger: rgb(0xFF878E),
    danger_tint: rgb(0x44232B),
    danger_strong: rgb(0xFF5364),
    on_danger: rgb(0x260E15),
    hairline: rgb(0x3E3B3F),
    hairline_strong: rgb(0x4F4A4E),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 110),
};

pub fn palette_of(c: Colorway) -> &'static Palette {
    match c {
        Colorway::ButterPaper => &BUTTER_PAPER,
        Colorway::SouthwestNight => &SOUTHWEST_NIGHT,
        Colorway::NordicNight => &NORDIC_NIGHT,
    }
}

/// The active colorway, process-wide. Render code is immediate-mode and
/// spread over many pure helpers (fix colors, captions) that have no
/// `Context`; one atomic read per token keeps them pure while letting the
/// switcher restyle the next frame. Only `set_colorway` writes it, and
/// tests never do, so they all see Butter Paper.
static ACTIVE: AtomicU8 = AtomicU8::new(0);

pub fn colorway() -> Colorway {
    match ACTIVE.load(Ordering::Relaxed) {
        1 => Colorway::SouthwestNight,
        2 => Colorway::NordicNight,
        _ => Colorway::ButterPaper,
    }
}

pub fn palette() -> &'static Palette {
    palette_of(colorway())
}

macro_rules! tokens {
    ($($name:ident),* $(,)?) => {
        $(
            #[doc = concat!("The active colorway's `", stringify!($name), "`.")]
            pub fn $name() -> Color32 {
                palette().$name
            }
        )*
    };
}

tokens!(
    paper_0,
    paper_2,
    ink_primary,
    ink_secondary,
    ink_muted,
    accent,
    success,
    success_tint,
    warning,
    danger,
);

/// Shared corner radius: 6 px on widgets, 8 px on cards and windows.
const RADIUS_WIDGET: CornerRadius = CornerRadius::same(6);
const RADIUS_CARD: CornerRadius = CornerRadius::same(8);

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

/// Startup: fonts, type scale, and the saved colorway.
pub fn apply(ctx: &egui::Context, c: Colorway) {
    install_fonts(ctx);
    install_text_styles(ctx);
    set_colorway(ctx, c);
}

/// Switch the whole app to colorway `c` from the next frame on. Both egui
/// theme slots get the same visuals: the colorway is the user's explicit
/// choice, so an OS light/dark flip must not restyle the app mid-session.
pub fn set_colorway(ctx: &egui::Context, c: Colorway) {
    let id = match c {
        Colorway::ButterPaper => 0,
        Colorway::SouthwestNight => 1,
        Colorway::NordicNight => 2,
    };
    ACTIVE.store(id, Ordering::Relaxed);
    let v = visuals_for(palette_of(c));
    ctx.set_visuals_of(egui::Theme::Light, v.clone());
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.request_repaint();
}

/// The next colorway in the switcher's cycle, the docs/forum order:
/// Butter Paper -> Southwest Night -> Nordic Night -> Butter Paper.
pub fn next_colorway(c: Colorway) -> Colorway {
    match c {
        Colorway::ButterPaper => Colorway::SouthwestNight,
        Colorway::SouthwestNight => Colorway::NordicNight,
        Colorway::NordicNight => Colorway::ButterPaper,
    }
}

/// The full egui mapping of one palette.
///
/// egui shares one widget style between button text, checkbox/radio labels
/// and combo text, so the global interactive style is the guide's
/// "secondary" button: the `accent_tint` face, the muted control outline,
/// ink text - with the accent arriving on hover, focus and selection. Solid
/// primary actions are opted into per-widget via [`accent_button`].
///
/// Why the face is not the editable plate: the inactive `weak_bg_fill`
/// paints every Button and ComboBox face, and while it was white an
/// unfocused "Save" or "Delete" was pixel-identical to an empty text field
/// (same plate, same border) - a toolbar read as a legend, not as controls.
/// Text fields keep the plate through `text_edit_bg_color`; DragValue
/// number fields (which egui builds from a Button) restore it via
/// [`number_field`].
pub fn visuals_for(p: &Palette) -> egui::Visuals {
    use egui::style::{Selection, TextCursorStyle, WidgetVisuals};

    let mut v = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.dark_mode = p.dark;
    v.override_text_color = None;

    v.widgets.noninteractive = WidgetVisuals {
        // Windows fall back to this fill; hairline strokes also draw
        // separators and default frame outlines.
        bg_fill: p.paper_2,
        weak_bg_fill: p.paper_2,
        bg_stroke: Stroke::new(1.0, p.hairline),
        corner_radius: RADIUS_WIDGET,
        fg_stroke: Stroke::new(1.0, p.ink_primary),
        expansion: 0.0,
    };
    v.widgets.inactive = WidgetVisuals {
        // bg_fill paints checkbox/radio interiors - editable, so the plate.
        bg_fill: p.paper_3,
        // weak_bg_fill is the Button/ComboBox face: clickable, not typeable.
        weak_bg_fill: p.accent_tint,
        // Opaque control outline, not a hairline: an unfocused input's
        // plate is barely off the paper, so the border is the sole "this is
        // editable/clickable" boundary and must clear the WCAG 1.4.11
        // non-text 3:1 floor. Decorative hairlines stay quiet.
        bg_stroke: Stroke::new(1.0, p.ink_muted),
        corner_radius: RADIUS_WIDGET,
        fg_stroke: Stroke::new(1.0, p.ink_primary),
        expansion: 0.0,
    };
    v.widgets.hovered = WidgetVisuals {
        // Hover = tint fill plus the accent "this is interactive" ring.
        bg_fill: p.paper_tint,
        weak_bg_fill: p.paper_tint,
        bg_stroke: Stroke::new(1.0, p.accent),
        corner_radius: RADIUS_WIDGET,
        fg_stroke: Stroke::new(1.5, p.ink_primary),
        expansion: 1.0,
    };
    v.widgets.active = WidgetVisuals {
        // Pressed and focused: the ring thickens into a focus ring.
        // fg_stroke doubles as `strong_text_color`, so it stays ink.
        bg_fill: p.paper_tint,
        weak_bg_fill: p.paper_tint,
        bg_stroke: Stroke::new(1.5, p.accent_hover),
        corner_radius: RADIUS_WIDGET,
        fg_stroke: Stroke::new(2.0, p.ink_primary),
        expansion: 1.0,
    };
    v.widgets.open = WidgetVisuals {
        bg_fill: p.paper_tint,
        weak_bg_fill: p.paper_tint,
        bg_stroke: Stroke::new(1.0, p.accent),
        corner_radius: RADIUS_WIDGET,
        fg_stroke: Stroke::new(1.0, p.ink_primary),
        expansion: 0.0,
    };

    // Text selection: the accent tint, NOT the paper tint. egui uses
    // `selection` for dragged-over text in inputs and labels, where the
    // paper tint barely differs from the editable plate.
    v.selection = Selection {
        bg_fill: p.accent_tint,
        stroke: Stroke::new(1.0, p.accent),
    };
    v.hyperlink_color = p.accent;

    v.panel_fill = p.paper_1;
    v.window_fill = p.paper_2;
    v.window_stroke = Stroke::new(1.0, p.hairline_strong);
    v.window_corner_radius = RADIUS_CARD;
    // Dialogs and popups are the only true overlays, so they keep a shadow.
    v.window_shadow.color = p.shadow;
    v.popup_shadow.color = p.shadow;
    v.menu_corner_radius = RADIUS_WIDGET;

    v.faint_bg_color = p.paper_0; // striped rows
    v.extreme_bg_color = p.paper_0; // wells: plot bed, scrollbar gutter
    v.text_edit_bg_color = Some(p.paper_3); // the plate = editable
    v.code_bg_color = p.paper_0;

    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;

    // Left unset, egui synthesizes weak text (RichText::weak, TextEdit hint
    // text) by fading the primary ink - under the AA floor. Weak text is
    // still text the user reads, so it takes the secondary ink.
    v.weak_text_color = Some(p.ink_secondary);

    v.text_cursor = TextCursorStyle {
        stroke: Stroke::new(2.0, p.accent),
        ..v.text_cursor
    };
    // The accent says clickable; the pointer cursor repeats it.
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    v
}

/// A card with a hairline border: the styled replacement for `ui.group(..)`
/// on group boxes and floating panels.
pub fn card(style: &egui::Style) -> egui::Frame {
    egui::Frame::group(style)
        .fill(palette().paper_2)
        .stroke(Stroke::new(1.0, palette().hairline))
        .corner_radius(RADIUS_CARD)
        .inner_margin(8)
}

/// The recessed bed under monospace log/code content: read-only diagnostic
/// text steps one elevation below its panel onto the theme's extreme
/// background (`paper_0`, the same bed the plot uses via `extreme_bg_color`)
/// inside a hairline border. Every monospace log surface (event log,
/// connection log, hex dump, unparsed sourcetable lines) must sit in this
/// well; a bed that floats flat on card paper breaks the depth model. Colors
/// come from the active `Visuals` so the well tracks the installed theme.
pub fn well_frame(visuals: &egui::Visuals) -> egui::Frame {
    egui::Frame::new()
        .fill(visuals.extreme_bg_color)
        .stroke(visuals.widgets.noninteractive.bg_stroke)
        .inner_margin(4)
}

/// Paint the current table-header cell's background as the recessed
/// `paper_0` band, one step BELOW the striped body rows, so the elevation
/// model never inverts at the top of a table (pinned by
/// `header_band_is_recessed_below_window_and_stripes`). Each cell expands
/// by half the item spacing so adjacent cells tile into one continuous band.
/// Call first inside a `header.col(..)` closure, before laying out the label.
pub fn header_band(ui: &egui::Ui) {
    let sp = ui.spacing().item_spacing;
    let rect = ui.max_rect().expand2(egui::vec2(sp.x / 2.0, sp.y / 2.0));
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, palette().paper_0);
}

/// Persistent danger banner (e.g. TLS verification disabled): danger rule
/// over the danger tint. Square corners - it is a full-width strip, not a
/// floating card.
pub fn danger_banner() -> egui::Frame {
    let p = palette();
    egui::Frame::new()
        .fill(p.danger_tint)
        .stroke(Stroke::new(1.0, p.danger))
        .inner_margin(egui::Margin::symmetric(8, 4))
}

/// Solid-accent primary action button (Connect, Save...): the one place a
/// widget opts out of the tinted "secondary" surface. Disabled rendering is
/// egui's own opacity pass, so the fill dims correctly.
pub fn accent_button(text: impl Into<String>) -> egui::Button<'static> {
    let p = palette();
    egui::Button::new(egui::RichText::new(text.into()).color(p.on_accent).strong()).fill(p.accent)
}

/// Solid destructive button (the confirm step of a delete). Only ever the
/// second, deliberate click - never a toolbar's first action.
pub fn danger_button(text: impl Into<String>) -> egui::Button<'static> {
    let p = palette();
    egui::Button::new(egui::RichText::new(text.into()).color(p.on_danger).strong())
        .fill(p.danger_strong)
}

/// A DragValue that keeps the editable plate. egui builds DragValue from a
/// Button, so the global button face would otherwise tint it like a
/// clickable control; a number field is an input. Every DragValue goes
/// through here (pinned by `number_fields_stay_editable`).
pub fn number_field(ui: &mut egui::Ui, field: egui::DragValue<'_>) -> egui::Response {
    ui.scope(|ui| {
        ui.visuals_mut().widgets.inactive.weak_bg_fill = palette().paper_3;
        ui.add(field)
    })
    .inner
}

/// Warning callout: warning rule over the warning tint, for a state the
/// user must see before acting (e.g. "disconnect to switch profiles").
/// Put primary ink inside, not the warning ink: orange text on the paper
/// tint is sub-AA.
pub fn warning_callout() -> egui::Frame {
    let p = palette();
    egui::Frame::new()
        .fill(p.warning_tint)
        .stroke(Stroke::new(1.0, p.warning))
        .corner_radius(RADIUS_WIDGET)
        .inner_margin(egui::Margin::symmetric(8, 5))
}

/// The bed a selectable list sits on inside a dialog: one paper step below
/// the `paper_2` dialog (so the list reads as a region, not floating rows),
/// framed by the strong hairline. Not the editable plate - nothing in it is
/// typed into except an inline rename, which brings its own field.
pub fn list_bed() -> egui::Frame {
    let p = palette();
    egui::Frame::new()
        .fill(p.paper_1)
        .stroke(Stroke::new(1.0, p.hairline_strong))
        .corner_radius(RADIUS_WIDGET)
        .inner_margin(4)
}

/// Background for one list row: the tint for hover and selection, plus -
/// on the selected row only - an accent outline and left bar, so "which row
/// will the buttons act on" survives a glance. Returned as a shape so a row
/// can lay out its content first and slot this beneath.
pub fn list_row_background(rect: egui::Rect, selected: bool, hovered: bool) -> egui::Shape {
    let p = palette();
    let mut shapes = Vec::new();
    if selected || hovered {
        shapes.push(egui::Shape::rect_filled(rect, RADIUS_WIDGET, p.paper_tint));
    }
    if selected {
        shapes.push(egui::Shape::rect_stroke(
            rect,
            RADIUS_WIDGET,
            Stroke::new(1.0, p.accent),
            egui::StrokeKind::Inside,
        ));
        let bar = egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height()));
        shapes.push(egui::Shape::rect_filled(bar, CornerRadius::ZERO, p.accent));
    }
    egui::Shape::Vec(shapes)
}

/// Card heading for one step of the setup flow: a quiet step number, then
/// the title a size above body text so it never reads as a field label, and
/// an "optional" badge on a step the user may skip. The number teaches the
/// order of the flow without a sentence of prose.
pub fn section_heading(ui: &mut egui::Ui, step: &str, title: &str, optional: bool) {
    let p = palette();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(step).color(p.ink_secondary));
        ui.label(egui::RichText::new(title).size(16.0).color(p.ink_primary));
        if optional {
            badge(ui, "optional", p.ink_secondary, p.paper_0);
        }
    });
}

/// Small secondary-ink text: notes, reasons under a disabled control,
/// sub-labels - the one style for "read this, but it is not the value".
pub fn caption(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .small()
        .color(palette().ink_secondary)
}

/// Small pill badge (e.g. "ACTIVE") in a semantic ink over its tint.
pub fn badge(ui: &mut egui::Ui, text: &str, ink: Color32, tint: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(tint)
        .stroke(Stroke::new(1.0, ink))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(egui::Margin::symmetric(6, 0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).small().strong().color(ink))
        })
        .response
}

/// A disclosure header: one clickable row - painted chevron (right closed,
/// down open), label in primary ink, then the summary in small
/// `summary_ink`. The summary is what lets a collapsed section state itself
/// (docs/one-surface.md); it truncates rather than pushing the row wider
/// than the space it was given. Sized to its content, so it can lead a row
/// that carries other controls. Clicking toggles `open`.
pub fn disclosure(
    ui: &mut egui::Ui,
    open: &mut bool,
    label: &str,
    summary: &str,
    summary_ink: Color32,
) -> egui::Response {
    let p = palette();
    let pad = 4.0;
    let chevron = 14.0;
    let gap = ui.spacing().item_spacing.x;
    let label_g = egui::WidgetText::from(egui::RichText::new(label).color(p.ink_primary))
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        );
    let fixed = pad + chevron + label_g.size().x + pad;
    let summary_g = (!summary.is_empty()).then(|| {
        let room = (ui.available_width() - fixed - gap).max(0.0);
        egui::WidgetText::from(egui::RichText::new(summary).small().color(summary_ink)).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            room,
            egui::TextStyle::Small,
        )
    });
    let width = fixed + summary_g.as_ref().map_or(0.0, |g| gap + g.size().x);
    let height = ui.spacing().interact_size.y;
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    if resp.clicked() {
        *open = !*open;
        resp.mark_changed();
    }
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, *open, label)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, RADIUS_WIDGET, p.paper_tint);
        }
        let mid = rect.center().y;
        // Right while closed, down while open: the platform convention for
        // "expand" vs "collapse", painted so it never depends on a glyph.
        let c = egui::pos2(rect.left() + pad + chevron / 2.0, mid);
        let (s, h) = (3.5, 1.75);
        let points = if *open {
            vec![
                c + egui::vec2(-s, -h),
                c + egui::vec2(0.0, h),
                c + egui::vec2(s, -h),
            ]
        } else {
            vec![
                c + egui::vec2(-h, -s),
                c + egui::vec2(h, 0.0),
                c + egui::vec2(-h, s),
            ]
        };
        painter.line(points, Stroke::new(1.5, p.ink_secondary));
        let mut x = rect.left() + pad + chevron;
        painter.galley(
            egui::pos2(x, mid - label_g.size().y / 2.0),
            label_g.clone(),
            p.ink_primary,
        );
        x += label_g.size().x + gap;
        if let Some(g) = summary_g {
            painter.galley(egui::pos2(x, mid - g.size().y / 2.0), g, summary_ink);
        }
    }
    resp
}

/// The appearance switcher: one icon button showing the CURRENT colorway
/// (sheet, moon over mesa, moon over pines - the docs/forum icons) that
/// steps to the next on click, with the sites' tooltip wording. Returns the
/// colorway the user switched to, if any; the caller persists it and
/// applies it (set_colorway) once the frame is drawn.
pub fn colorway_switch(ui: &mut egui::Ui) -> Option<Colorway> {
    let current = colorway();
    let next = next_colorway(current);
    let size = egui::vec2(28.0, ui.spacing().interact_size.y.max(24.0));
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let resp = resp.on_hover_text(format!(
        "Appearance: {}. Click for {}.",
        palette_of(current).name,
        palette_of(next).name
    ));
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!("Switch to {}", palette_of(next).name),
        )
    });
    if ui.is_rect_visible(rect) {
        let w = ui.style().interact(&resp);
        ui.painter().rect(
            rect.expand(w.expansion),
            w.corner_radius,
            w.weak_bg_fill,
            w.bg_stroke,
            egui::StrokeKind::Inside,
        );
        let side = 20.0; // the sites' render size
        let icon = egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
        super::icons::paint_colorway(ui.painter(), icon, current, palette().ink_primary);
    }
    // The caller applies it at the end of the frame: switching here would
    // paint the rest of this frame in new tokens over the old visuals.
    resp.clicked().then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [&Palette; 3] = [&BUTTER_PAPER, &SOUTHWEST_NIGHT, &NORDIC_NIGHT];

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

    /// Contrast is checked against the base (`paper_1`). Ink and every
    /// semantic/interactive foreground must clear WCAG AA (4.5:1) on it, in
    /// every colorway.
    #[test]
    fn foregrounds_clear_aa_on_base_paper() {
        for p in ALL {
            for (name, c) in [
                ("ink-primary", p.ink_primary),
                ("ink-secondary", p.ink_secondary),
                ("accent", p.accent),
                ("accent-hover", p.accent_hover),
                ("success", p.success),
                ("warning", p.warning),
                ("danger", p.danger),
            ] {
                let ratio = contrast(c, p.paper_1);
                assert!(ratio >= 4.5, "{}: {name} is {ratio:.2}:1", p.name);
            }
            // Text on the solid fills.
            assert!(contrast(p.on_accent, p.accent) >= 4.5, "{}", p.name);
            assert!(contrast(p.on_danger, p.danger_strong) >= 4.5, "{}", p.name);
        }
    }

    /// The surface stack must strictly lighten toward the viewer - that
    /// ordering IS the elevation model, on paper and at night alike.
    #[test]
    fn paper_stack_lightens_upward() {
        for p in ALL {
            let steps = [p.paper_0, p.paper_1, p.paper_2, p.paper_3];
            for pair in steps.windows(2) {
                assert!(luminance(pair[0]) < luminance(pair[1]), "{}", p.name);
            }
        }
    }

    /// The editable plate is reserved for editable surfaces; the widget
    /// mapping must not leak it anywhere else, and text must never be pure
    /// black or white.
    #[test]
    fn visuals_honor_the_paper_rules() {
        for p in ALL {
            let v = visuals_for(p);
            assert_eq!(v.dark_mode, p.dark);
            assert_eq!(v.text_edit_bg_color, Some(p.paper_3));
            assert_ne!(v.panel_fill, p.paper_3);
            assert_ne!(v.window_fill, p.paper_3);
            assert_ne!(v.extreme_bg_color, p.paper_3);
            let ink = v.widgets.noninteractive.fg_stroke.color;
            assert_ne!(ink, Color32::BLACK);
            assert_ne!(ink, Color32::WHITE);
            assert_eq!(v.hyperlink_color, p.accent);
            assert_eq!(v.selection.bg_fill, p.accent_tint);
            assert_eq!(v.widgets.hovered.bg_fill, p.paper_tint);
            assert_eq!(v.widgets.inactive.bg_stroke.color, p.ink_muted);
            // Weak text must not fall back to egui's synthesized fade.
            assert_eq!(v.weak_text_color, Some(p.ink_secondary));
        }
    }

    /// Paper-only rule: warning is signal orange, never yellow, because
    /// yellow vanishes on cream. (The nights' caution yellow is correct.)
    #[test]
    fn paper_warning_is_never_yellow() {
        let w = BUTTER_PAPER.warning;
        assert!(
            w.r() > w.g() * 2,
            "paper warning must read orange, not yellow"
        );
    }

    /// The well must step down to the same bed the plot uses
    /// (extreme_bg_color) with the theme's hairline stroke.
    #[test]
    fn well_uses_extreme_bg_and_hairline_in_every_colorway() {
        for p in ALL {
            let visuals = visuals_for(p);
            let frame = well_frame(&visuals);
            assert_eq!(frame.fill, visuals.extreme_bg_color);
            assert_eq!(frame.stroke, visuals.widgets.noninteractive.bg_stroke);
            assert_ne!(
                frame.fill, visuals.panel_fill,
                "the well must visually step off the panel"
            );
        }
    }

    /// The table contract: the header band is a RECESSED paper-0 fill,
    /// darker than the card and no lighter than the body stripes.
    #[test]
    fn header_band_is_recessed_below_window_and_stripes() {
        for p in ALL {
            assert!(luminance(p.paper_0) < luminance(p.paper_2));
            let stripes = visuals_for(p).faint_bg_color;
            assert!(luminance(p.paper_0) <= luminance(stripes));
        }
    }

    /// Every monospace log/code bed sits in the shared well. Source-scan,
    /// since frame styling leaves no other testable trace.
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

    /// The muted ink's two legal roles - disabled-control ink and widget
    /// outlines - are non-text contexts, so it must clear the WCAG 3:1
    /// floor on every surface it can border or sit on, in every colorway.
    #[test]
    fn muted_clears_non_text_floor_everywhere() {
        for p in ALL {
            for (name, paper) in [
                ("paper-0", p.paper_0),
                ("paper-1", p.paper_1),
                ("paper-2", p.paper_2),
                ("paper-3", p.paper_3),
                ("paper-tint", p.paper_tint),
                ("button face", p.accent_tint),
            ] {
                let ratio = contrast(p.ink_muted, paper);
                assert!(
                    ratio >= 3.0,
                    "{}: ink-muted is {ratio:.2}:1 on {name}",
                    p.name
                );
            }
        }
    }

    /// The muted ink is sub-AA by design, so no render code may paint
    /// readable text with it: outside this file it must not appear at all.
    /// A future legitimate use extends the allowlist here, deliberately.
    ///
    /// Allowlisted: status_strip.rs strokes the activity gauge's well with
    /// it - the token's non-text-boundary role, never text.
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
            if text.contains("ink_muted") || text.contains("INK_MUTED") {
                offenders.push(path);
            }
        }
        assert!(
            offenders.is_empty(),
            "muted ink referenced outside theme.rs: {offenders:?}"
        );
    }

    /// Paper's text-selection band must be COOL: the warm papers all have
    /// R >= B, so a blue-leaning tint stays distinguishable by hue.
    #[test]
    fn paper_selection_band_is_cool_on_warm_paper() {
        let p = &BUTTER_PAPER;
        assert!(p.accent_tint.b() > p.accent_tint.r());
        for paper in [p.paper_0, p.paper_1, p.paper_2, p.paper_tint] {
            assert!(paper.r() >= paper.b());
        }
        assert!(contrast(p.accent_tint, p.paper_3) > contrast(p.paper_tint, p.paper_3));
    }

    /// Buttons must not look like inputs: the face every Button/ComboBox
    /// paints is not the editable plate, sits off the base and the card,
    /// and keeps ink text at high contrast - in every colorway.
    #[test]
    fn buttons_read_as_buttons_not_inputs() {
        for p in ALL {
            let v = visuals_for(p);
            let face = v.widgets.inactive.weak_bg_fill;
            assert_ne!(
                Some(face),
                v.text_edit_bg_color,
                "{}: a button is not a field",
                p.name
            );
            assert_ne!(face, v.panel_fill);
            assert_ne!(face, v.window_fill);
            // Checkbox/radio interiors (bg_fill) stay the editable plate.
            assert_eq!(v.widgets.inactive.bg_fill, p.paper_3);
            assert!(contrast(p.ink_primary, face) >= 7.0, "{}", p.name);
            // Badge ink on its tint; callout text is primary ink on the
            // warning tint, the warning rule carries the signal (3:1).
            assert!(contrast(p.success, p.success_tint) >= 4.5, "{}", p.name);
            assert!(contrast(p.ink_primary, p.warning_tint) >= 7.0, "{}", p.name);
            assert!(contrast(p.warning, p.warning_tint) >= 3.0, "{}", p.name);
        }
    }

    /// Every DragValue keeps the editable plate through `number_field`
    /// (egui paints DragValue with the button face). Source-scan.
    #[test]
    fn number_fields_stay_editable() {
        let ui_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        for entry in std::fs::read_dir(&ui_dir).expect("src/ui exists") {
            let path = entry.expect("dir entry").path();
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("theme.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("source is UTF-8");
            let built = text.matches("DragValue::new").count();
            let routed = text.matches("number_field(").count();
            assert!(
                built <= routed,
                "{path:?} builds {built} DragValue(s), routes {routed} via theme::number_field"
            );
        }
    }

    /// The switcher cycles all three colorways in the docs/forum order and
    /// comes back around; every colorway maps to its own palette.
    #[test]
    fn colorway_cycle_visits_every_palette() {
        let mut c = Colorway::ButterPaper;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(palette_of(c).name);
            c = next_colorway(c);
        }
        assert_eq!(c, Colorway::ButterPaper);
        assert_eq!(seen, ["Butter Paper", "Southwest Night", "Nordic Night"]);
        // Tests never switch the global, so render helpers see paper.
        assert_eq!(colorway(), Colorway::ButterPaper);
    }

    /// Paper values are the ButterPaper v2 tokens verbatim.
    #[test]
    fn paper_matches_butterpaper_v2() {
        let p = &BUTTER_PAPER;
        assert_eq!(p.paper_1, rgb(0xFFF7E0));
        assert_eq!(p.ink_primary, rgb(0x2B2417));
        assert_eq!(p.accent, rgb(0x1E40AF));
        assert_eq!(p.ink_muted, rgb(0x897F6B));
        assert_eq!(SOUTHWEST_NIGHT.accent, rgb(0xF0A27A));
        assert_eq!(NORDIC_NIGHT.accent, rgb(0xFF914D));
    }
}
