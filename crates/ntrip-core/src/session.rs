//! The sans-IO session state machine.
//!
//! Design invariants:
//!
//! - Split invariance: any packetization of the same received byte stream
//!   produces the same outputs. Every buffer-boundary decision below (the
//!   partial ICY-prelude line held across feeds, partial header lines, chunk
//!   framing carried across feeds) exists to keep that property. This structurally
//!   prevents the original client's worst bug: correction bytes that arrived
//!   in the same TCP segment as the "ICY 200 OK" header were discarded.
//!   One documented exception: the raw bytes attached to the UnknownResponse
//!   produced by the 16 KiB header cap depend on where the cap tripped, i.e.
//!   on feed sizes - a terminator-less flood has no packetization-independent
//!   snapshot point, and the close itself still fires either way.
//! - Close is emitted exactly once, ever, and the session is Done (inert)
//!   afterwards. All closes funnel through `emit_close`.
//! - GgaDue is never emitted twice without an intervening `gga_sent`.
//! - The caller owns socket and clock; it promises monotonic `now` values
//!   and ticks at most 500 ms apart, which bounds how late a deadline fires.

use std::mem;
use std::time::{Duration, Instant};

use crate::chunked::Decoder;
use crate::{CloseReason, GgaPolicy, Output, SessionConfig, Transport, request};

/// Status/header accumulation cap. A legitimate response header block is a
/// few hundred bytes; anything growing past this is not NTRIP.
const HDR_CAP: usize = 16 * 1024;
const FIRST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const TABLE_TIMEOUT: Duration = Duration::from_secs(10);
const SILENCE_TIMEOUT: Duration = Duration::from_secs(30);
const GGA_FIRST_DELAY: Duration = Duration::from_millis(300);
const GGA_INTERVAL: Duration = Duration::from_secs(10);

pub struct NtripSession {
    /// True when the request named a mountpoint. Drives the
    /// MountpointNotFound-vs-Sourcetable decision when a table completes,
    /// and makes an ICY answer to a table request nonsensical.
    mountpoint_requested: bool,
    phase: Phase,
    /// Request log lines queued at construction; `new` has no output vec, so
    /// they drain into the first on_* call. Ordering is preserved: they
    /// always precede any received-side output.
    pending: Vec<Output>,
    /// Pre-classification receive buffer (status line + header block).
    hdr: Vec<u8>,
    /// Cursor: start of the next unparsed line within `hdr`.
    hdr_pos: usize,
    first_deadline: Instant,
    last_rx: Instant,
    gga: Gga,
    /// Debug tripwire for the exactly-once close invariant.
    closed: bool,
}

enum Phase {
    /// Waiting for a complete first line to classify the response.
    StatusLine,
    /// Got an HTTP/1.x status line; reading header lines to the blank line.
    HttpHeaders(HttpInfo),
    Table(TableState),
    Streaming(StreamState),
    Done,
}

struct HttpInfo {
    /// Status line as received (minus line terminator).
    status: String,
    /// Lowercased Content-Type value; empty if the header was absent.
    content_type: String,
    content_length: Option<u64>,
    chunked: bool,
}

struct TableState {
    body: Vec<u8>,
    /// Scan cursor for the ENDSOURCETABLE line search; avoids re-walking the
    /// whole body on every feed.
    scan: usize,
    mode: TableMode,
    deadline: Instant,
}

enum TableMode {
    /// Body bytes arrive as-is. `remaining: Some(n)` when delimited by
    /// Content-Length, None for ENDSOURCETABLE/read-until-close delimiting.
    Direct {
        remaining: Option<u64>,
    },
    Chunked(Decoder),
}

struct StreamState {
    prelude: Prelude,
    chunked: Option<Decoder>,
}

