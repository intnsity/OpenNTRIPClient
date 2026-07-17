//! NMEA 0183 sentence parsing for the receiver-side serial stream.
//!
//! Scope is deliberately narrow: the three sentences the client displays
//! (GGA, RMC, GSA) from any talker, everything else classified but not
//! parsed. The checksum is validated before any field is looked at, so a
//! corrupted sentence can never surface as plausible-looking data.
//!
//! Field philosophy: an empty or unparseable field becomes `None` (or 0 for
//! the two counters) - serial streams routinely omit fields mid-acquisition
//! and a diagnostic tool must keep displaying what it does have.

use std::str::FromStr;

#[derive(Debug, Clone, PartialEq)]
pub enum Sentence {
    Gga(Gga),
    Rmc(Rmc),
    Gsa(Gsa),
    /// Checksum-valid sentence we do not parse; identified for statistics.
    Other {
        talker: [u8; 2],
        kind: [u8; 3],
    },
}

/// Fix data: the client's primary display sentence.
#[derive(Debug, Clone, PartialEq)]
pub struct Gga {
    pub hms: Option<(u8, u8, f32)>,
    pub lat_deg: Option<f64>,
    pub lon_deg: Option<f64>,
    pub quality: u8,
    pub sats: u8,
    pub hdop: Option<f32>,
    pub alt_m: Option<f32>,
    pub geoid_sep_m: Option<f32>,
    pub age_s: Option<f32>,
    pub station_id: Option<u16>,
    /// The full sentence verbatim ("$...*XX", CRLF stripped): passthrough
    /// mode forwards it to the caster untouched.
    pub raw: String,
}

/// Recommended minimum data: speed, heading, and the date.
#[derive(Debug, Clone, PartialEq)]
pub struct Rmc {
    pub hms: Option<(u8, u8, f32)>,
    pub speed_knots: Option<f32>,
    pub track_deg: Option<f32>,
    /// (year, month, day). Two-digit years pivot at 80: 80..99 -> 19xx.
    pub date: Option<(i32, u8, u8)>,
    pub raw: String,
}

/// DOP and active satellites. Only the fix type and DOPs are surfaced.
#[derive(Debug, Clone, PartialEq)]
pub struct Gsa {
    pub fix: u8,
    pub pdop: Option<f32>,
    pub hdop: Option<f32>,
    pub vdop: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NmeaError {
    /// No "*XX" trailer with two uppercase hex digits.
    MissingChecksum,
    /// Trailer present but does not match the XOR of the body.
    ChecksumMismatch { computed: u8, stated: u8 },
    /// No leading '$' or the address field is not five characters.
    Malformed,
}

impl std::fmt::Display for NmeaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NmeaError::MissingChecksum => write!(f, "missing or malformed NMEA checksum"),
            NmeaError::ChecksumMismatch { computed, stated } => {
                write!(
                    f,
                    "NMEA checksum mismatch: computed {computed:02X}, sentence says {stated:02X}"
                )
            }
            NmeaError::Malformed => write!(f, "malformed NMEA sentence"),
        }
    }
}

impl std::error::Error for NmeaError {}

