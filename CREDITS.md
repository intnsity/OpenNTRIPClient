# Credits

## Lefebure NTRIP Client

This project is a clean-room rewrite inspired by the Lefebure NTRIP Client by
**Lance Lefebure** (<https://lefebure.com/software/ntripclient/>), the tool that has served
the GNSS/RTK community since 2009. The original is published at lefebure.com as free, open
source software. Its feature set, workflow, and file formats shaped this client; no code
was reused. These credits also appear in the application's About screen.

## Geocoding data

- **GeoNames** (<https://www.geonames.org/>) - world city centroids from `cities5000.txt`
  and admin/country name tables. Licensed under
  [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
- **US Census Bureau** (<https://www.census.gov/>) - ZIP Code Tabulation Area (ZCTA)
  centroids from the Gazetteer files. Public domain.

The embedded database `crates/geodb/data/geo.bin` is generated from these sources by
`tools/geodb-gen`.

## Embedded fonts

The user interface is set in **IBM Plex**, embedded in the binary as unmodified data:
**IBM Plex Sans** (Text weight) for proportional text and **IBM Plex Mono** for the
monospace log, hex, and table views, both (c) IBM Corp. under the
[SIL Open Font License 1.1](https://openfontlicense.org/) (license text at
`assets/fonts/OFL.txt`).

egui's default fonts remain embedded as fallbacks, covering glyphs IBM Plex does not
(emoji and non-Latin scripts): Hack and Noto Emoji under the SIL Open Font License 1.1,
and Ubuntu-Light under the
[Ubuntu Font Licence 1.0](https://ubuntu.com/legal/font-licence). See
[THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) for the full dependency license audit.

## Specifications

- NTRIP: "Networked Transport of RTCM via Internet Protocol", RTCM Standard 10410.x
  (versions 1.0 and 2.0).
- RTCM 3.x message framing per RTCM Standard 10403.x (frame structure and CRC-24Q are
  public knowledge; this tool decodes headers and a small set of diagnostic messages only).
