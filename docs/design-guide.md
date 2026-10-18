# ButterPaper - the Open NTRIP Client design system

The GUI uses the [ButterPaper](https://github.com/intnsity/ButterPaper) v2 token set in
three colorways, switched from the appearance icon on the profile bar (the same sheet /
moon-over-mesa / moon-over-pines icons as docs.igage.com and forum.igage.com):

| Colorway | id | Character |
|---|---|---|
| Butter Paper (default) | `paper` | warm cream paper, brown ink, cobalt interaction |
| Southwest Night | `desert` | desert dusk browns, bloom-orange interaction, caution-yellow warnings |
| Nordic Night | `nordic` | cabin greys, ember-orange interaction; no navy, teal or cobalt surfaces |

The single source of truth for values is `crates/app/src/ui/theme.rs`: one `Palette` per
colorway plus `visuals_for(palette)`. Render code reads tokens only through the accessor
functions (`theme::ink_secondary()`, `theme::accent()`, ...), which answer for the active
colorway; source-scan tests enforce that no window names a raw color outside that module, and
every contrast rule below is tested against all three palettes. This document records the
rules; the tables give the Butter Paper values (the night values are ButterPaper v2's
`desert` / `nordic` tokens, verbatim).

## Paper stack (depth by tint, never by shadow)

| Layer | Color | Used for |
|---|---|---|
| paper-0 (recessed) | `#FAF3D9` | wells: log panes, plot background, table headers |
| paper-1 (base) | `#FFF7E0` | window and panel background |
| paper-2 (raised) | `#FFFBEB` | group boxes, cards, secondary windows |
| white | `#FFFFFF` | editable fields only - white always means "you can type here" |
| tint | `#FFF4C9` | hover and selection states |

Depth comes from stepping through these tints; drop shadows are not used.

## Ink (never pure black)

| Token | Color | Used for |
|---|---|---|
| ink-primary | `#2B2417` | body text, values, headings |
| ink-secondary | `#6B6250` | labels, hints, placeholders, weak text |
| ink-muted | `#897F6B` | control outlines and disabled text only (ButterPaper `control-border`) |

Muted ink must clear the 3:1 non-text floor on every surface it borders, in every colorway,
and is test-enforced to stay out of readable content.

## Accent and semantics

- Interactive (buttons, links, focus, active tab, plot line): cobalt `#1E40AF`,
  hover `#1B3691`. Cobalt always means "you can click this".
- Success / RTK Fixed: emerald `#047857`.
- Warning / RTK Float: signal orange `#C2410C`. On paper, warning is NEVER yellow - yellow
  vanishes on cream. (The night colorways use caution yellow `#FFD447`, which is their correct
  warning.)
- Danger / Invalid fix / TLS-insecure banner: crimson `#BE123C` on a danger tint.
- Borders: warm hairline `rgba(43,36,23,0.14)`.

## Controls: buttons are not fields

White means "you can type here", so a clickable control must never be a white plate.

| Control | Look | Source |
|---|---|---|
| Text field, number field | white, muted-ink border | `text_edit_bg_color`; DragValues via `theme::number_field` |
| Secondary button, picker (combo) | cool accent-tint face `#E4EBFA`, muted-ink border, ink text | `widgets.inactive.weak_bg_fill` |
| Primary button (one per surface: Connect on the main window, the action in a dialog) | solid accent, `on_accent` text | `theme::accent_button` |
| Pending save (Save changes) | accent outline and accent ink on the button face - visible, never competing with Connect | `App::save_control` |
| Destructive confirm | solid danger fill, only inside a confirmation | `theme::danger_button` |
| Selected list row | paper tint, cobalt outline and left bar | `theme::list_row_background` |

The tint shifts hue rather than lightness, so it separates from every warm paper and from white
at a glance. egui builds DragValue from a Button, so number fields must go through
`theme::number_field` to keep the editable plate (source-scan test `number_fields_stay_editable`).
On the night colorways the editable plate is the raised surface, not white; the rule is the
same - the plate means "you can type here" and nothing clickable wears it.
Warning callouts (`theme::warning_callout`) use primary ink on the warning tint with an orange
rule: orange text on that tint is only ~4.4:1.

## Typography

The interface is set in **IBM Plex Sans** (Text weight) for proportional text and **IBM Plex
Mono** for the monospace beds (event log, Connection Log hex dump, the RTCM and sourcetable
tables). Both are OFL-1.1 and embedded in the binary from `assets/fonts/`, installed by
`theme::apply` ahead of the first frame with egui's default fonts kept as glyph fallbacks
(emoji, non-Latin scripts). Type scale: body 13.5, small 11, heading 18, monospace 12.5.
`strong()` recolors rather than re-weights (egui's model), so one weight per family carries the
whole UI.

## Accessibility contract

Every ink and accent token must clear WCAG AA (4.5:1 body, 3:1 large/secondary)
against EVERY paper layer it can appear on, checked against the worst case
(paper-0 `#FAF3D9`). Measured at v0.1.0: ink-primary 13.8:1+, ink-secondary
5.4:1+, accent 7.8:1+, success 4.9:1+, warning 4.7:1+, danger 5.7:1+ on all five
backgrounds. `theme.rs` pins `weak_text_color = ink-secondary` so egui never
synthesizes a sub-AA gray. If you change a token, re-verify these ratios.
