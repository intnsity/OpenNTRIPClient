//! Sans-IO NTRIP v1/v2 client protocol engine.
//!
//! The session owns no sockets: the caller feeds received bytes and clock
//! instants in, and the session emits outputs (verbatim protocol log lines,
//! correction payloads, GGA send requests, close reasons). Every caster quirk
//! is therefore unit-testable from canned byte feeds.
//!
//! The engine is deliberately liberal in what it accepts: nonconforming
//! casters are the norm in the field, and this is a diagnostic tool whose job
//! is to show support staff what actually happened, not to enforce a spec.

mod base64;
mod chunked;
mod request;
mod session;
pub mod sourcetable;

pub use session::NtripSession;

/// Protocol revision to request. V1 speaks the original ICY-flavored dialect
/// (HTTP/1.0 request line); V2 is the standardized HTTP/1.1 mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NtripVersion {
    V1,
    V2,
}

/// Ntrip performs the request/response handshake; RawTcp treats the socket
/// as a bare correction pipe (no request, no GGA, straight to streaming).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Ntrip,
    RawTcp,
}

/// When the session should ask the caller to send a NMEA GGA position.
/// `WhenRequired` carries the sourcetable's per-stream flag so the policy is
/// self-contained: the session never needs to see the table itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GgaPolicy {
    Off,
    WhenRequired { stream_requires: bool },
    Always,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub host: String,
    pub port: u16,
    /// Empty means this is a sourcetable request, not a stream request.
    pub mountpoint: String,
    /// Empty means no Authorization header is sent.
    pub username: String,
    pub password: String,
    pub version: NtripVersion,
    pub transport: Transport,
    pub user_agent: String,
    pub gga: GgaPolicy,
}

/// Everything a session can ask of its caller. Emitted in order; a caller
/// that replays the same byte feed gets the identical output sequence
/// regardless of how the bytes were packetized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// A request line we sent, verbatim without CRLF (for the connection log).
    ProtocolTx(String),
    /// A status/header line received, verbatim without CRLF.
    ProtocolRx(String),
    /// Caller must send a GGA sentence now and then call `gga_sent`.
    GgaDue,
    /// Correction payload bytes (already de-chunked when applicable).
    Corrections(Vec<u8>),
    /// A complete raw sourcetable body.
    Sourcetable(Vec<u8>),
    /// Terminal: the session is Done after this, exactly once, ever.
    Close(CloseReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    Unauthorized,
    /// The caster answered a mountpoint request with a sourcetable: NTRIP's
    /// way of saying "no such mountpoint". The table rides along because it
    /// is the top support artifact for diagnosing a typo'd mount name.
    MountpointNotFound {
        sourcetable: Vec<u8>,
    },
    UnknownResponse {
        raw: Vec<u8>,
    },
    FirstResponseTimeout,
    SourcetableTimeout,
    StreamSilence,
    StreamCorrupt {
        detail: String,
    },
    RemoteClosed,
    Cancelled,
}

impl CloseReason {
    /// Only transient, environment-shaped failures warrant reconnecting.
    /// Auth and protocol failures would just fail identically forever, and
    /// Cancelled was the user's own decision.
    pub fn auto_reconnect(&self) -> bool {
        matches!(
            self,
            CloseReason::FirstResponseTimeout
                | CloseReason::SourcetableTimeout
                | CloseReason::StreamSilence
                | CloseReason::StreamCorrupt { .. }
                | CloseReason::RemoteClosed
        )
    }
}
