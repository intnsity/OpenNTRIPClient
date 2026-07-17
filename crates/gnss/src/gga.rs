//! Fabricated GGA sentences for casters whose mountpoint requires a client
//! position (VRS/network RTK, geofenced services).
//!
//! The sentence claims a healthy RTK fix - quality 4, 10 satellites, HDOP
//! 1.0, altitude 200 m, geoid separation 1 m, station 0000 - byte-for-byte
//! parity with the original client's template so casters that fingerprint
//! clients see nothing new. Only position, UTC time, and the cycling
//! correction age vary.

use crate::clock::UtcTime;

/// Build the fabricated GGA sentence, terminated with CRLF.
///
/// Positions are emitted as NMEA ddmm.mmmmm / dddmm.mmmmm with the hemisphere
/// letter carrying the sign. The correction age cycles (sec % 6) + 3 so the
/// caster always sees a fresh-looking 3..8 s age.
pub fn fabricate(lat_deg: f64, lon_deg: f64, utc: &UtcTime) -> String {
    let (lat_hemi, lat_abs) = if lat_deg < 0.0 {
        ('S', -lat_deg)
    } else {
        ('N', lat_deg)
    };
    let (lon_hemi, lon_abs) = if lon_deg < 0.0 {
        ('W', -lon_deg)
    } else {
        ('E', lon_deg)
    };
    let (lat_whole, lat_min) = deg_min(lat_abs);
    let (lon_whole, lon_min) = deg_min(lon_abs);
    let age = u32::from(utc.sec) % 6 + 3;
    let body = format!(
        "GPGGA,{:02}{:02}{:02}.00,{lat_whole:02}{lat_min:08.5},{lat_hemi},{lon_whole:03}{lon_min:08.5},{lon_hemi},4,10,1.0,200.0,M,1.0,M,{age}.0,0000",
        utc.hour, utc.min, utc.sec,
    );
    let ck = body.bytes().fold(0u8, |a, b| a ^ b);
    format!("${body}*{ck:02X}\r\n")
}

/// Split an absolute coordinate into NMEA whole degrees and decimal minutes.
///
/// Minutes are later formatted `{:08.5}` = "mm.mmmmm"; a coordinate within
/// ~8e-8 deg below a whole degree makes the minutes ROUND to 60.00000, which
/// NMEA forbids (minutes must be < 60) - a strict caster parser may reject
/// the sentence or compute dd+1 via a different path than a lenient one. So
/// any minutes value that would round up to 60 at 5 decimals carries into
/// the degrees here, before formatting.
fn deg_min(abs_deg: f64) -> (u32, f64) {
    let whole = abs_deg.trunc();
    let min = (abs_deg - whole) * 60.0;
    if min >= 59.999_995 {
        (whole as u32 + 1, 0.0)
    } else {
        (whole as u32, min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nmea::{self, Sentence};

    fn at(hour: u8, min: u8, sec: u8) -> UtcTime {
        UtcTime {
            year: 2026,
            month: 7,
            day: 15,
            hour,
            min,
            sec,
            centis: 0,
        }
    }

    /// Round-trip a fabricated sentence through the strict parser and check
    /// the healthy-fix claims survive.
    fn assert_roundtrip(lat: f64, lon: f64, utc: &UtcTime) {
        let line = fabricate(lat, lon, utc);
        let Ok(Sentence::Gga(g)) = nmea::parse(&line) else {
            panic!("fabricated sentence failed to parse: {line:?}");
        };
        assert_eq!(g.quality, 4, "quality in {line:?}");
        assert_eq!(g.sats, 10, "sats in {line:?}");
        // 5 decimal digits of minutes = 1.67e-7 degrees of resolution.
        assert!((g.lat_deg.unwrap() - lat).abs() < 1e-6, "lat in {line:?}");
        assert!((g.lon_deg.unwrap() - lon).abs() < 1e-6, "lon in {line:?}");
        assert_eq!(g.station_id, Some(0));
    }

    #[test]
    fn golden_portland_north_west() {
        // Pinned complete sentence: any byte-level drift is a parity break.
        let got = fabricate(45.5152, -122.6784, &at(12, 34, 56));
        assert_eq!(
            got,
            "$GPGGA,123456.00,4530.91200,N,12240.70400,W,4,10,1.0,200.0,M,1.0,M,5.0,0000*6A\r\n"
        );
        assert_roundtrip(45.5152, -122.6784, &at(12, 34, 56));
    }

    #[test]
    fn golden_sydney_south_east() {
        let got = fabricate(-33.8688, 151.2093, &at(23, 59, 7));
        assert_eq!(
            got,
            "$GPGGA,235907.00,3352.12800,S,15112.55800,E,4,10,1.0,200.0,M,1.0,M,4.0,0000*65\r\n"
        );
        assert_roundtrip(-33.8688, 151.2093, &at(23, 59, 7));
    }

    #[test]
    fn age_cycles_three_through_eight() {
        for sec in 0..60u8 {
            let line = fabricate(1.0, 2.0, &at(0, 0, sec));
            let expect = format!(",{}.0,0000*", u32::from(sec) % 6 + 3);
            assert!(line.contains(&expect), "sec {sec}: {line:?}");
        }
    }

    #[test]
    fn zero_padding_of_small_angles() {
        let line = fabricate(5.0001, 7.5, &at(1, 2, 3));
        assert!(
            line.starts_with("$GPGGA,010203.00,0500.00600,N,00730.00000,E,"),
            "{line:?}"
        );
        assert_roundtrip(5.0001, 7.5, &at(1, 2, 3));
    }

    #[test]
    fn near_zero_negatives_keep_hemisphere() {
        let line = fabricate(-0.5, -0.25, &at(6, 7, 8));
        assert!(
            line.starts_with("$GPGGA,060708.00,0030.00000,S,00015.00000,W,"),
            "{line:?}"
        );
        assert_roundtrip(-0.5, -0.25, &at(6, 7, 8));
    }

    /// A coordinate a hair under a whole degree must carry into the degrees
    /// instead of printing the illegal minutes value "60.00000" (NMEA
    /// requires minutes < 60; "4460.00000" reads as 44 deg 60 min to a
    /// strict caster parser).
    #[test]
    fn minutes_that_round_to_sixty_carry_into_degrees() {
        let line = fabricate(44.999_999_99, -119.999_999_99, &at(1, 2, 3));
        assert!(
            line.starts_with("$GPGGA,010203.00,4500.00000,N,12000.00000,W,"),
            "{line:?}"
        );
        assert_roundtrip(44.999_999_99, -119.999_999_99, &at(1, 2, 3));
        // Just under the carry threshold, minutes stay below 60.
        let line = fabricate(44.999_99, 0.0, &at(1, 2, 3));
        assert!(
            line.starts_with("$GPGGA,010203.00,4459.99940,N,"),
            "{line:?}"
        );
    }

    #[test]
    fn equator_meridian_zero_is_north_east() {
        let line = fabricate(0.0, 0.0, &at(0, 0, 0));
        assert!(
            line.starts_with("$GPGGA,000000.00,0000.00000,N,00000.00000,E,"),
            "{line:?}"
        );
    }

    #[test]
    fn roundtrip_grid_of_positions() {
        let lats = [-89.9, -45.123456, -0.0001, 0.0, 33.0, 89.9];
        let lons = [-179.9, -122.6784, -0.5, 0.0, 151.2093, 179.9];
        for &lat in &lats {
            for &lon in &lons {
                assert_roundtrip(lat, lon, &at(11, 22, 33));
            }
        }
    }
}