/// What an ICY caster may send between its status line and the corrections.
/// Practice varies: nothing at all (payload coalesced with the status line),
/// a lone CRLF, or - CHCStream 1.0 - real HTTP-style header lines
/// ("Server: ...", "Date: ...") terminated by a blank line. Header lines
/// must NOT count as correction bytes (they let the selftest certify a dead
/// mount and pollute raw captures with ASCII), so they are consumed here and
/// logged as ProtocolRx. The first byte that cannot belong to a header line
/// ends consumption immediately, which preserves the sacred coalesced-ICY
/// property: RTCM frames start with 0xD3 (not printable ASCII), so a payload
/// glued to the status line is never eaten.
enum Prelude {
    /// Scanning possibly-header lines. `line` holds the raw bytes of the
    /// current incomplete line (headers may split across reads); `lines`
    /// counts headers already accepted, bounding the prelude.
    Headers { line: Vec<u8>, lines: u32 },
    /// Prelude fully consumed (or ruled out): every byte is payload now.
    Resolved,
}

/// Prelude bounds: a real ICY header block is a handful of short lines.
/// Anything past these caps is treated as payload, so a printable text-like
/// stream cannot stall in the header scanner forever.
const PRELUDE_LINE_CAP: usize = 256;
const PRELUDE_MAX_LINES: u32 = 32;

enum Gga {
    /// Policy or transport forbids GGA; never leaves this state.
    Disabled,
    /// Enabled but not streaming yet.
    Idle,
    Due(Instant),
    /// GgaDue emitted; waiting for the caller's gga_sent.
    AwaitSent,
}

/// Deferred effects computed while `self.phase` is mutably borrowed, applied
/// after the borrow ends. Keeps borrow scopes honest without cloning state.
enum BodyEvent {
    None,
    TableComplete,
    Corrupt(String),
    /// The streaming chunked body reached its terminal chunk: the server
    /// ended the response, semantically a remote close.
    ChunkStreamEnd,
}

impl NtripSession {
    /// Returns the session plus the initial request bytes the caller must
    /// write to its socket (empty for RawTcp, which starts streaming
    /// immediately).
    pub fn new(cfg: SessionConfig, now: Instant) -> (Self, Vec<u8>) {
        let mountpoint_requested = !cfg.mountpoint.is_empty();
        let gga_enabled = matches!(cfg.transport, Transport::Ntrip)
            && matches!(
                cfg.gga,
                GgaPolicy::Always
                    | GgaPolicy::WhenRequired {
                        stream_requires: true
                    }
            );
        let mut session = NtripSession {
            mountpoint_requested,
            phase: Phase::StatusLine,
            pending: Vec::new(),
            hdr: Vec::new(),
            hdr_pos: 0,
            first_deadline: now + FIRST_RESPONSE_TIMEOUT,
            last_rx: now,
            gga: if gga_enabled {
                Gga::Idle
            } else {
                Gga::Disabled
            },
            closed: false,
        };
        match cfg.transport {
            Transport::RawTcp => {
                session.phase = Phase::Streaming(StreamState {
                    prelude: Prelude::Resolved,
                    chunked: None,
                });
                (session, Vec::new())
            }
            Transport::Ntrip => {
                let (lines, wire) = request::build(&cfg);
                session.pending = lines.into_iter().map(Output::ProtocolTx).collect();
                (session, wire)
            }
        }
    }

    pub fn on_bytes(&mut self, data: &[u8], now: Instant, out: &mut Vec<Output>) {
        self.flush_pending(out);
        if matches!(self.phase, Phase::Done) {
            return;
        }
        self.last_rx = now;
        match self.phase {
            Phase::StatusLine | Phase::HttpHeaders(_) => {
                self.hdr.extend_from_slice(data);
                // A single feed can cross several phases (headers -> body),
                // so leftover bytes continue into the new phase immediately.
                // This is the coalesced-segment fix; see module invariants.
                if let Some(leftover) = self.drive_header(now, out) {
                    self.ingest_body(&leftover, out);
                } else if matches!(self.phase, Phase::StatusLine | Phase::HttpHeaders(_))
                    && self.hdr.len() > HDR_CAP
                {
                    // Cap check runs only on bytes that stayed unclassified:
                    // a valid status line coalesced with a large payload must
                    // never trip it, so parsing gets the first look.
                    let raw = self.take_hdr_all();
                    self.emit_close(CloseReason::UnknownResponse { raw }, out);
                }
            }
            Phase::Table(_) | Phase::Streaming(_) => self.ingest_body(data, out),
            Phase::Done => {}
        }
    }

