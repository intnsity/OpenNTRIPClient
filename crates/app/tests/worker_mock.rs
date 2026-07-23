//! Integration tests: the REAL ntrip worker + logger stack driven against a
//! 127.0.0.1 mock caster, asserting on the AppEvent stream. Same canned-
//! response pattern as crates/ntrip-core/tests/mock_caster.rs, but here the
//! full worker (thread, supervisor, queue, logging fan-out) is under test.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use ntrip_core::{NtripVersion, Transport};
use open_ntrip_client::bus::{AppEvent, Hub, Repaint};
use open_ntrip_client::logging::{CaptureTarget, Logger};
use open_ntrip_client::settings::{GgaMode, GgaSource};
use open_ntrip_client::workers::CorrQueue;
use open_ntrip_client::workers::ntrip::{self, NtripJob, ReconnectPolicy, collect_until_stopped};

/// Fixture dir under the OS tempdir; never the repo.
fn tempdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "open-ntrip-client-worker-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Served {
    port: u16,
    /// Joins to every byte the server received from the client.
    handle: thread::JoinHandle<Vec<u8>>,
}

/// One-shot caster: accept, read the request up to its blank line, write the
/// canned response, optionally keep reading for `linger` (to capture GGA
/// traffic), then close.
fn serve(response: Vec<u8>, linger: Duration) -> Served {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        sock.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut received = Vec::new();
        let mut buf = [0u8; 2048];
        while !received.windows(4).any(|w| w == b"\r\n\r\n") {
            match sock.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => received.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        sock.write_all(&response).expect("server write");
        let deadline = Instant::now() + linger;
        while Instant::now() < deadline {
            match sock.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => received.extend_from_slice(&buf[..n]),
                Err(_) => {}
            }
        }
        received
    });
    Served { port, handle }
}

fn job(port: u16, mount: &str) -> NtripJob {
    NtripJob {
        host: "127.0.0.1".to_string(),
        port,
        mountpoint: mount.to_string(),
        username: "alice".to_string(),
        password: "secret".to_string(),
        version: NtripVersion::V1,
        transport: Transport::Ntrip,
        tls: false,
        allow_invalid_certs: false,
        gga_mode: GgaMode::Off,
        gga_source: GgaSource::Manual,
        manual_lat: 0.0,
        manual_lon: 0.0,
        stream_requires_gga: None,
        reconnect: ReconnectPolicy::OptionsOff,
        max_attempts: 1,
        audio_alert: String::new(),
        capture: None,
        user_agent: open_ntrip_client::user_agent(),
    }
}

/// Real Hub + real logger thread (file sinks disabled): the exact fan-out
/// stack the GUI uses, headless. `tag` isolates each test's tempdir - tests
/// run in parallel and tempdir() wipes the directory it is given.
fn stack(tag: &str) -> (Hub, Receiver<AppEvent>, Logger, PathBuf) {
    let dir = tempdir(tag);
    let (tx, rx) = channel();
    let logger = Logger::start(dir.join("Logs"), dir.join("NMEA"), false, false, tx.clone());
    let hub = Hub::new(tx, logger.sender(), Repaint::headless());
    (hub, rx, logger, dir)
}

fn event_lines(events: &[AppEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AppEvent::EventLine(l) => Some(l.clone()),
            _ => None,
        })
        .collect()
}

fn conn_lines(events: &[AppEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AppEvent::ConnLine(l) => Some(l.clone()),
            _ => None,
        })
        .collect()
}

fn last_total(events: &[AppEvent]) -> u64 {
    events
        .iter()
        .filter_map(|e| match e {
            AppEvent::RxBytes { total } => Some(*total),
            _ => None,
        })
        .next_back()
        .unwrap_or(0)
}

#[test]
fn stream_with_coalesced_payload_counts_every_byte() {
    let payload: Vec<u8> = (0..=255u16).map(|b| b as u8).collect();
    let mut resp = b"ICY 200 OK\r\n".to_vec();
    resp.extend_from_slice(&payload);
    let served = serve(resp, Duration::ZERO);
    let (hub, rx, logger, _dir) = stack("stream");
    let corr = Arc::new(CorrQueue::new(256));
    corr.set_active(true); // pretend a serial worker is draining

    let handle = ntrip::spawn(
        job(served.port, "RTCM3"),
        hub,
        corr.clone(),
        Arc::new(RwLock::new(None)),
    );
    let (events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(8));
    assert!(handle.join(Duration::from_secs(2)));
    let request = served.handle.join().unwrap();
    logger.shutdown();

    let req_text = String::from_utf8_lossy(&request);
    assert!(
        req_text.starts_with("GET /RTCM3 HTTP/1.0\r\n"),
        "{req_text}"
    );
    assert!(req_text.contains("Authorization: Basic YWxpY2U6c2VjcmV0"));

    let lines = event_lines(&events);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Connecting to 127.0.0.1") && l.contains("attempt 1")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("< ICY 200 OK")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("Receiving data")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Connection closed by the caster")),
        "{lines:?}"
    );
    // The verbatim protocol exchange also landed in the connection log.
    let conn = conn_lines(&events);
    assert!(
        conn.iter().any(|l| l.contains("> GET /RTCM3 HTTP/1.0")),
        "{conn:?}"
    );

    // The marquee regression: bytes coalesced with the status line survive.
    assert_eq!(last_total(&events), payload.len() as u64);
    let mut forwarded = Vec::new();
    while let Some(block) = corr.try_pop() {
        forwarded.extend_from_slice(&block);
    }
    assert_eq!(forwarded, payload, "byte-exact serial forwarding");

    let (summary, failed) = stopped.expect("worker must stop");
    assert!(failed, "RemoteClosed without reconnect is failure-class");
    assert!(summary.contains("closed by the caster"), "{summary}");
}

