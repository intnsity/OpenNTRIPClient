# Butter Paper - the Open NTRIP Client design system

The GUI uses a warm light theme called Butter Paper. The single source of truth for
values is `crates/app/src/ui/theme.rs` (token constants + `visuals()`); source-scan
tests enforce that no window references a raw color outside that module. This
document records the rules those tokens implement so contributors can change the
theme without breaking its logic.

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
| ink-muted | `#8F8570` | disabled text and hairline-adjacent chrome only |

Note: the original guide proposed `#9C927C` for muted ink; it measures below the
3:1 non-text floor on this paper, so the shipped token is darkened to `#8F8570`.
Muted ink is test-enforced to stay out of readable content.

## Accent and semantics

- Interactive (buttons, links, focus, active tab, plot line): cobalt `#1E40AF`,
  hover `#1B3691`. Cobalt always means "you can click this".
- Success / RTK Fixed: emerald `#047857`.
- Warning / RTK Float: signal orange `#C2410C`. Warning is NEVER yellow - yellow
  vanishes on this paper.
- Danger / Invalid fix / TLS-insecure banner: crimson `#BE123C` on a danger tint.
- Borders: warm hairline `rgba(43,36,23,0.14)`.

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
