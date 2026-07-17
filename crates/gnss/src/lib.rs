//! GNSS data handling: NMEA 0183 parsing, GGA fabrication, RTCM3 framing and
//! diagnostic decoding, WGS-84 geodesy, and civil-time helpers.
//!
//! Module map:
//! - [`clock`]: UTC and local civil time without a date/time dependency.
//! - [`nmea`]: checksum-validated parsing of GGA / RMC / GSA sentences.
//! - [`gga`]: fabricated GGA sentences for casters that require a position.
//! - [`rtcm`]: RTCM 3.x CRC-24Q, stream deframing, and diagnostic decode.
//! - [`geodesy`]: WGS-84 ECEF <-> geodetic conversions and distances.

pub mod clock;
pub mod geodesy;
pub mod gga;
pub mod nmea;
pub mod rtcm;