    /// Caller promises ticks at <= 500 ms intervals; all deadlines are
    /// checked here rather than in on_bytes so a silent socket still times
    /// out.
    pub fn on_tick(&mut self, now: Instant, out: &mut Vec<Output>) {
        self.flush_pending(out);
        enum Action {
            None,
            Close(CloseReason),
            GgaDue,
        }
        let action = match &self.phase {
            Phase::Done => Action::None,
            Phase::StatusLine | Phase::HttpHeaders(_) => {
                if now >= self.first_deadline {
                    Action::Close(CloseReason::FirstResponseTimeout)
                } else {
                    Action::None
                }
            }
            Phase::Table(t) => {
                if now >= t.deadline {
                    Action::Close(CloseReason::SourcetableTimeout)
                } else {
                    Action::None
                }
            }
            Phase::Streaming(_) => {
                if now.duration_since(self.last_rx) >= SILENCE_TIMEOUT {
                    Action::Close(CloseReason::StreamSilence)
                } else if matches!(self.gga, Gga::Due(at) if now >= at) {
                    Action::GgaDue
                } else {
                    Action::None
                }
            }
        };
        match action {
            Action::None => {}
            Action::Close(reason) => self.emit_close(reason, out),
            Action::GgaDue => {
                self.gga = Gga::AwaitSent;
                out.push(Output::GgaDue);
            }
        }
    }

    /// The caller wrote a GGA sentence to its socket; schedule the next one.
    pub fn gga_sent(&mut self, now: Instant) {
        if !matches!(self.gga, Gga::Disabled) {
            self.gga = Gga::Due(now + GGA_INTERVAL);
        }
    }

    /// The remote end closed the connection (read returned 0 / reset).
    pub fn on_remote_close(&mut self, out: &mut Vec<Output>) {
        self.flush_pending(out);
        match self.phase {
            Phase::Done => {}
            Phase::StatusLine | Phase::HttpHeaders(_) => {
                let raw = self.take_hdr_all();
                self.emit_close(CloseReason::UnknownResponse { raw }, out);
            }
            // Liberal on purpose: even for ENDSOURCETABLE/Content-Length
            // delimited tables, a close mid-body still surfaces whatever was
            // collected - partial diagnostics beat none.
            Phase::Table(_) => self.finalize_table(Some(CloseReason::RemoteClosed), out),
            Phase::Streaming(_) => self.emit_close(CloseReason::RemoteClosed, out),
        }
    }

    pub fn cancel(&mut self, out: &mut Vec<Output>) {
        self.flush_pending(out);
        if !matches!(self.phase, Phase::Done) {
            self.emit_close(CloseReason::Cancelled, out);
        }
    }

    fn flush_pending(&mut self, out: &mut Vec<Output>) {
        out.append(&mut self.pending);
    }

    /// The single funnel for Close: emits it and makes the session inert.
    fn emit_close(&mut self, reason: CloseReason, out: &mut Vec<Output>) {
        debug_assert!(!self.closed, "Close must be emitted at most once");
        // Bytes held back by the ICY prelude scanner (a partial line that
        // never resolved into header-or-payload) surface verbatim as a
        // ProtocolRx line before the close - never as Corrections. A
        // corrections record here would make the caller treat a connection
        // that died mid-header-line as an established stream (arming drop
        // alerts, resetting reconnect budgets, letting a dead mount pass a
        // "delivered data" check) on the strength of a truncated header.
        // Nothing is silently dropped: the prelude scanner only holds
        // printable ASCII (plus TAB and a trailing CR), so the log line
        // carries the bytes faithfully.
        if let Phase::Streaming(s) = &mut self.phase
            && let Prelude::Headers { line, .. } = &mut s.prelude
            && !line.is_empty()
        {
            let held = mem::take(line);
            out.push(Output::ProtocolRx(
                String::from_utf8_lossy(&held).into_owned(),
            ));
        }
        self.closed = true;
        self.phase = Phase::Done;
        out.push(Output::Close(reason));
    }

