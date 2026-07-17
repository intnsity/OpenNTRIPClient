# Protocol and behavior notes

Clean-room behavioral record. This file describes, in prose, the observed behavior of the
original Lefebure NTRIP Client (studied for compatibility) and the protocol facts this
client is built on, so that contributors never need the original's source. No code was
copied from the original.

## NTRIP in one page

NTRIP (RTCM 10410.x) moves GNSS correction data over TCP using an HTTP-shaped handshake.

- A client requests a stream: `GET /<mountpoint>` with `Authorization: Basic ...` if the
  caster requires credentials. NTRIP v1 speaks HTTP/1.0 and answers a successful stream
  request with the non-standard status line `ICY 200 OK`, then raw correction bytes forever.
- NTRIP v2 is proper HTTP/1.1: `Ntrip-Version: Ntrip/2.0` + `Host:` headers, response
  `HTTP/1.1 200 OK` with headers, body often `Transfer-Encoding: chunked`.
- `GET /` (empty mountpoint) requests the sourcetable: status `SOURCETABLE 200 OK` (v1) or
  `HTTP/1.1 200 OK` + `Content-Type: gnss/sourcetable` (v2), body is semicolon-delimited
  records, terminated by a line `ENDSOURCETABLE`.
- Sourcetable records: `STR;` one per mountpoint (18 standard fields; field index 11 is the
  `nmea` flag - `1` means the client must send its position as an NMEA GGA sentence),
  `CAS;` casters, `NET;` networks.
- Requesting a mountpoint that does not exist typically returns the sourcetable instead of
  a stream - casters use it as a soft 404. Treat "asked for stream, got table" as
  "mountpoint not found" and say so plainly; it is one of the top support diagnoses.
- `401 Unauthorized` = bad credentials. Some casters send HTTP/1.1-style responses even to
  v1 requests; a tolerant client classifies on the status line, not the protocol version.
- Streams whose sourcetable entry sets the nmea flag expect periodic client GGA sentences;
  network/VRS casters use the position to pick or synthesize corrections, and licensed
  services use it for geofencing.

## Original client behavior (compatibility record)

Observed from the Lefebure NTRIP Client 2017.07.27; "ours" notes deliberate deltas.

### Request

HTTP/1.0 GET with `User-Agent: NTRIP <name>/<version>`, `Accept: */*`, `Connection: close`.
`Authorization: Basic base64(user:pass)` sent only when the username is non-empty. ASCII
encoding throughout.

### Response handling and timing

- Waits up to 30 s for first response bytes; a sourcetable gets 10 more seconds to finish.
- 30 s with no stream data = connection timed out.
- Auto-reconnect ~10 s after an unexpected drop, up to 10,000 attempts, optional .wav
  alert. Auth failures and unknown responses disable auto-reconnect.
- BUG (fixed in ours): correction bytes arriving in the same TCP segment as the
  `ICY 200 OK` status line were discarded. Ours consumes exactly the status line (and the
  optional following blank line) and treats every remaining byte as stream data.
- Some casters (observed: CHCStream 1.0) follow `ICY 200 OK` with real HTTP-style header
  lines (`Server: ...`, `Date: ...`) and a blank line before any RTCM. Ours consumes
  header-shaped lines up to the blank line and logs them verbatim; they never count as
  correction bytes. The first byte that cannot belong to a header line (RTCM frames start
  with 0xD3, which is not printable ASCII) ends header consumption immediately, so a
  payload coalesced with the status line is still delivered in full.
  "Header-shaped" means an RFC 7230 field line: a token name immediately followed by a
  colon. Documented tradeoff: an ICY stream that is itself printable ASCII can have lines
  that fit this shape (e.g. `time: 12:00:00`) consumed as trailing headers - bounded to
  32 lines of at most 256 bytes each, always visible verbatim in the Connection Log, and
  withheld from correction counters/capture/serial. Prose lines that do not start with
  `token:` (a space before the colon, or no colon) are replayed byte-exact as payload;
  raw TCP mode never runs the scanner at all.
- Only STR sourcetable records were parsed (mountpoint + nmea flag); CAS/NET were dropped.
  Ours parses and displays everything, and shows unparseable lines verbatim.