#[test]
fn sourcetable_fetch_posts_parsed_table() {
    let table: &[u8] = b"CAS;caster.example.com;2101;Example;Op;0;DEU;50.0;8.6\r\n\
NET;Net1;Op;B;N;http://n;http://s;mail@example.com\r\n\
STR;MOUNT1;City;RTCM 3.2;1005(1);2;GPS;Net1;DEU;50.0;8.6;1;1;Gen;none;B;N;3120\r\n\
STR;MOUNT2;Town;RTCM 3.1;1004(1);2;GPS+GLO;Net1;DEU;51.0;9.0;0;0;Gen;none;N;N;2400\r\n\
ENDSOURCETABLE\r\n";
    let mut resp = b"SOURCETABLE 200 OK\r\n".to_vec();
    resp.extend_from_slice(table);
    let served = serve(resp, Duration::ZERO);
    let (hub, rx, logger, dir) = stack("table");

    let handle = ntrip::spawn(
        job(served.port, ""),
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    let (events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(8));
    assert!(handle.join(Duration::from_secs(2)));
    served.handle.join().unwrap();
    logger.shutdown();

    let parsed = events.iter().find_map(|e| match e {
        AppEvent::SourcetableReady { host, port, table } => {
            Some((host.clone(), *port, table.clone()))
        }
        _ => None,
    });
    let (host, port, parsed) = parsed.expect("SourcetableReady must be posted");
    assert_eq!(host, "127.0.0.1");
    assert_eq!(port, served.port);
    assert_eq!(parsed.strs.len(), 2);
    assert_eq!(parsed.casters.len(), 1);
    assert_eq!(parsed.networks.len(), 1);
    assert!(parsed.strs[0].nmea_required);
    assert!(!parsed.strs[1].nmea_required);

    // The parsed table is delivered in-memory via the event; nothing is
    // written to disk (no SourceTables cache exists any more).
    assert!(!dir.join("SourceTables").exists(), "no disk cache written");

    let lines = event_lines(&events);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Downloaded sourcetable: 2 streams, 1 casters, 1 networks")),
        "{lines:?}"
    );
    let (summary, failed) = stopped.expect("worker must stop");
    assert!(!failed, "clean table run: {summary}");
    assert_eq!(summary, "Sourcetable downloaded");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unauthorized_stops_without_reconnect_despite_auto_reconnect() {
    let served = serve(
        b"HTTP/1.0 401 Unauthorized\r\n\r\n".to_vec(),
        Duration::ZERO,
    );
    let (hub, rx, logger, dir) = stack("unauth");
    let mut j = job(served.port, "RTCM3");
    j.reconnect = ReconnectPolicy::Auto;
    j.max_attempts = 10;

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    let started = Instant::now();
    let (events, stopped) = collect_until_stopped(&rx, started + Duration::from_secs(8));
    assert!(handle.join(Duration::from_secs(2)));
    served.handle.join().unwrap();
    logger.shutdown();

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "auth failure must not sit in the reconnect delay"
    );
    let connects = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                AppEvent::Ntrip(open_ntrip_client::bus::NtripStatus::Connecting { .. })
            )
        })
        .count();
    assert_eq!(connects, 1, "exactly one attempt");
    assert!(
        !events.iter().any(|e| matches!(
            e,
            AppEvent::Ntrip(open_ntrip_client::bus::NtripStatus::ReconnectWait { .. })
        )),
        "no reconnect wait for auth failures"
    );
    let (summary, failed) = stopped.expect("worker must stop");
    assert!(failed);
    assert_eq!(summary, "Invalid username or password");
    let lines = event_lines(&events);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Invalid username or password")),
        "{lines:?}"
    );
    // A permanent failure's no-reconnect decision is logged, not silent.
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Not reconnecting: this failure would repeat")),
        "{lines:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_mountpoint_reports_plainly_and_still_surfaces_table() {
    let table =
        b"STR;REAL1;City;RTCM 3.2;;2;GPS;;DEU;50.0;8.6;0;1;Gen;none;B;N;520\r\nENDSOURCETABLE\r\n";
    let mut resp = b"SOURCETABLE 200 OK\r\n".to_vec();
    resp.extend_from_slice(table);
    let served = serve(resp, Duration::ZERO);
    let (hub, rx, logger, dir) = stack("badmount");

    let handle = ntrip::spawn(
        job(served.port, "TYPO_MOUNT"),
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    let (events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(8));
    assert!(handle.join(Duration::from_secs(2)));
    served.handle.join().unwrap();
    logger.shutdown();

    let (summary, failed) = stopped.expect("worker must stop");
    assert!(failed);
    assert!(summary.contains("'TYPO_MOUNT' not found"), "{summary}");
    // The embedded table still populates the dropdown for the retry.
    assert!(
        events.iter().any(
            |e| matches!(e, AppEvent::SourcetableReady { table, .. } if table.strs.len() == 1)
        ),
        "table from MountpointNotFound must be posted"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn always_gga_from_manual_position_reaches_the_wire() {
    // Keep the socket open ~1.5 s after the ICY header so the 300 ms first
    // GGA lands; the server captures it.
    let served = serve(b"ICY 200 OK\r\n".to_vec(), Duration::from_millis(1500));
    let (hub, rx, logger, dir) = stack("gga");
    let mut j = job(served.port, "VRS1");
    j.gga_mode = GgaMode::Always;
    j.gga_source = GgaSource::Manual;
    j.manual_lat = 45.5152;
    j.manual_lon = -122.6784;

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    let (events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(10));
    assert!(handle.join(Duration::from_secs(2)));
    let received = served.handle.join().unwrap();
    logger.shutdown();

    let text = String::from_utf8_lossy(&received);
    let gga_line = text
        .lines()
        .find(|l| l.starts_with("$GPGGA,"))
        .unwrap_or_else(|| panic!("no GGA reached the caster; got: {text}"));
    // Fabrication template parity: healthy RTK fix at the manual position.
    assert!(
        gga_line.contains(",4530.91200,N,12240.70400,W,4,10,1.0,200.0,M,1.0,M,"),
        "{gga_line}"
    );
    // The sent sentence is mirrored into the connection log.
    assert!(
        conn_lines(&events).iter().any(|l| l.contains("> $GPGGA,")),
        "GGA missing from connection log"
    );
    stopped.expect("worker must stop");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Build one CRC-valid RTCM3 frame with the given message type: sync byte,
/// 10-bit length, payload whose first 12 bits carry the type, CRC-24Q.
fn rtcm_frame(msg_type: u16, payload_len: usize) -> Vec<u8> {
    let mut payload = vec![0u8; payload_len.max(2)];
    payload[0] = (msg_type >> 4) as u8;
    payload[1] = ((msg_type & 0x0F) << 4) as u8;
    let mut frame = vec![
        0xD3,
        ((payload.len() >> 8) & 0x03) as u8,
        (payload.len() & 0xFF) as u8,
    ];
    frame.extend_from_slice(&payload);
    let crc = gnss::rtcm::crc24q::crc24q(&frame);
    frame.extend_from_slice(&[(crc >> 16) as u8, (crc >> 8) as u8, crc as u8]);
    frame
}

/// The CHC APIS field regression, end to end through the real worker: the
/// caster answers "ICY 200 OK" plus CRNet-style trailing headers and then
/// HOLDS the stream until any NMEA GGA arrives (verified live against
/// apis-usa.chcnav.com; base-SN mounts are also absent from that caster's
/// sourcetable, so the requirement is always unknown). A when_required
/// profile with no table row - the shipped default - must send a GGA anyway
/// and receive corrections. Under the old "assume not required" policy this
/// test deadlocks into StreamSilence.
#[test]
fn apis_style_caster_streams_only_after_gga() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        sock.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut received = Vec::new();
        let mut buf = [0u8; 2048];
        while !received.windows(4).any(|w| w == b"\r\n\r\n") {
            match sock.read(&mut buf) {
                Ok(0) => return received,
                Ok(n) => received.extend_from_slice(&buf[..n]),
                Err(_) => {}
            }
        }
        sock.write_all(b"ICY 200 OK\r\nServer: CRNet 1.0\r\n\r\n")
            .expect("server write");
        // The APIS latch: no payload until a GGA sentence shows up.
        let deadline = Instant::now() + Duration::from_secs(8);
        while !received.contains(&b'$') && Instant::now() < deadline {
            match sock.read(&mut buf) {
                Ok(0) => return received,
                Ok(n) => received.extend_from_slice(&buf[..n]),
                Err(_) => {}
            }
        }
        if received.contains(&b'$') {
            sock.write_all(&rtcm_frame(1074, 40)).expect("stream write");
        }
        // Hold open briefly so the client's cancel ends the run, not a drop.
        let hold = Instant::now() + Duration::from_secs(5);
        while Instant::now() < hold {
            match sock.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => received.extend_from_slice(&buf[..n]),
                Err(_) => {}
            }
        }
        received
    });

    let (hub, rx, logger, dir) = stack("apis");
    let mut j = job(port, "4862676");
    j.gga_mode = GgaMode::WhenRequired;
    j.gga_source = GgaSource::Manual;
    j.manual_lat = 35.1069;
    j.manual_lon = -106.2909;
    j.stream_requires_gga = None; // APIS never lists its base-SN mounts

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    // Wait for first corrections, then disconnect like a user would.
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut events = Vec::new();
    let mut streamed = false;
    while !streamed && Instant::now() < deadline {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            if matches!(ev, AppEvent::RxBytes { total } if total > 0) {
                streamed = true;
            }
            events.push(ev);
        }
    }
    handle.cancel();
    let (more, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(5));
    assert!(handle.join(Duration::from_secs(2)));
    logger.shutdown();
    events.extend(more);
    let received = server.join().unwrap();

    assert!(
        streamed,
        "no corrections arrived: the GGA latch never opened"
    );
    let received_text = String::from_utf8_lossy(&received);
    assert!(
        received_text.contains("$GPGGA"),
        "the caster never saw a GGA: {received_text}"
    );
    let lines = event_lines(&events);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("will send GGA in case it is required")),
        "the unknown-requirement send assumption must be logged: {lines:?}"
    );
    let (_, failed) = stopped.expect("worker must stop");
    assert!(
        !failed,
        "a user cancel of a healthy stream is not a failure"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Contract bundle for the reconnect supervisor: a drop after healthy data
/// posts ReconnectWait numbered per-outage (counter reset by data), logs the
/// 10 s notice, and the wait is cancellable in 100 ms slices - a cancel must
/// end the worker in far less than the 10 s delay. Also pins the
/// when_required-with-unknown-requirement assumption line (GGA is SENT on
/// unknown requirements since the APIS fix).
#[test]
fn drop_with_auto_reconnect_waits_and_cancel_cuts_the_wait_short() {
    let mut resp = b"ICY 200 OK\r\n".to_vec();
    resp.extend_from_slice(&[1, 2, 3, 4]);
    let served = serve(resp, Duration::ZERO);
    let (hub, rx, logger, dir) = stack("reconnect");
    let mut j = job(served.port, "RTCM3");
    j.reconnect = ReconnectPolicy::Auto;
    j.max_attempts = 5;
    j.gga_mode = GgaMode::WhenRequired;
    j.stream_requires_gga = None; // no cached table row for this mountpoint

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut events = Vec::new();
    let mut next_attempt = None;
    while next_attempt.is_none() && Instant::now() < deadline {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            if let AppEvent::Ntrip(open_ntrip_client::bus::NtripStatus::ReconnectWait {
                next_attempt: n,
            }) = &ev
            {
                next_attempt = Some(*n);
            }
            events.push(ev);
        }
    }
    served.handle.join().unwrap();
    assert_eq!(
        next_attempt,
        Some(1),
        "healthy data must reset the attempt counter: the outage's first retry is attempt 1"
    );

    let cancelled_at = Instant::now();
    handle.cancel();
    let (more, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(5));
    let waited = cancelled_at.elapsed();
    assert!(
        waited < Duration::from_secs(2),
        "cancel took {waited:?}; the 10 s reconnect delay must be sliced"
    );
    assert!(handle.join(Duration::from_secs(2)));
    logger.shutdown();
    events.extend(more);

    let lines = event_lines(&events);
    // The drop of a HEALTHY stream carries the session drop history next to
    // the (reset) attempt budget: "attempt 1" alone read like a first-ever
    // failure on every kick of a flapping session.
    assert!(
        lines.iter().any(|l| l.contains(
            "Stream dropped (1st time this session) - reconnecting in 10 s (attempt 1 of 5)"
        )),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("will send GGA in case it is required")),
        "when_required with no cached table row must log its send assumption: {lines:?}"
    );
    let (summary, failed) = stopped.expect("worker must stop");
    assert_eq!(summary, "Disconnected by user");
    assert!(!failed, "a user cancel is not a failure");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn receiver_gga_passthrough_sends_last_sentence_verbatim() {
    let served = serve(b"ICY 200 OK\r\n".to_vec(), Duration::from_millis(1500));
    let (hub, rx, logger, dir) = stack("ggapass");
    let mut j = job(served.port, "VRS1");
    j.gga_mode = GgaMode::Always;
    j.gga_source = GgaSource::Receiver;
    // Manual position left at 0,0: if fabrication ran by mistake the wire
    // would carry 0000.00000 instead of the receiver's sentence.
    let body = "GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,";
    let ck = body.bytes().fold(0u8, |a, b| a ^ b);
    let raw = format!("${body}*{ck:02X}");
    let last = Arc::new(RwLock::new(Some(raw.clone())));

    let handle = ntrip::spawn(j, hub, Arc::new(CorrQueue::new(256)), last);
    let (events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(10));
    assert!(handle.join(Duration::from_secs(2)));
    let received = served.handle.join().unwrap();
    logger.shutdown();

    let text = String::from_utf8_lossy(&received);
    assert!(
        text.contains(&format!("{raw}\r\n")),
        "receiver GGA must go out verbatim with CRLF; got: {text}"
    );
    assert!(!text.contains("0000.00000"), "no fabricated GGA: {text}");
    assert!(
        conn_lines(&events)
            .iter()
            .any(|l| l.contains(&format!("> {raw}"))),
        "sent GGA missing from the connection log"
    );
    stopped.expect("worker must stop");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Raw TCP mode: no request bytes, no GGA even with an Always policy, the
/// stream forwarded byte-exact, and the deframer stats posted for the M3
/// inspector (a CRC-valid 1005 frame must be counted).
#[test]
fn raw_tcp_streams_without_request_or_gga_and_posts_rtcm_stats() {
    let payload = rtcm_frame(1005, 19);
    // Linger past the 300 ms GGA slot so a wrongly-sent GGA would be caught.
    let served = serve(payload.clone(), Duration::from_millis(800));
    let (hub, rx, logger, dir) = stack("rawtcp");
    let mut j = job(served.port, "IGNORED");
    j.transport = Transport::RawTcp;
    j.gga_mode = GgaMode::Always;
    j.manual_lat = 45.5;
    j.manual_lon = -122.6;
    let corr = Arc::new(CorrQueue::new(256));
    corr.set_active(true);

    let handle = ntrip::spawn(j, hub, corr.clone(), Arc::new(RwLock::new(None)));
    let (events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(8));
    assert!(handle.join(Duration::from_secs(2)));
    let received = served.handle.join().unwrap();
    logger.shutdown();

    assert!(
        received.is_empty(),
        "raw TCP must write nothing to the socket; got: {:?}",
        String::from_utf8_lossy(&received)
    );
    let mut forwarded = Vec::new();
    while let Some(block) = corr.try_pop() {
        forwarded.extend_from_slice(&block);
    }
    assert_eq!(forwarded, payload, "byte-exact forwarding in raw mode");
    assert!(
        events.iter().any(|e| matches!(
            e,
            AppEvent::Rtcm(batch) if batch.frames.iter().any(|&(t, n, _)| t == 1005 && n == 1)
        )),
        "the 1005 frame must be counted in the posted RTCM stats"
    );
    let conn = conn_lines(&events);
    assert!(
        conn.iter().all(|l| !l.contains("> ")),
        "no protocol TX lines in raw mode: {conn:?}"
    );
    stopped.expect("worker must stop");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A caster that accepts, reads the request, then closes without ONE byte of
/// response - the accept-then-die window of a restarting caster. Zero bytes
/// received is a drop, not a response: with auto-reconnect on, the worker
/// must enter the reconnect ladder instead of stopping permanently
/// mid-outage (which would defeat unattended outage-riding).
#[test]
fn close_before_any_response_is_transient_and_reconnects() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        sock.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut received = Vec::new();
        let mut buf = [0u8; 2048];
        while !received.windows(4).any(|w| w == b"\r\n\r\n") {
            match sock.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => received.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        // Drop the socket here: graceful close, zero response bytes.
    });
    let (hub, rx, logger, dir) = stack("emptyclose");
    let mut j = job(port, "RTCM3");
    j.reconnect = ReconnectPolicy::Auto;
    j.max_attempts = 5;

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut events = Vec::new();
    let mut saw_wait = false;
    while !saw_wait && Instant::now() < deadline {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            saw_wait = matches!(
                &ev,
                AppEvent::Ntrip(open_ntrip_client::bus::NtripStatus::ReconnectWait { .. })
            );
            events.push(ev);
        }
    }
    server.join().unwrap();
    assert!(
        saw_wait,
        "zero-byte close must enter the reconnect ladder, not stop permanently: {:?}",
        event_lines(&events)
    );
    handle.cancel();
    let (more, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(5));
    assert!(handle.join(Duration::from_secs(2)));
    logger.shutdown();
    events.extend(more);
    let lines = event_lines(&events);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Connection closed before any response")),
        "the close summary must read as a drop, not a response: {lines:?}"
    );
    stopped.expect("worker must stop after cancel");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Alert-on-drop parity (checklist row 15): an ESTABLISHED stream dropping
