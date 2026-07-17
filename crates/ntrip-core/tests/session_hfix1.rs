//! Regression pins for the post-integration review fixes (bucket 1). Every
//! test here exercises a split-invariance or misclassification bug found by
//! adversarial review, feeding the same byte stream whole-buffer and
//! byte-at-a-time and demanding identical results.

use std::time::{Duration, Instant};

use ntrip_core::{
    CloseReason, GgaPolicy, NtripSession, NtripVersion, Output, SessionConfig, Transport,
};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn cfg(mount: &str, version: NtripVersion) -> SessionConfig {
    SessionConfig {
        host: "caster.example.com".to_string(),
        port: 2101,
        mountpoint: mount.to_string(),
        username: "alice".to_string(),
        password: "secret".to_string(),
        version,
        transport: Transport::Ntrip,
        user_agent: "NTRIP OpenNtripClient/test".to_string(),
        gga: GgaPolicy::Off,
    }
}

fn corrections(outs: &[Output]) -> Vec<u8> {
    outs.iter()
        .filter_map(|o| match o {
            Output::Corrections(b) => Some(b.as_slice()),
            _ => None,
        })
        .flat_map(|b| b.iter().copied())
        .collect()
}

fn closes(outs: &[Output]) -> Vec<CloseReason> {
    outs.iter()
        .filter_map(|o| match o {
            Output::Close(r) => Some(r.clone()),
            _ => None,
        })
        .collect()
}

fn sourcetables(outs: &[Output]) -> Vec<Vec<u8>> {
    outs.iter()
        .filter_map(|o| match o {
            Output::Sourcetable(b) => Some(b.clone()),
            _ => None,
        })
        .collect()
}

fn has_rx(outs: &[Output], line: &str) -> bool {
    outs.iter()
        .any(|o| matches!(o, Output::ProtocolRx(l) if l == line))
}

/// Feed the whole stream in one on_bytes call.
fn run_whole(cfg: SessionConfig, stream: &[u8]) -> (NtripSession, Vec<Output>) {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg, t0);
    let mut out = Vec::new();
    s.on_bytes(stream, t0 + ms(10), &mut out);
    (s, out)
}

/// Feed the same stream one byte per on_bytes call.
fn run_split(cfg: SessionConfig, stream: &[u8]) -> (NtripSession, Vec<Output>) {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg, t0);
    let mut out = Vec::new();
    for (i, b) in stream.iter().enumerate() {
        s.on_bytes(&[*b], t0 + ms(10 + i as u64), &mut out);
    }
    (s, out)
}

/// Both packetizations of the same stream, for tests that assert each run.
type Run = fn(SessionConfig, &[u8]) -> (NtripSession, Vec<Output>);
const RUNS: [(&str, Run); 2] = [("whole", run_whole), ("split", run_split)];

fn chunk(data: &[u8]) -> Vec<u8> {
    let mut v = format!("{:X}\r\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\r\n");
    v
}

// ---------------------------------------------------------------------------
// Finding: chunked decode errors discarded bytes already decoded in the same
// feed. The minimized fuzz stream: one valid 5-byte chunk followed by a
// corrupt size line, all in ONE segment. The 5 payload bytes must be emitted
// before the StreamCorrupt close, exactly as they are when the segments are
// separate.
// ---------------------------------------------------------------------------

const CHUNKED_STREAM_HDR: &[u8] = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
/// Valid chunk "AAAAA", then a size line poisoned with 0xCD.
const CORRUPT_TAIL: &[u8] = b"5\r\nAAAAA\r\nd\xCD0\r\n";

#[test]
fn chunked_stream_error_still_delivers_decoded_bytes_whole_buffer() {
    let mut feed = CHUNKED_STREAM_HDR.to_vec();
    feed.extend_from_slice(CORRUPT_TAIL);
    let (_, out) = run_whole(cfg("RTK", NtripVersion::V2), &feed);
    assert_eq!(
        corrections(&out),
        b"AAAAA",
        "bytes decoded before the framing error must not be discarded"
    );
    match closes(&out).as_slice() {
        [CloseReason::StreamCorrupt { detail }] => {
            assert!(detail.contains("invalid chunk size"), "detail: {detail}");
        }
        other => panic!("expected StreamCorrupt, got {other:?}"),
    }
    // Ordering: the corrections precede the close.
    let corr = out
        .iter()
        .position(|o| matches!(o, Output::Corrections(_)))
        .unwrap();
    let close = out
        .iter()
        .position(|o| matches!(o, Output::Close(_)))
        .unwrap();
    assert!(corr < close);
}

