//! Headless verification mode: `OpenNtripClient --selftest --host H --port P
//! [--mount M] [--user U] [--pass W] [--v2] [--tcp] [--tls] [--insecure-tls]
//! [--gga LAT,LON] [--capture FILE] [--seconds N]`.
//!
//! Runs the REAL ntrip worker and logger stack - the same code the GUI
//! drives - with no GUI and no serial port, printing every event line to
//! stdout. CI and the orchestrator assert on the exit code:
//! 0 = clean sourcetable or stream run, 1 = failure-class close, 2 = usage.
//! A stream run (mountpoint or raw TCP) is only clean if it delivered at
//! least one correction byte: the default budget is shorter than the 30 s
//! silence timeout, so a dead-but-accepting mountpoint would otherwise be
//! cancelled at budget and certified healthy. A table run is likewise only
//! clean if a parsed sourcetable actually arrived - a big table on a slow
//! link cancelled at budget proves nothing.
//!
//! The release binary is GUI-subsystem (no console of its own), so `run`
//! first attaches to the parent's console - otherwise every line printed by
//! the shipped exe would silently vanish. Shells never WAIT for a GUI-
//! subsystem process, so exit-code consumers must wait explicitly:
//! PowerShell `(Start-Process OpenNtripClient.exe -ArgumentList ... -Wait
//! -PassThru).ExitCode`, cmd `start /wait`; piping the output also forces
//! PowerShell to wait. `cargo run -- --selftest ...` waits by itself.

use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use ntrip_core::{NtripVersion, Transport};

use crate::bus::{AppEvent, Hub, NtripStatus, Repaint};
use crate::logging::Logger;
use crate::settings::{GgaMode, GgaSource};
use crate::workers::CorrQueue;
use crate::workers::ntrip::{self, NtripJob};
use crate::{paths, user_agent};

const USAGE: &str = "usage: OpenNtripClient --selftest --host H --port P \
[--mount M] [--user U] [--pass W] [--v2] [--tcp] [--tls] [--insecure-tls] \
[--gga LAT,LON] [--capture FILE] [--seconds N]";

/// Ceiling for `--seconds`: one day. See the parse-time check for why.
const MAX_SECONDS: u64 = 86_400;

struct Args {
    host: String,
    port: u16,
    mount: String,
    user: String,
    pass: String,
    v2: bool,
    tcp: bool,
    tls: bool,
    /// Accept any certificate (implies --tls).
    insecure_tls: bool,
    /// Send a manual-position GGA (Always mode): needed to verify casters
    /// that hold the stream until a position arrives (CHC APIS).
    gga: Option<(f64, f64)>,
    /// Write raw correction bytes to this file.
    capture: Option<String>,
    seconds: u64,
}

/// "LAT,LON" in decimal degrees. (0, 0) exactly is rejected: the worker
/// treats it as "position never configured" and would send nothing, which
/// silently defeats the flag's purpose.
fn parse_gga(value: &str) -> Result<(f64, f64), String> {
    let err = || format!("invalid --gga (want LAT,LON in decimal degrees): {value}");
    let (lat, lon) = value.split_once(',').ok_or_else(err)?;
    let lat: f64 = lat.trim().parse().map_err(|_| err())?;
    let lon: f64 = lon.trim().parse().map_err(|_| err())?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(err());
    }
    if lat == 0.0 && lon == 0.0 {
        return Err("--gga 0,0 means 'position unset' and sends nothing; \
                    give a real position"
            .to_string());
    }
    Ok((lat, lon))
}