    /// Parse status/header lines out of `hdr`. Returns Some(leftover body
    /// bytes) when the session transitioned into Table or Streaming; None
    /// when more input is needed or the session closed.
    fn drive_header(&mut self, now: Instant, out: &mut Vec<Output>) -> Option<Vec<u8>> {
        loop {
            match self.phase {
                Phase::StatusLine => {
                    let line = self.next_line()?;
                    let upper = line.trim_ascii().to_ascii_uppercase();
                    if upper.starts_with("ICY 200") {
                        out.push(Output::ProtocolRx(line));
                        if !self.mountpoint_requested {
                            // ICY promises a correction stream; a table
                            // request can never sensibly get one.
                            let raw = self.take_hdr_consumed();
                            self.emit_close(CloseReason::UnknownResponse { raw }, out);
                            return None;
                        }
                        let leftover = self.take_hdr_leftover();
                        self.enter_streaming(
                            now,
                            Prelude::Headers {
                                line: Vec::new(),
                                lines: 0,
                            },
                            None,
                        );
                        return Some(leftover);
                    } else if upper.starts_with("SOURCETABLE 200") {
                        out.push(Output::ProtocolRx(line));
                        // v1 table: everything after the status line is body,
                        // including any header-ish lines quirky casters emit;
                        // the parser keeps those visible via `unparsed`.
                        let leftover = self.take_hdr_leftover();
                        self.phase = Phase::Table(TableState {
                            body: Vec::new(),
                            scan: 0,
                            mode: TableMode::Direct { remaining: None },
                            deadline: now + TABLE_TIMEOUT,
                        });
                        return Some(leftover);
                    } else if upper.starts_with("HTTP/1.") {
                        out.push(Output::ProtocolRx(line.clone()));
                        self.phase = Phase::HttpHeaders(HttpInfo {
                            status: line,
                            content_type: String::new(),
                            content_length: None,
                            chunked: false,
                        });
                        // Fall through: more lines may already be buffered.
                    } else {
                        let raw = self.take_hdr_consumed();
                        self.emit_close(CloseReason::UnknownResponse { raw }, out);
                        return None;
                    }
                }
                Phase::HttpHeaders(_) => {
                    let line = self.next_line()?;
                    if !line.trim_ascii().is_empty() {
                        out.push(Output::ProtocolRx(line.clone()));
                        let Phase::HttpHeaders(info) = &mut self.phase else {
                            unreachable!()
                        };
                        if let Some((name, value)) = line.split_once(':') {
                            let value = value.trim_ascii();
                            match name.trim_ascii().to_ascii_lowercase().as_str() {
                                "content-type" => info.content_type = value.to_ascii_lowercase(),
                                "content-length" => info.content_length = value.parse().ok(),
                                "transfer-encoding" => {
                                    info.chunked = value.to_ascii_lowercase().contains("chunked");
                                }
                                _ => {}
                            }
                        }
                        continue;
                    }
                    // Blank line: header block complete, classify.
                    return self.classify_http(now, out);
                }
                _ => unreachable!("drive_header called outside header phases"),
            }
        }
    }

    fn classify_http(&mut self, now: Instant, out: &mut Vec<Output>) -> Option<Vec<u8>> {
        let Phase::HttpHeaders(info) = &self.phase else {
            unreachable!()
        };
        let status = info.status.trim_ascii().to_ascii_uppercase();
        // Empty-mountpoint requests always expect a table; otherwise trust
        // the declared content type. Order matters: 401 beats everything.
        let is_table = !self.mountpoint_requested || info.content_type.contains("gnss/sourcetable");
        let content_length = info.content_length;
        let te_chunked = info.chunked;

        if status.contains(" 401") {
            self.emit_close(CloseReason::Unauthorized, out);
            return None;
        }
        if !status.contains(" 200") {
            // "Full header bytes": status line through blank line, exactly
            // what was consumed so far.
            let raw = self.take_hdr_consumed();
            self.emit_close(CloseReason::UnknownResponse { raw }, out);
            return None;
        }
        let leftover = self.take_hdr_leftover();
        if is_table {
            // Content-Length wins over Transfer-Encoding when a confused
            // caster declares both.
            let mode = match content_length {
                Some(n) => TableMode::Direct { remaining: Some(n) },
                None if te_chunked => TableMode::Chunked(Decoder::default()),
                None => TableMode::Direct { remaining: None },
            };
            self.phase = Phase::Table(TableState {
                body: Vec::new(),
                scan: 0,
                mode,
                deadline: now + TABLE_TIMEOUT,
            });
        } else {
            // Real casters declare chunked even on V1 requests; accept it.
            let chunked = te_chunked.then(Decoder::default);
            self.enter_streaming(now, Prelude::Resolved, chunked);
        }
        Some(leftover)
    }

