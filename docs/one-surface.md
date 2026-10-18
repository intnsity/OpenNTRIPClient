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

The surface reads **set up, verdict, evidence**. Step 1 (the caster) is read first and holds
the only solid primary button; step 2 (the receiver) sits beside it, marked optional; the
session band directly under them answers "is it working?"; everything below is the evidence.
This is Lefebure's three-button simplicity - pick a stream, connect, see it flow - with the
power one glance or one click away, never removed.

| Region | Module | What it is |
|---|---|---|
| Profile bar | `main_window::profile_strip` | Profile picker (activates on pick; hover shows each caster), the shared Save control, Save as profile, Manage profiles, the appearance switcher, Options, About, and "Disconnect to switch profiles" while connected. |
| TLS-insecure banner | `main_window` (conditional) | Crimson, undismissable while a live connection skips certificate verification. |
| Setup columns | `ntrip_block` \| `serial_block` | "1 Caster" (with the one state-honest primary: Connect / Get sourcetable / Disconnect) and "2 Receiver (optional)" with its Port settings disclosure. |
| GGA disclosure | `gga_section` | Collapsible, under the caster. Position reporting - the feature brought to the front page. |
| Session band | `session_band` | The verdict: state word in its semantic color and a colored left rule, target, session total, rate and activity bar; line 2 is the stream summary (click: Stream tab) or the next step / failure hint (click a position hint: GGA). Fixed height. |
| From receiver | `status_strip` | Fix banner (live / stale / no receiver) and the two configurable readout slots. |
| Bottom tab pane | `bottom_tabs` | One card: the four always-present diagnostic surfaces (below). |
| Graph disclosure | `plot_panel` | Collapsible elevation chart of the receiver. |

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
**Profiles Manager** (with its name prompt, delete confirmation and import/export). They are
not live session data, and they are rare enough that a dialog is the honest weight for them.

**One dialog pattern** (`ui::dialog`): every dialog is a modal card sized to its content with a
title row and an X. Dialogs that only show something or apply at once (About, Profiles, the
delete question) also close on Esc or a click outside; dialogs holding edits the user could
lose (Options, the name prompt, import/export) close only on purpose - X or Esc as Cancel, never
a stray click. Questions bring their own action + Cancel footer (Cancel far right); About needs
no footer. Being modal is what keeps the keyboard simple: while a dialog is up the surface is
inert.

**Profiles Manager** (`profiles_dialog`) is shaped like any profiles manager: a list of rows
(name, `host:port / MOUNT`, an ACTIVE badge on the active one) beside a column of verbs, with
one primary action, **Use profile**, that makes the selection active (Enter and double-click are
its accelerators; a click only selects). Rename is inline with an explicit commit (Enter/OK;
Esc, Cancel or any other action abandons; focus loss never commits) and refuses a taken name in
place via `profiles::check_name`. Delete confirms in an `egui::Modal`. A disabled verb prints its
reason beneath itself, and a live connection is announced in a warning callout at the top. The
dialog is built from panels (footer, verb column, list) on a fixed bed, so it never resizes
itself. No profile is created under a made-up name: New..., Duplicate... and the profile bar's
Save as profile... share one name prompt with a suggestion drawn from the stream.

## Laws the pieces obey

- **The four tabs are always present.** They are offered even when empty - an empty tab teaches
  the fetch path (e.g. the Sourcetable tab says how to load one), and a fixed set means the
  strip never changes width or reorders under the pointer. The active tab is
  `settings.window.tab`; the empty states are honest, never a blank void.
- **Disclosures state themselves while collapsed.** The GGA section shows a one-line summary of
  its configuration ("when required, manual 45.52, -122.67") without being opened, so a glance
  answers "will this send a position" without a click.
- **One verdict, in one place.** The session band is the only answer to "is it working?" - the
  caster card no longer carries its own status line and the graph strip no longer repeats the
  RX rate. Green never outlives the data: a stall reads `NO DATA FOR N s`, and bytes that never
  frame as RTCM3 read `RECEIVING - NOT RTCM3`.
- **Summaries are clickable shortcuts, not dead text.** The band's stream summary opens the
  Stream detail tab; a position hint opens the GGA section. When there is nothing to summarize
  the line teaches the next step instead of going blank.
- **Attention comes to the strip, it is not summoned.** When the worker captures an
  unclassified caster response (often an HTML error page where RTCM was expected), a warning
  dot appears on the Conn tab until the tech opens it. See the badge model below.
- **Per-tab actions stay in their tab.** Copy/Clear belong to the surface they act on; there is
  no strip-level action cluster, because the four tabs' Copy buttons have four different scopes.

## One save model

Every user edit - a front-page field, an Options toggle, a profile add/rename/switch/reorder/
delete - applies at once in memory. `App.last_saved` is the snapshot of what `settings.toml`
holds (the loaded settings at boot, replaced on each successful save), and
`App::has_unsaved_changes` compares against it with `Settings::persistable_eq`. The shared
`App::save_control` renders the result in both places it appears (profile strip, Profiles
footer): an accent-outlined **Save changes** while something is unwritten (outlined, not
solid, so Connect stays the one solid primary), a disabled **Saved** otherwise,
at a fixed width so the strip does not shift. The exit save still writes everything, so the
control answers "is this on disk yet", not "will this be lost". Before 0.2.2 profile CRUD wrote
the file immediately while field edits waited for Save - the two halves of the strip disagreed.

## Backing data model

The layout persists as UI geometry, not as user configuration - it rides the unconditional exit
save and is excluded from `Settings::persistable_eq`, so switching tabs or toggling a disclosure
never lights the profile-strip Save button.

| Field | Meaning |
|---|---|
| `settings.window.tab: BottomTab` | Active bottom tab (`Events`/`Conn`/`Stream`/`Sourcetable`). |
| `settings.window.gga_open: bool` | GGA disclosure open state. |
| `settings.window.graph_open: bool` | Elevation-graph disclosure open state. |
| `settings.window.serial_open: bool` | Receiver Port settings disclosure open state. |
| `settings.window.colorway: Colorway` | Appearance (`paper` / `desert` / `nordic`). |

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
