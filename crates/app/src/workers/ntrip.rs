//! NTRIP connection worker: one thread that owns the TCP socket, drives an
//! `ntrip_core::NtripSession` (sans-IO), and doubles as its own reconnect
//! supervisor. The GUI, the --selftest harness, and the integration tests all
//! run this exact code - there is no parallel implementation anywhere.
//!
//! Session contract highlights honored here:
//! - `NtripSession::new` queues the request log lines internally; they drain
//!   into the first on_bytes/on_tick call, so we tick immediately after
//!   connecting to get them into the connection log promptly.
//! - A cleanly completed sourcetable emits `Output::Sourcetable` and the
//!   session is Done WITHOUT any Close - Sourcetable is terminal for table
//!   requests.
//! - `on_tick` must run at least every 500 ms; the 400 ms socket read
//!   timeout is the tick source.
//! - `gga_sent` is called right after a GGA is written to the socket.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gnss::rtcm::frame::{Deframer, FrameEvent};
use ntrip_core::{
    CloseReason, GgaPolicy, NtripSession, NtripVersion, Output, SessionConfig, Transport,
};

use crate::bus::{AppEvent, Hub, NtripStatus, RtcmBatch};
use crate::logging::{CaptureTarget, LogCmd};
use crate::settings::{GgaMode, GgaSource};
use crate::workers::tls::{self, TlsFail};
use crate::workers::{CorrQueue, PushOutcome, join_timeout};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const READ_TIMEOUT: Duration = Duration::from_millis(400);
pub const RECONNECT_DELAY: Duration = Duration::from_secs(10);
/// Reconnect sleep slice: cancellation latency during the 10 s wait.
const CANCEL_SLICE: Duration = Duration::from_millis(100);

/// Everything one worker run needs, resolved by the caller (active profile +
/// app settings + cached sourcetable lookup). An empty mountpoint makes this
/// a sourcetable request.
#[derive(Debug, Clone)]
pub struct NtripJob {
    pub host: String,
    pub port: u16,
    pub mountpoint: String,
    pub username: String,
    pub password: String,
    pub version: NtripVersion,
    pub transport: Transport,
    /// Wrap the socket in TLS before any protocol bytes.
    pub tls: bool,
    /// Diagnostic override: accept ANY server certificate. The UI shows a
    /// persistent red banner while a connection runs this way.
    pub allow_invalid_certs: bool,
    pub gga_mode: GgaMode,
    pub gga_source: GgaSource,
    pub manual_lat: f64,
    pub manual_lon: f64,
    /// nmea_required from the cached sourcetable STR record; None when the
    /// table (or the mountpoint's row) is unknown.
    pub stream_requires_gga: Option<bool>,
    pub reconnect: ReconnectPolicy,
    pub max_attempts: u32,
    /// .wav played on the first drop of an outage; empty = silent.
    pub audio_alert: String,
    /// Raw-corrections capture destination; None = capture off.
    pub capture: Option<CaptureTarget>,
    pub user_agent: String,
}

impl NtripJob {
    pub fn is_table_request(&self) -> bool {
        matches!(self.transport, Transport::Ntrip) && self.mountpoint.is_empty()
    }
}

/// Whether (and WHY NOT) this job reconnects after transient closes. The
/// distinction is not cosmetic: the no-reconnect decision line names the
/// reason, and blaming the Options setting for a job that never consults it
/// (a sourcetable fetch, a --selftest run) is a false diagnosis the user
/// can waste time acting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectPolicy {
    /// Ride outages: reconnect transient closes up to max_attempts
    /// (the Options auto-reconnect setting, on).
    Auto,
    /// The Options auto-reconnect setting is off.
    OptionsOff,
    /// One-shot by design regardless of Options: sourcetable fetches and
    /// --selftest runs never reconnect.
    OneShot,
}

impl ReconnectPolicy {
    pub fn enabled(self) -> bool {
        matches!(self, ReconnectPolicy::Auto)
    }
}

/// Map profile GGA settings onto the session's policy. `when_required` with
/// an unknown requirement - no sourcetable cached, or the mount not listed
/// in it, the norm for dynamically provisioned mounts like CHC APIS base
/// serial numbers - defaults to SENDING; only an explicit nmea=0 row
/// suppresses it. The harm is asymmetric: casters ignore a GGA they did not
/// ask for, while APIS-style casters answer "ICY 200 OK" and then hold the
/// stream until a position arrives, which reads as a dead mount. The caller
/// logs the assumption so support staff can see why GGA went out.
pub fn gga_policy(mode: GgaMode, stream_requires: Option<bool>) -> GgaPolicy {
    match mode {
        GgaMode::Off => GgaPolicy::Off,
        GgaMode::Always => GgaPolicy::Always,
        GgaMode::WhenRequired => GgaPolicy::WhenRequired {
            stream_requires: stream_requires.unwrap_or(true),
        },
    }
}

/// A manual position of exactly (0, 0) is "never configured", not a real
/// point in the Gulf of Guinea: fabricating a confident quality-4 fix there
/// could bind a VRS to a nonsense reference position, which is strictly
/// worse than sending nothing and saying so. Non-finite or out-of-range
/// coordinates count as unset too - settings.toml is hand-editable and TOML
/// happily parses `nan`/`inf`, which fabricate() would render as garbage
/// digits under a valid checksum. This guard is what makes the
/// send-by-default `when_required` policy safe for manual-source profiles.
pub fn manual_position_set(lat: f64, lon: f64) -> bool {
    (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) && (lat != 0.0 || lon != 0.0)
}

/// The reconnect gate: transient reason, user opted in, attempt budget left.
pub fn should_reconnect(
    reason: &CloseReason,
    auto_reconnect: bool,
    attempts_done: u32,
    max_attempts: u32,
) -> bool {
    transient(reason) && auto_reconnect && attempts_done < max_attempts
}

/// Environment-shaped closes that a reconnect can ride out. Extends the
/// core's own gating with one case the core cannot classify better: a
/// connection that died before any COMPLETE response line (accept-then-
/// FIN/RST while a caster restarts, possibly after flushing a few status
/// bytes) surfaces as `UnknownResponse` whose raw holds no LF. No complete
/// line means classification never actually ran - that is a drop, not a
/// response, and treating it as permanent would self-terminate the ladder
/// mid-outage, defeating unattended outage-riding. A raw containing at
/// least one full line was classified and rejected: permanent.
fn transient(reason: &CloseReason) -> bool {
    match reason {
        CloseReason::UnknownResponse { raw } => !raw.contains(&b'\n'),
        _ => reason.auto_reconnect(),
    }
}

/// Mirrors ntrip-core's request-line encoding trigger (request.rs): bytes
/// the builder will percent-encode before they reach the wire. Duplicated
/// here (the core keeps its request module private) only to decide whether
/// to tell the user their mountpoint was rewritten - the encoding itself
/// happens exactly once, in the core.
pub fn mountpoint_needs_encoding(mount: &str) -> bool {
    mount.bytes().any(|b| b <= 0x20 || b >= 0x7f)
}

/// MiB milestone crossed by this chunk, if any ("first-bytes then every
/// 1 MiB milestone" event vocabulary).
pub fn mib_crossed(before: u64, after: u64) -> Option<u64> {
    let a = after >> 20;
    (a > before >> 20).then_some(a)
}

/// Event-log traffic milestone crossed by this chunk. The early proof points
/// (10 kB, 100 kB) confirm a healthy stream within seconds at real RTCM
/// rates (0.5-1 kB/s); a 1 MiB-only cadence left the log silent for 17-35
/// minutes after "Receiving data", which users read as a dead stream.
pub fn traffic_milestone(before: u64, after: u64) -> Option<String> {
    if let Some(mib) = mib_crossed(before, after) {
        return Some(format!("Received {mib} MiB"));
    }
    if before < 100_000 && after >= 100_000 {
        return Some("Received 100 kB".to_string());
    }
    if before < 10_000 && after >= 10_000 {
        return Some("Received 10 kB (stream healthy)".to_string());
    }
    None
}