/// Parse one NMEA sentence. The XOR checksum (over the bytes between '$' and
/// '*') is validated first; nothing is interpreted from a corrupt line.
pub fn parse(line: &str) -> Result<Sentence, NmeaError> {
    let s = line.trim();
    let rest = s.strip_prefix('$').ok_or(NmeaError::Malformed)?;
    // rfind: a stray '*' in the body must not truncate the checksum span.
    let star = rest.rfind('*').ok_or(NmeaError::MissingChecksum)?;
    let body = &rest[..star];
    let tail = &rest.as_bytes()[star + 1..];
    // The standard mandates exactly two uppercase hex digits.
    let is_ck_digit = |b: &u8| b.is_ascii_digit() || (b'A'..=b'F').contains(b);
    if tail.len() != 2 || !tail.iter().all(is_ck_digit) {
        return Err(NmeaError::MissingChecksum);
    }
    let stated = (hex_val(tail[0]) << 4) | hex_val(tail[1]);
    let computed = body.bytes().fold(0u8, |a, b| a ^ b);
    if computed != stated {
        return Err(NmeaError::ChecksumMismatch { computed, stated });
    }

    let f: Vec<&str> = body.split(',').collect();
    let addr = f[0].as_bytes();
    // Standard addresses are talker(2) + type(3), but checksum-valid
    // proprietary sentences use 4-char addresses ($PUBX, $PTNL, ...). A
    // diagnostic tool must not count a healthy receiver's proprietary chatter
    // as parse errors, so 3..=5 chars classify as Other (space-padded).
    if !(3..=5).contains(&addr.len()) {
        return Err(NmeaError::Malformed);
    }
    if addr.len() == 5 {
        let talker = [addr[0], addr[1]];
        let kind = [addr[2], addr[3], addr[4]];
        return Ok(match &kind {
            b"GGA" => Sentence::Gga(parse_gga(&f, s)),
            b"RMC" => Sentence::Rmc(parse_rmc(&f, s)),
            b"GSA" => Sentence::Gsa(parse_gsa(&f)),
            _ => Sentence::Other { talker, kind },
        });
    }
    let mut padded = [b' '; 5];
    padded[..addr.len()].copy_from_slice(addr);
    Ok(Sentence::Other {
        talker: [padded[0], padded[1]],
        kind: [padded[2], padded[3], padded[4]],
    })
}

fn hex_val(b: u8) -> u8 {
    if b.is_ascii_digit() {
        b - b'0'
    } else {
        b - b'A' + 10
    }
}

/// Field by index, 0 = the address token. Missing trailing fields read as "".
fn field<'a>(f: &[&'a str], i: usize) -> &'a str {
    f.get(i).copied().unwrap_or("")
}

/// Numeric field: empty or unparseable -> None.
fn num<T: FromStr>(f: &[&str], i: usize) -> Option<T> {
    let s = field(f, i);
    if s.is_empty() { None } else { s.parse().ok() }
}

/// "hhmmss[.sss]" -> (h, m, s).
fn parse_hms(s: &str) -> Option<(u8, u8, f32)> {
    let b = s.as_bytes();
    if b.len() < 6 || !b[..6].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let hh: u8 = s[0..2].parse().ok()?;
    let mm: u8 = s[2..4].parse().ok()?;
    let ss: f32 = s[4..].parse().ok()?;
    // < 61 admits a leap second announcement.
    (hh < 24 && mm < 60 && ss < 61.0).then_some((hh, mm, ss))
}

/// NMEA angle "d..dmm.mmm.." plus hemisphere -> signed decimal degrees.
/// `deg_digits` is 2 for latitude, 3 for longitude. Both the value and the
/// hemisphere must be present: a value with no hemisphere has no sign.
fn parse_angle(v: &str, hemi: &str, deg_digits: usize, neg: char, pos: char) -> Option<f64> {
    let b = v.as_bytes();
    if b.len() <= deg_digits || !b[..deg_digits].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let deg: f64 = v[..deg_digits].parse().ok()?;
    let minutes: f64 = v[deg_digits..].parse().ok()?;
    let val = deg + minutes / 60.0;
    match hemi.chars().next()? {
        h if h == pos => Some(val),
        h if h == neg => Some(-val),
        _ => None,
    }
}

/// "ddmmyy" -> (year, month, day).
fn parse_date(s: &str) -> Option<(i32, u8, u8)> {
    let b = s.as_bytes();
    if b.len() != 6 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let dd: u8 = s[0..2].parse().ok()?;
    let mm: u8 = s[2..4].parse().ok()?;
    let yy: i32 = s[4..6].parse().ok()?;
    if !(1..=31).contains(&dd) || !(1..=12).contains(&mm) {
        return None;
    }
    // GPS-era pivot: two-digit years 80..99 are 19xx, 00..79 are 20xx.
    let year = if yy >= 80 { 1900 + yy } else { 2000 + yy };
    Some((year, mm, dd))
}

