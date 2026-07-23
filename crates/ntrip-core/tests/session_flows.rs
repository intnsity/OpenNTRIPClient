//! Byte-feed tests for the session state machine: no sockets, synthetic
//! Instants. Every caster quirk the engine claims to handle is pinned here.

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

/// All correction bytes across outputs, concatenated in emission order.
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

fn gga_dues(outs: &[Output]) -> usize {
    outs.iter().filter(|o| matches!(o, Output::GgaDue)).count()
}

fn has_rx(outs: &[Output], line: &str) -> bool {
    outs.iter()
        .any(|o| matches!(o, Output::ProtocolRx(l) if l == line))
}

// ---------------------------------------------------------------------------
// ICY responses
// ---------------------------------------------------------------------------

/// The marquee regression: status line + N payload bytes arriving in ONE
/// on_bytes call must yield all N bytes. The original tool discarded them.
#[test]
fn icy_coalesced_payload_regression() {
    let t0 = Instant::now();
    let (mut s, req) = NtripSession::new(cfg("RTCM3", NtripVersion::V1), t0);
    assert!(req.starts_with(b"GET /RTCM3 HTTP/1.0\r\n"));

    let payload: Vec<u8> = (0..=99u8).collect();
    let mut feed = b"ICY 200 OK\r\n".to_vec();
    feed.extend_from_slice(&payload);

    let mut out = Vec::new();
    s.on_bytes(&feed, t0 + ms(50), &mut out);
    assert!(has_rx(&out, "ICY 200 OK"));
    assert_eq!(
        corrections(&out),
        payload,
        "every coalesced payload byte must survive"
    );
    assert!(closes(&out).is_empty());
}

/// Same, with the optional blank line after the status: it must be consumed,
/// not delivered as payload.
#[test]
fn icy_coalesced_with_blank_line() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTCM3", NtripVersion::V1), t0);
    let payload = [0xD3u8, 0x00, 0x13, 0x3E, 0xD0];
    let mut feed = b"ICY 200 OK\r\n\r\n".to_vec();
    feed.extend_from_slice(&payload);
    let mut out = Vec::new();
    s.on_bytes(&feed, t0 + ms(50), &mut out);
    assert_eq!(corrections(&out), payload);
}