fn next_value<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<String, String> {
    it.next()
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut out = Args {
        host: String::new(),
        port: 0,
        mount: String::new(),
        user: String::new(),
        pass: String::new(),
        v2: false,
        tcp: false,
        tls: false,
        insecure_tls: false,
        gga: None,
        capture: None,
        seconds: 10,
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--selftest" => {}
            "--host" => out.host = next_value(&mut it, "--host")?,
            "--port" => {
                let v = next_value(&mut it, "--port")?;
                out.port = v.parse().map_err(|_| format!("invalid port: {v}"))?;
            }
            "--mount" => out.mount = next_value(&mut it, "--mount")?,
            "--user" => out.user = next_value(&mut it, "--user")?,
            "--pass" => out.pass = next_value(&mut it, "--pass")?,
            "--v2" => out.v2 = true,
            "--tcp" => out.tcp = true,
            "--tls" => out.tls = true,
            "--insecure-tls" => out.insecure_tls = true,
            "--gga" => out.gga = Some(parse_gga(&next_value(&mut it, "--gga")?)?),
            "--capture" => out.capture = Some(next_value(&mut it, "--capture")?),
            "--seconds" => {
                let v = next_value(&mut it, "--seconds")?;
                out.seconds = v.parse().map_err(|_| format!("invalid seconds: {v}"))?;
                // A ceiling keeps `Instant + Duration` in run() from
                // overflowing (a u64::MAX budget panics before any output),
                // and anything past a day is a typo, not a test plan.
                if out.seconds > MAX_SECONDS {
                    return Err(format!("invalid seconds: {v} (max {MAX_SECONDS})"));
                }
            }
            other => return Err(format!("unknown flag: {other}")),
        }
    }
    if out.host.is_empty() {
        return Err("--host is required".to_string());
    }
    if out.port == 0 {
        return Err("--port is required".to_string());
    }
    if out.tcp && out.gga.is_some() {
        // Raw TCP has no NTRIP handshake and the session never asks for GGA;
        // silently accepting the combination would "certify" a caster while
        // sending nothing.
        return Err("--gga has no effect with --tcp".to_string());
    }
    Ok(out)
}

/// Console line for one bus event, or None for events the selftest does not
/// print. Deliberately ignores `ConnLine`: the worker mirrors every
/// connection-log line it can emit under a selftest config (protocol TX/RX,
/// reconnect notices) into the event sink too, so printing both sinks
/// echoed every protocol line twice. The one conn-exclusive line, a GGA
/// send under --gga, stays unprinted; the "Receiving data (...; sending GGA
/// every 10 s ...)" plan line plus the delivered byte count already certify
/// the GGA path.
fn console_line(ev: &AppEvent) -> Option<String> {
    match ev {
        AppEvent::EventLine(line) => Some(line.clone()),
        AppEvent::SourcetableReady { host, port, table } => Some(format!(
            "sourcetable from {host}:{port}: {} STR, {} CAS, {} NET, {} unparsed",
            table.strs.len(),
            table.casters.len(),
            table.networks.len(),
            table.unparsed.len()
        )),
        _ => None,
    }
}

/// Bind stdout/stderr to the console the user typed the command into. On a
/// windows-subsystem binary the std handles start unset: without this,
/// `OpenNtripClient.exe --selftest ...` from PowerShell/cmd prints nothing
/// at all and the documented output contract is unusable on the shipped
/// artifact. No-op when a console already exists (cargo run, tests) or when
/// there is no parent console (GUI double-click).
#[cfg(windows)]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(pid: u32) -> i32;
        fn GetStdHandle(which: u32) -> *mut core::ffi::c_void;
        fn SetStdHandle(which: u32, handle: *mut core::ffi::c_void) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const core::ffi::c_void,
            disposition: u32,
            flags: u32,
            template: *const core::ffi::c_void,
        ) -> *mut core::ffi::c_void;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX; // (DWORD)-1
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const OPEN_EXISTING: u32 = 3;
    const INVALID_HANDLE: isize = -1;

    // SAFETY: plain win32 calls; the CONOUT$ string is NUL-terminated
    // UTF-16 and outlives the call; handles set here live for the process.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return; // console already present, or no parent console at all
        }
        // AttachConsole does not reliably refresh std handles that were
        // never set. Bind any missing one to the attached console NOW,
        // before Rust's stdout/stderr are first used (they cache handles).
        let conout: Vec<u16> = "CONOUT$".encode_utf16().chain(std::iter::once(0)).collect();
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let h = GetStdHandle(which);
            if h.is_null() || h as isize == INVALID_HANDLE {
                let f = CreateFileW(
                    conout.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null(),
                );
                if !f.is_null() && f as isize != INVALID_HANDLE {
                    SetStdHandle(which, f);
                }
            }
        }
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

