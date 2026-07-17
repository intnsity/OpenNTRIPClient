//! Diagnostic decode of the RTCM 3.x message types the client explains to
//! support staff. This is not a positioning decoder: observables are never
//! decoded, only identified. Truncated or inconsistent payloads yield
//! `None`, never a panic - the input is an untrusted network stream.
//!
//! Bit layouts follow the RTCM 10403 data fields (DF numbers noted inline).

use crate::geodesy;

/// A decoded diagnostic message.
#[derive(Debug, Clone, PartialEq)]
pub enum Decoded {
    /// 1005/1006 (DF025-DF028): the base's advertised ECEF position, plus
    /// the geodetic equivalent for display.
    BasePosition {
        station_id: u16,
        is_1006: bool,
        ecef_x_m: f64,
        ecef_y_m: f64,
        ecef_z_m: f64,
        /// Only 1006 carries an antenna height.
        antenna_height_m: Option<f64>,
        /// Geodetic (lat deg, lon deg, ellipsoidal height m). None when the
        /// base broadcasts all-zero ECEF - the real-world pattern of an
        /// un-surveyed/un-configured station - so the UI can say "position
        /// not set" instead of showing a fictitious point.
        lla: Option<(f64, f64, f64)>,
    },
    /// 1008/1033 antenna and (1033 only) receiver descriptors. Fields the
    /// message does not carry - or carries as zero-length strings - are
    /// `None`.
    AntennaInfo {
        station_id: u16,
        antenna: String,
        setup_id: u8,
        antenna_serial: Option<String>,
        receiver: Option<String>,
        firmware: Option<String>,
        receiver_serial: Option<String>,
    },
    /// 1029 free-text message (UTF-8, decoded lossily).
    TextMessage {
        station_id: u16,
        mjd: u16,
        seconds_of_day: u32,
        text: String,
    },
    /// 1230 GLONASS code-phase biases: (signal index 0..3, bias meters) for
    /// each signal present in the mask. Signal order: L1 C/A, L1 P, L2 C/A,
    /// L2 P.
    GlonassBiases {
        station_id: u16,
        bias_indicator: bool,
        biases_m: Vec<(u8, f64)>,
    },
    /// MSM 1071-1137 header: enough to show who is sending what.
    /// `signal_ids` are the 1-based positions of set bits in the signal
    /// mask (msb = signal 1).
    MsmHeader {
        constellation: &'static str,
        msg_type: u16,
        station_id: u16,
        epoch: u32,
        num_sats: u8,
        num_signals: u8,
        signal_ids: Vec<u8>,
    },
}

/// MSB-first bit cursor over a payload. All reads bounds-check and return
/// `None` past the end, which `decode` propagates.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Read `bits` (1..=64) as an unsigned value.
    fn read_u(&mut self, bits: u32) -> Option<u64> {
        debug_assert!(bits <= 64);
        let end = self.pos.checked_add(bits as usize)?;
        if end > self.data.len() * 8 {
            return None;
        }
        let mut v = 0u64;
        for p in self.pos..end {
            let bit = (self.data[p >> 3] >> (7 - (p & 7))) & 1;
            v = (v << 1) | u64::from(bit);
        }
        self.pos = end;
        Some(v)
    }

    /// Read `bits` (1..=64) as a two's-complement signed value.
    fn read_s(&mut self, bits: u32) -> Option<i64> {
        debug_assert!((1..=64).contains(&bits));
        let v = self.read_u(bits)?;
        let shift = 64 - bits;
        // Shift the sign bit to bit 63, then arithmetic-shift back down.
        Some(((v << shift) as i64) >> shift)
    }

    fn skip(&mut self, bits: usize) -> Option<()> {
        let end = self.pos.checked_add(bits)?;
        if end > self.data.len() * 8 {
            return None;
        }
        self.pos = end;
        Some(())
    }

    /// Read `len` bytes (bit-aligned or not) as a lossily-decoded string.
    fn read_string(&mut self, len: usize) -> Option<String> {
        let mut bytes = Vec::with_capacity(len);
        for _ in 0..len {
            bytes.push(self.read_u(8)? as u8);
        }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Length-prefixed string (DF-style u8 counter + that many bytes),
    /// with zero length normalized to `None`.
    fn read_counted_string(&mut self) -> Option<Option<String>> {
        let n = self.read_u(8)? as usize;
        if n == 0 {
            return Some(None);
        }
        Some(Some(self.read_string(n)?))
    }
}