    /// Extract the next complete line (LF-terminated, CR stripped) from the
    /// header buffer. None means an incomplete line is waiting for bytes.
    fn next_line(&mut self) -> Option<String> {
        let rel = self.hdr[self.hdr_pos..].iter().position(|&b| b == b'\n')?;
        let end = self.hdr_pos + rel;
        let mut bytes = &self.hdr[self.hdr_pos..end];
        if bytes.last() == Some(&b'\r') {
            bytes = &bytes[..bytes.len() - 1];
        }
        let line = String::from_utf8_lossy(bytes).into_owned();
        self.hdr_pos = end + 1;
        Some(line)
    }

    fn take_hdr_all(&mut self) -> Vec<u8> {
        self.hdr_pos = 0;
        mem::take(&mut self.hdr)
    }

    /// The consumed header lines (status line through the last parsed line),
    /// snapshotted for UnknownResponse forensics. Unlike `take_hdr_all` this
    /// excludes unparsed bytes that happened to coalesce into the same feed,
    /// so the attached raw is identical under any packetization: had the
    /// trailing bytes arrived in a later segment, the session would already
    /// have been Done and never seen them.
    fn take_hdr_consumed(&mut self) -> Vec<u8> {
        let mut raw = mem::take(&mut self.hdr);
        raw.truncate(self.hdr_pos);
        self.hdr_pos = 0;
        raw
    }

    /// Unconsumed bytes past the parsed header lines; these are body bytes
    /// that must flow into the next phase within the same call.
    fn take_hdr_leftover(&mut self) -> Vec<u8> {
        let leftover = self.hdr[self.hdr_pos..].to_vec();
        self.hdr = Vec::new();
        self.hdr_pos = 0;
        leftover
    }

    fn enter_streaming(&mut self, now: Instant, prelude: Prelude, chunked: Option<Decoder>) {
        self.phase = Phase::Streaming(StreamState { prelude, chunked });
        self.last_rx = now;
        if !matches!(self.gga, Gga::Disabled) {
            self.gga = Gga::Due(now + GGA_FIRST_DELAY);
        }
    }

    /// Feed body bytes to the Table or Streaming phase. Must be called even
    /// with empty `data` after entering Table: a Content-Length of zero
    /// completes immediately.
    fn ingest_body(&mut self, data: &[u8], out: &mut Vec<Output>) {
        let event = match &mut self.phase {
            Phase::Table(t) => Self::feed_table(t, data),
            Phase::Streaming(s) => Self::feed_stream(s, data, out),
            _ => BodyEvent::None,
        };
        match event {
            BodyEvent::None => {}
            BodyEvent::TableComplete => self.finalize_table(None, out),
            BodyEvent::Corrupt(detail) => {
                self.emit_close(CloseReason::StreamCorrupt { detail }, out);
            }
            BodyEvent::ChunkStreamEnd => self.emit_close(CloseReason::RemoteClosed, out),
        }
    }

