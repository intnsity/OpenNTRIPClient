# Changelog

Format based on [Keep a Changelog](https://keepachangelog.com/); versions follow SemVer.

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