/// Decode one message. `payload` is the deframed payload (no header/CRC);
/// its embedded 12-bit message number must agree with `msg_type`. Unknown
/// or truncated messages yield `None`.
pub fn decode(msg_type: u16, payload: &[u8]) -> Option<Decoded> {
    let mut r = BitReader::new(payload);
    // DF002: every RTCM3 payload starts with its own message number.
    if r.read_u(12)? as u16 != msg_type {
        return None;
    }
    match msg_type {
        1005 => decode_base(&mut r, false),
        1006 => decode_base(&mut r, true),
        1008 => decode_1008(&mut r),
        1029 => decode_1029(&mut r),
        1033 => decode_1033(&mut r),
        1230 => decode_1230(&mut r),
        t => {
            let constellation = msm_constellation(t)?;
            decode_msm_header(&mut r, t, constellation)
        }
    }
}

/// 1005 body; 1006 appends DF028 antenna height.
fn decode_base(r: &mut BitReader, is_1006: bool) -> Option<Decoded> {
    let station_id = r.read_u(12)? as u16; // DF003
    // DF021 ITRF year, DF022-DF024 GPS/GLONASS/Galileo indicators,
    // DF141 reference-station indicator: not surfaced.
    r.skip(6 + 1 + 1 + 1 + 1)?;
    let ecef_x_m = r.read_s(38)? as f64 * 1e-4; // DF025, 0.0001 m units
    r.skip(1 + 1)?; // DF142 single oscillator, DF001 reserved
    let ecef_y_m = r.read_s(38)? as f64 * 1e-4; // DF026
    r.skip(2)?; // DF364 quarter-cycle indicator
    let ecef_z_m = r.read_s(38)? as f64 * 1e-4; // DF027
    let antenna_height_m = if is_1006 {
        Some(r.read_u(16)? as f64 * 1e-4) // DF028
    } else {
        None
    };
    let lla = if ecef_x_m == 0.0 && ecef_y_m == 0.0 && ecef_z_m == 0.0 {
        None
    } else {
        Some(geodesy::ecef_to_lla(ecef_x_m, ecef_y_m, ecef_z_m))
    };
    Some(Decoded::BasePosition {
        station_id,
        is_1006,
        ecef_x_m,
        ecef_y_m,
        ecef_z_m,
        antenna_height_m,
        lla,
    })
}

fn decode_1008(r: &mut BitReader) -> Option<Decoded> {
    let station_id = r.read_u(12)? as u16; // DF003
    let n = r.read_u(8)? as usize; // DF029
    let antenna = r.read_string(n)?; // DF030
    let setup_id = r.read_u(8)? as u8; // DF031
    let m = r.read_u(8)? as usize; // DF032
    let serial = r.read_string(m)?; // DF033
    Some(Decoded::AntennaInfo {
        station_id,
        antenna,
        setup_id,
        antenna_serial: if serial.is_empty() {
            None
        } else {
            Some(serial)
        },
        receiver: None,
        firmware: None,
        receiver_serial: None,
    })
}

fn decode_1029(r: &mut BitReader) -> Option<Decoded> {
    let station_id = r.read_u(12)? as u16; // DF003
    let mjd = r.read_u(16)? as u16; // DF051
    let seconds_of_day = r.read_u(17)? as u32; // DF052
    let _num_chars = r.read_u(7)?; // DF138: informational, bytes rule below
    let num_bytes = r.read_u(8)? as usize; // DF139
    let text = r.read_string(num_bytes)?; // DF140
    Some(Decoded::TextMessage {
        station_id,
        mjd,
        seconds_of_day,
        text,
    })
}