/// The spec mandates case-insensitive, whitespace-trimmed status matching;
/// real casters emit every capitalization. A refactor to exact-prefix
/// matching must fail here, not in the field.
#[test]
fn status_lines_match_case_insensitively_and_trimmed() {
    // Lowercase, padded ICY. The payload leads with 0xD3 (as real RTCM
    // always does) so the trailing-header scanner cannot mistake it for the
    // start of a header line.
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTCM3", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.on_bytes(b"  icy 200 ok  \r\n\xD3PAYLOAD", t0 + ms(10), &mut out);
    assert_eq!(corrections(&out), b"\xD3PAYLOAD");
    assert!(closes(&out).is_empty());

    // Lowercase sourcetable answer to a table request.
    let (mut s, _) = NtripSession::new(cfg("", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.on_bytes(
        b"sourcetable 200 ok\r\nSTR;M;;RTCM3;;;;;US;0;0;1;;;;N;N;0;\r\nENDSOURCETABLE\r\n",
        t0 + ms(10),
        &mut out,
    );
    assert_eq!(sourcetables(&out).len(), 1);

    // Lowercase HTTP 401.
    let (mut s, _) = NtripSession::new(cfg("RTCM3", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.on_bytes(b"http/1.1 401 unauthorized\r\n\r\n", t0 + ms(10), &mut out);
    assert_eq!(closes(&out), vec![CloseReason::Unauthorized]);
}

/// A CR held back by the ICY prelude scanner (blank-line start or payload
/// byte? unresolved) is reported when the session closes first - but as a
/// ProtocolRx line, never as Corrections: a corrections record for a
/// connection that died mid-prelude would let callers treat a dead mount as
/// an established stream (see the review fix pinned in session_hfix1.rs).
#[test]
fn held_cr_is_flushed_on_close() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTCM3", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.on_bytes(b"ICY 200 OK\r\n", t0 + ms(10), &mut out);
    s.on_bytes(b"\r", t0 + ms(20), &mut out); // lone CR: held for disambiguation
    assert!(corrections(&out).is_empty());
    s.on_remote_close(&mut out);
    assert!(
        corrections(&out).is_empty(),
        "a never-resolved prelude byte is not stream data"
    );
    assert!(
        has_rx(&out, "\r"),
        "the held CR must still be reported, verbatim, in the protocol log"
    );
    assert_eq!(closes(&out), vec![CloseReason::RemoteClosed]);
}

#[test]
fn icy_split_byte_at_a_time() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTCM3", NtripVersion::V1), t0);
    let payload = [0xD3u8, 0x00, 0x13, 0x3E, 0xD0, 0x00, 0x03];
    let mut feed = b"ICY 200 OK\r\n\r\n".to_vec();
    feed.extend_from_slice(&payload);

    let mut out = Vec::new();
    for (i, b) in feed.iter().enumerate() {
        s.on_bytes(&[*b], t0 + ms(10 + i as u64), &mut out);
    }
    assert!(has_rx(&out, "ICY 200 OK"));
    assert_eq!(
        corrections(&out),
        payload,
        "byte-at-a-time must decode identically"
    );
}

/// A payload whose first byte is CR, with no blank line: the held-CR logic
/// must release it as payload once the next byte proves it was not a blank
/// line. Split-invariance at its nastiest.
#[test]
fn icy_payload_starting_with_bare_cr() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTCM3", NtripVersion::V1), t0);
    let mut out = Vec::new();
    for (i, b) in b"ICY 200 OK\r\n\rX\x01".iter().enumerate() {
        s.on_bytes(&[*b], t0 + ms(10 + i as u64), &mut out);
    }
    assert_eq!(corrections(&out), b"\rX\x01");
}

/// An ICY answer to a sourcetable request (empty mountpoint) is nonsense.
/// The attached raw is the classified status line only: body bytes that
/// happened to coalesce into the same segment are excluded, so the forensic
/// evidence does not vary with packetization (pinned in session_hfix1.rs).
#[test]
fn icy_on_table_request_is_unknown_response() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.on_bytes(b"ICY 200 OK\r\npayload", t0 + ms(10), &mut out);
    match closes(&out).as_slice() {
        [CloseReason::UnknownResponse { raw }] => {
            assert_eq!(raw, b"ICY 200 OK\r\n");
        }
        other => panic!("expected UnknownResponse, got {other:?}"),
    }
}

/// A status line coalesced with a payload burst larger than the header cap
/// must classify, not trip the cap.
#[test]
fn icy_coalesced_payload_larger_than_header_cap() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTCM3", NtripVersion::V1), t0);
    let payload = vec![0xAAu8; 20 * 1024];
    let mut feed = b"ICY 200 OK\r\n".to_vec();
    feed.extend_from_slice(&payload);
    let mut out = Vec::new();
    s.on_bytes(&feed, t0 + ms(10), &mut out);
    assert_eq!(corrections(&out), payload);
    assert!(closes(&out).is_empty());
}

/// CHCStream 1.0 sends real HTTP-style header lines after "ICY 200 OK",
/// then a blank line, then RTCM. The headers must surface as ProtocolRx -
/// NEVER as correction bytes (counting them let the selftest certify a dead
/// mount and put ASCII at the front of --capture files).
#[test]
fn icy_chcstream_trailing_headers_one_segment() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("OGD1_RTCM3", NtripVersion::V1), t0);
    let payload = [0xD3u8, 0x00, 0x13, 0x3E, 0xD0, 0x00, 0x03];
    let mut feed = b"ICY 200 OK\r\n\
                     Server: CHCStream 1.0\r\n\
                     Date: Wed, 16 Jul 2026 00:00:00 GMT\r\n\
                     \r\n"
        .to_vec();
    feed.extend_from_slice(&payload);
    let mut out = Vec::new();
    s.on_bytes(&feed, t0 + ms(50), &mut out);
    assert!(has_rx(&out, "ICY 200 OK"));
    assert!(has_rx(&out, "Server: CHCStream 1.0"));
    assert!(has_rx(&out, "Date: Wed, 16 Jul 2026 00:00:00 GMT"));
    assert_eq!(
        corrections(&out),
        payload,
        "exactly the RTCM bytes are corrections; headers are not"
    );
    assert!(closes(&out).is_empty());
}