#[test]
fn chunked_stream_error_split_invariant() {
    let mut feed = CHUNKED_STREAM_HDR.to_vec();
    feed.extend_from_slice(CORRUPT_TAIL);
    let (_, whole) = run_whole(cfg("RTK", NtripVersion::V2), &feed);
    let (_, split) = run_split(cfg("RTK", NtripVersion::V2), &feed);
    assert_eq!(corrections(&whole), corrections(&split));
    assert_eq!(closes(&whole), closes(&split));
}

// ---------------------------------------------------------------------------
// Finding (same root cause, table path): a chunked sourcetable whose
// ENDSOURCETABLE is decoded in the same feed as a later framing error must
// complete - the outcome may not flip between Sourcetable and
// Close(StreamCorrupt) on packetization.
// ---------------------------------------------------------------------------

const TABLE_HDR: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: gnss/sourcetable\r\nTransfer-Encoding: chunked\r\n\r\n";
const TABLE_PAYLOAD: &[u8] = b"STR;A;1\r\nENDSOURCETABLE\r\n";

fn table_feed_with_corrupt_tail() -> Vec<u8> {
    let mut feed = TABLE_HDR.to_vec();
    feed.extend_from_slice(&chunk(TABLE_PAYLOAD));
    feed.extend_from_slice(b"d\xCD0\r\n"); // corrupt size line after the table
    feed
}

#[test]
fn chunked_table_completes_despite_trailing_framing_error() {
    let feed = table_feed_with_corrupt_tail();
    let (_, whole) = run_whole(cfg("", NtripVersion::V2), &feed);
    let (_, split) = run_split(cfg("", NtripVersion::V2), &feed);
    for (label, out) in [("whole", &whole), ("split", &split)] {
        assert_eq!(
            sourcetables(out),
            vec![TABLE_PAYLOAD.to_vec()],
            "{label}: the completed table must be delivered"
        );
        assert!(
            closes(out).is_empty(),
            "{label}: clean table completion carries no Close, got {:?}",
            closes(out)
        );
    }
}

/// Same stream answering a mountpoint request: MountpointNotFound with the
/// table attached, never StreamCorrupt, under any packetization.
#[test]
fn chunked_table_answer_to_mount_request_survives_trailing_error() {
    let feed = table_feed_with_corrupt_tail();
    for (label, run) in RUNS {
        let (_, out) = run(cfg("TYPO", NtripVersion::V2), &feed);
        match closes(&out).as_slice() {
            [CloseReason::MountpointNotFound { sourcetable }] => {
                assert_eq!(sourcetable, TABLE_PAYLOAD, "{label}");
            }
            other => panic!("{label}: expected MountpointNotFound, got {other:?}"),
        }
    }
}

/// When the framing error comes BEFORE any terminator, both packetizations
/// must converge on StreamCorrupt (no outcome flip the other way either).
#[test]
fn chunked_table_error_without_terminator_is_corrupt_in_both() {
    let mut feed = TABLE_HDR.to_vec();
    feed.extend_from_slice(&chunk(b"STR;A;1\r\n"));
    feed.extend_from_slice(b"d\xCD0\r\n");
    let (_, whole) = run_whole(cfg("", NtripVersion::V2), &feed);
    let (_, split) = run_split(cfg("", NtripVersion::V2), &feed);
    for (label, out) in [("whole", &whole), ("split", &split)] {
        assert!(
            matches!(closes(out).as_slice(), [CloseReason::StreamCorrupt { .. }]),
            "{label}: expected StreamCorrupt, got {:?}",
            closes(out)
        );
        assert!(sourcetables(out).is_empty(), "{label}");
    }
}

// ---------------------------------------------------------------------------
// Finding: a partial ICY prelude line flushed at close was emitted as
// Corrections, so a caster dying mid-header-line counted as "stream data" -
// arming drop alerts, resetting reconnect budgets, and letting a dead mount
// pass the selftest's zero-byte guard. It must surface as ProtocolRx.
// ---------------------------------------------------------------------------

#[test]
fn prelude_partial_line_at_close_is_protocol_rx_not_corrections() {
    // CHCStream-style caster flushes "ICY 200 OK\r\nSer" and then dies; the
    // user (or the selftest budget) cancels.
    let stream = b"ICY 200 OK\r\nSer";
    for (label, run) in RUNS {
        let (mut s, mut out) = run(cfg("OGD1_RTCM3", NtripVersion::V1), stream);
        assert!(corrections(&out).is_empty(), "{label}");
        s.cancel(&mut out);
        assert!(
            corrections(&out).is_empty(),
            "{label}: a truncated header line is not correction data"
        );
        assert!(
            has_rx(&out, "Ser"),
            "{label}: the held bytes must still be reported verbatim"
        );
        assert_eq!(closes(&out), vec![CloseReason::Cancelled], "{label}");
    }
}