/// Human duration for event lines: sub-10 s keeps one decimal (first-data
/// latency lives here), longer spans round to the natural unit.
pub fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 10.0 {
        format!("{s:.1} s")
    } else if s < 120.0 {
        format!("{s:.0} s")
    } else if s < 7200.0 {
        format!("{:.0} min", s / 60.0)
    } else {
        format!("{:.1} h", s / 3600.0)
    }
}

/// Human byte count for event lines (decimal units, one decimal place).
pub fn fmt_bytes(n: u64) -> String {
    if n < 1000 {
        format!("{n} B")
    } else if n < 1_000_000 {
        format!("{:.1} kB", n as f64 / 1000.0)
    } else {
        format!("{:.1} MB", n as f64 / 1e6)
    }
}

/// Close reasons in the plain words the event log shows.
pub fn close_summary(reason: &CloseReason, mountpoint: &str) -> String {
    match reason {
        CloseReason::Unauthorized => "Invalid username or password".to_string(),
        CloseReason::MountpointNotFound { .. } => format!(
            "Mountpoint '{mountpoint}' not found: the caster returned its sourcetable instead"
        ),
        CloseReason::UnknownResponse { raw } if raw.is_empty() => {
            "Connection closed before any response from the caster".to_string()
        }
        CloseReason::UnknownResponse { raw } => {
            format!("Unexpected response from caster ({} bytes)", raw.len())
        }
        CloseReason::FirstResponseTimeout => {
            "Connection timed out: no response from the caster".to_string()
        }
        CloseReason::SourcetableTimeout => "Sourcetable download timed out".to_string(),
        CloseReason::StreamSilence => "Data timeout: no data received for 30 seconds".to_string(),
        CloseReason::StreamCorrupt { detail } => format!("Stream corrupted: {detail}"),
        CloseReason::RemoteClosed => "Connection closed by the caster".to_string(),
        // "by user": a bare "Disconnected" in a pasted support log is
        // indistinguishable from a caster drop; only a cancel produces this.
        CloseReason::Cancelled => "Disconnected by user".to_string(),
    }
}

/// The full close line for the event and connection logs: the plain-words
/// summary plus how long the connection lived and what it delivered - the
/// facts a pasted support log needs to explain a disconnect by itself.
pub fn close_event_line(
    reason: &CloseReason,
    mountpoint: &str,
    connected_for: Duration,
    bytes_received: u64,
) -> String {
    let base = close_summary(reason, mountpoint);
    if bytes_received > 0 {
        format!(
            "{base} (after {}, {} received)",
            fmt_duration(connected_for),
            fmt_bytes(bytes_received)
        )
    } else {
        format!("{base} (after {})", fmt_duration(connected_for))
    }
}

/// One clause describing what this connection will do about GGA, appended to
/// "Receiving data" so a plan that sends nothing (mode off, an nmea=0 row,
/// or an enabled mode with no position to send) is visible the moment
/// streaming starts instead of only when a caster kicks the position-silent
/// client. `receiver_gga_ready` is whether a receiver GGA is available RIGHT
/// NOW and `manual_set` whether the profile holds a real manual position:
/// both position arms must read as intent, not fact, while nothing exists to
/// send - otherwise the plan claims GGA is flowing seconds before a "not
/// sent" miss contradicts it in the same log.
pub fn gga_plan(
    transport: Transport,
    mode: GgaMode,
    stream_requires: Option<bool>,
    source: GgaSource,
    receiver_gga_ready: bool,
    manual_set: bool,
) -> &'static str {
    if matches!(transport, Transport::RawTcp) {
        return "GGA not applicable (raw TCP)";
    }
    let enabled = match gga_policy(mode, stream_requires) {
        GgaPolicy::Off => false,
        GgaPolicy::Always => true,
        GgaPolicy::WhenRequired { stream_requires } => stream_requires,
    };
    match (enabled, source) {
        (false, _) => "GGA off",
        (true, GgaSource::Manual) if manual_set => "sending GGA every 10 s (manual position)",
        (true, GgaSource::Manual) => {
            "GGA wanted but the manual position is not set - edit the profile"
        }
        (true, GgaSource::Receiver) if receiver_gga_ready => {
            "sending GGA every 10 s (receiver passthrough)"
        }
        (true, GgaSource::Receiver) => {
            "will send receiver GGA every 10 s (none until the receiver supplies a fix)"
        }
    }
}

/// The actionable hint appended after a close that matches the classic
/// GGA-starvation kick: an NTRIP stream connection ended by the caster (or
/// starved into the silence timeout) while zero GGA sentences went out.
/// VRS/geofenced casters drop position-silent clients within seconds, which
/// otherwise reads as an inexplicable disconnect.
///
/// Two event lines, [diagnosis, remedy], each short enough (<=80 chars) to
/// fit the default-width event log: one long sentence used to clip exactly
/// at the point where the remedy started, so the user the hint was written
/// for never saw the instructions.
pub fn gga_hint(
    reason: &CloseReason,
    transport: Transport,
    mountpoint: &str,
    stream_requires: Option<bool>,
    gga_sent: u32,
    connected_for: Duration,
) -> Option<[String; 2]> {
    let kick_shaped = matches!(
        reason,
        CloseReason::RemoteClosed | CloseReason::StreamSilence
    );
    if !kick_shaped
        || gga_sent > 0
        || !matches!(transport, Transport::Ntrip)
        || mountpoint.is_empty()
    {
        return None;
    }
    // A close quick enough to look like a kick rather than a genuine outage.
    let died_fast = connected_for < Duration::from_secs(60);
    match stream_requires {
        Some(true) => Some([
            "This mount requests NMEA GGA (sourcetable nmea=1) and none was sent".to_string(),
            "Enable Send GGA in the profile and check the position source".to_string(),
        ]),
        // Requirement unknown (no cached sourcetable row): with the
        // send-by-default policy, zero GGA sent means the mode was off or no
        // position was available to send - point at both, on a fast death.
        None if died_fast => Some([
            "If this mount requires NMEA GGA, casters drop silent clients".to_string(),
            "Check Send GGA is on and a position source exists (receiver or manual)".to_string(),
        ]),
        // The cached table says no GGA is needed - but tables go stale, and
        // an operator enabling the requirement later reproduces exactly this
        // kick-shaped close. Soft pointer only on a fast death.
        Some(false) if died_fast => Some([
            "The cached sourcetable says this mount needs no GGA (nmea=0)".to_string(),
            "If that table is old it may be wrong - refetch it with Get Sourcetable".to_string(),
        ]),
        _ => None,
    }
}

/// The negative reconnect decision, spelled out. The positive path already
/// logs "Reconnecting in 10 s (attempt N of M)"; without this line the log
/// just ends after a close summary and the user cannot tell a deliberate
/// policy stop from a hang. `is_table` distinguishes the two OneShot jobs:
/// only a sourcetable fetch has a retry button to point at.
pub fn no_reconnect_line(
    was_transient: bool,
    policy: ReconnectPolicy,
    is_table: bool,
    max_attempts: u32,
) -> String {
    if !was_transient {
        return "Not reconnecting: this failure would repeat until the settings change".to_string();
    }
    match policy {
        ReconnectPolicy::OneShot if is_table => {
            "Sourcetable fetches are one-shot - use Get Sourcetable to retry".to_string()
        }
        ReconnectPolicy::OneShot => "This run is one-shot - not reconnecting".to_string(),
        ReconnectPolicy::OptionsOff => {
            "Auto-reconnect is off - staying disconnected (enable it in Options to ride outages)"
                .to_string()
        }
        ReconnectPolicy::Auto => format!(
            "Reconnect attempt limit reached ({max_attempts} attempts) - staying disconnected"
        ),
    }
}

/// The positive reconnect notice. The attempt counter is a budget that
/// resets after every healthy stream, so a session of repeated caster kicks
/// would log "attempt 1" forever; the session-lifetime drop count carries
/// the outage history the budget deliberately forgets.
pub fn reconnect_notice(drops: u32, next_attempt: u32, max_attempts: u32) -> String {
    let budget = format!("(attempt {next_attempt} of {max_attempts})");
    if drops == 0 {
        format!("Reconnecting in 10 s {budget}")
    } else {
        format!(
            "Stream dropped ({} this session) - reconnecting in 10 s {budget}",
            ordinal_times(drops)
        )
    }
}

