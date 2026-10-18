# Changelog

Format based on [Keep a Changelog](https://keepachangelog.com/); versions follow SemVer.

## [0.3.0] - 2026-10-18

A UX pass: the same power, easier to digest. The window now reads set up -> verdict ->
evidence, every dialog behaves the same way, and profiles are named by you and shareable as
small files.

### Added

- **Session band** under the caster card: the one place that answers "is it working?" -
  `NOT CONNECTED` / `CONNECTING` / `STREAMING` / `NO DATA FOR N s` / `RECEIVING - NOT RTCM3` /
  `FAILED` in its semantic color with a colored left rule, the target, session total, rate and
  activity bar, and a second line with the stream summary or the next step
- **Position hints**: when a caster accepts but starves the connection, or refuses it for a
  reason other than credentials, while no GGA position was sent, the band says so in one
  sentence and a click opens GGA position reporting; the GGA header turns orange with
  "- no position to send" when GGA is on but there is nothing to send
- **Save as profile...**: set up a caster on the main window, Connect to check it, then keep it
  under your own name (suggested from the stream). The source profile keeps its saved settings
- **Profile export and import**: **Export...** writes one or all profiles to a small
  `.oncprofile` file (passwords optional); **Import...**, drag and drop, or opening the app with
  the file brings them in, with per-profile rename or replace. A whole `settings.toml` from
  another install imports too
- **Colorways**: Butter Paper, Southwest Night and Nordic Night (ButterPaper v2 tokens),
  switched from the appearance icon on the profile bar and remembered
- **Not-RTCM3 diagnosis**: bytes that never frame as RTCM3 (Trimble RT27, CMR) read as such on
  the band and in Stream detail instead of a green "Streaming"
- README "Change it yourself": prerequisites, build, a worked label edit, the checks, and
  where each part of the screen lives; `scripts/dev.ps1` runs or checks in one command

### Changed

- Layout: "1 Caster" first with the window's one solid primary button, which says what it will
  do (**Connect** / **Get sourcetable** / **Disconnect**, with the reason underneath when it
  cannot be pressed); "2 Receiver (optional)" beside it with baud, framing and NovAtel in a
  self-describing **Port settings** disclosure; diagnostic tabs framed as one card; empty states
  teach the next step
- Caster card: one **Protocol** choice (NTRIP 1.0 / NTRIP 2.0 / Raw TCP); **Mountpoint** with a
  proper picker button; **Browse mounts** fetches the sourcetable when needed; "show" password
  checkbox; "Fields are locked while connected" stated on the card
- Receiver readouts: the fix banner says `No receiver` / `Waiting for NMEA` / `No GGA from
  receiver`, and a fix greys out with "(receiver silent)" or "(last seen ...)" once NMEA stops
  instead of staying green
- One dialog pattern: Options, About, Profiles and their prompts are modal cards with a title
  and an X; Esc closes them; information dialogs also close on a click outside, dialogs with
  edits close only on purpose. **Options** gets **Done** / **Cancel** (Cancel reverts)
- Profiles window behaves like any profiles manager: rows show the caster each profile points
  at and a green ACTIVE badge; one primary action, **Use profile** (Enter and double-click too);
  keyboard (Up/Down, Enter, F2, Delete, Alt+Up/Down to reorder) and a right-click menu; rename
  is explicit (Enter/OK saves, Esc/Cancel abandons, clicking elsewhere never commits); a taken
  name is refused in place, never silently suffixed; New... and Duplicate... ask for the name
  first; rows tag "unsaved edits"; delete confirms in a dialog; disabled actions explain
  themselves under the button
- One save model: profile edits no longer write `settings.toml` immediately while field edits
  waited for Save. Every edit applies at once; **Save changes** (accent outline) writes them
  all; closing the app still saves
- Buttons look like buttons: secondary buttons and pickers wear a tinted face instead of the
  plate reserved for text fields; number fields keep the input look
- **Manage...** is now **Manage profiles...**

### Fixed

- Elevation reads to the millimetre: the RTCM 1005/1006 base height (`h 1615.876 m (ellipsoidal)`,
  was `alt 1615.9 m`), the Elevation (ft) / Elevation (m) slots (fixed-width, right-aligned
  monospace, so the row never shifts), and the elevation graph's cur/min/max/range. The base
  latitude/longitude go to 8 decimals (~1.1 mm) to match the height beside them
- The GGA header no longer warns about a missing position on Raw TCP, where none is sent

### Security

- rustls 0.23.46 (RUSTSEC-2026-0285, -0346, -0348, -0349) and webbrowser 1.2.4
  (RUSTSEC-2026-0257).

## [0.2.1] - 2026-07-23

Field fix: CHC APIS casters connected but never streamed ("Connected - waiting for data",
then a silence timeout on loop). APIS answers `ICY 200 OK` and then holds the stream until
the client sends any NMEA GGA - and its base-serial-number mountpoints are absent from its
own sourcetable, so the old "send GGA only when the sourcetable requires it" default never
sent one. Verified live against apis-usa.chcnav.com:2201 (`Server: CRNet 1.0`).

### Changed

- `when required` GGA mode now treats an unknown requirement (no sourcetable, or the mount
  not listed in it) as "required" and sends a position; only an explicit nmea=0 sourcetable
  entry suppresses it. Casters that do not want a GGA ignore it - withholding one deadlocks
  APIS-style casters
- A due GGA with no position to send (receiver source with no fix yet, manual position not
  set) retries every 2 s instead of every 10 s, so the receiver's first fix reaches the
  caster promptly instead of racing the 30 s silence timeout; the misses are explained in
  the event log and stop repeating once corrections flow
- A manual position left at its unset default (exactly 0, 0) now sends nothing and says so,
  instead of fabricating a confident fix on Null Island for a VRS to act on - this applies
  to `always` mode too

### Added

- `--selftest` accepts `--gga LAT,LON` (decimal degrees), which sends a manual-position GGA
  regardless of the sourcetable: the headless way to certify an APIS-style caster and
  customer credentials end to end

## [0.2.0] - 2026-07-17

"One Surface" pass: the diagnostic windows that used to float are now one always-present
workspace, and the scattered position-reporting controls collapse into a single inline
disclosure. Sourcetables stop touching disk.

### Added

- One-line clickable stream summary above the bottom tabs - a compact readout that expands
  the pane when clicked
- Attention dot on the Connection tab when the caster returns a response the client cannot
  classify (neither a stream nor a sourcetable), so an unexpected reply is noticed without
  digging through the log
- cargo-deny supply-chain gate in CI: license, security-advisory, and crate-source checks
  fail the build on a violation

### Changed

- Connection Log, RTCM3 inspector (Stream detail), and Sourcetable browser are no longer
  floating windows; they are tabs in an always-present bottom pane
- Per-profile GGA position-reporting controls (the former "Details" dialog) and the offline
  city/ZIP location picker are folded into one inline "GGA position reporting" disclosure in
  the NTRIP column
- Interface typeface set in IBM Plex Sans (proportional) and IBM Plex Mono (monospace),
  embedded from `assets/fonts` (OFL-1.1)
- Default status-strip readouts are now elevation (ft) and base-stream data age; a saved
  `settings.toml` still overrides both

### Removed

- On-disk `SourceTables\` cache: sourcetables are held in memory only and fetched fresh each
  session

## [0.1.0] - 2026-07-16

Initial internal build (never published): a clean-room Rust rewrite of the Lefebure NTRIP Client (studied for
behavior only; no code reused) with full feature parity plus new diagnostics. Everything
below is new by definition; "Fixed" lists deliberate behavior deltas from the original.

### Added - parity with the original client

- NTRIP v1 client: `GET /<mount> HTTP/1.0`, `ICY 200 OK` streaming, Basic auth only when
  a username is set, 30 s first-response and stream-silence timeouts, 10 s extra for
  sourcetable completion
- Sourcetable download (`SOURCETABLE 200 OK` and HTTP-styled responses), cached per
  caster under `SourceTables\`, mountpoint dropdown, bad-mountpoint detection ("asked
  for a stream, got the table") reported in plain words
- Raw TCP mode: no request, no GGA, straight to the byte stream
- Serial forwarding of corrections to the receiver with port/baud/data-bits/stop-bits/
  parity settings and NovAtel OEMV auto-configuration (rate 1/5/10 Hz, six formats)
- Receiver NMEA decoding (GGA/RMC/GSA across `$GP/$GN/$GL/$GA` talkers, checksums
  enforced): fix quality with the original's ten names, satellites, DOPs, correction age,
  base station id, speed and smoothed speed, heading, altitude
- GGA position reporting to the caster: verbatim receiver passthrough or the original's
  exact fabricated-sentence template (quality 4, 10 sats, HDOP 1.0, alt 200 m, age
  (s mod 6)+3), first send ~0.3 s after connect, then every 10 s
- Auto-reconnect ~10 s after unexpected drops (never after auth/protocol failures) with
  attempt counter and optional .wav alert
- Event log pane with the original's event vocabulary; optional daily `Logs\YYYYMMDD.txt`
  and `NMEA\YYYYMMDD.txt` files with local-midnight rollover
- Elevation strip-chart with start/pause/reset and min/max/current/range readouts; two
  configurable status readout slots (age/DOPs/elevation/speed/heading, original ids, plus
  the new stream-side Data Age and Data Rate ids)
- Green data-activity indicator pulsing once per correction burst, like the original's
  bar (deltas: a live kB/s caption, an orange "no data N s" stall readout after 2 s of
  stream silence, and RX totals shown in kB below 1 MB)
- Portable-folder persistence: `settings.toml` next to the exe, window geometry included;
  no AppData, no registry. First run imports `Settings.txt`, `ntripconfig.txt`, and
  `sourcetable.dat` using the original's parse rules, logs every imported line, and
  leaves the legacy files untouched

### Added - beyond the original

- Connection profiles with a manager dialog (add/duplicate/rename/remove)
- UI-editable mountpoint combo (type it or pick it)
- Sourcetable browser: STR/CAS/NET tabs, filter, sort, click-to-fill mountpoint; records
  the original silently dropped are shown, unparseable lines kept verbatim
- Offline geocoder: worldwide city names (GeoNames) and US ZIP centroids (US Census)
  embedded in the binary, diacritic-folded type-ahead ("nairobi", "portland, or",
  "97201"), manual lat/lon always available
- Connection Log: every TX/RX protocol line verbatim, GGA sends, reconnect decisions,
  hex view for non-standard replies, copy-all
- RTCM3 inspector: live message-type table (count, rate, age, size), decoded base
  position (1005/1006) with baseline distance, antenna/receiver descriptors (1008/1033),
  caster text messages (1029), GLONASS biases (1230), MSM header decode, CRC-24Q failure
  counter used as a stream-health metric
- NTRIP v2 per profile: HTTP/1.1 with `Ntrip-Version: Ntrip/2.0` and chunked
  transfer decoding (also armed on v1 responses that declare it)
- TLS per profile (rustls, IP-address SANs supported) plus a diagnostic-only
  "accept invalid certificates" override with a persistent red warning banner
- Raw correction-stream capture to `Captures\YYYYMMDD_HHMMSS_{mount}.rtcm`
- Headless `--selftest` CLI mode driving the real worker stack for scripted verification
  (exit code 0 = clean run, 1 = failure-class close, 2 = usage)
- Crash reports to `Logs\crash-*.txt` via panic hook
- Windows exe carries icon + VERSIONINFO; releases ship with SHA256SUMS

### Changed - deliberate deltas from the original

- The binary self-updater is gone (it fetched an exe over plain HTTP); "check for
  updates" now opens the releases page in a browser
- Settings live in one human-editable `settings.toml` (passwords stay plaintext, as in
  the original - the folder is the security boundary; the README says so)

### Fixed - long-standing quirks of the original

- Correction bytes arriving in the same TCP segment as the `ICY 200 OK` status line are
  no longer discarded
- GSA PDOP is read from the PDOP field (the original showed HDOP twice)
- The stop-bits serial setting is honored instead of silently forced to 1
- CAS/NET sourcetable records and unknown lines are shown instead of dropped