/// Same CHCStream shape delivered one byte per feed: headers arrive split
/// across reads and must classify identically (split invariance).
#[test]
fn icy_chcstream_trailing_headers_byte_at_a_time() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("OGD1_RTCM3", NtripVersion::V1), t0);
    let payload = [0xD3u8, 0x00, 0x13, 0x3E, 0xD0];
    let mut feed = b"ICY 200 OK\r\n\
                     Server: CHCStream 1.0\r\n\
                     Date: Wed, 16 Jul 2026 00:00:00 GMT\r\n\
                     \r\n"
        .to_vec();
    feed.extend_from_slice(&payload);
    let mut out = Vec::new();
    for (i, b) in feed.iter().enumerate() {
        s.on_bytes(&[*b], t0 + ms(10 + i as u64), &mut out);
    }
    assert!(has_rx(&out, "Server: CHCStream 1.0"));
    assert!(has_rx(&out, "Date: Wed, 16 Jul 2026 00:00:00 GMT"));
    assert_eq!(corrections(&out), payload);
    assert!(closes(&out).is_empty());
}

/// A dead-but-polite CHCStream mount: status, headers, blank line, then
/// silence. Zero correction bytes must be reported - this is exactly the
/// case the selftest must fail on.
#[test]
fn icy_chcstream_headers_then_silence_yields_zero_corrections() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("OGD1_RTCM3", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.on_bytes(
        b"ICY 200 OK\r\nServer: CHCStream 1.0\r\nDate: x\r\n\r\n",
        t0 + ms(10),
        &mut out,
    );
    assert!(corrections(&out).is_empty());
    s.on_remote_close(&mut out);
    assert!(
        corrections(&out).is_empty(),
        "no phantom bytes may appear at close"
    );
    assert_eq!(closes(&out), vec![CloseReason::RemoteClosed]);
}

/// ICY immediately followed by 0xD3 in the same segment: the first byte can
/// never open a header line, so header consumption ends instantly and every
/// byte is corrections (the pre-header-support behavior, unchanged).
#[test]
fn icy_immediate_rtcm_same_segment_unchanged() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTCM3", NtripVersion::V1), t0);
    let payload = [0xD3u8, 0x00, 0x04, 0x4C, 0xE0, 0x00, 0x80];
    let mut feed = b"ICY 200 OK\r\n".to_vec();
    feed.extend_from_slice(&payload);
    let mut out = Vec::new();
    s.on_bytes(&feed, t0 + ms(50), &mut out);
    assert_eq!(corrections(&out), payload);
    assert!(closes(&out).is_empty());
}

/// A printable line after ICY that is NOT header-shaped (no colon) is
/// payload, replayed byte-exact including its CRLF terminator.
#[test]
fn icy_printable_non_header_line_is_payload() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTCM3", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.on_bytes(
        b"ICY 200 OK\r\nHELLO\r\n\xD3\x00\x01",
        t0 + ms(10),
        &mut out,
    );
    assert_eq!(corrections(&out), b"HELLO\r\n\xD3\x00\x01");
    assert!(closes(&out).is_empty());
}

// ---------------------------------------------------------------------------
// Sourcetable (v1 SOURCETABLE flavor)
// ---------------------------------------------------------------------------

const TABLE_BODY: &[u8] = b"CAS;caster.example.com;2101;Example;Op;0;DEU;50.0;8.6\r\n\
STR;MOUNT1;City;RTCM 3.2;1005(1);2;GPS;Net;DEU;50.0;8.6;1;1;Gen;none;B;N;3120\r\n\
ENDSOURCETABLE\r\n";

#[test]
fn sourcetable_happy_path() {
    let t0 = Instant::now();
    let (mut s, req) = NtripSession::new(cfg("", NtripVersion::V1), t0);
    assert!(req.starts_with(b"GET / HTTP/1.0\r\n"));

    let mut out = Vec::new();
    s.on_bytes(b"SOURCETABLE 200 OK\r\n", t0 + ms(10), &mut out);
    s.on_bytes(TABLE_BODY, t0 + ms(20), &mut out);

    assert!(has_rx(&out, "SOURCETABLE 200 OK"));
    assert_eq!(sourcetables(&out), vec![TABLE_BODY.to_vec()]);
    assert!(
        closes(&out).is_empty(),
        "clean table completion carries no Close"
    );

    // Session is Done: nothing more comes out, ever.
    let mut after = Vec::new();
    s.on_bytes(b"junk", t0 + ms(30), &mut after);
    s.on_tick(t0 + ms(60_000), &mut after);
    s.cancel(&mut after);
    assert!(after.is_empty());
}