/// "1st time", "2nd time", "3rd time", "11th time", ...
fn ordinal_times(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix} time")
}

pub struct NtripHandle {
    cancel: Arc<AtomicBool>,
    sock: Arc<Mutex<Option<TcpStream>>>,
    join: Option<JoinHandle<()>>,
}

impl NtripHandle {
    /// Cooperative cancel that actually unblocks: sets the flag, then
    /// shutdowns the socket clone so a blocked read returns immediately.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Ok(guard) = self.sock.lock()
            && let Some(s) = guard.as_ref()
        {
            let _ = s.shutdown(Shutdown::Both);
        }
    }

    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Join with a deadline; true when the thread exited in time.
    pub fn join(mut self, timeout: Duration) -> bool {
        match self.join.take() {
            Some(h) => join_timeout(h, timeout),
            None => true,
        }
    }

    pub fn cancel_and_join(self, timeout: Duration) -> bool {
        self.cancel();
        self.join(timeout)
    }
}

pub fn spawn(
    job: NtripJob,
    hub: Hub,
    corr: Arc<CorrQueue>,
    last_gga: Arc<RwLock<Option<String>>>,
) -> NtripHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let sock = Arc::new(Mutex::new(None));
    let cancel2 = cancel.clone();
    let sock2 = sock.clone();
    let join = std::thread::Builder::new()
        .name("ntrip".to_string())
        .spawn(move || {
            let mut cx = Cx {
                job,
                hub,
                corr,
                last_gga,
                cancel: cancel2,
                sock_slot: sock2,
                attempts_done: 0,
                drops: 0,
                alert_armed: false,
                total_bytes: 0,
                crc_base: 0,
                garbage_base: 0,
                last_health_posted: (0, 0),
                last_overrun_event: None,
                gga_assumption_logged: false,
                gga_off_warning_logged: false,
                mount_encoding_logged: false,
            };
            let (summary, failed) = supervise(&mut cx);
            cx.hub
                .status(AppEvent::Ntrip(NtripStatus::Stopped { summary, failed }));
        })
        .expect("spawn ntrip thread");
    NtripHandle {
        cancel,
        sock,
        join: Some(join),
    }
}

/// Worker-lifetime state threaded through the supervisor and per-connection
/// driver. Byte totals and CRC-failure counts span reconnects: they describe
/// the whole outage-riding session the user started.
struct Cx {
    job: NtripJob,
    hub: Hub,
    corr: Arc<CorrQueue>,
    last_gga: Arc<RwLock<Option<String>>>,
    cancel: Arc<AtomicBool>,
    sock_slot: Arc<Mutex<Option<TcpStream>>>,
    /// Consecutive attempts without a healthy stream; reset by first
    /// correction bytes, gates against max_attempts.
    attempts_done: u32,
    /// Session-lifetime count of healthy streams that dropped. Never reset:
    /// the reconnect notice reports it so a flapping session's history stays
    /// visible even though the attempt budget resets on every recovery.
    drops: u32,
    /// Armed by the first correction bytes of a connection; the alert plays
    /// when an ARMED session fails - i.e. an established stream dropped -
    /// and disarms until data flows again. Connect failures with no stream
    /// yet, and the retries themselves, stay silent.
    alert_armed: bool,
    total_bytes: u64,
    /// CRC failures from deframers of previous connections this run.
    crc_base: u64,
    /// Garbage bytes from deframers of previous connections this run.
    garbage_base: u64,
    /// (crc, garbage) as last posted, so quiet chunks skip the event.
    last_health_posted: (u64, u64),
    last_overrun_event: Option<Instant>,
    gga_assumption_logged: bool,
    /// One warning per run for the configuration certain to be kicked by a
    /// GGA-requiring caster: sourcetable nmea=1 but Send GGA is off.
    gga_off_warning_logged: bool,
    /// One plain-words notice per run when the mountpoint had to be
    /// percent-encoded for the request line.
    mount_encoding_logged: bool,
}

impl Cx {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// How one connection ended.
enum SessionEnd {
    /// Sourcetable delivered on a table request: clean success, worker done.
    TableDone,
    Closed(CloseReason),
    /// Transport-level failure before/outside the session (connect, request
    /// write). Environment-shaped, treated as reconnectable.
    Failed(String),
    /// Deterministic failure that would repeat forever (TLS certificate
    /// rejection): never reconnected, already logged where it happened.
    FailedPermanent(String),
}

/// The Stopped summary for every cancel path; see close_summary(Cancelled).
const CANCEL_SUMMARY: &str = "Disconnected by user";

/// Returns the terminal (summary, failed) pair for the Stopped status.
fn supervise(cx: &mut Cx) -> (String, bool) {
    loop {
        if cx.cancelled() {
            return (CANCEL_SUMMARY.to_string(), false);
        }
        cx.attempts_done += 1;
        let attempt = cx.attempts_done;
        cx.hub
            .status(AppEvent::Ntrip(NtripStatus::Connecting { attempt }));
        cx.hub.event(format!(
            "Connecting to {}:{} (attempt {attempt})",
            cx.job.host, cx.job.port
        ));

        let end = match connect(&cx.job.host, cx.job.port, &cx.cancel) {
            ConnectOutcome::Connected(sock) => drive_connection(cx, sock),
            ConnectOutcome::Cancelled => return (CANCEL_SUMMARY.to_string(), false),
            ConnectOutcome::Failed(e) => SessionEnd::Failed(format!("Connect failed: {e}")),
        };
        *cx.sock_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;

        // `was_transient` feeds the no-reconnect decision line: a permanent
        // failure explains itself differently from an exhausted retry policy.
        let (summary, reconnectable, was_transient) = match end {
            SessionEnd::TableDone => return ("Sourcetable downloaded".to_string(), false),
            SessionEnd::Closed(CloseReason::Cancelled) => {
                return (CANCEL_SUMMARY.to_string(), false);
            }
            SessionEnd::Closed(reason) => {
                let summary = close_summary(&reason, &cx.job.mountpoint);
                let ok = should_reconnect(
                    &reason,
                    cx.job.reconnect.enabled(),
                    cx.attempts_done,
                    cx.job.max_attempts,
                );
                let t = transient(&reason);
                (summary, ok, t)
            }
            SessionEnd::Failed(msg) => {
                cx.hub.event(&msg);
                let ok = cx.job.reconnect.enabled() && cx.attempts_done < cx.job.max_attempts;
                (msg, ok, true)
            }
            // Already logged at the failure site; retrying cannot help.
            SessionEnd::FailedPermanent(msg) => (msg, false, false),
        };
        if cx.cancelled() {
            return (CANCEL_SUMMARY.to_string(), false);
        }
        // "Alert on drop": an established stream failing rings the bell once
        // per outage, whether or not a reconnect follows. Failures before any
        // stream (bad network on Connect) and the retries themselves are
        // silent - nothing the user had was lost. An armed alert here is
        // also, by construction, exactly "a healthy stream just dropped":
        // the session drop history counts the same moments.
        if cx.alert_armed {
            cx.alert_armed = false;
            cx.drops += 1;
            if let Err(e) = crate::audio::play_wav(&cx.job.audio_alert) {
                cx.hub.event(format!("Audio alert failed: {e}"));
            }
        }
        if !reconnectable {
            // The decision NOT to reconnect used to be silent; the log just
            // ended after the close summary. Both logs get the reason.
            let line = no_reconnect_line(
                was_transient,
                cx.job.reconnect,
                cx.job.is_table_request(),
                cx.job.max_attempts,
            );
            cx.hub.event(&line);
            cx.hub.conn(&line);
            return (summary, true);
        }

        let next = cx.attempts_done + 1;
        let notice = reconnect_notice(cx.drops, next, cx.job.max_attempts);
        cx.hub.event(&notice);
        cx.hub.conn(&notice);
        cx.hub.status(AppEvent::Ntrip(NtripStatus::ReconnectWait {
            next_attempt: next,
        }));
        let deadline = Instant::now() + RECONNECT_DELAY;
        while Instant::now() < deadline {
            if cx.cancelled() {
                return (CANCEL_SUMMARY.to_string(), false);
            }
            std::thread::sleep(CANCEL_SLICE);
        }
    }
}

enum ConnectOutcome {
    Connected(TcpStream),
    /// The user cancelled while resolve/connect was still in flight.
    Cancelled,
    Failed(String),
}

/// Cancellable connect phase. DNS resolution and `connect_timeout` are
/// blocking calls with no cancellation hook, so they run on a disposable
/// helper thread while this caller polls the result in 100 ms cancel-checked
/// slices. On cancel the helper is abandoned: it self-terminates once its
/// own timeouts expire, and an orphaned socket is closed when its send into
/// the dropped channel fails. Without this, a Disconnect during the connect
/// phase used to stall for the full 10 s per resolved address.
fn connect(host: &str, port: u16, cancel: &Arc<AtomicBool>) -> ConnectOutcome {
    let (tx, rx) = std::sync::mpsc::channel();
    let host2 = host.to_string();
    let port2 = port;
    let cancel2 = cancel.clone();
    let spawned = std::thread::Builder::new()
        .name("ntrip-connect".to_string())
        .spawn(move || {
            let _ = tx.send(resolve_and_connect(&host2, port2, &cancel2));
        });
    if let Err(e) = spawned {
        return ConnectOutcome::Failed(format!("spawn connect helper: {e}"));
    }
    match await_cancellable(&rx, cancel) {
        None => ConnectOutcome::Cancelled,
        Some(Ok(sock)) => ConnectOutcome::Connected(sock),
        Some(Err(msg)) => ConnectOutcome::Failed(msg),
    }
}

/// Wait for a helper thread's result, checking `cancel` every slice.
/// Returns None on cancellation (the helper is abandoned) or if the helper
/// vanished without sending.
fn await_cancellable<T>(rx: &Receiver<T>, cancel: &AtomicBool) -> Option<T> {
    loop {
        if cancel.load(Ordering::SeqCst) {
            return None;
        }
        match rx.recv_timeout(CANCEL_SLICE) {
            Ok(v) => return Some(v),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// The blocking resolve + per-address connect chain, run on the helper
/// thread. Re-checks `cancel` between addresses so a multi-A-record host
/// cannot stack 10 s timeouts after the user already gave up.
fn resolve_and_connect(host: &str, port: u16, cancel: &AtomicBool) -> Result<TcpStream, String> {
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?
        .collect();
    let mut last_err = format!("no addresses for {host}:{port}");
    for addr in addrs {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".to_string());
        }
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(sock) => return Ok(sock),
            Err(e) => last_err = format!("{addr}: {e}"),
        }
    }
    Err(last_err)
}

/// The connection's byte pipe: plain TCP or a rustls-wrapped stream. Cancel
/// keeps working through TLS because the shared slot holds a clone of the
/// RAW TcpStream underneath - `shutdown()` on it fails rustls's inner reads.
enum Conn {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            Conn::Tls(t) => t.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(buf),
            Conn::Tls(t) => t.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(t) => t.flush(),
        }
    }
}

