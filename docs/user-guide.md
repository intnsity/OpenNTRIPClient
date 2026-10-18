# Open NTRIP Client - user guide

Back to the [README](../README.md).

The whole client is a single window. This guide walks through every region of that
window, the GGA position handshake, the diagnostic tabs, the files the app writes, and
the headless self-test.

## The interface: a guided tour

The window reads top to bottom as **set up, verdict, evidence**: you fill in the caster
(step 1), optionally a receiver (step 2), press **Connect**, and the session band directly
below tells you whether it is working. Everything else is the evidence behind that answer.
The numbered regions below map to this screenshot:

![Annotated overview of the main window](screenshots/ui-overview.png)

| # | Region | What it does |
|---|--------|--------------|
| 1 | Profile bar | Switch, save and manage connection profiles; appearance; Options and About. |
| 2 | 1 Caster | Caster address, credentials, mountpoint, protocol; the one **Connect** button. |
| 3 | GGA position reporting | Whether and how the client reports its position to the caster. |
| 4 | 2 Receiver (optional) | Forward corrections to a GNSS receiver on a COM port. |
| 5 | Session band | The verdict: state, target, data rate and total, what is in the stream, the next step. |
| 6 | From receiver | Fix state and two configurable readouts from the receiver's NMEA. |
| 7 | Diagnostic tabs | Event log / Connection log / Stream detail / Sourcetable. |
| 8 | Elevation graph | Collapsible chart of the receiver's elevation. |

### 1. Profile bar

- **Profile** - a named preset of the whole caster configuration (host, port, credentials,
  mountpoint, protocol, TLS, GGA settings). Picking one from the dropdown makes it active
  immediately (hover an entry to see the caster it points at). While connected the picker
  is locked and the bar says "Disconnect to switch profiles".
- **Save changes / Saved** - one save model for the whole app: every edit (a field, an
  Options choice, adding, renaming, switching, reordering or deleting a profile) takes effect
  at once and is held in memory until you click **Save changes**, which writes
  `settings.toml`. Closing the app also writes it. The button is outlined in the accent color
  while something is unwritten and reads a greyed **Saved** when there is nothing to write.
- **Save as profile...** - keep what is on the screen as a new profile under a name you
  choose. The intended flow: type a caster's settings, press **Connect** to check they work,
  then **Save as profile...**. The name is pre-filled from the stream (sourcetable name and
  mountpoint, or `MOUNT @ host`) for you to accept or overwrite. The new profile becomes
  active and is written to disk; the profile you started from keeps its last saved settings.
- **Manage profiles...** - opens the Profiles window (see Dialogs below).
- **Appearance** (the icon left of Options) - cycles the three ButterPaper colorways: Butter
  Paper (light, sheet icon), Southwest Night (moon over mesa) and Nordic Night (moon over
  pines). The choice is remembered.
- **Options** - readout slots, audio alert, log files, raw capture, auto-reconnect, update
  checks.
- **About** - version, credits, and the "check for updates" link.

### 2. Caster (step 1)

- **Host** / **Port** - the caster address (default NTRIP port is 2101).
- **TLS** - encrypt the connection. Certificates verify against the bundled root store. (A
  loud, diagnostic-only "accept invalid certificates" override appears only while TLS is on -
  for reaching self-signed or bare-IP casters. Connecting with verification disabled shows an
  undismissable red banner.)
- **User** / **Password** - caster credentials; tick **show** to reveal the password.
- **Mountpoint** - type it, pick it from the fetched sourcetable with the arrow button beside
  the field, or open the full **Sourcetable** tab with **Browse mounts** (which fetches the
  table first if none is loaded). Leave it empty to fetch the sourcetable itself.
- **Protocol** - **NTRIP 1.0**, **NTRIP 2.0** (HTTP/1.1, chunked transfer), or **Raw TCP**
  (no HTTP handshake - straight to the byte stream; iGage DIP ports work this way).