fn decode_1033(r: &mut BitReader) -> Option<Decoded> {
    let station_id = r.read_u(12)? as u16; // DF003
    // The antenna descriptor is the one string field that is not optional
    // in the public struct: an empty one stays an empty `String`.
    let n = r.read_u(8)? as usize; // DF029
    let antenna = r.read_string(n)?; // DF030
    let setup_id = r.read_u(8)? as u8; // DF031
    let antenna_serial = r.read_counted_string()?; // DF032/DF033
    let receiver = r.read_counted_string()?; // DF227/DF228
    let firmware = r.read_counted_string()?; // DF229/DF230
    let receiver_serial = r.read_counted_string()?; // DF231/DF232
    Some(Decoded::AntennaInfo {
        station_id,
        antenna,
        setup_id,
        antenna_serial,
        receiver,
        firmware,
        receiver_serial,
    })
}

fn decode_1230(r: &mut BitReader) -> Option<Decoded> {
    let station_id = r.read_u(12)? as u16; // DF003
    let bias_indicator = r.read_u(1)? == 1; // DF421
    r.skip(3)?; // DF001 reserved
    let mask = r.read_u(4)? as u8; // DF422, msb = signal index 0 (L1 C/A)
    let mut biases_m = Vec::new();
    for signal in 0..4u8 {
        if mask & (0x8 >> signal) != 0 {
            let raw = r.read_s(16)?; // DF423..DF426, 0.02 m units
            biases_m.push((signal, raw as f64 * 0.02));
        }
    }
    Some(Decoded::GlonassBiases {
        station_id,
        bias_indicator,
        biases_m,
    })
}

fn decode_msm_header(
    r: &mut BitReader,
    msg_type: u16,
    constellation: &'static str,
) -> Option<Decoded> {
    let station_id = r.read_u(12)? as u16; // DF003
    let epoch = r.read_u(30)? as u32; // GNSS epoch time (DF004 family)
    // DF393 multiple-message, DF409 IODS, DF001 reserved, DF411 clock
    // steering, DF412 external clock, DF417 smoothing type, DF418 interval.
    r.skip(1 + 3 + 7 + 2 + 2 + 1 + 3)?;
    let sat_mask = r.read_u(64)?; // DF394, msb = satellite 1
    let sig_mask = r.read_u(32)? as u32; // DF395, msb = signal 1
    let num_sats = sat_mask.count_ones() as u8;
    let num_signals = sig_mask.count_ones() as u8;
    // DF396 cell mask is num_sats x num_signals bits; it must be present
    // even though we do not decode the observables that follow it.
    r.skip(usize::from(num_sats) * usize::from(num_signals))?;
    let signal_ids = (0u32..32)
        .filter(|i| sig_mask & (0x8000_0000 >> i) != 0)
        .map(|i| (i + 1) as u8)
        .collect();
    Some(Decoded::MsmHeader {
        constellation,
        msg_type,
        station_id,
        epoch,
        num_sats,
        num_signals,
        signal_ids,
    })
}

/// Constellation for an MSM message number, `None` if not an MSM type.
fn msm_constellation(t: u16) -> Option<&'static str> {
    if !(1..=7).contains(&(t % 10)) {
        return None;
    }
    match t / 10 {
        107 => Some("GPS"),
        108 => Some("GLONASS"),
        109 => Some("Galileo"),
        110 => Some("SBAS"),
        111 => Some("QZSS"),
        112 => Some("BeiDou"),
        113 => Some("NavIC"),
        _ => None,
    }
}

const MSM_NAMES: [[&str; 7]; 7] = [
    [
        "GPS MSM1", "GPS MSM2", "GPS MSM3", "GPS MSM4", "GPS MSM5", "GPS MSM6", "GPS MSM7",
    ],
    [
        "GLONASS MSM1",
        "GLONASS MSM2",
        "GLONASS MSM3",
        "GLONASS MSM4",
        "GLONASS MSM5",
        "GLONASS MSM6",
        "GLONASS MSM7",
    ],
    [
        "Galileo MSM1",
        "Galileo MSM2",
        "Galileo MSM3",
        "Galileo MSM4",
        "Galileo MSM5",
        "Galileo MSM6",
        "Galileo MSM7",
    ],
    [
        "SBAS MSM1",
        "SBAS MSM2",
        "SBAS MSM3",
        "SBAS MSM4",
        "SBAS MSM5",
        "SBAS MSM6",
        "SBAS MSM7",
    ],
    [
        "QZSS MSM1",
        "QZSS MSM2",
        "QZSS MSM3",
        "QZSS MSM4",
        "QZSS MSM5",
        "QZSS MSM6",
        "QZSS MSM7",
    ],
    [
        "BeiDou MSM1",
        "BeiDou MSM2",
        "BeiDou MSM3",
        "BeiDou MSM4",
        "BeiDou MSM5",
        "BeiDou MSM6",
        "BeiDou MSM7",
    ],
    [
        "NavIC MSM1",
        "NavIC MSM2",
        "NavIC MSM3",
        "NavIC MSM4",
        "NavIC MSM5",
        "NavIC MSM6",
        "NavIC MSM7",
    ],
];