#[test]
fn sourcetable_truncates_at_endsourcetable_line() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("", NtripVersion::V1), t0);
    let mut feed = b"SOURCETABLE 200 OK\r\n".to_vec();
    feed.extend_from_slice(TABLE_BODY);
    feed.extend_from_slice(b"trailing junk the caster kept sending");
    let mut out = Vec::new();
    s.on_bytes(&feed, t0 + ms(10), &mut out);
    assert_eq!(sourcetables(&out), vec![TABLE_BODY.to_vec()]);
}

/// A SOURCETABLE answer to a mountpoint request means "no such mountpoint";
/// the table rides along inside the close reason.
#[test]
fn mountpoint_not_found() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("TYPO", NtripVersion::V1), t0);
    let mut feed = b"SOURCETABLE 200 OK\r\n".to_vec();
    feed.extend_from_slice(TABLE_BODY);
    let mut out = Vec::new();
    s.on_bytes(&feed, t0 + ms(10), &mut out);

    assert!(
        sourcetables(&out).is_empty(),
        "table travels inside the close, not separately"
    );
    match closes(&out).as_slice() {
        [reason @ CloseReason::MountpointNotFound { sourcetable }] => {
            assert_eq!(sourcetable, TABLE_BODY);
            assert!(
                !reason.auto_reconnect(),
                "retrying a typo'd mountpoint is pointless"
            );
        }
        other => panic!("expected MountpointNotFound, got {other:?}"),
    }
}

#[test]
fn sourcetable_timeout() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("", NtripVersion::V1), t0);
    let mut out = Vec::new();
    let t1 = t0 + ms(1000);
    s.on_bytes(b"SOURCETABLE 200 OK\r\nSTR;PARTIAL\r\n", t1, &mut out);
    s.on_tick(t1 + ms(9_900), &mut out);
    assert!(closes(&out).is_empty());
    s.on_tick(t1 + ms(10_000), &mut out);
    match closes(&out).as_slice() {
        [reason @ CloseReason::SourcetableTimeout] => assert!(reason.auto_reconnect()),
        other => panic!("expected SourcetableTimeout, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// HTTP responses
// ---------------------------------------------------------------------------

#[test]
fn unauthorized_v1_style() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTCM3", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.on_bytes(b"HTTP/1.0 401 Unauthorized\r\n\r\n", t0 + ms(10), &mut out);
    assert!(has_rx(&out, "HTTP/1.0 401 Unauthorized"));
    match closes(&out).as_slice() {
        [reason @ CloseReason::Unauthorized] => assert!(!reason.auto_reconnect()),
        other => panic!("expected Unauthorized, got {other:?}"),
    }
}

#[test]
fn unauthorized_http11_style_with_headers() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTCM3", NtripVersion::V2), t0);
    let mut out = Vec::new();
    s.on_bytes(
        b"HTTP/1.1 401 Unauthorized\r\n\
          WWW-Authenticate: Basic realm=\"NTRIP\"\r\n\
          Content-Length: 9\r\n\
          \r\n\
          forbidden",
        t0 + ms(10),
        &mut out,
    );
    assert!(has_rx(&out, "WWW-Authenticate: Basic realm=\"NTRIP\""));
    assert_eq!(closes(&out), vec![CloseReason::Unauthorized]);
    assert!(
        corrections(&out).is_empty(),
        "401 body must not leak as corrections"
    );
}

/// HTTP headers and stream payload coalesced into one segment: the same
/// discard bug class as ICY, pinned for the HTTP path.
#[test]
fn http_headers_coalesced_with_payload() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V2), t0);
    let payload = [0xD3u8, 0x00, 0x04, 0x4C, 0xE0, 0x00, 0x80];
    let mut feed = b"HTTP/1.1 200 OK\r\nContent-Type: gnss/data\r\n\r\n".to_vec();
    feed.extend_from_slice(&payload);
    let mut out = Vec::new();
    s.on_bytes(&feed, t0 + ms(10), &mut out);
    assert!(has_rx(&out, "HTTP/1.1 200 OK"));
    assert!(has_rx(&out, "Content-Type: gnss/data"));
    assert_eq!(corrections(&out), payload);
}

fn chunk(data: &[u8]) -> Vec<u8> {
    let mut v = format!("{:X}\r\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\r\n");
    v
}