/// rings the bell even with auto-reconnect off - the one drop the user most
/// needs to hear. The alert path is observed through its failure event line
/// (the configured .wav deliberately does not exist).
#[test]
fn established_stream_drop_alerts_even_without_reconnect() {
    let mut resp = b"ICY 200 OK\r\n".to_vec();
    resp.extend_from_slice(&[1, 2, 3, 4]);
    let served = serve(resp, Duration::ZERO);
    let (hub, rx, logger, dir) = stack("alertdrop");
    let mut j = job(served.port, "RTCM3");
    j.reconnect = ReconnectPolicy::OptionsOff;
    j.audio_alert = dir.join("missing-alert.wav").to_string_lossy().into_owned();

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    let (events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(8));
    assert!(handle.join(Duration::from_secs(2)));
    served.handle.join().unwrap();
    logger.shutdown();

    let lines = event_lines(&events);
    assert!(
        lines.iter().any(|l| l.contains("Receiving data")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("Audio alert failed")),
        "a streamed connection's drop must fire the alert even without reconnect: {lines:?}"
    );
    let (_, failed) = stopped.expect("worker must stop");
    assert!(failed, "RemoteClosed without reconnect is failure-class");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The complement: a connect failure when nothing ever streamed stays
/// silent - nothing the user had was lost, so an "outage bell" on attempt 1
/// against a down network would be noise.
#[test]
fn connect_failure_without_stream_stays_silent() {
    // A loopback port with no listener: refused fast, nothing ever streams.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let (hub, rx, logger, dir) = stack("silentfail");
    let mut j = job(port, "RTCM3");
    j.reconnect = ReconnectPolicy::Auto;
    j.max_attempts = 3;
    j.audio_alert = dir.join("missing-alert.wav").to_string_lossy().into_owned();

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut events = Vec::new();
    let mut saw_wait = false;
    while !saw_wait && Instant::now() < deadline {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            saw_wait = matches!(
                &ev,
                AppEvent::Ntrip(open_ntrip_client::bus::NtripStatus::ReconnectWait { .. })
            );
            events.push(ev);
        }
    }
    assert!(
        saw_wait,
        "the reconnect ladder must engage on connect failure"
    );
    handle.cancel();
    let (more, _) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(5));
    assert!(handle.join(Duration::from_secs(2)));
    logger.shutdown();
    events.extend(more);
    let lines = event_lines(&events);
    assert!(
        !lines.iter().any(|l| l.contains("Audio alert")),
        "no alert may fire when no stream was ever established: {lines:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Exit code 0 for a clean run: the budget expires mid-stream, the selftest
/// cancels, and a user-shaped cancel is not a failure.
#[test]
fn selftest_exit_code_zero_on_clean_stream() {
    let mut resp = b"ICY 200 OK\r\n".to_vec();
    resp.extend_from_slice(&rtcm_frame(1074, 40));
    let served = serve(resp, Duration::from_secs(6));
    let args: Vec<String> = [
        "--selftest",
        "--host",
        "127.0.0.1",
        "--port",
        &served.port.to_string(),
        "--mount",
        "ANY",
        "--seconds",
        "1",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(open_ntrip_client::selftest::run(&args), 0);
    served.handle.join().unwrap();
}

/// Raw capture through the worker + logger stack: the sink file must be
/// byte-exact against what the mock caster served, and the logger must
/// receipt the capture (open notice + close notice with the byte count) as
/// ordinary event lines.
#[test]
fn capture_file_is_byte_exact_and_receipted() {
    let payload: Vec<u8> = {
        let mut p = rtcm_frame(1074, 40);
        p.extend_from_slice(&rtcm_frame(1005, 19));
        p
    };
    let mut resp = b"ICY 200 OK\r\n".to_vec();
    resp.extend_from_slice(&payload);
    let served = serve(resp, Duration::ZERO);
    let (hub, rx, logger, dir) = stack("capture");
    let capture_path = dir.join("captured.rtcm");
    let mut j = job(served.port, "RTCM3");
    j.capture = Some(CaptureTarget::File(capture_path.clone()));

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    let (mut events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(8));
    assert!(handle.join(Duration::from_secs(2)));
    served.handle.join().unwrap();
    // The logger receipts the close asynchronously; shutdown() joins the
    // thread (flushing the sink), after which the lines are all queued.
    logger.shutdown();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    stopped.expect("worker must stop");

    assert_eq!(
        std::fs::read(&capture_path).unwrap(),
        payload,
        "capture file must match the served correction bytes exactly"
    );
    let lines = event_lines(&events);
    assert!(
        lines.iter().any(|l| l.contains("Capturing corrections to")),
        "{lines:?}"
    );
    let receipt = format!("({} bytes)", payload.len());
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Correction capture closed") && l.contains(&receipt)),
        "close receipt with byte count missing: {lines:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A TLS mock caster: rcgen self-signed certificate, rustls server side.
/// Returns every plaintext byte received from the client; an aborted
/// handshake (the fail-closed test) returns empty.
fn tls_serve(response: Vec<u8>, linger: Duration) -> Served {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let ck = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
        ])
        .expect("self-signed cert");
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(ck.key_pair.serialize_der().into());
        let config = Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![ck.cert.der().clone()], key)
                .expect("server config"),
        );
        let (mut sock, _) = listener.accept().expect("accept");
        sock.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut conn = rustls::ServerConnection::new(config).expect("server conn");
        let deadline = Instant::now() + Duration::from_secs(8);
        while conn.is_handshaking() && Instant::now() < deadline {
            match conn.complete_io(&mut sock) {
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                // A verified client rejecting our self-signed cert aborts
                // here with a fatal alert; that is a valid test outcome.
                Err(_) => return Vec::new(),
            }
        }
        let mut tls = rustls::StreamOwned::new(conn, sock);
        let mut received = Vec::new();
        let mut buf = [0u8; 2048];
        while !received.windows(4).any(|w| w == b"\r\n\r\n") && Instant::now() < deadline {
            match tls.read(&mut buf) {
                Ok(0) => return received,
                Ok(n) => received.extend_from_slice(&buf[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => return received,
            }
        }
        if tls.write_all(&response).is_err() {
            return received;
        }
        let end = Instant::now() + linger;
        while Instant::now() < end {
            match tls.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => received.extend_from_slice(&buf[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
        received
    });
    Served { port, handle }
}

/// Fail closed: verified TLS against a self-signed caster must refuse the
/// connection, say so in the event stream, and never enter the reconnect
/// ladder (the failure is deterministic; retrying is noise) even with
/// auto-reconnect enabled.
#[test]
fn tls_verified_mode_fails_closed_against_self_signed_cert() {
    let served = tls_serve(b"ICY 200 OK\r\n".to_vec(), Duration::ZERO);
    let (hub, rx, logger, dir) = stack("tlsverified");
    let mut j = job(served.port, "RTCM3");
    j.tls = true;
    j.allow_invalid_certs = false;
    j.reconnect = ReconnectPolicy::Auto;
    j.max_attempts = 10;

    let started = Instant::now();
    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    let (events, stopped) = collect_until_stopped(&rx, started + Duration::from_secs(8));
    assert!(handle.join(Duration::from_secs(2)));
    served.handle.join().unwrap();
    logger.shutdown();

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "certificate rejection must not sit in the reconnect delay"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            AppEvent::Ntrip(open_ntrip_client::bus::NtripStatus::ReconnectWait { .. })
        )),
        "a deterministic TLS failure must never reconnect"
    );
    let (summary, failed) = stopped.expect("worker must stop");
    assert!(failed, "cert rejection is failure-class");
    assert!(summary.contains("TLS handshake failed"), "{summary}");
    let lines = event_lines(&events);
    assert!(
        lines.iter().any(|l| l.contains("TLS handshake failed")),
        "the rejection must be visible in the event log: {lines:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The diagnostic override: insecure mode accepts the self-signed cert,
/// streams corrections byte-exact, and the handshake receipt in the logs
/// says loudly that the certificate was NOT verified.
#[test]
fn tls_insecure_mode_streams_and_logs_unverified_handshake() {
    let payload = rtcm_frame(1074, 40);
    let mut resp = b"ICY 200 OK\r\n".to_vec();
    resp.extend_from_slice(&payload);
    let served = tls_serve(resp, Duration::ZERO);
    let (hub, rx, logger, dir) = stack("tlsinsecure");
    let mut j = job(served.port, "RTCM3");
    j.tls = true;
    j.allow_invalid_certs = true;
    let corr = Arc::new(CorrQueue::new(256));
    corr.set_active(true);

    let handle = ntrip::spawn(j, hub, corr.clone(), Arc::new(RwLock::new(None)));
    let (events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(8));
    assert!(handle.join(Duration::from_secs(2)));
    let request = served.handle.join().unwrap();
    logger.shutdown();

    // The request travelled inside TLS and decrypted server-side.
    let req_text = String::from_utf8_lossy(&request);
    assert!(
        req_text.starts_with("GET /RTCM3 HTTP/1.0\r\n"),
        "{req_text}"
    );

    let lines = event_lines(&events);
    let receipt = lines
        .iter()
        .find(|l| l.contains("TLS handshake complete"))
        .unwrap_or_else(|| panic!("no handshake receipt: {lines:?}"));
    assert!(
        receipt.contains("NOT verified"),
        "insecure mode must be loud about itself: {receipt}"
    );
    assert!(
        receipt.contains("TLS"),
        "negotiated protocol version missing: {receipt}"
    );
    // The receipt is mirrored to the connection log verbatim.
    assert!(
        conn_lines(&events)
            .iter()
            .any(|l| l.contains("TLS handshake complete")),
        "handshake receipt missing from the connection log"
    );

    let mut forwarded = Vec::new();
    while let Some(block) = corr.try_pop() {
        forwarded.extend_from_slice(&block);
    }
    assert_eq!(forwarded, payload, "byte-exact streaming through TLS");
    let (_, failed) = stopped.expect("worker must stop");
    assert!(failed, "RemoteClosed without reconnect is failure-class");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Cancel during the TLS handshake: a caster that accepts TCP but never
/// answers the ClientHello leaves rustls blocked mid-handshake; cancel must
/// still end the worker promptly (flag checked every 400 ms read slice, and
/// the raw-socket shutdown unblocks the read underneath rustls).
#[test]
fn tls_cancel_during_handshake_joins_fast() {
    // Plain TCP acceptor that never writes: the handshake can never finish.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (sock, _) = listener.accept().expect("accept");
        // Hold the socket open well past the test's cancel.
        thread::sleep(Duration::from_secs(4));
        drop(sock);
    });
    let (hub, rx, logger, dir) = stack("tlscancelhs");
    let mut j = job(port, "RTCM3");
    j.tls = true;

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    // Give the worker time to connect and enter the handshake.
    thread::sleep(Duration::from_millis(600));
    let cancelled_at = Instant::now();
    handle.cancel();
    assert!(
        handle.join(Duration::from_secs(2)),
        "cancel during TLS handshake must join within 2 s"
    );
    assert!(cancelled_at.elapsed() < Duration::from_secs(2));
    let (_, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(2));
    logger.shutdown();
    server.join().unwrap();
    let (summary, failed) = stopped.expect("worker must post Stopped");
    assert_eq!(summary, "Disconnected by user");
    assert!(!failed, "a user cancel is not a failure");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Cancel during TLS streaming: shutdown on the raw try_clone must fail the
/// read rustls performs underneath StreamOwned, ending the worker promptly.
#[test]
fn tls_cancel_during_streaming_joins_fast() {
    let mut resp = b"ICY 200 OK\r\n".to_vec();
    resp.extend_from_slice(&rtcm_frame(1074, 40));
    let served = tls_serve(resp, Duration::from_secs(6));
    let (hub, rx, logger, dir) = stack("tlscancelstream");
    let mut j = job(served.port, "RTCM3");
    j.tls = true;
    j.allow_invalid_certs = true;

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    // Wait until the stream is established before cancelling.
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut events = Vec::new();
    let mut streaming = false;
    while !streaming && Instant::now() < deadline {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            streaming = matches!(
                &ev,
                AppEvent::Ntrip(open_ntrip_client::bus::NtripStatus::Streaming)
            );
            events.push(ev);
        }
    }
    assert!(streaming, "stream never established over TLS");
    let cancelled_at = Instant::now();
    handle.cancel();
    assert!(
        handle.join(Duration::from_secs(2)),
        "cancel during TLS streaming must join within 2 s"
    );
    assert!(cancelled_at.elapsed() < Duration::from_secs(2));
    let (more, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(2));
    logger.shutdown();
    served.handle.join().unwrap();
    events.extend(more);
    let (summary, failed) = stopped.expect("worker must post Stopped");
    assert_eq!(summary, "Disconnected by user");
    assert!(!failed);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Wrap `body` in RFC 9112 chunked framing, split into two chunks at an
/// awkward offset so the decoder's boundary handling is exercised.
fn chunk_encode(body: &[u8]) -> Vec<u8> {
    let split = (body.len() / 2).max(1).min(body.len());
    let mut out = Vec::new();
    for part in [&body[..split], &body[split..]] {
        if part.is_empty() {
            continue;
        }
        out.extend_from_slice(format!("{:X}\r\n", part.len()).as_bytes());
        out.extend_from_slice(part);
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// NTRIP v2 end to end through the real worker: HTTP/1.1 request with the
/// Ntrip-Version and Host headers, chunked response body decoded back to the
/// exact correction bytes, RTCM stats posted from the decoded stream.
#[test]
fn v2_chunked_stream_decodes_through_the_worker() {
    let payload: Vec<u8> = {
        let mut p = rtcm_frame(1074, 40);
        p.extend_from_slice(&rtcm_frame(1005, 19));
        p
    };
    let mut resp =
        b"HTTP/1.1 200 OK\r\nContent-Type: gnss/data\r\nTransfer-Encoding: chunked\r\n\r\n"
            .to_vec();
    resp.extend_from_slice(&chunk_encode(&payload));
    // No terminal chunk: real casters stream forever; the server closing the
    // socket mid-body is the ordinary end of a v2 session.
    let served = serve(resp, Duration::ZERO);
    let (hub, rx, logger, dir) = stack("v2chunked");
    let mut j = job(served.port, "RTCM3");
    j.version = NtripVersion::V2;
    let corr = Arc::new(CorrQueue::new(256));
    corr.set_active(true);

    let handle = ntrip::spawn(j, hub, corr.clone(), Arc::new(RwLock::new(None)));
    let (events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(8));
    assert!(handle.join(Duration::from_secs(2)));
    let request = served.handle.join().unwrap();
    logger.shutdown();

    let req_text = String::from_utf8_lossy(&request);
    assert!(
        req_text.starts_with("GET /RTCM3 HTTP/1.1\r\n"),
        "{req_text}"
    );
    assert!(
        req_text.contains("Ntrip-Version: Ntrip/2.0\r\n"),
        "{req_text}"
    );
    assert!(
        req_text.contains(&format!("Host: 127.0.0.1:{}\r\n", served.port)),
        "{req_text}"
    );

    // Chunk framing stripped: the forwarded bytes are the pure payload.
    let mut forwarded = Vec::new();
    while let Some(block) = corr.try_pop() {
        forwarded.extend_from_slice(&block);
    }
    assert_eq!(forwarded, payload, "chunk framing must be stripped exactly");
    assert_eq!(last_total(&events), payload.len() as u64);
    assert!(
        events.iter().any(|e| matches!(
            e,
            AppEvent::Rtcm(batch) if batch.frames.iter().any(|&(t, n, _)| t == 1074 && n == 1)
        )),
        "RTCM stats must come from the DECODED stream"
    );
    stopped.expect("worker must stop");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The close-diagnostics contract, end to end: a healthy stream that the
/// caster drops must leave a self-explanatory log - enriched "Receiving
/// data" (first-data latency + GGA plan), an early traffic milestone, a
/// close line carrying duration and byte count mirrored into the connection
/// log, and the spelled-out decision not to reconnect.
#[test]
fn remote_close_logs_duration_bytes_and_reconnect_decision() {
    let mut resp = b"ICY 200 OK\r\n".to_vec();
    resp.extend_from_slice(&vec![0xAAu8; 12_000]); // past the 10 kB milestone
    let served = serve(resp, Duration::ZERO);
    let (hub, rx, logger, dir) = stack("closediag");
    let j = job(served.port, "RTCM3"); // reconnect OptionsOff, GGA off

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    let (events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(8));
    assert!(handle.join(Duration::from_secs(2)));
    served.handle.join().unwrap();
    logger.shutdown();

    let lines = event_lines(&events);
    let receiving = lines
        .iter()
        .find(|l| l.contains("Receiving data"))
        .unwrap_or_else(|| panic!("{lines:?}"));
    assert!(receiving.contains("first data"), "{receiving}");
    assert!(receiving.contains("GGA off"), "{receiving}");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Received 10 kB (stream healthy)")),
        "early milestone missing: {lines:?}"
    );
    let close = lines
        .iter()
        .find(|l| l.contains("Connection closed by the caster"))
        .unwrap_or_else(|| panic!("{lines:?}"));
    assert!(close.contains("(after "), "duration missing: {close}");
    assert!(
        close.contains("12.0 kB received)"),
        "bytes missing: {close}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Auto-reconnect is off - staying disconnected")),
        "the negative reconnect decision must be logged: {lines:?}"
    );
    // The connection log tells the same story without cross-referencing.
    let conn = conn_lines(&events);
    assert!(
        conn.iter()
            .any(|l| l.contains("Connection closed by the caster")),
        "close summary missing from the connection log: {conn:?}"
    );
    assert!(
        conn.iter().any(|l| l.contains("Auto-reconnect is off")),
        "{conn:?}"
    );
    let (_, failed) = stopped.expect("worker must stop");
    assert!(failed);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The user-report scenario: a mount whose sourcetable row says nmea=1, a
/// profile whose GGA source is the receiver, and no GPS attached. The first
/// missed GGA slot explains cause and remedy, and the caster's kick carries
/// the GGA-starvation hint.
#[test]
fn gga_required_mount_dropped_without_gga_gets_the_hint() {
    let mut resp = b"ICY 200 OK\r\n".to_vec();
    resp.extend_from_slice(&[1, 2, 3, 4]);
    // Linger past the 300 ms first GGA slot so the "no receiver GGA" miss
    // happens before the server closes.
    let served = serve(resp, Duration::from_millis(1500));
    let (hub, rx, logger, dir) = stack("ggahint");
    let mut j = job(served.port, "VRS1");
    j.gga_mode = GgaMode::WhenRequired;
    j.gga_source = GgaSource::Receiver;
    j.stream_requires_gga = Some(true); // cached table row: nmea=1

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)), // no receiver GGA will ever exist
    );
    let (events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(10));
    assert!(handle.join(Duration::from_secs(2)));
    served.handle.join().unwrap();
    logger.shutdown();

    let lines = event_lines(&events);
    let miss = lines
        .iter()
        .find(|l| l.contains("No receiver GGA available"))
        .unwrap_or_else(|| panic!("{lines:?}"));
    assert!(
        miss.contains("no GPS receiver is connected"),
        "first miss must explain cause and remedy: {miss}"
    );
    let hint = lines
        .iter()
        .find(|l| l.contains("This mount requests NMEA GGA"))
        .unwrap_or_else(|| panic!("kick hint missing: {lines:?}"));
    assert!(hint.contains("nmea=1"), "{hint}");
    assert!(hint.contains("none was sent"), "{hint}");
    stopped.expect("worker must stop");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The configuration certain to be kicked - sourcetable nmea=1 with Send GGA
