//! End-to-end tests against a real 127.0.0.1 TcpListener: a thread serves a
//! canned response, the client side runs a minimal blocking driver identical
//! in shape to the probe example (and the future GUI worker loop). Asserts on
//! the collected Outputs and on the request bytes the server received.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use ntrip_core::{
    CloseReason, GgaPolicy, NtripSession, NtripVersion, Output, SessionConfig, Transport,
};

fn cfg(port: u16, mount: &str, version: NtripVersion) -> SessionConfig {
    SessionConfig {
        host: "127.0.0.1".to_string(),
        port,
        mountpoint: mount.to_string(),
        username: "alice".to_string(),
        password: "secret".to_string(),
        version,
        transport: Transport::Ntrip,
        user_agent: "NTRIP OpenNtripClient/test".to_string(),
        gga: GgaPolicy::Off,
    }
}

struct Served {
    port: u16,
    /// Joins to the raw request bytes the server received.
    handle: thread::JoinHandle<Vec<u8>>,
}

/// One-shot caster: accept, read the request up to its blank line, write the
/// canned response, close (FIN). Queued response data is delivered before the
/// client observes EOF, so no artificial delays are needed.
fn serve(response: Vec<u8>) -> Served {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        sock.set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let mut req = Vec::new();
        let mut buf = [0u8; 1024];
        while !req.windows(4).any(|w| w == b"\r\n\r\n") {
            match sock.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => req.extend_from_slice(&buf[..n]),
            }
        }
        sock.write_all(&response).expect("server write");
        req
    });
    Served { port, handle }
}

/// Minimal blocking driver, same shape as the probe example's loop: read with
/// a short timeout, feed bytes, tick, stop on a terminal output.
fn drive(cfg: SessionConfig) -> Vec<Output> {
    let (mut session, req) = NtripSession::new(cfg.clone(), Instant::now());
    let mut sock = TcpStream::connect(("127.0.0.1", cfg.port)).expect("connect");
    sock.set_read_timeout(Some(Duration::from_millis(25)))
        .unwrap();
    if !req.is_empty() {
        sock.write_all(&req).expect("client write");
    }
    let mut outs = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut buf = [0u8; 4096];
    loop {
        // Close ends every failure path; a clean sourcetable ends with the
        // session Done and no Close, so Sourcetable is terminal too.
        if outs
            .iter()
            .any(|o| matches!(o, Output::Close(_) | Output::Sourcetable(_)))
        {
            break;
        }
        if Instant::now() >= deadline {
            session.cancel(&mut outs);
            break;
        }
        match sock.read(&mut buf) {
            Ok(0) => {
                session.on_remote_close(&mut outs);
                break;
            }
            Ok(n) => session.on_bytes(&buf[..n], Instant::now(), &mut outs),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => panic!("client socket error: {e}"),
        }
        session.on_tick(Instant::now(), &mut outs);
    }
    outs
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

fn chunk(data: &[u8]) -> Vec<u8> {
    let mut v = format!("{:X}\r\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\r\n");
    v
}

#[test]
fn mock_icy_with_coalesced_payload() {
    let payload: Vec<u8> = (0..=255).map(|b| b as u8).collect();
    let mut resp = b"ICY 200 OK\r\n".to_vec();
    resp.extend_from_slice(&payload);
    let served = serve(resp);

    let outs = drive(cfg(served.port, "RTCM3", NtripVersion::V1));
    let req = served.handle.join().unwrap();
    let req_text = String::from_utf8_lossy(&req);
    assert!(
        req_text.starts_with("GET /RTCM3 HTTP/1.0\r\n"),
        "request: {req_text}"
    );
    assert!(req_text.contains("\r\nAuthorization: Basic YWxpY2U6c2VjcmV0\r\n"));
    assert!(req_text.ends_with("\r\n\r\n"));

    assert!(
        outs.iter()
            .any(|o| matches!(o, Output::ProtocolRx(l) if l == "ICY 200 OK"))
    );
    assert_eq!(
        corrections(&outs),
        payload,
        "every payload byte must survive"
    );
    match closes(&outs).as_slice() {
        [reason @ CloseReason::RemoteClosed] => assert!(reason.auto_reconnect()),
        other => panic!("expected RemoteClosed, got {other:?}"),
    }
}

#[test]
fn mock_sourcetable_fetch_and_parse() {
    let table: &[u8] = b"CAS;caster.example.com;2101;Example;Op;0;DEU;50.0;8.6\r\n\
NET;Net1;Op;B;N;http://n;http://s;mail@example.com\r\n\
STR;MOUNT1;City;RTCM 3.2;1005(1);2;GPS;Net1;DEU;50.0;8.6;1;1;Gen;none;B;N;3120\r\n\
STR;MOUNT2;Town;RTCM 3.1;1004(1);2;GPS+GLO;Net1;DEU;51.0;9.0;0;0;Gen;none;N;N;2400\r\n\
ENDSOURCETABLE\r\n";
    let mut resp = b"SOURCETABLE 200 OK\r\n".to_vec();
    resp.extend_from_slice(table);
    let served = serve(resp);

    let outs = drive(cfg(served.port, "", NtripVersion::V1));
    served.handle.join().unwrap();

    let tables: Vec<&Vec<u8>> = outs
        .iter()
        .filter_map(|o| match o {
            Output::Sourcetable(b) => Some(b),
            _ => None,
        })
        .collect();
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].as_slice(), table);
    assert!(
        closes(&outs).is_empty(),
        "clean table completion has no Close"
    );

    let parsed = ntrip_core::sourcetable::parse(tables[0]);
    assert_eq!(parsed.strs.len(), 2);
    assert_eq!(parsed.casters.len(), 1);
    assert_eq!(parsed.networks.len(), 1);
    assert_eq!(parsed.strs[0].mountpoint, "MOUNT1");
    assert!(parsed.strs[0].nmea_required);
    assert!(!parsed.strs[1].nmea_required);
    assert!(parsed.unparsed.is_empty());
}