- **Connect** - the one solid button on the window, and it says what it will do: **Connect**,
  **Get sourcetable** when the mountpoint is empty, **Disconnect** while connected. When it
  cannot be pressed it says why underneath (for example "Enter a caster host"). Fields lock
  while connected, and the card says so.

### 3. GGA position reporting

Some casters (VRS and geofenced mounts) will not stream unless the client tells them where it
is, as an NMEA GGA sentence. This disclosure - collapsed by default, with a one-line summary
of its current setting - is where you configure that. The summary turns orange and ends
"- no position to send" when GGA is on but there is nothing to send (receiver source with no
receiver, or a manual position still at 0, 0). Click the header to expand it:

![The GGA position-reporting section, expanded](screenshots/gga-section.png)

1. **Send GGA** - the mode:
   - `off` - never send a position.
   - `when required` (the default) - send unless the sourcetable's entry for the mount says
     it takes no NMEA. Mounts the caster does not list - CHC APIS base serial numbers, for
     example - get a position too: APIS-style casters accept the connection and then hold
     the stream until a GGA arrives, so withholding one reads as a dead mount.
   - `always` - send regardless.
2. **Position source** - `receiver` passes your connected receiver's live GGA through
   verbatim; `manual` fabricates a GGA at the position you enter below.
3. **Latitude / Longitude** - the manual position, in decimal degrees (only shown for the
   `manual` source).
4. **City / ZIP search** - type a city (`Nairobi`, `Portland, OR`) or a US ZIP code
   (`97201`) and pick a result to fill the coordinates. Fully offline - a worldwide geocoder
   is embedded in the binary; no internet lookup.
5. **Use receiver position** - copy the connected receiver's current position into the manual
   fields.

The GGA sentence is sent about 0.3 s after connect, then every 10 s. If there is nothing to
send yet, the client retries every 2 s and explains the miss in the event log; the first
position to appear goes out promptly.

If a caster accepts the connection but sends nothing, or refuses it for a reason other than
credentials, while no position was sent, the session band shows one sentence naming the
likely fix - for example "The caster may want your position: turn on GGA (a nearby manual
point is fine) and reconnect." Click it to open this section.

### 4. Receiver (step 2, optional)

Connect a GNSS receiver over a serial/COM port so the caster's corrections are forwarded to
it and its NMEA output drives the readouts. You can diagnose a caster with no receiver at
all.

- **Port** - the COM port dropdown lists each port with its device name and whether it is a
  USB or Bluetooth link (for example `COM5 - Bluetooth`), so you can tell a GNSS receiver
  from other serial devices. **Refresh** re-scans.
- **Port settings** - a disclosure that states its setting while collapsed
  (`115200 8-N-1, NovAtel off`). Inside: **Baud** (2400 to 115200); **Framing** - data bits,
  parity, and stop bits (all honored, unlike the original which ignored stop bits); and
  **NovAtel auto-config**, which sends a NovAtel receiver the commands to emit corrections in
  the chosen **Format** at the chosen **Rate**.
- **Connect receiver** - opens the port. The status beside it shows connection state and a
  serial overrun counter if the receiver outruns the link.

### 5. Session band

The one place that answers "is it working?", directly under the Connect you just pressed. A
colored rule down its left edge repeats the state at a glance.

- **Line 1** - the state in its color: `NOT CONNECTED`, `CONNECTING`, `CONNECTED - WAITING
  FOR DATA`, `STREAMING` (green), `NO DATA FOR N s` (orange - a stall), `RECEIVING - NOT
  RTCM3` (orange - bytes arrive but none frame as RTCM3, e.g. a Trimble RT27 or CMR port),
  `RECONNECTING - attempt N`, `FAILED` (red) or `DISCONNECTED`; then the target
  (`host:port / MOUNT`, `host:port raw TCP`); and on the right the session total, the live
  kB/s rate and the activity bar, which pulses with each burst of corrections.