    fn feed_table(t: &mut TableState, data: &[u8]) -> BodyEvent {
        match &mut t.mode {
            TableMode::Direct { remaining } => {
                let take = match remaining {
                    Some(left) => (*left).min(data.len() as u64) as usize,
                    None => data.len(),
                };
                t.body.extend_from_slice(&data[..take]);
                if let Some(left) = remaining {
                    *left -= take as u64;
                }
                if let Some(end) = scan_end_line(&t.body, &mut t.scan) {
                    t.body.truncate(end);
                    return BodyEvent::TableComplete;
                }
                if matches!(remaining, Some(0)) {
                    return BodyEvent::TableComplete;
                }
                BodyEvent::None
            }
            TableMode::Chunked(dec) => {
                // A feed that dies on a framing error may still have decoded
                // valid chunks first. Those bytes were received: they join the
                // body (and can even complete it) BEFORE the error surfaces,
                // exactly as they would have had they arrived in an earlier
                // segment - split invariance forbids the error from erasing
                // them. TableComplete beats Corrupt for the same reason: a
                // split delivery would have finished the table before the
                // corrupt bytes ever arrived.
                let mut decoded = Vec::new();
                let err = dec.feed(data, &mut decoded).err();
                t.body.extend_from_slice(&decoded);
                if let Some(end) = scan_end_line(&t.body, &mut t.scan) {
                    t.body.truncate(end);
                    return BodyEvent::TableComplete;
                }
                if let Some(detail) = err {
                    return BodyEvent::Corrupt(detail);
                }
                if dec.is_done() {
                    return BodyEvent::TableComplete;
                }
                BodyEvent::None
            }
        }
    }

    fn feed_stream(s: &mut StreamState, data: &[u8], out: &mut Vec<Output>) -> BodyEvent {
        // Payload owns a copy anyway (Corrections carries Vec<u8>), so
        // prelude bytes that turn out to be payload rather than headers can
        // simply be prepended.
        let mut payload: Vec<u8> = Vec::new();
        let mut rest = data;
        if let Prelude::Headers { line, lines } = &mut s.prelude {
            match consume_prelude(line, lines, rest, out) {
                PreludeStep::NeedMore => return BodyEvent::None,
                PreludeStep::Payload { replay, from } => {
                    payload = replay;
                    rest = &rest[from..];
                    s.prelude = Prelude::Resolved;
                }
            }
        }
        payload.extend_from_slice(rest);
        if payload.is_empty() {
            return BodyEvent::None;
        }
        match &mut s.chunked {
            None => {
                out.push(Output::Corrections(payload));
                BodyEvent::None
            }
            Some(dec) => {
                // Chunks decoded before a framing error in the same feed are
                // real received corrections; emitting them before the error
                // surfaces keeps split invariance (delivered in a separate
                // segment they would have flowed through) and keeps the byte
                // count honest.
                let mut decoded = Vec::new();
                let err = dec.feed(&payload, &mut decoded).err();
                if !decoded.is_empty() {
                    out.push(Output::Corrections(decoded));
                }
                if let Some(detail) = err {
                    return BodyEvent::Corrupt(detail);
                }
                if dec.is_done() {
                    BodyEvent::ChunkStreamEnd
                } else {
                    BodyEvent::None
                }
            }
        }
    }

    /// A table body is complete (or as complete as it will ever get). For a
    /// mountpoint request the table IS the failure diagnosis; for a table
    /// request it is the product. `close_after` carries the abnormal-close
    /// reason when finalization was forced by a remote close.
    fn finalize_table(&mut self, close_after: Option<CloseReason>, out: &mut Vec<Output>) {
        let Phase::Table(t) = mem::replace(&mut self.phase, Phase::Done) else {
            unreachable!("finalize_table outside Table phase");
        };
        if self.mountpoint_requested {
            self.emit_close(
                CloseReason::MountpointNotFound {
                    sourcetable: t.body,
                },
                out,
            );
        } else {
            out.push(Output::Sourcetable(t.body));
            if let Some(reason) = close_after {
                self.emit_close(reason, out);
            }
            // else: clean completion, session is Done without a Close.
        }
    }
}

/// Outcome of one `consume_prelude` feed.
enum PreludeStep {
    /// Every fed byte was consumed into a (possibly still partial) header
    /// line; nothing to emit yet.
    NeedMore,
    /// Header consumption is over. `replay` holds previously buffered bytes
    /// that turned out to be payload (byte-exact, terminators included);
    /// payload continues at data[from..].
    Payload { replay: Vec<u8>, from: usize },
}