### Position reporting (GGA)

Two sources: pass through the most recent GGA read from the receiver's serial port, or
fabricate one from a manually entered lat/lon. The fabricated sentence claims a healthy
RTK fix: quality 4, 10 satellites, HDOP 1.0, altitude 200 m, geoid separation 1 m,
correction age (seconds mod 6) + 3, station id 0; UTC time from the PC clock; standard
XOR checksum. Sent ~0.3 s after connect, then every 10 s, only when the stream's nmea
flag requires it. Ours keeps the exact template and cadence, adds an "always send"
override, and keeps manual entry alongside the new city/ZIP lookup.

### Serial side

Corrections are forwarded verbatim to the receiver COM port (no parsing of RTCM). The same
port is read back for NMEA: GGA (fix quality 0-9, satellites, HDOP, altitude, geoid
separation, correction age, station id), RMC (speed, heading), GSA (fix type, PDOP/HDOP/
VDOP). Talkers `$GP/$GN/$GL/$GA` accepted. Fix quality names: Invalid, GPS, DGPS, PPS,
RTK Fixed, RTK Float, Estimated, Manual, Simulation, WAAS (9).
BUGS (fixed in ours): the GSA handler read HDOP where PDOP was meant; the stop-bits
setting was stored but ignored (always 1).

NovAtel OEMV auto-config on serial connect (rate 1/5/10 Hz -> ontime 1.0/0.2/0.1):

```
unlogall thisport
log thisport gpggalong ontime <sec>
log thisport gprmc ontime <sec>
interfacemode thisport <rtcm|rtcmv3|cmr|rtca|omnistar|novatel> novatel
```

### Files (all next to the exe; ours imports these on first run)

- `ntripconfig.txt`: `NTRIP Caster`, `NTRIP Caster Port`, `NTRIP Username`,
  `NTRIP Password`, `NTRIP MountPoint` (plaintext `Key=Value`).
- `Settings.txt`: serial port/speed/data bits/stop bits, protocol, manual-GGA toggle and
  lat/lon, audio alert file, event/NMEA logging toggles, two display-slot identifiers
  (`age|hdop|vdop|pdop|elevation-feet|elevation-meters|speed-mph|speed-mph-smoothed|`
  `speed-kmh|speed-kmh-smoothed|heading`), update-check keys, NovAtel receiver keys.
- Parse rules for both: trim each line; skip lines shorter than 3 chars; skip lines
  starting `#`; require `=` at index >= 2; split on the first `=`; keys are
  case-insensitive; unknown keys are logged and skipped.
- `sourcetable.dat`: raw sourcetable as last downloaded, reloaded at startup.
- `Logs\YYYYMMDD.txt` (events), `NMEA\YYYYMMDD.txt` (raw GGA record), when enabled.

### Event vocabulary (log parity)

Connect attempts with attempt number, connected/waiting-for-data, running byte counts,
timeout, invalid-credentials, downloaded-sourcetable, fix-quality transitions (old -> new,
by name), satellite-count changes, HDOP changes, correction-age warning when RTK degrades,
base-station-id changes.

### Not carried forward

The original's binary self-updater (fetched and executed an exe over plain HTTP). Replaced
with a "check for updates" action that opens the project's releases page in a browser.

## RTCM 3.x framing (for the inspector)

Frame: `0xD3`, 6 reserved bits (zero), 10-bit payload length, payload, 24-bit CRC-24Q over
everything before it. Message type = first 12 bits of the payload. CRC-24Q: polynomial
0x1864CFB, init 0, no reflection, no final XOR ("123456789" check value 0xCDE703 - note
the OpenPGP CRC-24 in catalogs uses a different init and value; do not confuse them).
On CRC failure, advance one byte and rescan - never skip a whole frame on a failed check,
a corrupted length byte would desync the stream. Diagnostic decodes: 1005/1006 (base ECEF
position; 1006 adds antenna height), 1008/1033 (antenna/receiver descriptors), 1029 (UTF-8
text message), 1230 (GLONASS biases), MSM 1071-1137 (header only: station, epoch, satellite
and signal masks; the 1131-1137 decade is NavIC).
