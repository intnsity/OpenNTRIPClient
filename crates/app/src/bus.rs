//! Worker -> UI event bus.
//!
//! Workers never touch AppState: they post `AppEvent`s through a `Hub` clone
//! and the UI thread drains the channel each frame. The Hub also fans event
//! lines out to the logger thread (file sinks) and pokes the egui context so
//! a repaint actually happens - `Repaint` is optional so the identical worker
//! stack runs headless under --selftest and in integration tests.

use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use ntrip_core::sourcetable::SourceTable;

use crate::logging::LogCmd;

#[derive(Debug, Clone, PartialEq)]
pub enum NtripStatus {
    Idle,
    Connecting {
        attempt: u32,
    },
    WaitingForData,
    Streaming,
    /// Sleeping out the 10 s reconnect delay; `next_attempt` is what the
    /// upcoming connect will be numbered.
    ReconnectWait {
        next_attempt: u32,
    },
    /// Terminal: the worker thread is about to exit. `failed` separates
    /// failure-class closes (auth, timeouts, drops) from user cancels and
    /// clean sourcetable runs - the selftest exit code hangs off it.
    Stopped {
        summary: String,
        failed: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SerialStatus {
    Connected { port: String, detail: String },
    Disconnected { reason: String },
}

/// One report from the worker's RTCM deframer, batched per read chunk.
/// Counters are cumulative for the worker run (they span reconnects);
/// `frames` and `decoded` are deltas.
#[derive(Debug, Clone, Default)]
pub struct RtcmBatch {
    /// Per message type since the last report: (type, frames completed,
    /// wire size in bytes of the last complete frame incl. framing + CRC).
    pub frames: Vec<(u16, u32, u32)>,
    pub crc_failures: u64,
    pub garbage_bytes: u64,
    /// Diagnostic decodes (base position, antenna info, 1029 text, 1230
    /// biases) in arrival order; the inspector keeps the latest of each.
    pub decoded: Vec<gnss::rtcm::decode::Decoded>,
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    /// A timestamped event-log line (already sent to the file sink too).
    EventLine(String),
    /// A timestamped connection-log line (verbatim protocol TX/RX, GGA sent,
    /// reconnect decisions) for the Connection Log window's ring.
    ConnLine(String),
    Ntrip(NtripStatus),
    /// Cumulative correction bytes received this app run.
    RxBytes {
        total: u64,
    },
    /// RTCM stream stats + decodes for the inspector.
    Rtcm(RtcmBatch),
    /// A parsed sourcetable (fetched or returned by a bad-mountpoint close).
    SourcetableReady {
        host: String,
        port: u16,
        table: Arc<SourceTable>,
    },
    /// Raw bytes of an UnknownResponse close; the Connection Log window
    /// offers a hex view of the most recent one.
    UnknownResponse {
        raw: Vec<u8>,
    },
    Nmea(gnss::nmea::Sentence),
    Serial(SerialStatus),
    /// Cumulative serial-overrun count (corrections dropped, oldest first).
    Overruns(u64),
}

/// Repaint requester that degrades to a no-op without an egui context.
#[derive(Clone, Default)]
pub struct Repaint(Option<egui::Context>);

impl Repaint {
    pub fn ui(ctx: egui::Context) -> Self {
        Repaint(Some(ctx))
    }

    pub fn headless() -> Self {
        Repaint(None)
    }

    pub fn now(&self) {
        if let Some(ctx) = &self.0 {
            ctx.request_repaint();
        }
    }

    /// Throttled wakeup for data-rate events: at worst the UI lags 50 ms,
    /// and a busy stream cannot force a repaint per TCP segment.
    pub fn soon(&self) {
        if let Some(ctx) = &self.0 {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

/// "HH:MM:SS " local-time prefix - the original's event log format.
pub fn stamp(text: &str) -> String {
    let t = gnss::clock::now_local();
    format!("{:02}:{:02}:{:02} {}", t.hour, t.min, t.sec, text)
}

#[derive(Clone)]
pub struct Hub {
    tx: Sender<AppEvent>,
    log: Sender<LogCmd>,
    repaint: Repaint,
}

impl Hub {
    pub fn new(tx: Sender<AppEvent>, log: Sender<LogCmd>, repaint: Repaint) -> Self {
        Hub { tx, log, repaint }
    }

    /// One event-log line: stamped once, fanned out to the UI ring and the
    /// daily file sink. Send failures mean the receiving side is gone
    /// (shutdown races); dropping the line is the only sane behavior.
    pub fn event(&self, text: impl AsRef<str>) {
        let line = stamp(text.as_ref());
        let _ = self.log.send(LogCmd::Event(line.clone()));
        let _ = self.tx.send(AppEvent::EventLine(line));
        self.repaint.now();
    }

    /// One connection-log line (ring buffer only in M2).
    pub fn conn(&self, text: impl AsRef<str>) {
        let _ = self.tx.send(AppEvent::ConnLine(stamp(text.as_ref())));
        self.repaint.now();
    }

    /// State-change event: repaint immediately.
    pub fn status(&self, ev: AppEvent) {
        let _ = self.tx.send(ev);
        self.repaint.now();
    }

    /// Data-rate event: repaint within 50 ms.
    pub fn data(&self, ev: AppEvent) {
        let _ = self.tx.send(ev);
        self.repaint.soon();
    }

    /// Raw NMEA sentence for the daily NMEA file sink.
    pub fn nmea_record(&self, raw: &str) {
        let _ = self.log.send(LogCmd::Nmea(raw.to_string()));
    }

    /// Any other logger-thread command (capture control and data). The
    /// logger owns ALL file IO, so raw capture bytes ride this channel
    /// instead of being written on the network thread.
    pub fn log_cmd(&self, cmd: LogCmd) {
        let _ = self.log.send(cmd);
    }

    pub fn log_sender(&self) -> Sender<LogCmd> {
        self.log.clone()
    }
}