/// Incrementally scan the ICY prelude for header-shaped lines. A header line
/// is printable ASCII (plus TAB) in "Name: value" shape - Name being an RFC
/// 7230 token, see `header_shaped` - terminated by CRLF or LF; each one is
/// emitted verbatim as ProtocolRx, exactly like the
/// HTTP-path headers. A blank line ends the block. Any byte that cannot
/// continue a header line - non-printable (0xD3 opens every RTCM frame), CR
/// not followed by LF, an over-long line - or a completed line without the
/// "Name: value" shape proves the bytes were payload all along, and they are
/// replayed byte-exact. Called per feed with the carried partial `line`, so
/// the decision is split-invariant.
fn consume_prelude(
    line: &mut Vec<u8>,
    lines: &mut u32,
    data: &[u8],
    out: &mut Vec<Output>,
) -> PreludeStep {
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        let breaks_line = if b == b'\n' {
            false
        } else if line.last() == Some(&b'\r') {
            // CR inside a line: only valid immediately before LF.
            true
        } else if b == b'\r' {
            false
        } else {
            !(b == b'\t' || (0x20..=0x7e).contains(&b)) || line.len() >= PRELUDE_LINE_CAP
        };
        if breaks_line {
            return PreludeStep::Payload {
                replay: mem::take(line),
                from: i,
            };
        }
        i += 1;
        if b != b'\n' {
            line.push(b);
            continue;
        }
        // Line complete. The LF (and a preceding CR) belongs to the line: it
        // is consumed with a header/blank line, replayed with payload.
        let mut txt: &[u8] = line;
        if txt.last() == Some(&b'\r') {
            txt = &txt[..txt.len() - 1];
        }
        if txt.is_empty() {
            // Blank line: end of the header block, payload starts after it.
            line.clear();
            return PreludeStep::Payload {
                replay: Vec::new(),
                from: i,
            };
        }
        let is_header = *lines < PRELUDE_MAX_LINES && header_shaped(txt);
        if is_header {
            out.push(Output::ProtocolRx(
                String::from_utf8_lossy(txt).into_owned(),
            ));
            *lines += 1;
            line.clear();
        } else {
            // Printable but not header-shaped (or past the line budget):
            // the whole line, terminator included, is payload.
            let mut replay = mem::take(line);
            replay.push(b'\n');
            return PreludeStep::Payload { replay, from: i };
        }
    }
    PreludeStep::NeedMore
}

/// RFC 7230 field-line shape: one or more token characters immediately
/// followed by ':'. Deliberately stricter than "any line containing a
/// colon": an ICY caster that legitimately streams printable ASCII prose
/// ("position age: 5 s" report lines) must not have its payload eaten as
/// trailing headers, while real header names ("Server", "Date") are plain
/// tokens. A prose line whose first word is itself a bare token before the
/// colon still slips through - that residual loss is bounded by the
/// prelude line/count caps and every consumed line stays visible verbatim
/// as ProtocolRx (documented tradeoff in docs/protocol-notes.md).
fn header_shaped(txt: &[u8]) -> bool {
    match txt.iter().position(|&c| c == b':') {
        Some(colon) if colon >= 1 => txt[..colon].iter().all(|&c| is_tchar(c)),
        _ => false,
    }
}

/// HTTP token character (RFC 7230 tchar).
fn is_tchar(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c)
}

/// Search body[*scan..] for a complete line equal to ENDSOURCETABLE
/// (CR-stripped, whitespace-trimmed, case-insensitive - liberal costs
/// nothing here). Returns the index just past that line's LF.
fn scan_end_line(body: &[u8], scan: &mut usize) -> Option<usize> {
    while let Some(rel) = body[*scan..].iter().position(|&b| b == b'\n') {
        let nl = *scan + rel;
        let mut line = &body[*scan..nl];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        if line.trim_ascii().eq_ignore_ascii_case(b"ENDSOURCETABLE") {
            return Some(nl + 1);
        }
        *scan = nl + 1;
    }
    None
}
