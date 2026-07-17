//! Dev diagnostic CLI: connect to a caster and drive an NtripSession exactly
//! like the future GUI worker loop will (short read timeout as the tick
//! source, outputs dispatched in order). std::net only, no dependencies.
//!
//! Examples:
//!   probe --host caster.example.com --port 2101                 (sourcetable)
//!   probe --host caster.example.com --port 2101 --mount RTCM3 \
//!         --user u --pass p --gga "$GPGGA,...*68" --out rtcm.bin --seconds 30

use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ntrip_core::{
    CloseReason, GgaPolicy, NtripSession, NtripVersion, Output, SessionConfig, Transport,
};

const USAGE: &str = "usage: probe --host H --port P [--mount M] [--user U] [--pass W] \
[--v2] [--gga SENTENCE] [--out FILE] [--seconds N]";

struct Args {
    host: String,
    port: u16,
    mount: String,
    user: String,
    pass: String,
    v2: bool,
    gga: Option<String>,
    out: Option<String>,
    seconds: u64,
}

fn next_value(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        host: String::new(),
        port: 0,
        mount: String::new(),
        user: String::new(),
        pass: String::new(),
        v2: false,
        gga: None,
        out: None,
        seconds: 15,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--host" => args.host = next_value(&mut it, "--host")?,
            "--port" => {
                let v = next_value(&mut it, "--port")?;
                args.port = v.parse().map_err(|_| format!("invalid port: {v}"))?;
            }
            "--mount" => args.mount = next_value(&mut it, "--mount")?,
            "--user" => args.user = next_value(&mut it, "--user")?,
            "--pass" => args.pass = next_value(&mut it, "--pass")?,
            "--v2" => args.v2 = true,
            "--gga" => args.gga = Some(next_value(&mut it, "--gga")?),
            "--out" => args.out = Some(next_value(&mut it, "--out")?),
            "--seconds" => {
                let v = next_value(&mut it, "--seconds")?;
                args.seconds = v.parse().map_err(|_| format!("invalid seconds: {v}"))?;
            }
            other => return Err(format!("unknown flag: {other}")),
        }
    }
    if args.host.is_empty() {
        return Err("--host is required".to_string());
    }
    if args.port == 0 {
        return Err("--port is required".to_string());
    }
    Ok(args)
}

fn connect(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?
        .collect();
    let mut last_err = format!("no addresses for {host}:{port}");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(10)) {
            Ok(sock) => {
                eprintln!("* connected to {addr}");
                return Ok(sock);
            }
            Err(e) => last_err = format!("connect {addr}: {e}"),
        }
    }
    Err(last_err)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Ok(true) = success exit: a sourcetable was delivered or the run ended
/// cleanly at the time budget. Ok(false) = the session failed (auth, bad
/// mountpoint, timeout, corrupt or dropped stream).
fn run(args: &Args) -> Result<bool, String> {
    let cfg = SessionConfig {
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
        transport: Transport::Ntrip,
        user_agent: "NTRIP OpenNtripClient/0.1.0-probe".to_string(),
        gga: if args.gga.is_some() {
            GgaPolicy::Always
        } else {
            GgaPolicy::Off
        },
    };
    let (mut session, request) = NtripSession::new(cfg, Instant::now());

    let mut sock = connect(&args.host, args.port)?;
    // The 400 ms read timeout doubles as the tick source; the session only
    // requires ticks at <= 500 ms intervals.
    sock.set_read_timeout(Some(Duration::from_millis(400)))
        .map_err(|e| format!("set_read_timeout: {e}"))?;
    if !request.is_empty() {
        sock.write_all(&request)
            .map_err(|e| format!("send request: {e}"))?;
    }

    let mut out_file = match &args.out {
        Some(path) => Some(File::create(path).map_err(|e| format!("create {path}: {e}"))?),
        None => None,
    };

    let start = Instant::now();
    let deadline = start + Duration::from_secs(args.seconds);
    let mut outputs: Vec<Output> = Vec::new();
    let mut buf = [0u8; 8192];
    let mut correction_bytes: u64 = 0;
    let mut got_sourcetable = false;
    let mut close_reason: Option<CloseReason> = None;
    let mut done = false;

    while !done {
        if Instant::now() >= deadline {
            session.cancel(&mut outputs);
            done = true;
        } else {
            match sock.read(&mut buf) {
                Ok(0) => {
                    session.on_remote_close(&mut outputs);
                    done = true;
                }
                Ok(n) => session.on_bytes(&buf[..n], Instant::now(), &mut outputs),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => {
                    eprintln!("* read error: {e}");
                    session.on_remote_close(&mut outputs);
                    done = true;
                }
            }
            session.on_tick(Instant::now(), &mut outputs);
        }

        for output in outputs.drain(..) {
            match output {
                Output::ProtocolTx(line) => eprintln!("> {line}"),
                Output::ProtocolRx(line) => eprintln!("< {line}"),
                Output::GgaDue => {
                    if let Some(gga) = &args.gga {
                        let mut sentence = gga.clone().into_bytes();
                        sentence.extend_from_slice(b"\r\n");
                        match sock.write_all(&sentence) {
                            Ok(()) => {
                                eprintln!("> {gga}");
                                session.gga_sent(Instant::now());
                            }
                            Err(e) => eprintln!("* GGA send failed: {e}"),
                        }
                    }
                }
                Output::Corrections(bytes) => {
                    correction_bytes += bytes.len() as u64;
                    if let Some(f) = &mut out_file {
                        f.write_all(&bytes)
                            .map_err(|e| format!("write --out: {e}"))?;
                    }
                }
                Output::Sourcetable(table) => {
                    got_sourcetable = true;
                    done = true;
                    deliver_table(&table, &mut out_file)?;
                }
                Output::Close(reason) => {
                    if let CloseReason::MountpointNotFound { sourcetable } = &reason {
                        eprintln!("* caster answered with its sourcetable instead:");
                        deliver_table(sourcetable, &mut out_file)?;
                    }
                    eprintln!(
                        "* closed: {reason:?} (auto-reconnect would be {})",
                        if reason.auto_reconnect() {
                            "allowed"
                        } else {
                            "suppressed"
                        }
                    );
                    close_reason = Some(reason);
                    done = true;
                }
            }
        }
    }

    eprintln!(
        "* totals: {correction_bytes} correction bytes in {:.1} s",
        start.elapsed().as_secs_f64()
    );
    // Success: a delivered sourcetable, or a run that simply hit its time
    // budget (Cancelled) or finished without any failure close.
    let success = got_sourcetable || matches!(close_reason, Some(CloseReason::Cancelled) | None);
    Ok(success)
}

/// A sourcetable (or the table embedded in MountpointNotFound) goes to the
/// --out file when given, else to stdout; a summary always goes to stderr.
fn deliver_table(table: &[u8], out_file: &mut Option<File>) -> Result<(), String> {
    match out_file {
        Some(f) => f
            .write_all(table)
            .map_err(|e| format!("write --out: {e}"))?,
        None => {
            std::io::stdout()
                .write_all(table)
                .map_err(|e| format!("write stdout: {e}"))?;
        }
    }
    let parsed = ntrip_core::sourcetable::parse(table);
    eprintln!(
        "* sourcetable: {} STR, {} CAS, {} NET, {} unparsed lines",
        parsed.strs.len(),
        parsed.casters.len(),
        parsed.networks.len(),
        parsed.unparsed.len()
    );
    Ok(())
}