/// Optionally wrap the fresh socket in TLS, logging every handshake outcome
/// (negotiated version + cipher + verification stance, or the failure) to
/// both the event and connection logs.
fn establish(cx: &Cx, sock: TcpStream) -> Result<Conn, SessionEnd> {
    if !cx.job.tls {
        return Ok(Conn::Plain(sock));
    }
    match tls::handshake(
        sock,
        cx.job.host.trim(),
        cx.job.allow_invalid_certs,
        &cx.cancel,
        tls::HANDSHAKE_TIMEOUT,
    ) {
        Ok((stream, info)) => {
            let stance = if cx.job.allow_invalid_certs {
                "certificate NOT verified (diagnostic mode)"
            } else {
                "certificate verified"
            };
            let line = format!(
                "TLS handshake complete: {}, {}, {stance}",
                info.protocol, info.cipher
            );
            cx.hub.event(&line);
            cx.hub.conn(&line);
            Ok(Conn::Tls(stream))
        }
        Err(TlsFail::Cancelled) => Err(SessionEnd::Closed(CloseReason::Cancelled)),
        Err(TlsFail::Tls(msg)) => {
            let line = format!("TLS handshake failed: {msg}");
            cx.hub.event(&line);
            cx.hub.conn(&line);
            Err(SessionEnd::FailedPermanent(line))
        }
        Err(TlsFail::Io(msg)) => {
            let line = format!("TLS handshake failed: {msg}");
            cx.hub.conn(&line);
            // supervise() logs Failed messages to the event log itself.
            Err(SessionEnd::Failed(line))
        }
    }
}

fn drive_connection(cx: &mut Cx, sock: TcpStream) -> SessionEnd {
    // The clone in the shared slot lets cancel() shutdown a blocked read.
    if let Ok(clone) = sock.try_clone() {
        *cx.sock_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(clone);
    }
    if let Err(e) = sock.set_read_timeout(Some(READ_TIMEOUT)) {
        return SessionEnd::Failed(format!("set_read_timeout: {e}"));
    }
    let mut io = match establish(cx, sock) {
        Ok(io) => io,
        Err(end) => return end,
    };

    let stream_requires = cx.job.stream_requires_gga;
    if matches!(cx.job.gga_mode, GgaMode::WhenRequired)
        && stream_requires.is_none()
        && matches!(cx.job.transport, Transport::Ntrip)
        && !cx.job.mountpoint.is_empty()
        && !cx.gga_assumption_logged
    {
        cx.gga_assumption_logged = true;
        // "will send", not "sending": with no position source available yet
        // this is intent, and the miss notes explain any gap.
        cx.hub.event(format!(
            "No sourcetable entry for '{}'; will send GGA in case it is required",
            cx.job.mountpoint
        ));
    }
    // The one configuration certain to be kicked by a GGA-requiring caster:
    // the sourcetable says nmea=1 but the profile will never send a position.
    if matches!(cx.job.gga_mode, GgaMode::Off)
        && stream_requires == Some(true)
        && matches!(cx.job.transport, Transport::Ntrip)
        && !cx.job.mountpoint.is_empty()
        && !cx.gga_off_warning_logged
    {
        cx.gga_off_warning_logged = true;
        cx.hub.event(format!(
            "The sourcetable marks '{}' as requiring NMEA GGA, but Send GGA is off",
            cx.job.mountpoint
        ));
        cx.hub
            .event("The caster may disconnect this position-silent connection");
    }

    if mountpoint_needs_encoding(&cx.job.mountpoint) && !cx.mount_encoding_logged {
        cx.mount_encoding_logged = true;
        cx.hub.event(format!(
            "Mountpoint '{}' contains spaces or control characters; \
             sending it percent-encoded",
            cx.job.mountpoint.escape_default()
        ));
    }

    let cfg = SessionConfig {
        host: cx.job.host.clone(),
        port: cx.job.port,
        mountpoint: cx.job.mountpoint.clone(),
        username: cx.job.username.clone(),
        password: cx.job.password.clone(),
        version: cx.job.version,
        transport: cx.job.transport,
        user_agent: cx.job.user_agent.clone(),
        gga: gga_policy(cx.job.gga_mode, stream_requires),
    };
    let (mut session, request) = NtripSession::new(cfg, Instant::now());
    if !request.is_empty()
        && let Err(e) = io.write_all(&request)
    {
        return SessionEnd::Failed(format!("Send request failed: {e}"));
    }
    cx.hub.event("Connected. Waiting for data...");
    cx.hub.status(AppEvent::Ntrip(NtripStatus::WaitingForData));

    let mut outs: Vec<Output> = Vec::new();
    // Drain the queued request ProtocolTx lines into the log right away
    // instead of waiting out the first 400 ms read timeout.
    session.on_tick(Instant::now(), &mut outs);

    if let Some(target) = cx.job.capture.clone() {
        cx.hub.log_cmd(LogCmd::CaptureBegin {
            target,
            mount: cx.job.mountpoint.clone(),
        });
    }
    let mut conn = ConnState {
        deframer: Deframer::new(),
        streaming: false,
        connected_at: Instant::now(),
        bytes_at_connect: cx.total_bytes,
        gga_sent: 0,
        last_miss_note: None,
        terminal: None,
    };
    let mut buf = [0u8; 8192];
    loop {
        dispatch(cx, &mut session, &mut io, &mut outs, &mut conn);
        if let Some(end) = conn.terminal.take() {
            cx.crc_base += conn.deframer.crc_failures;
            cx.garbage_base += conn.deframer.garbage_bytes;
            if cx.job.capture.is_some() {
                // Close the per-connection capture file; the logger reports
                // the byte count as an event line.
                cx.hub.log_cmd(LogCmd::CaptureEnd);
            }
            return end;
        }
        if cx.cancelled() {
            session.cancel(&mut outs);
            continue;
        }
        match io.read(&mut buf) {
            Ok(0) => {
                if cx.cancelled() {
                    session.cancel(&mut outs);
                } else {
                    session.on_remote_close(&mut outs);
                }
            }
            Ok(n) => session.on_bytes(&buf[..n], Instant::now(), &mut outs),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) if cx.cancelled() => session.cancel(&mut outs),
            // rustls reports a close without close_notify this way; casters
            // drop connections ungracefully as a matter of routine, so it is
            // a remote close, not an error worth alarming words.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                session.on_remote_close(&mut outs);
            }
            Err(e) => {
                cx.hub.event(format!("Socket error: {e}"));
                session.on_remote_close(&mut outs);
            }
        }
        session.on_tick(Instant::now(), &mut outs);
    }
}