fn parse_gga(f: &[&str], raw: &str) -> Gga {
    Gga {
        hms: parse_hms(field(f, 1)),
        lat_deg: parse_angle(field(f, 2), field(f, 3), 2, 'S', 'N'),
        lon_deg: parse_angle(field(f, 4), field(f, 5), 3, 'W', 'E'),
        quality: num::<u8>(f, 6).unwrap_or(0),
        sats: num::<u8>(f, 7).unwrap_or(0),
        hdop: num(f, 8),
        alt_m: num(f, 9),
        geoid_sep_m: num(f, 11),
        age_s: num(f, 13),
        station_id: num(f, 14),
        raw: raw.to_string(),
    }
}

fn parse_rmc(f: &[&str], raw: &str) -> Rmc {
    Rmc {
        hms: parse_hms(field(f, 1)),
        speed_knots: num(f, 7),
        track_deg: num(f, 8),
        date: parse_date(field(f, 9)),
        raw: raw.to_string(),
    }
}

fn parse_gsa(f: &[&str]) -> Gsa {
    // Indices with 0 = the header token: 12 satellite slots occupy 3..=14,
    // so PDOP = 15, HDOP = 16, VDOP = 17. The original client read HDOP
    // where PDOP was meant; see the dedicated regression test.
    Gsa {
        fix: num::<u8>(f, 2).unwrap_or(0),
        pdop: num(f, 15),
        hdop: num(f, 16),
        vdop: num(f, 17),
    }
}

/// Human name for a GGA fix-quality digit, matching the original client's
/// vocabulary (event log parity depends on these exact strings).
pub fn quality_name(q: u8) -> &'static str {
    match q {
        0 => "Invalid",
        1 => "GPS",
        2 => "DGPS",
        3 => "PPS",
        4 => "RTK Fixed",
        5 => "RTK Float",
        6 => "Estimated",
        7 => "Manual",
        8 => "Simulation",
        9 => "WAAS",
        _ => "Unknown",
    }
}

/// 1 international knot = 1.852 km/h exactly; 1 statute mile = 1609.344 m.
pub fn knots_to_mph(knots: f32) -> f32 {
    (f64::from(knots) * (1852.0 / 1609.344)) as f32
}

/// 1 international knot = 1.852 km/h exactly.
pub fn knots_to_kmh(knots: f32) -> f32 {
    (f64::from(knots) * 1.852) as f32
}