#[test]
fn v2_chunked_streaming() {
    let t0 = Instant::now();
    let (mut s, req) = NtripSession::new(cfg("RTK", NtripVersion::V2), t0);
    assert!(req.starts_with(b"GET /RTK HTTP/1.1\r\n"));

    let mut out = Vec::new();
    let mut feed = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    feed.extend_from_slice(&chunk(b"first-block"));
    s.on_bytes(&feed, t0 + ms(10), &mut out);
    assert_eq!(corrections(&out), b"first-block");

    // A chunk split across feeds decodes identically.
    let c = chunk(b"second-block");
    let (a, b) = c.split_at(5);
    s.on_bytes(a, t0 + ms(20), &mut out);
    s.on_bytes(b, t0 + ms(30), &mut out);
    assert_eq!(corrections(&out), b"first-blocksecond-block");

    // Terminal chunk: the server ended the response body - a remote close.
    s.on_bytes(b"0\r\n\r\n", t0 + ms(40), &mut out);
    match closes(&out).as_slice() {
        [reason @ CloseReason::RemoteClosed] => assert!(reason.auto_reconnect()),
        other => panic!("expected RemoteClosed, got {other:?}"),
    }
}

/// Real casters declare chunked even when the request was V1; accept it.
#[test]
fn chunked_on_v1_request() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V1), t0);
    let mut out = Vec::new();
    let mut feed = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    feed.extend_from_slice(&chunk(&[0xD3, 0x00, 0x01]));
    s.on_bytes(&feed, t0 + ms(10), &mut out);
    assert_eq!(corrections(&out), [0xD3, 0x00, 0x01]);
    assert!(closes(&out).is_empty());
}

#[test]
fn chunk_decode_error_closes_stream_corrupt() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V2), t0);
    let mut out = Vec::new();
    s.on_bytes(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
        t0 + ms(10),
        &mut out,
    );
    s.on_bytes(b"ZZZ\r\n", t0 + ms(20), &mut out);
    match closes(&out).as_slice() {
        [reason @ CloseReason::StreamCorrupt { detail }] => {
            assert!(detail.contains("invalid chunk size"), "detail: {detail}");
            assert!(reason.auto_reconnect());
        }
        other => panic!("expected StreamCorrupt, got {other:?}"),
    }
}

/// v2 table delimited by Content-Length, body split across feeds; clean
/// completion without ENDSOURCETABLE and without a Close.
#[test]
fn http_table_content_length_delimited() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("", NtripVersion::V2), t0);
    let body = b"STR;M1;Id;RTCM 3.2\r\nSTR;M2;Id2;RTCM 3.2\r\n";
    let mut out = Vec::new();
    let hdr = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: gnss/sourcetable\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    s.on_bytes(hdr.as_bytes(), t0 + ms(10), &mut out);
    let (a, b) = body.split_at(11);
    s.on_bytes(a, t0 + ms(20), &mut out);
    assert!(sourcetables(&out).is_empty());
    s.on_bytes(b, t0 + ms(30), &mut out);
    assert_eq!(sourcetables(&out), vec![body.to_vec()]);
    assert!(closes(&out).is_empty());
}

/// v2 chunked table response to a mountpoint request -> MountpointNotFound
/// with the de-chunked table attached.
#[test]
fn http_chunked_table_means_mountpoint_not_found() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("TYPO", NtripVersion::V2), t0);
    let mut out = Vec::new();
    let mut feed =
        b"HTTP/1.1 200 OK\r\nContent-Type: gnss/sourcetable\r\nTransfer-Encoding: chunked\r\n\r\n"
            .to_vec();
    feed.extend_from_slice(&chunk(b"STR;REAL1;x\r\n"));
    feed.extend_from_slice(&chunk(b"ENDSOURCETABLE\r\n"));
    s.on_bytes(&feed, t0 + ms(10), &mut out);
    match closes(&out).as_slice() {
        [CloseReason::MountpointNotFound { sourcetable }] => {
            assert_eq!(sourcetable, b"STR;REAL1;x\r\nENDSOURCETABLE\r\n");
        }
        other => panic!("expected MountpointNotFound, got {other:?}"),
    }
}