/// Per-connection dispatch state.
struct ConnState {
    deframer: Deframer,
    /// First correction bytes seen on this connection.
    streaming: bool,
    /// When this connection's socket came up: anchors first-data latency and
    /// the close line's "after N s" duration.
    connected_at: Instant,
    /// cx.total_bytes at connection start. Totals span reconnects, so the
    /// close line subtracts this to report THIS connection's bytes.
    bytes_at_connect: u64,
    /// GGA sentences actually written to the wire on this connection; zero
    /// at close time is what triggers the GGA-starvation hint.
    gga_sent: u32,
    /// When the last due-GGA miss was logged, None before the first. Misses
    /// retry every 2 s, far too often to log each one; see note_gga_miss.
    last_miss_note: Option<Instant>,
    terminal: Option<SessionEnd>,
}

/// Cadence for repeated miss lines while the stream is still silent.
const MISS_NOTE_INTERVAL: Duration = Duration::from_secs(10);

/// Miss texts as [diagnosis, remedy]; each line must fit the default-width
/// event log (<= 80 chars, pinned by test) - the remedy is exactly the part
/// clipping would hide.
const MISS_NO_RECEIVER: [&str; 2] = [
    "No receiver GGA available; position not sent (no GPS receiver is connected)",
    "Connect one on the Serial side, or switch the position source to manual",
];
const MISS_NO_MANUAL: [&str; 2] = [
    "No manual position set; GGA not sent (the profile has no usable coordinates)",
    "Set the manual position in the profile, or switch the source to receiver",
];
const MISS_REPEAT: &str = "Still no position to send; the caster may be waiting for a GGA";

/// Log a due-GGA miss without flooding the event log: the first miss per
/// connection gets diagnosis + remedy as two short lines, later misses
/// repeat one line every 10 s while the stream is still silent (the state
/// where the missing position is the probable cause, exactly the APIS
/// deadlock), and go quiet once corrections flow - a healthy stream does
/// not need a nag every retry slot.
fn note_gga_miss(cx: &mut Cx, conn: &mut ConnState, texts: &[&str; 2]) {
    let now = Instant::now();
    match conn.last_miss_note {
        None => {
            cx.hub.event(texts[0]);
            cx.hub.event(texts[1]);
            conn.last_miss_note = Some(now);
        }
        Some(prev) if !conn.streaming && now.duration_since(prev) >= MISS_NOTE_INTERVAL => {
            cx.hub.event(MISS_REPEAT);
            conn.last_miss_note = Some(now);
        }
        Some(_) => {}
    }
}

fn dispatch(
    cx: &mut Cx,
    session: &mut NtripSession,
    io: &mut Conn,
    outs: &mut Vec<Output>,
    conn: &mut ConnState,
) {
    for output in outs.drain(..) {
        match output {
            Output::ProtocolTx(line) => {
                // Verbatim into both logs: full protocol verbosity is the
                // product, and the event log is what non-experts read first.
                cx.hub.event(format!("> {line}"));
                cx.hub.conn(format!("> {line}"));
            }
            Output::ProtocolRx(line) => {
                cx.hub.event(format!("< {line}"));
                cx.hub.conn(format!("< {line}"));
            }
            Output::GgaDue => on_gga_due(cx, session, io, conn),
            Output::Corrections(bytes) => on_corrections(cx, conn, &bytes),
            Output::Sourcetable(raw) => {
                on_sourcetable(cx, &raw, "Downloaded sourcetable");
                if conn.terminal.is_none() {
                    conn.terminal = Some(SessionEnd::TableDone);
                }
            }
            Output::Close(reason) => {
                if let CloseReason::MountpointNotFound { sourcetable } = &reason {
                    // The table rides along on the top support diagnosis;
                    // cache and surface it so the dropdown fills anyway.
                    on_sourcetable(cx, sourcetable, "Caster answered with its sourcetable");
                }
                if let CloseReason::UnknownResponse { raw } = &reason
                    && !raw.is_empty()
                {
                    // The raw bytes back the Connection Log's hex view.
                    cx.hub
                        .status(AppEvent::UnknownResponse { raw: raw.clone() });
                }
                // Both logs get the full close story (duration + bytes):
                // without the conn mirror the Connection Log showed protocol
                // lines and then an unexplained "Reconnecting...".
                let connected_for = conn.connected_at.elapsed();
                let line = close_event_line(
                    &reason,
                    &cx.job.mountpoint,
                    connected_for,
                    cx.total_bytes.saturating_sub(conn.bytes_at_connect),
                );
                cx.hub.event(&line);
                cx.hub.conn(&line);
                if let Some(hint) = gga_hint(
                    &reason,
                    cx.job.transport,
                    &cx.job.mountpoint,
                    cx.job.stream_requires_gga,
                    conn.gga_sent,
                    connected_for,
                ) {
                    for line in &hint {
                        cx.hub.event(line);
                        cx.hub.conn(line);
                    }
                }
                if conn.terminal.is_none() {
                    conn.terminal = Some(SessionEnd::Closed(reason));
                }
            }
        }
    }
}

fn on_corrections(cx: &mut Cx, conn: &mut ConnState, bytes: &[u8]) {
    if !conn.streaming {
        conn.streaming = true;
        // A healthy stream ends the outage: attempt budget resets and the
        // drop alert arms (a later failure of THIS stream rings the bell).
        cx.attempts_done = 0;
        cx.alert_armed = true;
        // First-data latency plus the GGA plan: a user staring at "Receiving
        // data" deserves to know the client decided to send no position.
        let receiver_gga_ready = cx.last_gga.read().is_ok_and(|g| g.is_some());
        cx.hub.event(format!(
            "Receiving data (first data {} after connect; {})",
            fmt_duration(conn.connected_at.elapsed()),
            gga_plan(
                cx.job.transport,
                cx.job.gga_mode,
                cx.job.stream_requires_gga,
                cx.job.gga_source,
                receiver_gga_ready,
                manual_position_set(cx.job.manual_lat, cx.job.manual_lon),
            ),
        ));
        cx.hub.status(AppEvent::Ntrip(NtripStatus::Streaming));
    }
    let before = cx.total_bytes;
    cx.total_bytes += bytes.len() as u64;
    if let Some(milestone) = traffic_milestone(before, cx.total_bytes) {
        cx.hub.event(milestone);
    }

    match cx.corr.push(bytes.to_vec()) {
        PushOutcome::Inactive | PushOutcome::Queued => {}
        PushOutcome::DroppedOldest(n) => {
            cx.hub.data(AppEvent::Overruns(n));
            // Overruns come in bursts; one event line per second is plenty.
            let now = Instant::now();
            let due = cx
                .last_overrun_event
                .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1));
            if due {
                cx.last_overrun_event = Some(now);
                cx.hub.event(format!(
                    "Serial overrun: receiver link too slow, oldest corrections dropped ({n} total)"
                ));
            }
        }
    }

    if cx.job.capture.is_some() {
        cx.hub.log_cmd(LogCmd::CaptureData(bytes.to_vec()));
    }

    // RTCM stats + diagnostic decodes for the inspector, posted as deltas.
    let mut batch = RtcmBatch::default();
    deframe(conn, bytes, &mut batch);
    batch.crc_failures = cx.crc_base + conn.deframer.crc_failures;
    batch.garbage_bytes = cx.garbage_base + conn.deframer.garbage_bytes;
    let health = (batch.crc_failures, batch.garbage_bytes);
    if !batch.frames.is_empty() || health != cx.last_health_posted {
        cx.last_health_posted = health;
        cx.hub.data(AppEvent::Rtcm(batch));
    }
    cx.hub.data(AppEvent::RxBytes {
        total: cx.total_bytes,
    });
}