- **Line 2** - while frames flow, the stream summary: base station id and position,
  message-type and rate figures, and framing health (click it for Stream detail). Otherwise
  the next step ("Press Connect to start.") or what just happened (the reason a session
  stopped, the GGA hint above).

### 6. From receiver

- **Fix state** (the large text) - `No receiver`, `Waiting for NMEA`, `No GGA from
  receiver`, or the fix quality (`GPS`, `DGPS`, `RTK Float`, `RTK Fixed`, ...) colored by
  quality. A fix only shows in color while NMEA is live; if the receiver goes quiet or is
  unplugged, the last fix greys out with "(receiver silent)" or "(last seen 2 min ago)".
- **Two configurable slots** - pick what each shows from a dropdown: correction age,
  HDOP/VDOP/PDOP, satellite count, speed (raw or smoothed, mph or km/h), heading, elevation,
  base station id, and two stream-side values (**Data Age** and **Data Rate**) that work with
  no receiver attached. Defaults are chosen in Options. Elevation reads to three decimals
  (millimetres, or thousandths of a foot), right-aligned in a fixed-width monospace bed so the
  row does not shift as the value ticks.

When no receiver is attached, the receiver-derived readouts say "needs receiver" so you know
why they are blank.

### 7. The diagnostic tabs

The four surfaces that used to be separate floating windows are always-present tabs filling
the lower half of the window. Each is described below. The active tab is remembered between
runs.

**Event log** - the application's own running commentary: what it connected to, what it
decoded, what went wrong, and what to do about it. Trouble lines (errors, auth failures,
overruns) stand out. **Copy** and **Clear** act on the log.

**Connection log** - the verbatim protocol exchange, byte-for-byte: every request line the
client sends (`>`), every response line the caster returns (`<`), TLS results, and reconnect
decisions. A live **filter** narrows the view; **Hex dump** shows the raw bytes of the most
recent response the client could not classify (for example an HTML error page where RTCM was
expected). A small attention dot appears on this tab when such a response is captured.

![The Connection log tab](screenshots/tab-connection-log.png)

**Stream detail** - a live RTCM3 inspector: a per-message-type table with counts, rates,
sizes, and ages; total-frame and **CRC failure** / **garbage byte** counters (the top "is
this actually RTCM3" signals); and the latest decoded diagnostics - base station position
(from RTCM 1005/1006, all to the millimetre: latitude/longitude to 8 decimal places, about
1.1 mm, and ellipsoidal height to 0.001 m, e.g. `35.54344168, -108.74125211  h 2042.893 m (ellipsoidal)`),
the 1006 antenna height at the message's own 0.1 mm resolution (most CORS bases broadcast the
antenna reference point itself, so 0.0000 m is normal), its baseline distance to you,
antenna/receiver descriptors, caster text messages (1029), and GLONASS bias presence (1230).
With no frames it says what to do, and when bytes arrive but none are RTCM3 it says the
stream may be another format (Trimble RT27, CMR).

![The Stream detail tab](screenshots/tab-stream-detail.png)

**Sourcetable** - a browsable, filterable, sortable view of the caster's sourcetable. Sub-tabs
for STR (streams), CAS (casters), and NET (networks) - including the CAS/NET records the
original client dropped - plus an Unparsed tab if the caster leaks non-standard lines. Click a
column to sort; double-click a stream (or select it and press **Use**) to set it as your
mountpoint.

![The Sourcetable tab](screenshots/tab-sourcetable.png)

### 8. Elevation graph

A collapsible strip at the very bottom. Expand **Elevation graph (receiver)** for a live plot
of the receiver's elevation over time, with **Pause** / **Reset** and cur/min/max/range
readouts to the millimetre. (The correction byte rate and session total live in the session
band.)

## Dialogs: Options, Profiles, About

Settings and information that are not live session data open as dialogs. They all work the
same way: a card over the window with its title and an **X**; **Esc** closes it. Dialogs that
only show something (About, Profiles) also close on a click outside them; dialogs holding
edits you could lose (Options, naming a profile) close only on purpose.