fn msm_name(t: u16) -> Option<&'static str> {
    let c = usize::from((t / 10).checked_sub(107)?);
    let n = t % 10;
    if c > 6 || !(1..=7).contains(&n) {
        return None;
    }
    Some(MSM_NAMES[c][usize::from(n - 1)])
}

/// Human name for the common message types; `None` for anything else.
pub fn type_name(msg_type: u16) -> Option<&'static str> {
    if let Some(name) = msm_name(msg_type) {
        return Some(name);
    }
    let name = match msg_type {
        1001 => "GPS L1 observables",
        1002 => "GPS L1 observables (extended)",
        1003 => "GPS L1/L2 observables",
        1004 => "GPS L1/L2 observables (extended)",
        1005 => "Station coordinates",
        1006 => "Station coordinates + antenna height",
        1007 => "Antenna descriptor",
        1008 => "Antenna descriptor + serial",
        1009 => "GLONASS L1 observables",
        1010 => "GLONASS L1 observables (extended)",
        1011 => "GLONASS L1/L2 observables",
        1012 => "GLONASS L1/L2 observables (extended)",
        1013 => "System parameters",
        1019 => "GPS ephemeris",
        1020 => "GLONASS ephemeris",
        1029 => "Text message",
        1033 => "Receiver + antenna descriptors",
        1042 => "BeiDou ephemeris",
        1044 => "QZSS ephemeris",
        1045 => "Galileo F/NAV ephemeris",
        1046 => "Galileo I/NAV ephemeris",
        1230 => "GLONASS code-phase biases",
        4070..=4095 => "Proprietary vendor message",
        _ => return None,
    };
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MSB-first bit assembler, the mirror of BitReader, for synthesizing
    /// payloads bit-exactly.
    struct BitWriter {
        bytes: Vec<u8>,
        len_bits: usize,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                len_bits: 0,
            }
        }

        fn put(&mut self, bits: u32, v: u64) {
            assert!(bits <= 64);
            if bits < 64 {
                assert!(v < 1u64 << bits, "value {v} does not fit {bits} bits");
            }
            for i in (0..bits).rev() {
                let bit = ((v >> i) & 1) as u8;
                if self.len_bits.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                let idx = self.len_bits / 8;
                self.bytes[idx] |= bit << (7 - (self.len_bits % 8));
                self.len_bits += 1;
            }
        }

        fn put_s(&mut self, bits: u32, v: i64) {
            let mask = if bits == 64 {
                u64::MAX
            } else {
                (1u64 << bits) - 1
            };
            self.put(bits, (v as u64) & mask);
        }

        fn put_str(&mut self, s: &str) {
            for b in s.bytes() {
                self.put(8, u64::from(b));
            }
        }

        fn finish(self) -> Vec<u8> {
            self.bytes
        }
    }

    /// Payload of the canonical 1005 example frame (RTCM 3 documentation).
    const EXAMPLE_1005: [u8; 19] = [
        0x3E, 0xD7, 0xD3, 0x02, 0x02, 0x98, 0x0E, 0xDE, 0xEF, 0x34, 0xB4, 0xBD, 0x62, 0xAC, 0x09,
        0x41, 0x98, 0x6F, 0x33,
    ];

    fn assert_close(a: f64, b: f64, tol: f64, what: &str) {
        assert!((a - b).abs() < tol, "{what}: {a} vs {b}");
    }

    /// Every strict prefix of a payload must decode to None, never panic.
    fn assert_all_truncations_none(msg_type: u16, payload: &[u8]) {
        for len in 0..payload.len() {
            assert_eq!(
                decode(msg_type, &payload[..len]),
                None,
                "truncation to {len} bytes"
            );
        }
    }

    fn build_1005_body(w: &mut BitWriter, station: u64, x: i64, y: i64, z: i64) {
        w.put(12, station);
        w.put(6, 5); // ITRF year
        w.put(1, 1); // GPS
        w.put(1, 1); // GLONASS
        w.put(1, 0); // Galileo
        w.put(1, 0); // reference station
        w.put_s(38, x);
        w.put(1, 1); // single oscillator
        w.put(1, 0); // reserved
        w.put_s(38, y);
        w.put(2, 2); // quarter cycle
        w.put_s(38, z);
    }

    #[test]
    fn decode_1005_canonical_example() {
        // Independently bit-decoded and cross-checked against published
        // values for this documentation frame.
        let Some(Decoded::BasePosition {
            station_id,
            is_1006,
            ecef_x_m,
            ecef_y_m,
            ecef_z_m,
            antenna_height_m,
            lla,
        }) = decode(1005, &EXAMPLE_1005)
        else {
            panic!("expected BasePosition");
        };
        assert_eq!(station_id, 2003);
        assert!(!is_1006);
        assert_eq!(antenna_height_m, None);
        assert_close(ecef_x_m, 1_114_104.599_9, 1e-6, "x");
        assert_close(ecef_y_m, -4_850_729.710_8, 1e-6, "y");
        assert_close(ecef_z_m, 3_975_521.464_3, 1e-6, "z");
        let (lat_deg, lon_deg, alt_m) = lla.expect("surveyed base has a geodetic position");
        assert_close(lat_deg, 38.804_759_4, 1e-6, "lat");
        assert_close(lon_deg, -77.064_773_6, 1e-6, "lon");
        assert_close(alt_m, 114.561_1, 1e-3, "alt");
        assert_all_truncations_none(1005, &EXAMPLE_1005);
    }

    #[test]
    fn roundtrip_1005_negative_ecef() {
        let mut w = BitWriter::new();
        w.put(12, 1005);
        // -1114104.5999 m, -4850729.7108 m, +3975521.4643 m in 0.1 mm units.
        build_1005_body(
            &mut w,
            4095,
            -11_141_045_999,
            -48_507_297_108,
            39_755_214_643,
        );
        let payload = w.finish();
        assert_eq!(payload.len(), 19, "1005 is 152 bits");

        let Some(Decoded::BasePosition {
            station_id,
            is_1006,
            ecef_x_m,
            ecef_y_m,
            ecef_z_m,
            antenna_height_m,
            lla,
        }) = decode(1005, &payload)
        else {
            panic!("expected BasePosition");
        };
        assert_eq!(station_id, 4095);
        assert!(!is_1006);
        assert_eq!(antenna_height_m, None);
        assert_close(ecef_x_m, -1_114_104.599_9, 1e-9, "x");
        assert_close(ecef_y_m, -4_850_729.710_8, 1e-9, "y");
        assert_close(ecef_z_m, 3_975_521.464_3, 1e-9, "z");
        // The geodetic fields must be exactly what geodesy computes.
        let (lat, lon, alt) = geodesy::ecef_to_lla(ecef_x_m, ecef_y_m, ecef_z_m);
        assert_eq!(lla, Some((lat, lon, alt)));
        assert_all_truncations_none(1005, &payload);
    }

    #[test]
    fn all_zero_ecef_means_position_not_set() {
        // Un-surveyed/un-configured bases really broadcast 1005 with zero
        // ECEF; that must surface as "no position", never as a fictitious
        // geodetic point.
        let mut w = BitWriter::new();
        w.put(12, 1005);
        build_1005_body(&mut w, 1, 0, 0, 0);
        let payload = w.finish();
        let Some(Decoded::BasePosition { lla, ecef_x_m, .. }) = decode(1005, &payload) else {
            panic!("expected BasePosition");
        };
        assert_eq!(ecef_x_m, 0.0);
        assert_eq!(lla, None);
    }

    #[test]
    fn roundtrip_1006_with_antenna_height() {
        let mut w = BitWriter::new();
        w.put(12, 1006);
        build_1005_body(&mut w, 77, 11_141_045_999, -48_507_297_108, 39_755_214_643);
        w.put(16, 12_345); // 1.2345 m
        let payload = w.finish();
        assert_eq!(payload.len(), 21, "1006 is 168 bits");

        let Some(Decoded::BasePosition {
            station_id,
            is_1006,
            antenna_height_m,
            ecef_x_m,
            ..
        }) = decode(1006, &payload)
        else {
            panic!("expected BasePosition");
        };
        assert_eq!(station_id, 77);
        assert!(is_1006);
        assert_close(antenna_height_m.unwrap(), 1.234_5, 1e-9, "height");
        assert_close(ecef_x_m, 1_114_104.599_9, 1e-9, "x");
        assert_all_truncations_none(1006, &payload);
    }

    #[test]
    fn roundtrip_1008_antenna_and_serial() {
        let mut w = BitWriter::new();
        w.put(12, 1008);
        w.put(12, 512);
        w.put(8, 16);
        w.put_str("TRM55971.00 TZGD");
        w.put(8, 3);
        w.put(8, 10);
        w.put_str("1441112601");
        let payload = w.finish();

        assert_eq!(
            decode(1008, &payload),
            Some(Decoded::AntennaInfo {
                station_id: 512,
                antenna: "TRM55971.00 TZGD".to_string(),
                setup_id: 3,
                antenna_serial: Some("1441112601".to_string()),
                receiver: None,
                firmware: None,
                receiver_serial: None,
            })
        );
        assert_all_truncations_none(1008, &payload);
    }

    #[test]
    fn decode_1008_empty_serial_is_none() {
        let mut w = BitWriter::new();
        w.put(12, 1008);
        w.put(12, 1);
        w.put(8, 4);
        w.put_str("ADVN");
        w.put(8, 0); // setup
        w.put(8, 0); // zero-length serial
        let payload = w.finish();
        let Some(Decoded::AntennaInfo { antenna_serial, .. }) = decode(1008, &payload) else {
            panic!("expected AntennaInfo");
        };
        assert_eq!(antenna_serial, None);
    }

    #[test]
    fn roundtrip_1033_receiver_and_antenna() {
        let mut w = BitWriter::new();
        w.put(12, 1033);
        w.put(12, 1234);
        w.put(8, 13);
        w.put_str("JAV_GRANT-G3T");
        w.put(8, 9); // setup id
        w.put(8, 5);
        w.put_str("00082");
        w.put(8, 20);
        w.put_str("JAVAD TRE_G3TH DELTA");
        w.put(8, 5);
        w.put_str("3.6.7");
        w.put(8, 5);
        w.put_str("02460");
        let payload = w.finish();

        assert_eq!(
            decode(1033, &payload),
            Some(Decoded::AntennaInfo {
                station_id: 1234,
                antenna: "JAV_GRANT-G3T".to_string(),
                setup_id: 9,
                antenna_serial: Some("00082".to_string()),
                receiver: Some("JAVAD TRE_G3TH DELTA".to_string()),
                firmware: Some("3.6.7".to_string()),
                receiver_serial: Some("02460".to_string()),
            })
        );
        assert_all_truncations_none(1033, &payload);
    }

    #[test]
    fn decode_1033_empty_optionals_are_none() {
        let mut w = BitWriter::new();
        w.put(12, 1033);
        w.put(12, 9);
        w.put(8, 3);
        w.put_str("ANT");
        w.put(8, 0); // setup id
        w.put(8, 0); // antenna serial: absent
        w.put(8, 4);
        w.put_str("RCVR");
        w.put(8, 0); // firmware: absent
        w.put(8, 0); // receiver serial: absent
        let payload = w.finish();
        assert_eq!(
            decode(1033, &payload),
            Some(Decoded::AntennaInfo {
                station_id: 9,
                antenna: "ANT".to_string(),
                setup_id: 0,
                antenna_serial: None,
                receiver: Some("RCVR".to_string()),
                firmware: None,
                receiver_serial: None,
            })
        );
    }

    #[test]
    fn roundtrip_1029_non_ascii_text() {
        let text = "M\u{fc}nchen \u{2713} base moved";
        let mut w = BitWriter::new();
        w.put(12, 1029);
        w.put(12, 300);
        w.put(16, 60_677); // MJD
        w.put(17, 45_000); // seconds of day
        w.put(7, text.chars().count() as u64);
        w.put(8, text.len() as u64);
        w.put_str(text);
        let payload = w.finish();

        assert_eq!(
            decode(1029, &payload),
            Some(Decoded::TextMessage {
                station_id: 300,
                mjd: 60_677,
                seconds_of_day: 45_000,
                text: text.to_string(),
            })
        );
        assert_all_truncations_none(1029, &payload);
    }

    #[test]
    fn decode_1029_invalid_utf8_is_lossy_not_fatal() {
        let mut w = BitWriter::new();
        w.put(12, 1029);
        w.put(12, 1);
        w.put(16, 60_000);
        w.put(17, 1);
        w.put(7, 2);
        w.put(8, 2);
        w.put(8, 0xC3); // dangling UTF-8 lead byte
        w.put(8, 0x28);
        let payload = w.finish();
        let Some(Decoded::TextMessage { text, .. }) = decode(1029, &payload) else {
            panic!("expected TextMessage");
        };
        assert_eq!(text, "\u{fffd}(");
    }

    #[test]
    fn roundtrip_1230_masked_biases() {
        let mut w = BitWriter::new();
        w.put(12, 1230);
        w.put(12, 42);
        w.put(1, 1); // bias indicator
        w.put(3, 0); // reserved
        w.put(4, 0b1010); // signals 0 (L1 C/A) and 2 (L2 C/A)
        w.put_s(16, -733); // -14.66 m
        w.put_s(16, 512); // 10.24 m
        let payload = w.finish();

        let Some(Decoded::GlonassBiases {
            station_id,
            bias_indicator,
            biases_m,
        }) = decode(1230, &payload)
        else {
            panic!("expected GlonassBiases");
        };
        assert_eq!(station_id, 42);
        assert!(bias_indicator);
        assert_eq!(biases_m.len(), 2);
        assert_eq!(biases_m[0].0, 0);
        assert_close(biases_m[0].1, -14.66, 1e-9, "bias 0");
        assert_eq!(biases_m[1].0, 2);
        assert_close(biases_m[1].1, 10.24, 1e-9, "bias 2");
        assert_all_truncations_none(1230, &payload);
    }

    #[test]
    fn decode_1230_empty_mask() {
        let mut w = BitWriter::new();
        w.put(12, 1230);
        w.put(12, 7);
        w.put(1, 0);
        w.put(3, 0);
        w.put(4, 0);
        let payload = w.finish();
        assert_eq!(
            decode(1230, &payload),
            Some(Decoded::GlonassBiases {
                station_id: 7,
                bias_indicator: false,
                biases_m: Vec::new(),
            })
        );
    }

    #[test]
    fn roundtrip_msm7_header() {
        let mut w = BitWriter::new();
        w.put(12, 1077); // GPS MSM7
        w.put(12, 1234);
        w.put(30, 123_456_789); // epoch
        w.put(1, 1); // multiple message
        w.put(3, 3); // IODS
        w.put(7, 0); // reserved
        w.put(2, 2); // clock steering
        w.put(2, 1); // external clock
        w.put(1, 1); // smoothing
        w.put(3, 5); // smoothing interval
        // Satellites 1, 8, 64 (msb = sat 1).
        w.put(64, (1u64 << 63) | (1u64 << 56) | 1);
        // Signals 2, 15, 32 (msb = signal 1).
        w.put(32, (1u64 << 30) | (1u64 << 17) | 1);
        w.put(9, 0b1_0101_0011); // 3x3 cell mask, arbitrary pattern
        w.put(24, 0xABCDEF); // pretend observables follow the header
        let payload = w.finish();

        let Some(Decoded::MsmHeader {
            constellation,
            msg_type,
            station_id,
            epoch,
            num_sats,
            num_signals,
            signal_ids,
        }) = decode(1077, &payload)
        else {
            panic!("expected MsmHeader");
        };
        assert_eq!(constellation, "GPS");
        assert_eq!(msg_type, 1077);
        assert_eq!(station_id, 1234);
        assert_eq!(epoch, 123_456_789);
        assert_eq!(num_sats, 3);
        assert_eq!(num_signals, 3);
        assert_eq!(signal_ids, vec![2, 15, 32]);
        // Header is 178 bits: any payload under 23 bytes must be rejected.
        for len in 0..23 {
            assert_eq!(decode(1077, &payload[..len]), None, "truncation to {len}");
        }
    }

    #[test]
    fn msm_empty_masks_and_all_constellations() {
        for (t, want) in [
            (1071u16, "GPS"),
            (1082, "GLONASS"),
            (1093, "Galileo"),
            (1104, "SBAS"),
            (1115, "QZSS"),
            (1126, "BeiDou"),
            (1137, "NavIC"),
        ] {
            let mut w = BitWriter::new();
            w.put(12, u64::from(t));
            w.put(12, 55);
            w.put(30, 1);
            w.put(1 + 3 + 7 + 2 + 2 + 1 + 3, 0);
            w.put(64, 0); // no satellites
            w.put(32, 0); // no signals -> zero-bit cell mask
            let payload = w.finish();
            let Some(Decoded::MsmHeader {
                constellation,
                num_sats,
                num_signals,
                signal_ids,
                ..
            }) = decode(t, &payload)
            else {
                panic!("expected MsmHeader for {t}");
            };
            assert_eq!(constellation, want);
            assert_eq!((num_sats, num_signals), (0, 0));
            assert!(signal_ids.is_empty());
        }
    }

    #[test]
    fn embedded_message_number_must_match() {
        let mut w = BitWriter::new();
        w.put(12, 1006);
        build_1005_body(&mut w, 1, 1, 1, 1);
        w.put(16, 0);
        let payload = w.finish();
        assert_eq!(decode(1005, &payload), None);
        assert!(decode(1006, &payload).is_some());
    }

    #[test]
    fn unknown_and_reserved_types_are_none() {
        // Non-MSM numbers inside and around the MSM block, plus types we
        // name but deliberately do not decode (1004, 1007, 1019...).
        for t in [
            0u16, 999, 1004, 1007, 1013, 1019, 1070, 1078, 1080, 1138, 4076, 4095,
        ] {
            let mut w = BitWriter::new();
            w.put(12, u64::from(t));
            w.put(64, 0xDEAD_BEEF_0BAD_F00D); // arbitrary body
            let payload = w.finish();
            assert_eq!(decode(t, &payload), None, "type {t}");
        }
    }

    #[test]
    fn empty_payload_is_none() {
        assert_eq!(decode(1005, &[]), None);
        assert_eq!(decode(1077, &[0x43]), None); // 8 bits < message number
    }

    #[test]
    fn type_names_common_set() {
        for (t, want) in [
            (1001u16, "GPS L1 observables"),
            (1004, "GPS L1/L2 observables (extended)"),
            (1005, "Station coordinates"),
            (1006, "Station coordinates + antenna height"),
            (1007, "Antenna descriptor"),
            (1008, "Antenna descriptor + serial"),
            (1012, "GLONASS L1/L2 observables (extended)"),
            (1013, "System parameters"),
            (1019, "GPS ephemeris"),
            (1020, "GLONASS ephemeris"),
            (1029, "Text message"),
            (1033, "Receiver + antenna descriptors"),
            (1042, "BeiDou ephemeris"),
            (1044, "QZSS ephemeris"),
            (1045, "Galileo F/NAV ephemeris"),
            (1046, "Galileo I/NAV ephemeris"),
            (1230, "GLONASS code-phase biases"),
            (1074, "GPS MSM4"),
            (1077, "GPS MSM7"),
            (1081, "GLONASS MSM1"),
            (1087, "GLONASS MSM7"),
            (1097, "Galileo MSM7"),
            (1101, "SBAS MSM1"),
            (1117, "QZSS MSM7"),
            (1127, "BeiDou MSM7"),
            (1131, "NavIC MSM1"),
            (1137, "NavIC MSM7"),
            (4070, "Proprietary vendor message"),
            (4095, "Proprietary vendor message"),
        ] {
            assert_eq!(type_name(t), Some(want), "type {t}");
        }
    }

    #[test]
    fn type_names_unassigned_are_none() {
        for t in [
            0u16, 1000, 1014, 1021, 1043, 1047, 1070, 1078, 1079, 1080, 1090, 1138, 1140, 4069,
            4096,
        ] {
            assert_eq!(type_name(t), None, "type {t}");
        }
    }
}