fn deframe(conn: &mut ConnState, bytes: &[u8], batch: &mut RtcmBatch) {
    conn.deframer.feed(bytes, &mut |ev| {
        if let FrameEvent::Frame { msg_type, payload } = ev {
            // Wire size: 3 header bytes + payload + 3 CRC bytes.
            let wire_len = payload.len() as u32 + 6;
            match batch.frames.iter_mut().find(|(t, ..)| *t == msg_type) {
                Some((_, n, last)) => {
                    *n += 1;
                    *last = wire_len;
                }
                None => batch.frames.push((msg_type, 1, wire_len)),
            }
            // Only the types the decoded panel displays are worth decoding
            // on the network thread; MSM headers and observables are not.
            if matches!(msg_type, 1005 | 1006 | 1008 | 1029 | 1033 | 1230)
                && let Some(d) = gnss::rtcm::decode::decode(msg_type, payload)
            {
                batch.decoded.push(d);
            }
        }
    });
}

fn on_gga_due(cx: &mut Cx, session: &mut NtripSession, io: &mut Conn, conn: &mut ConnState) {
    let sentence = match cx.job.gga_source {
        GgaSource::Manual => {
            if !manual_position_set(cx.job.manual_lat, cx.job.manual_lon) {
                note_gga_miss(cx, conn, &MISS_NO_MANUAL);
                // Retry on the short slot: the user may set a position while
                // the connection waits, and GgaDue never repeats without a
                // re-arm.
                session.gga_missed(Instant::now());
                return;
            }
            gnss::gga::fabricate(
                cx.job.manual_lat,
                cx.job.manual_lon,
                &gnss::clock::now_utc(),
            )
        }
        GgaSource::Receiver => {
            let last = cx.last_gga.read().ok().and_then(|g| g.as_ref().cloned());
            match last {
                Some(raw) => format!("{raw}\r\n"),
                None => {
                    // On a receiver-source profile with no GPS attached this
                    // is the permanent state; the miss notes explain it.
                    note_gga_miss(cx, conn, &MISS_NO_RECEIVER);
                    // Short-slot retry: the receiver's first fix should reach
                    // the caster within ~2 s, not up to 10 s later - APIS
                    // holds the stream until it arrives.
                    session.gga_missed(Instant::now());
                    return;
                }
            }
        }
    };
    match io.write_all(sentence.as_bytes()) {
        Ok(()) => {
            conn.gga_sent += 1;
            session.gga_sent(Instant::now());
            cx.hub.conn(format!("> {}", sentence.trim_end()));
        }
        Err(e) => {
            // Usually the socket is dying and the read loop will surface the
            // close - but re-arm the retry slot anyway: a connection that
            // survives a transient write failure must not fall GGA-silent
            // for its remaining lifetime (an APIS caster would then hold the
            // stream forever, the exact deadlock this machinery prevents).
            cx.hub.event(format!("Failed to send GGA: {e}"));
            session.gga_missed(Instant::now());
        }
    }
}

fn on_sourcetable(cx: &mut Cx, raw: &[u8], what: &str) {
    let table = ntrip_core::sourcetable::parse(raw);
    cx.hub.event(format!(
        "{what}: {} streams, {} casters, {} networks",
        table.strs.len(),
        table.casters.len(),
        table.networks.len()
    ));
    // Held in memory only (the UI keeps the parsed table for the browser,
    // click-to-fill, and the GGA nmea-flag lookup); never written to disk. A
    // cached table can only answer "is this caster alive now" with a stale
    // yes/no, so it is fetched fresh each session instead.
    cx.hub.status(AppEvent::SourcetableReady {
        host: cx.job.host.clone(),
        port: cx.job.port,
        table: Arc::new(table),
    });
}