- **Options** - default readout slots; the audio alert `.wav`; whether to write the event log
  (`Logs\`) and NMEA log (`NMEA\`); raw-corrections capture to `Captures\`; auto-reconnect;
  and how often to check for updates. Changes preview live; **Done** keeps them, **Cancel**
  (or X / Esc) puts back what the dialog opened with.
- **Profiles** (**Manage profiles...**) - the list of connection profiles. Each row shows
  the profile's name and the caster it points at (`host:port / MOUNT`, plus `TLS` or
  `raw TCP` when used); the active one wears a green **ACTIVE** badge. Beside the list:
  - **Use profile** (blue) makes the selected profile active. Enter and double-click do the
    same. Click a row only to select it.
  - **New...** and **Duplicate...** ask for the new profile's name first (Duplicate suggests
    "<name> copy"); nothing is ever created under a made-up name. A row tagged **unsaved
    edits** or **not saved yet** differs from `settings.toml`.
  - **Rename** edits the name in place. **Enter** or **OK** saves the name, **Esc** or
    **Cancel** keeps the old one; clicking elsewhere does not save it, and starting any
    other action abandons the edit. A name another profile already uses is refused on the
    spot ("... is already used by another profile") - nothing is renamed behind your back.
  - **Export...** saves the selected profile, or all of them, as a small `.oncprofile` file
    in the `Profiles\` folder and shows it in Explorer - send it to anyone. **Include
    passwords** is on by default; the file holds them as plain text.
  - **Import...** lists recent `.oncprofile` files (Profiles, Downloads, Desktop); you can
    also drop one onto the window or open the app with it. Untick profiles you don't want,
    rename any, or tick **Replace existing** for a name you already have (a file shared
    without passwords keeps the password you already have for the same user). A whole
    `settings.toml` from another install imports the same way.
  - **Delete...** asks for confirmation in a dialog that says what is removed and, for
    the active profile, which profile becomes active.

  Keyboard: Up/Down select, Enter uses, F2 renames, Delete deletes, Alt+Up/Down moves a
  profile up or down the list. Right-click a row for the same actions plus Move up/down.
  A button that cannot be used right now says why directly underneath it ("Already the
  active profile", "The last profile can't be deleted"). While connected, a banner at the
  top of the window explains that switching and deleting wait until you disconnect;
  adding, duplicating and renaming still work. Profile edits follow the same save model as
  everything else - the window's footer carries the same **Save changes** button.

  ![The Profiles window](screenshots/profiles.png)

  ![Exporting a profile](screenshots/export.png)
- **About** - version, credits (Lefebure, GeoNames, fonts), and a "check for updates" link
  that opens the releases page in your browser. (The original's binary self-updater is
  deliberately not reproduced - security.)

## Files the app writes

All next to the exe, only when the relevant feature is on:

| Path | Contents |
|------|----------|
| `settings.toml` | All settings and profiles (passwords in plaintext - see the security note). |
| `Logs\` | Daily event-log files (Options: "write event log"). |
| `NMEA\` | Daily NMEA capture from the receiver (Options: "write NMEA log"). |
| `Captures\*.rtcm` | Raw correction captures for offline analysis (Options: "capture corrections"). |
| `Profiles\*.oncprofile` | Exported profiles (Profiles > Export...): TOML, the same fields as `settings.toml`. |
| `crash-pending.txt` + a crash report | Written only if a previous run crashed; announced once, then cleared. |

Sourcetables are **not** written to disk - they are held in memory and fetched fresh each
session (a cached table can only give a stale answer to "is this caster alive now").

## Headless self-test

`OpenNtripClient --selftest --host H --port P [--mount M --user U --pass P ...]` drives a full
caster connection with no GUI and exits non-zero on failure - for scripts and CI. Run
`--selftest` with no arguments for the full usage.