/// v2 table with neither Content-Length nor chunked: read until remote close.
#[test]
fn read_until_close_v2_table() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("", NtripVersion::V2), t0);
    let mut out = Vec::new();
    s.on_bytes(
        b"HTTP/1.1 200 OK\r\nContent-Type: gnss/sourcetable\r\n\r\n",
        t0 + ms(10),
        &mut out,
    );
    s.on_bytes(b"STR;A;1\r\n", t0 + ms(20), &mut out);
    s.on_bytes(b"STR;B;2\r\n", t0 + ms(30), &mut out);
    assert!(sourcetables(&out).is_empty());

    s.on_remote_close(&mut out);
    assert_eq!(sourcetables(&out), vec![b"STR;A;1\r\nSTR;B;2\r\n".to_vec()]);
    assert_eq!(closes(&out), vec![CloseReason::RemoteClosed]);
    // Sourcetable must precede the Close.
    let st = out
        .iter()
        .position(|o| matches!(o, Output::Sourcetable(_)))
        .unwrap();
    let cl = out
        .iter()
        .position(|o| matches!(o, Output::Close(_)))
        .unwrap();
    assert!(st < cl);
}

#[test]
fn http_other_status_is_unknown_response_with_header_block() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V2), t0);
    let mut out = Vec::new();
    let hdr = b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 120\r\n\r\n";
    let mut feed = hdr.to_vec();
    feed.extend_from_slice(b"body junk");
    s.on_bytes(&feed, t0 + ms(10), &mut out);
    match closes(&out).as_slice() {
        [CloseReason::UnknownResponse { raw }] => {
            assert_eq!(raw, hdr, "raw carries exactly the header block");
        }
        other => panic!("expected UnknownResponse, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Garbage and limits
// ---------------------------------------------------------------------------

#[test]
fn garbage_first_line_is_unknown_response() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.on_bytes(b"ERROR - Bad Password\r\n", t0 + ms(10), &mut out);
    match closes(&out).as_slice() {
        [reason @ CloseReason::UnknownResponse { raw }] => {
            assert_eq!(raw, b"ERROR - Bad Password\r\n");
            assert!(!reason.auto_reconnect());
        }
        other => panic!("expected UnknownResponse, got {other:?}"),
    }
}

#[test]
fn header_buffer_cap_16k() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V1), t0);
    let mut out = Vec::new();
    // No newline ever arrives: the buffer can only grow.
    let blob = vec![b'A'; 6000];
    s.on_bytes(&blob, t0 + ms(10), &mut out);
    s.on_bytes(&blob, t0 + ms(20), &mut out);
    assert!(
        closes(&out).is_empty(),
        "12000 bytes is still under the cap"
    );
    s.on_bytes(&blob, t0 + ms(30), &mut out);
    match closes(&out).as_slice() {
        [CloseReason::UnknownResponse { raw }] => assert_eq!(raw.len(), 18000),
        other => panic!("expected UnknownResponse, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Timeouts
// ---------------------------------------------------------------------------

#[test]
fn first_response_timeout() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.on_tick(t0 + ms(29_900), &mut out);
    assert!(closes(&out).is_empty());
    s.on_tick(t0 + ms(30_000), &mut out);
    match closes(&out).as_slice() {
        [reason @ CloseReason::FirstResponseTimeout] => assert!(reason.auto_reconnect()),
        other => panic!("expected FirstResponseTimeout, got {other:?}"),
    }
}

/// Headers that trickle in but never complete still count as "no first
/// response": nothing else would ever bound this phase.
#[test]
fn first_response_timeout_with_partial_headers() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V2), t0);
    let mut out = Vec::new();
    s.on_bytes(b"HTTP/1.1 200 OK\r\nContent-", t0 + ms(1000), &mut out);
    s.on_tick(t0 + ms(30_000), &mut out);
    assert_eq!(closes(&out), vec![CloseReason::FirstResponseTimeout]);
}