/// 1 international foot = 0.3048 m exactly.
pub fn m_to_ft(m: f32) -> f32 {
    (f64::from(m) / 0.3048) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_eq(a: Option<f32>, b: f32) {
        let a = a.expect("field expected Some");
        assert!((a - b).abs() < 1e-4, "{a} != {b}");
    }

    /// Wrap a body in "$...*XX" with a freshly computed checksum. Used for
    /// synthetic sentences; the golden sentences below pin literal checksums.
    fn wrap(body: &str) -> String {
        let ck = body.bytes().fold(0u8, |a, b| a ^ b);
        format!("${body}*{ck:02X}")
    }

    #[test]
    fn rejects_missing_checksum() {
        assert_eq!(
            parse("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,"),
            Err(NmeaError::MissingChecksum)
        );
    }

    #[test]
    fn rejects_lowercase_or_short_checksum() {
        assert_eq!(parse("$GPGSA,A,3*3a"), Err(NmeaError::MissingChecksum));
        assert_eq!(parse("$GPGSA,A,3*3"), Err(NmeaError::MissingChecksum));
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let r = parse("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*48");
        assert_eq!(
            r,
            Err(NmeaError::ChecksumMismatch {
                computed: 0x47,
                stated: 0x48
            })
        );
    }

    #[test]
    fn rejects_missing_dollar_and_bad_address() {
        assert_eq!(
            parse("GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47"),
            Err(NmeaError::Malformed)
        );
        // A 4-char address is not an error: it classifies as Other, exactly
        // like proprietary $PUBX/$PTNL traffic (space-padded kind).
        assert_eq!(
            parse(&wrap("GPGG,1")),
            Ok(Sentence::Other {
                talker: *b"GP",
                kind: *b"GG "
            })
        );
    }

    #[test]
    fn gga_classic_full_decode() {
        // Textbook GGA sentence; checksum 47 is the published value.
        let line = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
        let Ok(Sentence::Gga(g)) = parse(line) else {
            panic!("expected Gga");
        };
        assert_eq!(g.hms, Some((12, 35, 19.0)));
        let lat = g.lat_deg.unwrap();
        let lon = g.lon_deg.unwrap();
        assert!((lat - (48.0 + 7.038 / 60.0)).abs() < 1e-9, "lat {lat}");
        assert!((lon - (11.0 + 31.0 / 60.0)).abs() < 1e-9, "lon {lon}");
        assert_eq!(g.quality, 1);
        assert_eq!(g.sats, 8);
        f32_eq(g.hdop, 0.9);
        f32_eq(g.alt_m, 545.4);
        f32_eq(g.geoid_sep_m, 46.9);
        assert_eq!(g.age_s, None);
        assert_eq!(g.station_id, None);
        assert_eq!(g.raw, line);
    }

    #[test]
    fn gga_south_west_are_negative() {
        let line =
            wrap("GPGGA,001122.00,3352.12800,S,15112.55800,W,4,10,1.0,200.0,M,1.0,M,5.0,0000");
        let Ok(Sentence::Gga(g)) = parse(&line) else {
            panic!("expected Gga");
        };
        assert!((g.lat_deg.unwrap() + 33.8688).abs() < 1e-9);
        assert!((g.lon_deg.unwrap() + 151.2093).abs() < 1e-9);
        assert_eq!(g.quality, 4);
        assert_eq!(g.station_id, Some(0));
        f32_eq(g.age_s, 5.0);
    }

    #[test]
    fn gga_empty_fields_become_none() {
        let line = wrap("GNGGA,,,,,,0,00,,,M,,M,,");
        let Ok(Sentence::Gga(g)) = parse(&line) else {
            panic!("expected Gga");
        };
        assert_eq!(g.hms, None);
        assert_eq!(g.lat_deg, None);
        assert_eq!(g.lon_deg, None);
        assert_eq!(g.quality, 0);
        assert_eq!(g.sats, 0);
        assert_eq!(g.hdop, None);
        assert_eq!(g.alt_m, None);
        assert_eq!(g.geoid_sep_m, None);
        assert_eq!(g.age_s, None);
        assert_eq!(g.station_id, None);
    }

    #[test]
    fn gga_value_without_hemisphere_is_none() {
        let line = wrap("GPGGA,123519,4807.038,,01131.000,,1,08,0.9,545.4,M,46.9,M,,");
        let Ok(Sentence::Gga(g)) = parse(&line) else {
            panic!("expected Gga");
        };
        assert_eq!(g.lat_deg, None);
        assert_eq!(g.lon_deg, None);
    }

    #[test]
    fn rmc_classic_full_decode() {
        let line = "$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
        let Ok(Sentence::Rmc(r)) = parse(line) else {
            panic!("expected Rmc");
        };
        assert_eq!(r.hms, Some((12, 35, 19.0)));
        f32_eq(r.speed_knots, 22.4);
        f32_eq(r.track_deg, 84.4);
        assert_eq!(r.date, Some((1994, 3, 23)));
        assert_eq!(r.raw, line);
    }

    #[test]
    fn rmc_date_pivot_and_empties() {
        let Ok(Sentence::Rmc(r)) = parse(&wrap("GPRMC,,V,,,,,,,010126,,")) else {
            panic!("expected Rmc");
        };
        assert_eq!(r.date, Some((2026, 1, 1)));
        assert_eq!(r.hms, None);
        assert_eq!(r.speed_knots, None);
        assert_eq!(r.track_deg, None);
    }

    /// Regression guard: the original client displayed HDOP in the PDOP slot
    /// (both read field 16). Ours must map field 15 -> PDOP, 16 -> HDOP,
    /// 17 -> VDOP, with all three values distinct to catch any swap.
    #[test]
    fn gsa_dop_fields_not_swapped() {
        let line = "$GPGSA,A,3,04,05,,09,12,,,24,,,,,2.5,1.3,2.1*39";
        let Ok(Sentence::Gsa(g)) = parse(line) else {
            panic!("expected Gsa");
        };
        assert_eq!(g.fix, 3);
        f32_eq(g.pdop, 2.5);
        f32_eq(g.hdop, 1.3);
        f32_eq(g.vdop, 2.1);
        // The swap bug would have shown 1.3 here.
        assert!(g.pdop.unwrap() > g.hdop.unwrap());

        let Ok(Sentence::Gsa(g)) = parse("$GNGSA,A,3,10,23,,,,,,,,,,,9.9,1.1,5.5*2C") else {
            panic!("expected Gsa");
        };
        f32_eq(g.pdop, 9.9);
        f32_eq(g.hdop, 1.1);
        f32_eq(g.vdop, 5.5);
    }

    #[test]
    fn gsa_missing_dops_tolerated() {
        let Ok(Sentence::Gsa(g)) = parse(&wrap("GPGSA,A,1,,,,,,,,,,,,")) else {
            panic!("expected Gsa");
        };
        assert_eq!(g.fix, 1);
        assert_eq!((g.pdop, g.hdop, g.vdop), (None, None, None));
    }

    #[test]
    fn other_sentences_classified_not_parsed() {
        let line = "$GPVTG,054.7,T,034.4,M,005.5,N,010.2,K*48";
        assert_eq!(
            parse(line),
            Ok(Sentence::Other {
                talker: *b"GP",
                kind: *b"VTG"
            })
        );
    }

    #[test]
    fn proprietary_four_char_addresses_are_other_not_errors() {
        // Trimble ($PTNL) and u-blox ($PUBX) interleave proprietary sentences
        // with standard ones; a healthy stream must not accrue parse errors.
        assert_eq!(
            parse("$PUBX,00*33"),
            Ok(Sentence::Other {
                talker: *b"PU",
                kind: *b"BX "
            })
        );
        assert_eq!(
            parse("$PTNL,GGK*61"),
            Ok(Sentence::Other {
                talker: *b"PT",
                kind: *b"NL "
            })
        );
        // Too-short addresses are still malformed (checksum is valid here,
        // so the failure is address length, not checksum).
        assert_eq!(parse("$GP,x*43"), Err(NmeaError::Malformed));
    }

    #[test]
    fn any_talker_accepted() {
        let Ok(Sentence::Gsa(g)) = parse(&wrap("ZZGSA,A,2,,,,,,,,,,,,,4.0,2.0,3.0")) else {
            panic!("expected Gsa");
        };
        assert_eq!(g.fix, 2);
    }

    #[test]
    fn crlf_and_whitespace_trimmed() {
        let line = "$GPGSA,A,3,04,05,,09,12,,,24,,,,,2.5,1.3,2.1*39\r\n";
        assert!(matches!(parse(line), Ok(Sentence::Gsa(_))));
    }

    #[test]
    fn quality_names_match_original_vocabulary() {
        let expect = [
            (0, "Invalid"),
            (1, "GPS"),
            (2, "DGPS"),
            (3, "PPS"),
            (4, "RTK Fixed"),
            (5, "RTK Float"),
            (6, "Estimated"),
            (7, "Manual"),
            (8, "Simulation"),
            (9, "WAAS"),
            (10, "Unknown"),
            (255, "Unknown"),
        ];
        for (q, name) in expect {
            assert_eq!(quality_name(q), name);
        }
    }

    #[test]
    fn unit_conversions() {
        assert!((knots_to_kmh(1.0) - 1.852).abs() < 1e-6);
        assert!((knots_to_mph(1.0) - 1.150_779_4).abs() < 1e-6);
        assert!((m_to_ft(1.0) - 3.280_84).abs() < 1e-6);
        assert!((m_to_ft(0.3048) - 1.0).abs() < 1e-6);
    }
}