/// Returns the process exit code (0 clean, 1 failure, 2 usage).
pub fn run(args: &[String]) -> u8 {
    attach_parent_console();
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("{USAGE}");
            return 2;
        }
    };

    let base = paths::exe_dir();
    let (tx, rx) = channel::<AppEvent>();
    // Real logger thread, file sinks disabled: no writes, same code path.
    let logger = Logger::start(
        paths::logs_dir(&base),
        paths::nmea_dir(&base),
        false,
        false,
        tx.clone(),
    );
    let hub = Hub::new(tx, logger.sender(), Repaint::headless());
    let corr = Arc::new(CorrQueue::new(256)); // inactive: no serial worker
    let last_gga = Arc::new(RwLock::new(None));

    let job = NtripJob {
        host: args.host.clone(),
        port: args.port,
        mountpoint: args.mount.clone(),
        username: args.user.clone(),
        password: args.pass.clone(),
        version: if args.v2 {
            NtripVersion::V2
        } else {
            NtripVersion::V1
        },
        transport: if args.tcp {
            Transport::RawTcp
        } else {
            Transport::Ntrip
        },
        tls: args.tls || args.insecure_tls,
        allow_invalid_certs: args.insecure_tls,
        // --gga forces Always+Manual: the point of the flag is to certify
        // casters that hold the stream until a position arrives (CHC APIS),
        // so the send must not depend on any sourcetable lookup.
        gga_mode: if args.gga.is_some() {
            GgaMode::Always
        } else {
            GgaMode::Off
        },
        gga_source: GgaSource::Manual,
        manual_lat: args.gga.map_or(0.0, |(lat, _)| lat),
        manual_lon: args.gga.map_or(0.0, |(_, lon)| lon),
        stream_requires_gga: None,
        // One shot: a selftest that retried for minutes would be useless.
        reconnect: ntrip::ReconnectPolicy::OneShot,
        max_attempts: 1,
        audio_alert: String::new(),
        capture: args
            .capture
            .as_ref()
            .map(|f| crate::logging::CaptureTarget::File(f.into())),
        user_agent: user_agent(),
    };
    let handle = ntrip::spawn(job, hub, corr, last_gga);

    let budget = Instant::now() + Duration::from_secs(args.seconds);
    // Once cancelled, the worker still needs a moment to close down.
    let hard_stop = budget + Duration::from_secs(5);
    let mut cancelled = false;
    let mut total_bytes: u64 = 0;
    let mut table_seen = false;
    let mut stopped: Option<(String, bool)> = None;

    while stopped.is_none() && Instant::now() < hard_stop {
        if !cancelled && Instant::now() >= budget {
            cancelled = true;
            handle.cancel();
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ev) => {
                if let Some(line) = console_line(&ev) {
                    println!("{line}");
                }
                match ev {
                    AppEvent::RxBytes { total } => total_bytes = total,
                    AppEvent::SourcetableReady { .. } => table_seen = true,
                    AppEvent::Ntrip(NtripStatus::Stopped { summary, failed }) => {
                        stopped = Some((summary, failed));
                    }
                    _ => {}
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    if stopped.is_none() {
        handle.cancel();
    }
    let joined = handle.join(Duration::from_secs(2));
    // Shutdown joins the logger thread, so straggler notices it emits after
    // the worker's Stopped (the capture-closed receipt with its byte count)
    // are guaranteed to be queued; drain and print them.
    logger.shutdown();
    while let Ok(ev) = rx.try_recv() {
        if let Some(line) = console_line(&ev) {
            println!("{line}");
        }
        if matches!(ev, AppEvent::SourcetableReady { .. }) {
            table_seen = true;
        }
    }

    println!("selftest: {total_bytes} correction bytes total");
    // Mirrors NtripJob::is_table_request: NTRIP transport + empty mountpoint.
    let table_request = !args.tcp && args.mount.is_empty();
    match (stopped, joined) {
        (Some((summary, failed)), _) => {
            if !failed && !table_request && total_bytes == 0 {
                // The close was clean (typically our own budget cancel), but
                // a stream request that never delivered a byte proves
                // nothing except that the caster accepts sockets. Trailing
                // ICY headers no longer count as corrections, so this also
                // catches casters that answer politely and then send nothing.
                println!(
                    "selftest: FAIL (connected and authenticated but 0 correction bytes \
                     in {} s; the mount is not delivering data) ({summary})",
                    args.seconds
                );
                return 1;
            }
            if !failed && table_request && !table_seen {
                // Same certification rule for tables: a clean close (our own
                // budget cancel) with no parsed sourcetable proves nothing.
                // Without this, a slow link or a huge table cancelled at the
                // budget exits 0 - "clean sourcetable run" per the contract -
                // when zero table content ever surfaced.
                println!(
                    "selftest: FAIL (no sourcetable delivered within {} s; \
                     the budget expired mid-download) ({summary})",
                    args.seconds
                );
                return 1;
            }
            println!(
                "selftest: {} ({summary})",
                if failed { "FAIL" } else { "OK" }
            );
            u8::from(failed)
        }
        (None, _) => {
            println!("selftest: FAIL (worker did not stop in time)");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    /// Under cargo test a console (or piped stdio) already exists, so the
    /// attach must be a harmless no-op - and repeatable, since run() is
    /// invoked many times in-process by these tests.
    #[test]
    fn attach_parent_console_is_idempotent_and_harmless() {
        attach_parent_console();
        attach_parent_console();
        println!("stdout still works after attach attempts");
    }

    #[test]
    fn usage_errors_exit_2() {
        assert_eq!(run(&s(&["--selftest"])), 2, "missing host/port");
        assert_eq!(run(&s(&["--selftest", "--host", "h"])), 2, "missing port");
        assert_eq!(
            run(&s(&["--selftest", "--host", "h", "--port", "nope"])),
            2,
            "bad port"
        );
        assert_eq!(
            run(&s(&[
                "--selftest",
                "--host",
                "h",
                "--port",
                "2101",
                "--bogus"
            ])),
            2,
            "unknown flag"
        );
    }

    #[test]
    fn console_prints_event_lines_once_and_suppresses_conn_echoes() {
        // Protocol lines reach the bus twice (event sink + conn sink, both
        // fed by the worker); the console must print exactly one of them.
        let text = "12:00:00 > GET /RTCM3 HTTP/1.0".to_string();
        assert_eq!(
            console_line(&AppEvent::EventLine(text.clone())).as_deref(),
            Some(text.as_str())
        );
        assert_eq!(
            console_line(&AppEvent::ConnLine(text)),
            None,
            "conn lines duplicate event lines under a selftest config"
        );
        // Data-rate and status events are consumed, not printed.
        assert_eq!(console_line(&AppEvent::RxBytes { total: 42 }), None);
        assert_eq!(console_line(&AppEvent::Ntrip(NtripStatus::Streaming)), None);
    }

    /// The dead-mount regression: a caster that accepts the socket, answers
    /// 200, and then never sends a correction byte must FAIL (exit 1) even
    /// when the run ends inside the time budget with a clean cancel -
    /// exit-code consumers were certifying dead mountpoints as healthy.
    /// The mock answers CHCStream-style (trailing headers + blank line):
    /// those ASCII header bytes used to count as corrections and defeated
    /// the zero-byte guard entirely.
    #[test]
    fn dead_stream_mount_fails_even_within_budget() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let _ = sock.write_all(
                b"ICY 200 OK\r\nServer: CHCStream 1.0\r\nDate: Wed, 16 Jul 2026 00:00:00 GMT\r\n\r\n",
            );
            // Hold the connection silent until the client hangs up.
            let mut buf = [0u8; 1024];
            loop {
                match sock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
        let code = run(&s(&[
            "--selftest",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--mount",
            "DEAD",
            "--seconds",
            "1",
        ]));
        assert_eq!(code, 1, "zero correction bytes must never exit 0");
        server.join().unwrap();
    }

    /// A sourcetable request whose download never completes must FAIL: the
    /// budget cancel is a clean close, and without tracking table delivery
    /// the exit-code contract ("0 = clean sourcetable ... run") certified a
    /// caster whose table never arrived. The mock answers the request and
    /// trickles STR lines but never sends ENDSOURCETABLE.
    #[test]
    fn table_request_cancelled_mid_download_fails() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let _ = sock.write_all(
                b"SOURCETABLE 200 OK\r\nContent-Type: text/plain\r\n\r\n\
STR;SLOW1;Somewhere;RTCM 3.2;;2;GPS;NET;USA;45.0;-122.0;0;0;GEN;none;B;N;2400;\r\n",
            );
            // Never finish the table; hold until the client hangs up.
            let mut buf = [0u8; 1024];
            loop {
                match sock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
        let code = run(&s(&[
            "--selftest",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--seconds",
            "1",
        ]));
        assert_eq!(code, 1, "an undelivered sourcetable must never exit 0");
        server.join().unwrap();
    }

    /// The good-path counterpart: a complete table (ENDSOURCETABLE) inside
    /// the budget still exits 0, so the mid-download guard cannot regress
    /// legitimate table runs.
    #[test]
    fn table_request_completed_within_budget_passes() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            // Consume the request head before answering: closing the socket
            // while the client is still writing aborts its send (WSAECONNABORTED).
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match sock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            let _ = sock.write_all(
                b"SOURCETABLE 200 OK\r\nContent-Type: text/plain\r\n\r\n\
STR;OK1;Somewhere;RTCM 3.2;;2;GPS;NET;USA;45.0;-122.0;0;0;GEN;none;B;N;2400;\r\n\
ENDSOURCETABLE\r\n",
            );
        });
        let code = run(&s(&[
            "--selftest",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--seconds",
            "5",
        ]));
        assert_eq!(code, 0, "a delivered sourcetable is a clean run");
        server.join().unwrap();
    }

    /// `--seconds` is capped at parse time: an absurd value used to survive
    /// into `Instant + Duration` and panic (release builds abort with
    /// 0xC0000409) before any output-contract line was printed. It must be
    /// the usual usage error instead.
    #[test]
    fn absurd_seconds_is_a_usage_error_not_a_panic() {
        assert_eq!(
            run(&s(&[
                "--selftest",
                "--host",
                "h",
                "--port",
                "1",
                "--seconds",
                "18446744073709551615",
            ])),
            2,
            "u64::MAX seconds must exit 2, not overflow an Instant"
        );
        let over = (MAX_SECONDS + 1).to_string();
        assert!(
            parse_args(&s(&[
                "--selftest",
                "--host",
                "h",
                "--port",
                "1",
                "--seconds",
                &over
            ]))
            .is_err()
        );
        let at_max = MAX_SECONDS.to_string();
        let a = parse_args(&s(&[
            "--selftest",
            "--host",
            "h",
            "--port",
            "1",
            "--seconds",
            &at_max,
        ]))
        .unwrap();
        assert_eq!(a.seconds, MAX_SECONDS, "the ceiling itself is legal");
    }

    #[test]
    fn parse_args_full_set() {
        let a = parse_args(&s(&[
            "--selftest",
            "--host",
            "caster",
            "--port",
            "2101",
            "--mount",
            "M1",
            "--user",
            "u",
            "--pass",
            "p",
            "--v2",
            "--tcp",
            "--tls",
            "--capture",
            "out.rtcm",
            "--seconds",
            "3",
        ]))
        .unwrap();
        assert_eq!(a.host, "caster");
        assert_eq!(a.port, 2101);
        assert_eq!(a.mount, "M1");
        assert_eq!(a.user, "u");
        assert_eq!(a.pass, "p");
        assert!(a.v2 && a.tcp && a.tls);
        assert!(!a.insecure_tls);
        assert_eq!(a.capture.as_deref(), Some("out.rtcm"));
        assert_eq!(a.seconds, 3);
    }

    /// --gga parses decimal-degree pairs, rejects malformed and
    /// out-of-range values, and rejects exactly 0,0 (the worker's "unset"
    /// sentinel - accepting it would silently send nothing).
    #[test]
    fn gga_flag_parses_and_rejects_unset_sentinel() {
        let a = parse_args(&s(&[
            "--selftest",
            "--host",
            "h",
            "--port",
            "2201",
            "--gga",
            "35.1069,-106.2909",
        ]))
        .unwrap();
        assert_eq!(a.gga, Some((35.1069, -106.2909)));
        assert!(parse_gga("35.1, -106.3").is_ok(), "spaces after the comma");
        assert!(parse_gga("0,0").is_err(), "0,0 means unset");
        assert!(parse_gga("91,0").is_err(), "latitude range");
        assert!(parse_gga("0,181").is_err(), "longitude range");
        assert!(parse_gga("nope").is_err());
        assert!(parse_gga("1;2").is_err());
        assert_eq!(
            run(&s(&[
                "--selftest",
                "--host",
                "h",
                "--port",
                "1",
                "--gga",
                "0,0"
            ])),
            2,
            "0,0 must be a usage error, not a silent no-op run"
        );
        // Raw TCP never sends GGA; the combination must refuse loudly
        // rather than certify a caster while sending nothing.
        assert_eq!(
            run(&s(&[
                "--selftest",
                "--host",
                "h",
                "--port",
                "1",
                "--tcp",
                "--gga",
                "35,-106",
            ])),
            2,
            "--gga with --tcp is a usage error"
        );
    }

    /// The CHC APIS shape at the exit-code level: a caster that answers ICY
    /// and holds the stream until a GGA arrives must FAIL without --gga
    /// (zero correction bytes) and PASS with it. This is the headless
    /// verification path for the field failure that motivated 0.2.1.
    #[test]
    fn apis_style_latch_passes_only_with_gga_flag() {
        use std::io::{Read, Write};

        let serve_apis = || {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let handle = std::thread::spawn(move || {
                let (mut sock, _) = listener.accept().unwrap();
                sock.set_read_timeout(Some(Duration::from_millis(100)))
                    .unwrap();
                let mut received = Vec::new();
                let mut buf = [0u8; 2048];
                while !received.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut buf) {
                        Ok(0) => return,
                        Ok(n) => received.extend_from_slice(&buf[..n]),
                        Err(_) => {}
                    }
                }
                let _ = sock.write_all(b"ICY 200 OK\r\nServer: CRNet 1.0\r\n\r\n");
                let deadline = Instant::now() + Duration::from_secs(6);
                while !received.contains(&b'$') && Instant::now() < deadline {
                    match sock.read(&mut buf) {
                        Ok(0) => return,
                        Ok(n) => received.extend_from_slice(&buf[..n]),
                        Err(_) => {}
                    }
                }
                if received.contains(&b'$') {
                    let _ = sock.write_all(&[0xD3, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]);
                }
                // Hold until the client hangs up.
                loop {
                    match sock.read(&mut buf) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(_) if Instant::now() > deadline + Duration::from_secs(6) => break,
                        Err(_) => {}
                    }
                }
            });
            (port, handle)
        };

        let (port, server) = serve_apis();
        let code = run(&s(&[
            "--selftest",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--mount",
            "4862676",
            "--gga",
            "35.1069,-106.2909",
            "--seconds",
            "5",
        ]));
        assert_eq!(code, 0, "with --gga the latch must open and bytes flow");
        server.join().unwrap();

        let (port, server) = serve_apis();
        let code = run(&s(&[
            "--selftest",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--mount",
            "4862676",
            "--seconds",
            "2",
        ]));
        assert_eq!(code, 1, "without a GGA the mount delivers nothing");
        server.join().unwrap();
    }

    #[test]
    fn insecure_tls_flag_parses_and_capture_requires_value() {
        let a = parse_args(&s(&[
            "--selftest",
            "--host",
            "h",
            "--port",
            "443",
            "--insecure-tls",
        ]))
        .unwrap();
        assert!(a.insecure_tls && !a.tls, "flags parse independently");
        assert!(
            parse_args(&s(&[
                "--selftest",
                "--host",
                "h",
                "--port",
                "1",
                "--capture"
            ]))
            .is_err()
        );
    }
}