/// Drain helper for tests and the selftest harness: pull events until the
/// worker posts Stopped or the deadline passes.
pub fn collect_until_stopped(
    rx: &Receiver<AppEvent>,
    deadline: Instant,
) -> (Vec<AppEvent>, Option<(String, bool)>) {
    let mut events = Vec::new();
    let mut stopped = None;
    while stopped.is_none() && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(ev) => {
                if let AppEvent::Ntrip(NtripStatus::Stopped { summary, failed }) = &ev {
                    stopped = Some((summary.clone(), *failed));
                }
                events.push(ev);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    // Drain anything already queued after the Stopped marker.
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    (events, stopped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gga_policy_mapping() {
        assert_eq!(gga_policy(GgaMode::Off, None), GgaPolicy::Off);
        assert_eq!(gga_policy(GgaMode::Off, Some(true)), GgaPolicy::Off);
        assert_eq!(gga_policy(GgaMode::Always, None), GgaPolicy::Always);
        assert_eq!(
            gga_policy(GgaMode::WhenRequired, Some(true)),
            GgaPolicy::WhenRequired {
                stream_requires: true
            }
        );
        assert_eq!(
            gga_policy(GgaMode::WhenRequired, Some(false)),
            GgaPolicy::WhenRequired {
                stream_requires: false
            }
        );
        // Unknown requirement -> assume required and send (the worker logs
        // this assumption). APIS-style casters hold the stream until a GGA
        // arrives and never list their base-SN mounts in the sourcetable;
        // only an explicit nmea=0 row may suppress sending.
        assert_eq!(
            gga_policy(GgaMode::WhenRequired, None),
            GgaPolicy::WhenRequired {
                stream_requires: true
            }
        );
    }

    /// (0, 0) exactly means "never configured"; any real coordinate - even
    /// one at a zero latitude OR longitude - is a position. Non-finite and
    /// out-of-range values (a hand-edited settings.toml parses `nan`/`inf`
    /// without complaint) must count as unset, never as something to
    /// fabricate a sentence from.
    #[test]
    fn manual_position_zero_zero_and_garbage_mean_unset() {
        assert!(!manual_position_set(0.0, 0.0));
        assert!(
            !manual_position_set(-0.0, 0.0),
            "negative zero is still unset"
        );
        assert!(manual_position_set(35.1, -106.3));
        assert!(manual_position_set(0.0, -106.3), "zero latitude is legal");
        assert!(manual_position_set(51.5, 0.0), "zero longitude is legal");
        assert!(!manual_position_set(f64::NAN, -106.3));
        assert!(!manual_position_set(35.1, f64::NAN));
        assert!(!manual_position_set(f64::INFINITY, 0.0));
        assert!(!manual_position_set(91.0, 0.0), "latitude out of range");
        assert!(!manual_position_set(0.0, -181.0), "longitude out of range");
    }

    #[test]
    fn reconnect_gating_truth_table() {
        let transient = [
            CloseReason::FirstResponseTimeout,
            CloseReason::SourcetableTimeout,
            CloseReason::StreamSilence,
            CloseReason::StreamCorrupt {
                detail: "x".to_string(),
            },
            CloseReason::RemoteClosed,
            // Deliberate revision of an earlier pin: a close with ZERO
            // received bytes is a drop (caster restarting under us), not a
            // response - it must not end an unattended outage-riding session.
            CloseReason::UnknownResponse { raw: Vec::new() },
            // Partial status line (no LF): the dying caster flushed a few
            // bytes before the RST. Classification never ran; still a drop.
            CloseReason::UnknownResponse {
                raw: b"HTTP/1.1 5".to_vec(),
            },
        ];
        let permanent = [
            CloseReason::Unauthorized,
            CloseReason::MountpointNotFound {
                sourcetable: Vec::new(),
            },
            // A response with a complete line was classified and rejected.
            CloseReason::UnknownResponse {
                raw: b"HTTP/1.1 302 Found\r\n".to_vec(),
            },
            CloseReason::UnknownResponse {
                raw: b"HTTP/1.1 200 OK\r\nPartial-".to_vec(),
            },
            CloseReason::Cancelled,
        ];
        for reason in &transient {
            assert!(should_reconnect(reason, true, 0, 10_000), "{reason:?}");
            assert!(should_reconnect(reason, true, 9_999, 10_000), "{reason:?}");
            // Budget exhausted.
            assert!(
                !should_reconnect(reason, true, 10_000, 10_000),
                "{reason:?}"
            );
            // User disabled auto-reconnect.
            assert!(!should_reconnect(reason, false, 0, 10_000), "{reason:?}");
        }
        for reason in &permanent {
            assert!(
                !should_reconnect(reason, true, 0, 10_000),
                "{reason:?} must never reconnect"
            );
        }
    }

    #[test]
    fn mount_encoding_notice_trigger_matches_core_encoding() {
        assert!(!mountpoint_needs_encoding("RTCM3"));
        assert!(!mountpoint_needs_encoding("P401-A.B_7"));
        assert!(mountpoint_needs_encoding("MY MOUNT"), "space");
        assert!(mountpoint_needs_encoding("M\r\nX"), "CRLF injection");
        assert!(mountpoint_needs_encoding("M\tX"), "control byte");
        assert!(mountpoint_needs_encoding("M\u{7f}"), "DEL");
        assert!(mountpoint_needs_encoding("Mont\u{e9}e"), "non-ASCII");
        assert!(!mountpoint_needs_encoding(""), "sourcetable request");
    }

    #[test]
    fn mib_milestones() {
        const MIB: u64 = 1 << 20;
        assert_eq!(mib_crossed(0, 100), None);
        assert_eq!(mib_crossed(MIB - 1, MIB), Some(1));
        assert_eq!(mib_crossed(MIB, MIB + 1), None);
        assert_eq!(mib_crossed(MIB + 5, 3 * MIB + 2), Some(3));
        assert_eq!(mib_crossed(0, 0), None);
    }

    /// Early proof points fill the 17-35 minute log gap between "Receiving
    /// data" and the first MiB at real correction rates.
    #[test]
    fn traffic_milestones_early_then_mib() {
        const MIB: u64 = 1 << 20;
        assert_eq!(traffic_milestone(0, 500), None);
        assert_eq!(
            traffic_milestone(9_999, 10_400).as_deref(),
            Some("Received 10 kB (stream healthy)")
        );
        // Already past 10 kB: no repeat.
        assert_eq!(traffic_milestone(10_400, 11_000), None);
        assert_eq!(
            traffic_milestone(99_000, 101_000).as_deref(),
            Some("Received 100 kB")
        );
        assert_eq!(
            traffic_milestone(MIB - 1, MIB).as_deref(),
            Some("Received 1 MiB")
        );
        // A chunk crossing both an early mark and a MiB reports the MiB.
        assert_eq!(
            traffic_milestone(0, 2 * MIB).as_deref(),
            Some("Received 2 MiB")
        );
    }

    #[test]
    fn duration_and_byte_formats_are_humane() {
        assert_eq!(fmt_duration(Duration::from_millis(600)), "0.6 s");
        assert_eq!(fmt_duration(Duration::from_secs(19)), "19 s");
        assert_eq!(fmt_duration(Duration::from_secs(300)), "5 min");
        assert_eq!(fmt_duration(Duration::from_secs(9000)), "2.5 h");
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(999), "999 B");
        assert_eq!(fmt_bytes(9_400), "9.4 kB");
        assert_eq!(fmt_bytes(2_500_000), "2.5 MB");
    }

    /// The user-facing kick line: duration and byte count make a pasted log
    /// self-explanatory ("after 19 s, 9.4 kB received" was the field case).
    #[test]
    fn close_event_line_carries_duration_and_bytes() {
        let line = close_event_line(
            &CloseReason::RemoteClosed,
            "RTCM32",
            Duration::from_secs(19),
            9_400,
        );
        assert_eq!(
            line,
            "Connection closed by the caster (after 19 s, 9.4 kB received)"
        );
        // Zero bytes: no misleading "0 B received" clause, just the duration.
        let line = close_event_line(
            &CloseReason::StreamSilence,
            "RTCM32",
            Duration::from_secs(45),
            0,
        );
        assert_eq!(
            line,
            "Data timeout: no data received for 30 seconds (after 45 s)"
        );
        let line = close_event_line(&CloseReason::Cancelled, "", Duration::from_secs(25), 18_200);
        assert_eq!(line, "Disconnected by user (after 25 s, 18.2 kB received)");
    }

    /// One clause per GGA plan, shown with "Receiving data": every plan that
    /// will send nothing (mode off, an nmea=0 row, no position configured)
    /// must say so, and neither position arm may claim GGA is flowing while
    /// nothing exists to send.
    #[test]
    fn gga_plan_covers_all_configurations() {
        use GgaMode::*;
        use GgaSource::*;
        let p = gga_plan(Transport::RawTcp, Always, Some(true), Manual, true, true);
        assert_eq!(p, "GGA not applicable (raw TCP)");
        assert_eq!(
            gga_plan(Transport::Ntrip, Off, Some(true), Manual, true, true),
            "GGA off"
        );
        // Unknown requirement now sends by default (the APIS fix): the plan
        // reflects the receiver-wait state, not a silent downgrade.
        assert_eq!(
            gga_plan(Transport::Ntrip, WhenRequired, None, Receiver, false, false),
            "will send receiver GGA every 10 s (none until the receiver supplies a fix)"
        );
        // Only an explicit nmea=0 row turns when_required off.
        assert_eq!(
            gga_plan(
                Transport::Ntrip,
                WhenRequired,
                Some(false),
                Receiver,
                false,
                false
            ),
            "GGA off"
        );
        assert_eq!(
            gga_plan(
                Transport::Ntrip,
                WhenRequired,
                Some(true),
                Manual,
                false,
                true
            ),
            "sending GGA every 10 s (manual position)"
        );
        // Manual source with the coordinates never set: intent cannot be
        // honored, and the plan must say why instead of claiming a send.
        assert_eq!(
            gga_plan(Transport::Ntrip, WhenRequired, None, Manual, false, false),
            "GGA wanted but the manual position is not set - edit the profile"
        );
        // Receiver source with a fix in hand: passthrough is a fact.
        assert_eq!(
            gga_plan(Transport::Ntrip, Always, None, Receiver, true, false),
            "sending GGA every 10 s (receiver passthrough)"
        );
        // Receiver source with no fix yet: intent, not fact - the log must
        // not assert GGA is being sent when zero will go out.
        assert_eq!(
            gga_plan(Transport::Ntrip, Always, None, Receiver, false, false),
            "will send receiver GGA every 10 s (none until the receiver supplies a fix)"
        );
    }

    /// Every miss-note line must fit the default-width event log; the
    /// remedy is exactly the part clipping would hide (the gga_hint test
    /// pins the same rule for close-time hints).
    #[test]
    fn miss_note_lines_fit_the_default_width_log() {
        for l in MISS_NO_RECEIVER
            .iter()
            .chain(MISS_NO_MANUAL.iter())
            .chain(std::iter::once(&MISS_REPEAT))
        {
            assert!(l.len() <= 80, "{} chars: {l}", l.len());
        }
    }

    /// The GGA-starvation hint fires only for kick-shaped closes of an NTRIP
    /// stream where no GGA ever went out - and the unknown-requirement and
    /// stale-table variants only when the connection died fast enough to
    /// look like a kick. Every variant is [diagnosis, remedy], both lines
    /// short enough to survive the default-width event log.
    #[test]
    fn gga_hint_truth_table() {
        let quick = Duration::from_secs(19);
        let long = Duration::from_secs(300);
        let hint = |reason: &CloseReason, requires, sent, dur| {
            gga_hint(reason, Transport::Ntrip, "RTCM32", requires, sent, dur)
        };
        // Declared requirement + nothing sent: always hinted, remedy split
        // onto its own line.
        let [diag, remedy] = hint(&CloseReason::RemoteClosed, Some(true), 0, long).unwrap();
        assert!(
            diag.contains("nmea=1") && diag.contains("none was sent"),
            "{diag}"
        );
        assert!(remedy.contains("Enable Send GGA"), "{remedy}");
        assert!(hint(&CloseReason::StreamSilence, Some(true), 0, quick).is_some());
        // Unknown requirement: hinted only on a fast death. Zero sends with
        // the send-by-default policy means mode off or no position existed;
        // the remedy points at both.
        let [diag, remedy] = hint(&CloseReason::RemoteClosed, None, 0, quick).unwrap();
        assert!(diag.contains("drop silent clients"), "{diag}");
        assert!(remedy.contains("position source"), "{remedy}");
        assert!(hint(&CloseReason::RemoteClosed, None, 0, long).is_none());
        // Table says no GGA needed: soft stale-table pointer on a fast
        // death only (a caster operator enabling the requirement after the
        // table was cached reproduces exactly this close shape).
        let [diag, remedy] = hint(&CloseReason::RemoteClosed, Some(false), 0, quick).unwrap();
        assert!(diag.contains("nmea=0"), "{diag}");
        assert!(remedy.contains("refetch"), "{remedy}");
        assert!(hint(&CloseReason::RemoteClosed, Some(false), 0, long).is_none());
        // A GGA actually went out: starvation is not the story.
        assert!(hint(&CloseReason::RemoteClosed, Some(true), 1, quick).is_none());
        // Every hint line fits the default-width log with its timestamp.
        for requires in [Some(true), Some(false), None] {
            if let Some(lines) = hint(&CloseReason::RemoteClosed, requires, 0, quick) {
                for l in &lines {
                    assert!(l.len() <= 80, "{} chars: {l}", l.len());
                }
            }
        }
        // Non-kick closes never carry the hint.
        assert!(hint(&CloseReason::Unauthorized, Some(true), 0, quick).is_none());
        assert!(hint(&CloseReason::Cancelled, Some(true), 0, quick).is_none());
        // Raw TCP and sourcetable requests have no GGA story at all.
        assert!(
            gga_hint(
                &CloseReason::RemoteClosed,
                Transport::RawTcp,
                "X",
                Some(true),
                0,
                quick
            )
            .is_none()
        );
        assert!(
            gga_hint(
                &CloseReason::RemoteClosed,
                Transport::Ntrip,
                "",
                Some(true),
                0,
                quick
            )
            .is_none()
        );
    }

    /// The negative reconnect decision always has words; permanence beats
    /// the policy explanations, and one-shot jobs (sourcetable fetches,
    /// --selftest) must never blame the Options setting they do not consult.
    #[test]
    fn no_reconnect_line_explains_each_policy() {
        use ReconnectPolicy::*;
        let l = no_reconnect_line(false, Auto, false, 10_000);
        assert!(l.contains("would repeat"), "{l}");
        let l = no_reconnect_line(true, OptionsOff, false, 10_000);
        assert!(l.contains("Auto-reconnect is off"), "{l}");
        let l = no_reconnect_line(true, Auto, false, 5);
        assert!(l.contains("limit reached (5 attempts)"), "{l}");
        // A timed-out sourcetable fetch is one-shot BY DESIGN: the line must
        // point at the retry gesture, not at an Options setting that was
        // never consulted (and may even be enabled).
        let l = no_reconnect_line(true, OneShot, true, 10_000);
        assert!(l.contains("Sourcetable fetches are one-shot"), "{l}");
        assert!(l.contains("Get Sourcetable"), "{l}");
        assert!(!l.contains("Options"), "{l}");
        // A one-shot stream run (--selftest) has no Options either.
        let l = no_reconnect_line(true, OneShot, false, 1);
        assert!(l.contains("one-shot"), "{l}");
        assert!(!l.contains("Options"), "{l}");
    }

    /// The reconnect notice keeps the attempt BUDGET semantics but carries
    /// the session's drop history: a flapping session must not read as an
    /// endless string of first-ever failures.
    #[test]
    fn reconnect_notice_carries_drop_history() {
        assert_eq!(
            reconnect_notice(0, 1, 10_000),
            "Reconnecting in 10 s (attempt 1 of 10000)"
        );
        assert_eq!(
            reconnect_notice(1, 1, 10_000),
            "Stream dropped (1st time this session) - reconnecting in 10 s (attempt 1 of 10000)"
        );
        assert_eq!(
            reconnect_notice(3, 2, 5),
            "Stream dropped (3rd time this session) - reconnecting in 10 s (attempt 2 of 5)"
        );
        // Ordinal suffixes, including the 11-13 exceptions.
        assert_eq!(ordinal_times(2), "2nd time");
        assert_eq!(ordinal_times(11), "11th time");
        assert_eq!(ordinal_times(12), "12th time");
        assert_eq!(ordinal_times(13), "13th time");
        assert_eq!(ordinal_times(21), "21st time");
        assert_eq!(ordinal_times(111), "111th time");
        assert_eq!(ordinal_times(122), "122nd time");
    }

    #[test]
    fn close_summaries_are_plain_words() {
        assert_eq!(
            close_summary(&CloseReason::Unauthorized, "M"),
            "Invalid username or password"
        );
        let s = close_summary(
            &CloseReason::MountpointNotFound {
                sourcetable: Vec::new(),
            },
            "RTCM3",
        );
        assert!(s.contains("'RTCM3' not found"), "{s}");
        assert!(s.contains("sourcetable"), "{s}");
        // A cancel must be distinguishable from a drop in a pasted log.
        assert_eq!(
            close_summary(&CloseReason::Cancelled, ""),
            "Disconnected by user"
        );
        assert!(close_summary(&CloseReason::StreamSilence, "").contains("30 seconds"));
        // Zero bytes received is worded as a drop, not a response.
        let s = close_summary(&CloseReason::UnknownResponse { raw: Vec::new() }, "");
        assert_eq!(s, "Connection closed before any response from the caster");
        let s = close_summary(
            &CloseReason::UnknownResponse {
                raw: vec![b'x'; 20],
            },
            "",
        );
        assert_eq!(s, "Unexpected response from caster (20 bytes)");
    }

    /// The connect phase's cancellation seam: a cancel must cut the wait
    /// short even while the helper thread is stuck in a blocking call.
    #[test]
    fn await_cancellable_returns_fast_on_cancel() {
        let (tx, rx) = std::sync::mpsc::channel::<u8>();
        let cancel = Arc::new(AtomicBool::new(false));
        // Helper "stuck in connect" for far longer than the test budget.
        let cancel2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(10));
            let _ = tx.send(1);
            drop(cancel2); // keep the flag alive as the real helper does
        });
        std::thread::spawn({
            let cancel = cancel.clone();
            move || {
                std::thread::sleep(Duration::from_millis(150));
                cancel.store(true, Ordering::SeqCst);
            }
        });
        let started = Instant::now();
        assert!(await_cancellable(&rx, &cancel).is_none());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "cancel during connect took {:?}; must be sliced",
            started.elapsed()
        );
    }

    #[test]
    fn await_cancellable_delivers_result_when_not_cancelled() {
        let (tx, rx) = std::sync::mpsc::channel::<u8>();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let _ = tx.send(7);
        });
        let cancel = AtomicBool::new(false);
        assert_eq!(await_cancellable(&rx, &cancel), Some(7));
    }

    #[test]
    fn resolve_and_connect_checks_cancel_between_addresses() {
        // Pre-set cancel: the helper must bail before dialing any address,
        // so a multi-A-record host cannot stack connect timeouts.
        let cancel = AtomicBool::new(true);
        let started = Instant::now();
        let err = resolve_and_connect("localhost", 9, &cancel).unwrap_err();
        assert_eq!(err, "cancelled");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