/// Same shape ending in a remote close (caster restarted mid-greeting).
#[test]
fn prelude_partial_line_at_remote_close_is_protocol_rx() {
    let (mut s, mut out) = run_whole(cfg("RTK", NtripVersion::V1), b"ICY 200 OK\r\nSer");
    s.on_remote_close(&mut out);
    assert!(corrections(&out).is_empty());
    assert!(has_rx(&out, "Ser"));
    assert_eq!(closes(&out), vec![CloseReason::RemoteClosed]);
}

// ---------------------------------------------------------------------------
// Finding: the prelude header test ("line contains a colon") ate printable
// payload lines wholesale. Header consumption now requires an RFC 7230
// token immediately before the colon.
// ---------------------------------------------------------------------------

#[test]
fn prose_line_with_space_before_colon_is_payload() {
    let stream = b"ICY 200 OK\r\nposition age: 5 s\r\n\xD3\x00\x01";
    for (label, run) in RUNS {
        let (_, out) = run(cfg("RTK", NtripVersion::V1), stream);
        assert_eq!(
            corrections(&out),
            b"position age: 5 s\r\n\xD3\x00\x01",
            "{label}: a prose line is payload, replayed byte-exact"
        );
        assert!(closes(&out).is_empty(), "{label}");
    }
}

/// Field names using the full RFC 7230 tchar set are still consumed as
/// headers; the tightening must not reject legitimate exotic headers.
#[test]
fn tchar_field_names_still_consumed_as_headers() {
    let stream = b"ICY 200 OK\r\nX-Cache_1.2!: hit\r\n\r\n\xD3\x00\x01";
    let (_, out) = run_whole(cfg("RTK", NtripVersion::V1), stream);
    assert!(has_rx(&out, "X-Cache_1.2!: hit"));
    assert_eq!(corrections(&out), b"\xD3\x00\x01");
}

/// A line whose colon comes first (empty field name) is payload.
#[test]
fn leading_colon_line_is_payload() {
    let stream = b"ICY 200 OK\r\n:notaheader\r\n\xD3";
    let (_, out) = run_whole(cfg("RTK", NtripVersion::V1), stream);
    assert_eq!(corrections(&out), b":notaheader\r\n\xD3");
}

// ---------------------------------------------------------------------------
// Finding: the raw payload attached to UnknownResponse from a classified
// status line depended on TCP packetization (whole-buffer feeds dragged
// coalesced body bytes into the hex dump). It must be the consumed lines
// only, identical under any packetization.
// ---------------------------------------------------------------------------

#[test]
fn unknown_response_raw_is_packetization_invariant() {
    let stream = b"ERROR - Bad Password\r\ntrailing junk the caster flushed";
    let (_, whole) = run_whole(cfg("RTK", NtripVersion::V1), stream);
    let (_, split) = run_split(cfg("RTK", NtripVersion::V1), stream);
    let raw_of = |outs: &[Output]| match closes(outs).as_slice() {
        [CloseReason::UnknownResponse { raw }] => raw.clone(),
        other => panic!("expected UnknownResponse, got {other:?}"),
    };
    let (rw, rs) = (raw_of(&whole), raw_of(&split));
    assert_eq!(rw, rs, "forensic raw must not vary with packetization");
    assert_eq!(rw, b"ERROR - Bad Password\r\n");
}

/// ICY answer to a table request: same invariance for the other classified
/// UnknownResponse site.
#[test]
fn icy_on_table_request_raw_is_packetization_invariant() {
    let stream = b"ICY 200 OK\r\n\xD3\x00\x01coalesced payload";
    let (_, whole) = run_whole(cfg("", NtripVersion::V1), stream);
    let (_, split) = run_split(cfg("", NtripVersion::V1), stream);
    for (label, out) in [("whole", &whole), ("split", &split)] {
        match closes(out).as_slice() {
            [CloseReason::UnknownResponse { raw }] => {
                assert_eq!(raw, b"ICY 200 OK\r\n", "{label}");
            }
            other => panic!("{label}: expected UnknownResponse, got {other:?}"),
        }
    }
}