/// off - warns at connect time, before the caster gets a chance to act.
#[test]
fn gga_off_but_required_warns_at_connect() {
    let mut resp = b"ICY 200 OK\r\n".to_vec();
    resp.extend_from_slice(&[1, 2, 3, 4]);
    let served = serve(resp, Duration::ZERO);
    let (hub, rx, logger, dir) = stack("ggaoffwarn");
    let mut j = job(served.port, "VRS1");
    j.gga_mode = GgaMode::Off;
    j.stream_requires_gga = Some(true);

    let handle = ntrip::spawn(
        j,
        hub,
        Arc::new(CorrQueue::new(256)),
        Arc::new(RwLock::new(None)),
    );
    let (events, stopped) = collect_until_stopped(&rx, Instant::now() + Duration::from_secs(8));
    assert!(handle.join(Duration::from_secs(2)));
    served.handle.join().unwrap();
    logger.shutdown();

    let lines = event_lines(&events);
    let warn = lines
        .iter()
        .find(|l| l.contains("Send GGA is off"))
        .unwrap_or_else(|| panic!("connect-time warning missing: {lines:?}"));
    assert!(warn.contains("requiring NMEA GGA"), "{warn}");
    assert!(warn.contains("'VRS1'"), "{warn}");
    // Streaming starts with the honest plan on display.
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Receiving data") && l.contains("GGA off")),
        "{lines:?}"
    );
    stopped.expect("worker must stop");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn selftest_exit_codes_usage_and_auth_failure() {
    // Usage error: 2, before any network activity.
    assert_eq!(
        open_ntrip_client::selftest::run(&["--selftest".to_string()]),
        2
    );

    // Real 401 through the whole selftest stack: 1.
    let served = serve(
        b"HTTP/1.0 401 Unauthorized\r\n\r\n".to_vec(),
        Duration::ZERO,
    );
    let args: Vec<String> = [
        "--selftest",
        "--host",
        "127.0.0.1",
        "--port",
        &served.port.to_string(),
        "--mount",
        "ANY",
        "--seconds",
        "8",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(open_ntrip_client::selftest::run(&args), 1);
    served.handle.join().unwrap();
}