#[test]
fn stream_silence_timeout() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V1), t0);
    let mut out = Vec::new();
    let t1 = t0 + ms(500);
    s.on_bytes(b"ICY 200 OK\r\n\xD3\x00\x01", t1, &mut out);
    s.on_tick(t1 + ms(29_500), &mut out);
    assert!(closes(&out).is_empty());
    // Any received byte resets the clock.
    let t2 = t1 + ms(29_600);
    s.on_bytes(&[0xD3], t2, &mut out);
    s.on_tick(t2 + ms(29_900), &mut out);
    assert!(closes(&out).is_empty());
    s.on_tick(t2 + ms(30_000), &mut out);
    match closes(&out).as_slice() {
        [reason @ CloseReason::StreamSilence] => assert!(reason.auto_reconnect()),
        other => panic!("expected StreamSilence, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// GGA scheduling
// ---------------------------------------------------------------------------

fn gga_cfg(policy: GgaPolicy) -> SessionConfig {
    SessionConfig {
        gga: policy,
        ..cfg("RTK", NtripVersion::V1)
    }
}

#[test]
fn gga_cadence_first_at_300ms_then_10s_after_sent() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(gga_cfg(GgaPolicy::Always), t0);
    let mut out = Vec::new();
    let t1 = t0 + ms(100);
    s.on_bytes(b"ICY 200 OK\r\n\xD3", t1, &mut out);

    s.on_tick(t1 + ms(299), &mut out);
    assert_eq!(
        gga_dues(&out),
        0,
        "first GgaDue is not due before streaming_start + 300 ms"
    );
    s.on_tick(t1 + ms(300), &mut out);
    assert_eq!(gga_dues(&out), 1);

    // INVARIANT: no second GgaDue without an intervening gga_sent, no matter
    // how many ticks pass.
    for i in 1..20 {
        s.on_tick(t1 + ms(300 + i * 500), &mut out);
    }
    assert_eq!(gga_dues(&out), 1);

    // Monotonic clock: gga_sent happens after the last tick above (t1+9800ms).
    let t_sent = t1 + ms(10_000);
    s.gga_sent(t_sent);
    s.on_tick(t_sent + ms(9_900), &mut out);
    assert_eq!(gga_dues(&out), 1);
    s.on_tick(t_sent + ms(10_000), &mut out);
    assert_eq!(gga_dues(&out), 2);
}

/// A miss (the caller had no position for a due GGA) re-arms on the short
/// 2 s retry slot, not the full 10 s interval: against a caster that holds
/// the stream until a position arrives (CHC APIS), the first available fix
/// must reach the wire promptly - a 10 s miss cadence can lose the race
/// against the 30 s silence timeout when the receiver fixes late.
#[test]
fn gga_missed_retries_on_the_short_slot() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(gga_cfg(GgaPolicy::Always), t0);
    let mut out = Vec::new();
    let t1 = t0 + ms(100);
    s.on_bytes(b"ICY 200 OK\r\n\xD3", t1, &mut out);
    s.on_tick(t1 + ms(300), &mut out);
    assert_eq!(gga_dues(&out), 1);

    let t_miss = t1 + ms(400);
    s.gga_missed(t_miss);
    s.on_tick(t_miss + ms(1_900), &mut out);
    assert_eq!(gga_dues(&out), 1, "not due before the 2 s retry slot");
    s.on_tick(t_miss + ms(2_000), &mut out);
    assert_eq!(gga_dues(&out), 2);

    // A successful send restores the full 10 s cadence.
    let t_sent = t_miss + ms(2_100);
    s.gga_sent(t_sent);
    s.on_tick(t_sent + ms(9_900), &mut out);
    assert_eq!(gga_dues(&out), 2);
    s.on_tick(t_sent + ms(10_000), &mut out);
    assert_eq!(gga_dues(&out), 3);
}

#[test]
fn gga_policy_gating() {
    let t0 = Instant::now();
    for policy in [
        GgaPolicy::Off,
        GgaPolicy::WhenRequired {
            stream_requires: false,
        },
    ] {
        let (mut s, _) = NtripSession::new(gga_cfg(policy), t0);
        let mut out = Vec::new();
        let t1 = t0 + ms(100);
        s.on_bytes(b"ICY 200 OK\r\n\xD3", t1, &mut out);
        for i in 0..25 {
            s.on_tick(t1 + ms(i * 500), &mut out);
        }
        assert_eq!(
            gga_dues(&out),
            0,
            "policy {policy:?} must never emit GgaDue"
        );
    }

    let (mut s, _) = NtripSession::new(
        gga_cfg(GgaPolicy::WhenRequired {
            stream_requires: true,
        }),
        t0,
    );
    let mut out = Vec::new();
    let t1 = t0 + ms(100);
    s.on_bytes(b"ICY 200 OK\r\n\xD3", t1, &mut out);
    s.on_tick(t1 + ms(300), &mut out);
    assert_eq!(
        gga_dues(&out),
        1,
        "stream_requires: true behaves like Always"
    );
}

