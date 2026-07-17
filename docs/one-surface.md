# One Surface - the Open NTRIP Client layout law

This is a diagnostic tool. A support tech opens it to answer one question - "why is this
correction stream not working right now" - often while a customer waits on the phone. Every
second spent hunting through menus or juggling floating windows is a second not spent reading
the diagnosis. So the client obeys one rule:

> **Everything relevant to the current session is visible on one surface, without opening a
> window or hunting through a menu.**

Two consequences follow, and they are the whole design:

1. **Fewer clicks.** Nothing the tech needs mid-session lives behind a button that summons a
   window. Live data is always on screen; settings that are not live data may be dialogs.
2. **Features on the front page.** Capabilities that were buried behind poorly-named menus in
   the original client are surfaced where they are used. The GGA position control is the
   headline example: it decided whether a caster would silently starve a client, yet it hid
   two windows deep behind a "Details..." button. It now sits in the caster column.

The `main_window::ui` composition is the single source of truth for the regions below; this
document records the intent so the layout can be changed without losing the reasoning.

## The surface, top to bottom

| Region | Module | What it is |
|---|---|---|
| Profile strip | `main_window::profile_strip` | Profile picker, Save, Manage, and the two remaining dialogs (Options, About). |
| TLS-insecure banner | `main_window` (conditional) | Crimson, undismissable while a live connection skips certificate verification. |
| Config columns | `serial_block` \| `ntrip_block` | Two side-by-side cards: the serial receiver and the NTRIP caster. |
| GGA disclosure | `gga_section` | Collapsible, in the NTRIP column. Position reporting - the feature brought to the front page. |
| Status strip | `status_strip` | Fix readout, two configurable slots, and the stream-activity bar. |
| Stream summary | `stream_summary` | One clickable line: base identity, position, type/rate, framing health. Opens the Stream tab. |
| Bottom tab pane | `bottom_tabs` | The four always-present diagnostic surfaces (below). |
| Graph disclosure | `plot_panel` | Collapsible elevation chart + RX-rate readout. |

## What folded, and why

The original client (and this client's own first cut) scattered live diagnostics across
floating windows the tech had to summon, position, and dismiss. One Surface folded every one
of them into the surface:

- **Connection Log window -> Conn tab.** The verbatim protocol exchange.
- **RTCM Inspector window -> Stream detail tab.** Per-type statistics and decoded base data.
- **Sourcetable Browser window -> Sourcetable tab.** Browse/filter/sort/pick a mountpoint.
- **Event log (was an inline pane) -> Events tab.** Kept, now one tab among the four.
- **Details dialog + Location Picker window -> `gga_section` disclosure.** GGA mode and source,
  the manual position, and the offline city/ZIP search, unified into one inline editor. The
  fold also removed a redundancy: the two windows had each edited the manual position their own
  way (validated text fields feeding an `Action`, and `DragValue`s on the profile). The single
  surface has one editor - range-clamped `DragValue`s bound straight to the profile - and the
  search and "use receiver position" simply write those same two floats.

Only genuine settings/informational surfaces stayed as dialogs: **Options**, **About**, and the
**Profiles Manager**. They are not live session data, and they are rare enough that a window is
the honest weight for them.

## Laws the pieces obey

- **The four tabs are always present.** They are offered even when empty - an empty tab teaches
  the fetch path (e.g. the Sourcetable tab says how to load one), and a fixed set means the
  strip never changes width or reorders under the pointer. The active tab is
  `settings.window.tab`; the empty states are honest, never a blank void.
- **Disclosures state themselves while collapsed.** The GGA section shows a one-line summary of
  its configuration ("when required, manual 45.52, -122.67") without being opened, so a glance
  answers "will this send a position" without a click.
- **Summaries are clickable shortcuts, not dead text.** The stream-summary row opens the Stream
  detail tab; it exists only while a live session has produced frames, and yields its height
  back otherwise (no idle placeholder).
- **Attention comes to the strip, it is not summoned.** When the worker captures an
  unclassified caster response (often an HTML error page where RTCM was expected), a warning
  dot appears on the Conn tab until the tech opens it. See the badge model below.
- **Per-tab actions stay in their tab.** Copy/Clear belong to the surface they act on; there is
  no strip-level action cluster, because the four tabs' Copy buttons have four different scopes.

## Backing data model

The layout persists as UI geometry, not as user configuration - it rides the unconditional exit
save and is excluded from `Settings::persistable_eq`, so switching tabs or toggling a disclosure
never lights the profile-strip Save button.

| Field | Meaning |
|---|---|
| `settings.window.tab: BottomTab` | Active bottom tab (`Events`/`Conn`/`Stream`/`Sourcetable`). |
| `settings.window.gga_open: bool` | GGA disclosure open state. |
| `settings.window.graph_open: bool` | Elevation-graph disclosure open state. |

**The Conn attention badge** is a two-counter handshake, so "something unusual arrived" survives
across stream resets without a live callback:

- `state.ntrip.unknown_response_gen` bumps once per captured unclassified response and is
  deliberately *not* reset by `reset_stream_stats` (the evidence outlives the session).
- `App.conn_unknown_ack` is the generation the tech has seen; `bottom_tabs` stamps it to the
  current generation on every frame the Conn tab is visible.
- The dot shows while `unknown_response_gen > conn_unknown_ack` and the Conn tab is not active.
  Opening the tab acks the generation and clears the dot.

## Related decisions

- Sourcetables are held in memory only - no `SourceTables\` disk cache. A cached table can only
  give a stale answer to a "is this caster alive now" question, so it is fetched fresh each
  session. See the sourcetable browser's empty-state text and `workers::ntrip::on_sourcetable`.
- Colors and depth come only from `theme.rs` (Butter Paper); see `docs/design-guide.md`.
