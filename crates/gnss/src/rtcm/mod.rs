//! RTCM 3.x transport and diagnostic decode.
//!
//! - [`crc24q`]: the CRC-24Q frame integrity check.
//! - [`frame`]: resilient stream deframing with garbage/CRC accounting.
//! - [`decode`]: bit-level decode of the message types the client explains
//!   to support staff (base position, antenna info, text, biases, MSM
//!   headers). Everything else is counted by type, not decoded.

pub mod crc24q;
pub mod decode;
pub mod frame;