/// GgaDue is only ever emitted while streaming: a session stuck before the
/// first response never asks for GGA.
#[test]
fn gga_not_due_before_streaming() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(gga_cfg(GgaPolicy::Always), t0);
    let mut out = Vec::new();
    for i in 0..20 {
        s.on_tick(t0 + ms(i * 500), &mut out);
    }
    assert_eq!(gga_dues(&out), 0);
}

// ---------------------------------------------------------------------------
// Raw TCP transport
// ---------------------------------------------------------------------------

#[test]
fn raw_tcp_streams_immediately_and_never_asks_for_gga() {
    let t0 = Instant::now();
    let raw_cfg = SessionConfig {
        transport: Transport::RawTcp,
        gga: GgaPolicy::Always,
        ..cfg("IGNORED", NtripVersion::V1)
    };
    let (mut s, req) = NtripSession::new(raw_cfg, t0);
    assert!(req.is_empty(), "RawTcp sends no request");

    let mut out = Vec::new();
    let payload = [0xD3u8, 0x00, 0x02, 0x41, 0x42];
    s.on_bytes(&payload, t0 + ms(10), &mut out);
    assert!(
        out.iter().all(|o| matches!(o, Output::Corrections(_))),
        "no protocol chatter"
    );
    assert_eq!(corrections(&out), payload);

    for i in 0..25 {
        s.on_tick(t0 + ms(10 + i * 500), &mut out);
    }
    assert_eq!(
        gga_dues(&out),
        0,
        "GgaDue never fires on RawTcp regardless of policy"
    );

    s.on_remote_close(&mut out);
    assert_eq!(closes(&out), vec![CloseReason::RemoteClosed]);
}

// ---------------------------------------------------------------------------
// Lifecycle discipline
// ---------------------------------------------------------------------------

#[test]
fn done_discipline_nothing_after_close() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.on_bytes(b"HTTP/1.0 401 Unauthorized\r\n\r\n", t0 + ms(10), &mut out);
    assert_eq!(closes(&out), vec![CloseReason::Unauthorized]);

    let mut after = Vec::new();
    s.on_bytes(b"ICY 200 OK\r\n\xD3\x01\x02", t0 + ms(20), &mut after);
    s.on_tick(t0 + ms(90_000), &mut after);
    s.on_remote_close(&mut after);
    s.cancel(&mut after);
    assert!(after.is_empty(), "a Done session is inert: {after:?}");
}

#[test]
fn cancel_emits_cancelled_once() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.cancel(&mut out);
    // Request Tx lines drain first, then the close.
    assert_eq!(closes(&out), vec![CloseReason::Cancelled]);
    assert!(!CloseReason::Cancelled.auto_reconnect());

    let mut again = Vec::new();
    s.cancel(&mut again);
    assert!(again.is_empty());
}

#[test]
fn remote_close_during_headers_is_unknown_response() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V2), t0);
    let mut out = Vec::new();
    s.on_bytes(b"HTTP/1.1 200 OK\r\nPartial-", t0 + ms(10), &mut out);
    s.on_remote_close(&mut out);
    match closes(&out).as_slice() {
        [CloseReason::UnknownResponse { raw }] => {
            assert_eq!(raw, b"HTTP/1.1 200 OK\r\nPartial-");
        }
        other => panic!("expected UnknownResponse, got {other:?}"),
    }
}

/// The queued request Tx lines drain into the first on_* call, before any
/// received-side output, and only once.
#[test]
fn request_tx_lines_drain_once_in_order() {
    let t0 = Instant::now();
    let (mut s, _) = NtripSession::new(cfg("RTK", NtripVersion::V1), t0);
    let mut out = Vec::new();
    s.on_tick(t0 + ms(100), &mut out);
    let tx: Vec<&str> = out
        .iter()
        .filter_map(|o| match o {
            Output::ProtocolTx(l) => Some(l.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        tx,
        vec![
            "GET /RTK HTTP/1.0",
            "User-Agent: NTRIP OpenNtripClient/test",
            "Accept: */*",
            "Connection: close",
            // Log copy: masked. The wire (pinned by mock_caster.rs) carries
            // the real credential; ProtocolTx lines reach support tickets.
            "Authorization: Basic ****",
        ]
    );

    let mut second = Vec::new();
    s.on_tick(t0 + ms(200), &mut second);
    assert!(second.is_empty(), "Tx lines drain exactly once");
}