#[test]
fn mock_unauthorized() {
    let served = serve(b"HTTP/1.0 401 Unauthorized\r\n\r\n".to_vec());
    let outs = drive(cfg(served.port, "RTCM3", NtripVersion::V1));
    served.handle.join().unwrap();
    match closes(&outs).as_slice() {
        [reason @ CloseReason::Unauthorized] => {
            assert!(
                !reason.auto_reconnect(),
                "retrying bad credentials is pointless"
            );
        }
        other => panic!("expected Unauthorized, got {other:?}"),
    }
}

#[test]
fn mock_v2_chunked_stream() {
    let mut resp = b"HTTP/1.1 200 OK\r\n\
Ntrip-Version: Ntrip/2.0\r\n\
Content-Type: gnss/data\r\n\
Transfer-Encoding: chunked\r\n\
\r\n"
        .to_vec();
    resp.extend_from_slice(&chunk(b"rtcm-frame-1"));
    resp.extend_from_slice(&chunk(b"rtcm-frame-2"));
    resp.extend_from_slice(b"0\r\n\r\n");
    let served = serve(resp);

    let outs = drive(cfg(served.port, "RTK", NtripVersion::V2));
    let req = served.handle.join().unwrap();
    let req_text = String::from_utf8_lossy(&req);
    assert!(
        req_text.starts_with("GET /RTK HTTP/1.1\r\n"),
        "request: {req_text}"
    );
    assert!(req_text.contains("\r\nNtrip-Version: Ntrip/2.0\r\n"));
    assert!(req_text.contains("\r\nHost: 127.0.0.1:"));

    assert_eq!(corrections(&outs), b"rtcm-frame-1rtcm-frame-2");
    // The terminal chunk ends the response body: semantically a remote close.
    assert_eq!(closes(&outs), vec![CloseReason::RemoteClosed]);
}
