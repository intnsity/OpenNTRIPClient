//! Serial worker: forwards correction bytes to the receiver verbatim and
//! reads its NMEA stream back. Also the source of the parity event
//! vocabulary that derives from NMEA: fix-quality transitions, satellite
//! count, HDOP, base-station id, and the correction-age warning when an RTK
//! fix degrades - computed here, next to the data, so the UI stays a dumb
//! renderer.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use gnss::nmea::{Gga, Sentence, quality_name};

use crate::bus::{AppEvent, Hub, SerialStatus};
use crate::settings::{NovatelFormat, ParityCfg, SerialCfg};
use crate::workers::{CorrQueue, join_timeout};

const READ_TIMEOUT: Duration = Duration::from_millis(50);
/// NMEA line assembly cap: a healthy sentence is < 100 bytes. Binary noise
/// (a receiver echoing RTCM, wrong baud rate) must not grow the buffer.
const LINE_CAP: usize = 1024;

/// The NovAtel OEMV auto-config command sequence, exactly as the original
/// sent it. Rate mapping: 1 Hz -> "1.0", 5 Hz -> "0.2", 10 Hz -> "0.1".
pub fn novatel_commands(format: NovatelFormat, rate_hz: u8) -> Vec<String> {
    let ontime = match rate_hz {
        1 => "1.0",
        10 => "0.1",
        _ => "0.2",
    };
    vec![
        "unlogall thisport".to_string(),
        format!("log thisport gpggalong ontime {ontime}"),
        format!("log thisport gprmc ontime {ontime}"),
        format!("interfacemode thisport {} novatel", format.as_str()),
    ]
}

/// "115200 8-N-1" style summary for status lines and open events.
pub fn port_summary(cfg: &SerialCfg) -> String {
    let parity = match cfg.parity {
        ParityCfg::None => 'N',
        ParityCfg::Even => 'E',
        ParityCfg::Odd => 'O',
    };
    format!(
        "{} {}-{}-{}",
        cfg.baud, cfg.data_bits, parity, cfg.stop_bits
    )
}

/// Tracks previous NMEA-derived values and produces the parity event lines
/// on change. Pure state machine - unit-tested without a serial port.
#[derive(Default)]
pub struct NmeaWatch {
    quality: Option<u8>,
    sats: Option<u8>,
    /// HDOP compared at display precision (one decimal) so jitter in the
    /// hundredths does not spam the log.
    hdop_tenths: Option<i32>,
    station: Option<u16>,
}

impl NmeaWatch {
    pub fn on_gga(&mut self, g: &Gga) -> Vec<String> {
        let mut lines = Vec::new();

        let prev_q = self.quality.unwrap_or(0);
        if g.quality != prev_q {
            lines.push(format!(
                "{} -> {}",
                quality_name(prev_q),
                quality_name(g.quality)
            ));
            // RTK degrade warning: correction age is the usual culprit.
            let was_rtk = matches!(prev_q, 4 | 5);
            let degraded = match g.quality {
                4 => false,
                5 => prev_q == 4,
                _ => true,
            };
            if was_rtk && degraded {
                match g.age_s {
                    Some(age) => {
                        lines.push(format!("RTK degraded; correction data age was {age:.1} s"))
                    }
                    None => lines.push("RTK degraded; correction data age unknown".to_string()),
                }
            }
        }
        self.quality = Some(g.quality);

        if self.sats != Some(g.sats) {
            if let Some(prev) = self.sats {
                lines.push(format!("Satellites: {prev} -> {}", g.sats));
            }
            self.sats = Some(g.sats);
        }

        let tenths = g.hdop.map(|h| (f64::from(h) * 10.0).round() as i32);
        if let Some(t) = tenths
            && self.hdop_tenths != Some(t)
        {
            if let Some(prev) = self.hdop_tenths {
                lines.push(format!(
                    "HDOP: {:.1} -> {:.1}",
                    f64::from(prev) / 10.0,
                    f64::from(t) / 10.0
                ));
            }
            self.hdop_tenths = Some(t);
        }

        if let Some(id) = g.station_id
            && self.station != Some(id)
        {
            match self.station {
                Some(prev) => lines.push(format!("Base station ID: {prev} -> {id}")),
                None => lines.push(format!("Base station ID: {id}")),
            }
            self.station = Some(id);
        }

        lines
    }
}

pub struct SerialHandle {
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl SerialHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(JoinHandle::is_finished)
    }

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
    cfg: SerialCfg,
    hub: Hub,
    corr: Arc<CorrQueue>,
    last_gga: Arc<RwLock<Option<String>>>,
) -> SerialHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel2 = cancel.clone();
    let join = std::thread::Builder::new()
        .name("serial".to_string())
        .spawn(move || run(&cfg, &hub, &corr, &last_gga, &cancel2))
        .expect("spawn serial thread");
    SerialHandle {
        cancel,
        join: Some(join),
    }
}

fn open_port(cfg: &SerialCfg) -> Result<Box<dyn serialport::SerialPort>, serialport::Error> {
    let data_bits = match cfg.data_bits {
        7 => serialport::DataBits::Seven,
        _ => serialport::DataBits::Eight,
    };
    let stop_bits = match cfg.stop_bits {
        2 => serialport::StopBits::Two,
        _ => serialport::StopBits::One,
    };
    let parity = match cfg.parity {
        ParityCfg::None => serialport::Parity::None,
        ParityCfg::Even => serialport::Parity::Even,
        ParityCfg::Odd => serialport::Parity::Odd,
    };
    serialport::new(cfg.port.as_str(), cfg.baud)
        .data_bits(data_bits)
        .stop_bits(stop_bits)
        .parity(parity)
        .flow_control(serialport::FlowControl::None)
        .timeout(READ_TIMEOUT)
        .open()
}

fn run(
    cfg: &SerialCfg,
    hub: &Hub,
    corr: &CorrQueue,
    last_gga: &RwLock<Option<String>>,
    cancel: &AtomicBool,
) {
    let mut port = match open_port(cfg) {
        Ok(p) => p,
        Err(e) => {
            let reason = format!("Could not open {}: {e}", cfg.port);
            hub.event(&reason);
            hub.status(AppEvent::Serial(SerialStatus::Disconnected { reason }));
            return;
        }
    };
    let summary = port_summary(cfg);
    hub.event(format!("Serial port {} opened ({summary})", cfg.port));
    hub.status(AppEvent::Serial(SerialStatus::Connected {
        port: cfg.port.clone(),
        detail: summary,
    }));
    corr.set_active(true);

    // NovAtel auto-config runs once, right after open, before any traffic.
    let mut failure: Option<String> = None;
    if cfg.novatel_autoconfig {
        for cmd in novatel_commands(cfg.novatel_format, cfg.novatel_rate_hz) {
            match port.write_all(format!("{cmd}\r\n").as_bytes()) {
                Ok(()) => hub.event(format!("Sent to receiver: {cmd}")),
                Err(e) => {
                    failure = Some(format!("Receiver command failed: {e}"));
                    break;
                }
            }
        }
    }

    let mut watch = NmeaWatch::default();
    let mut line_buf: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    while failure.is_none() && !cancel.load(Ordering::SeqCst) {
        // 1) Forward every queued correction block verbatim. Under sustained
        //    overrun (stream faster than the port drains) the queue never
        //    goes empty, so THIS loop is where the worker lives - it must
        //    re-check cancellation itself or a Disconnect starves behind
        //    baud-paced writes for as long as the overrun lasts.
        if let Err(e) = drain_corrections(&mut port, corr, cancel) {
            failure = Some(e);
            break;
        }
        // 2) Read whatever NMEA arrived; the 50 ms timeout paces the loop.
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => on_rx_bytes(&buf[..n], &mut line_buf, &mut watch, hub, last_gga),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => {
                // Port vanished mid-session (USB unplug): surface, never crash.
                failure = Some(format!("Serial error: {e}"));
            }
        }
    }

    corr.set_active(false);
    let reason = match failure {
        Some(msg) => {
            hub.event(format!("{msg}; port closed"));
            msg
        }
        None => {
            hub.event(format!("Serial port {} closed", cfg.port));
            String::new()
        }
    };
    hub.status(AppEvent::Serial(SerialStatus::Disconnected { reason }));
}

/// Drain queued correction blocks until the queue is empty, a write fails,
/// or the worker is cancelled. On cancel any undrained bytes are dropped by
/// design: the queue's whole premise is that stale corrections are worthless.
fn drain_corrections(
    port: &mut dyn Write,
    corr: &CorrQueue,
    cancel: &AtomicBool,
) -> Result<(), String> {
    while !cancel.load(Ordering::SeqCst) {
        let Some(block) = corr.try_pop() else {
            return Ok(());
        };
        forward_block(port, &block, cancel).map_err(|e| format!("Serial write failed: {e}"))?;
    }
    Ok(())
}

/// Forward one block, checking cancellation between partial writes. A slow
/// link (MSM7 stream into a 9600 baud receiver) can take seconds to drain a
/// single 8 KiB block; each underlying `write` is bounded by the port's
/// short write timeout, so checking the flag per partial write caps cancel
/// latency near one timeout instead of one whole block. Semantics otherwise
/// match `write_all` (zero-progress writes fail, errors propagate). Returns
/// false when cancelled mid-block.
fn forward_block(port: &mut dyn Write, block: &[u8], cancel: &AtomicBool) -> std::io::Result<bool> {
    let mut rest = block;
    while !rest.is_empty() {
        if cancel.load(Ordering::SeqCst) {
            return Ok(false);
        }
        match port.write(rest) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "failed to write whole buffer",
                ));
            }
            Ok(n) => rest = &rest[n..],
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

fn on_rx_bytes(
    data: &[u8],
    line_buf: &mut Vec<u8>,
    watch: &mut NmeaWatch,
    hub: &Hub,
    last_gga: &RwLock<Option<String>>,
) {
    for &b in data {
        if b == b'\n' {
            let line = String::from_utf8_lossy(line_buf).trim().to_string();
            line_buf.clear();
            if !line.is_empty() {
                on_line(&line, watch, hub, last_gga);
            }
        } else {
            if line_buf.len() >= LINE_CAP {
                line_buf.clear();
            }
            line_buf.push(b);
        }
    }
}

fn on_line(line: &str, watch: &mut NmeaWatch, hub: &Hub, last_gga: &RwLock<Option<String>>) {
    // Non-NMEA chatter (NovAtel command echos/acks) is not an error; it is
    // just not data we parse.
    if !line.starts_with('$') {
        return;
    }
    let Ok(sentence) = gnss::nmea::parse(line) else {
        // Corrupt sentences are silently dropped: at 5-10 Hz a flaky cable
        // would otherwise flood the event log.
        return;
    };
    if let Sentence::Gga(g) = &sentence {
        // Checksum-valid GGA: the passthrough source and the NMEA record.
        if let Ok(mut slot) = last_gga.write() {
            *slot = Some(g.raw.clone());
        }
        hub.nmea_record(&g.raw);
        for event in watch.on_gga(g) {
            hub.event(event);
        }
    }
    hub.data(AppEvent::Nmea(sentence));
}

/// Enumerate real serial ports as (port_name, human label). USB devices get
/// their product string; the label is display-only.
pub fn list_ports() -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(ports) = serialport::available_ports() {
        for p in ports {
            let label = match &p.port_type {
                serialport::SerialPortType::UsbPort(usb) => usb.product.clone().unwrap_or_default(),
                serialport::SerialPortType::BluetoothPort => "Bluetooth".to_string(),
                serialport::SerialPortType::PciPort | serialport::SerialPortType::Unknown => {
                    String::new()
                }
            };
            out.push((p.port_name, label));
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Writer accepting at most `chunk` bytes per call (a serial port's
    /// short write timeout produces exactly this partial-progress shape);
    /// flips `cancel` once `trip_at` total bytes have been written.
    struct SlowPort {
        written: Vec<u8>,
        chunk: usize,
        cancel: Arc<AtomicBool>,
        trip_at: Option<usize>,
    }

    impl SlowPort {
        fn new(chunk: usize) -> Self {
            SlowPort {
                written: Vec::new(),
                chunk,
                cancel: Arc::new(AtomicBool::new(false)),
                trip_at: None,
            }
        }
    }

    impl Write for SlowPort {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.chunk);
            self.written.extend_from_slice(&buf[..n]);
            if let Some(t) = self.trip_at
                && self.written.len() >= t
            {
                self.cancel.store(true, Ordering::SeqCst);
            }
            Ok(n)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn forward_block_writes_everything_when_not_cancelled() {
        let mut port = SlowPort::new(7);
        let cancel = port.cancel.clone();
        let block: Vec<u8> = (0..=99).collect();
        assert!(forward_block(&mut port, &block, &cancel).unwrap());
        assert_eq!(port.written, block, "byte-exact across partial writes");
    }

    #[test]
    fn forward_block_stops_between_partial_writes_on_cancel() {
        let mut port = SlowPort::new(4);
        port.trip_at = Some(12);
        let cancel = port.cancel.clone();
        let block = vec![0u8; 8192];
        let done = forward_block(&mut port, &block, &cancel).unwrap();
        assert!(!done, "cancel mid-block must abandon the rest");
        assert_eq!(
            port.written.len(),
            12,
            "no further slices after the cancel flag flipped"
        );
    }

    #[test]
    fn forward_block_zero_progress_is_an_error_like_write_all() {
        let mut port = SlowPort::new(0);
        let cancel = port.cancel.clone();
        let err = forward_block(&mut port, &[1, 2, 3], &cancel).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WriteZero);
    }

    /// The sustained-overrun regime: the queue refills faster than the port
    /// drains, so only the drain loop's own cancel checks can end it.
    #[test]
    fn drain_corrections_honors_cancel_mid_backlog() {
        let corr = CorrQueue::new(16);
        corr.set_active(true);
        for _ in 0..8 {
            corr.push(vec![0u8; 1024]);
        }
        let mut port = SlowPort::new(64);
        port.trip_at = Some(256); // cancel lands inside the first block
        let cancel = port.cancel.clone();
        drain_corrections(&mut port, &corr, &cancel).unwrap();
        assert_eq!(port.written.len(), 256);
        assert!(
            corr.try_pop().is_some(),
            "remaining backlog stays queued (set_active(false) clears it later)"
        );
    }

    #[test]
    fn drain_corrections_empties_queue_and_returns() {
        let corr = CorrQueue::new(16);
        corr.set_active(true);
        corr.push(vec![1, 2, 3]);
        corr.push(vec![4, 5]);
        let mut port = SlowPort::new(2);
        let cancel = port.cancel.clone();
        drain_corrections(&mut port, &corr, &cancel).unwrap();
        assert_eq!(port.written, vec![1, 2, 3, 4, 5]);
        assert!(corr.try_pop().is_none());
    }

    /// The UI reaps the serial handle via is_finished(): a worker that posts
    /// Disconnected must actually exit shortly after, or the frame-level
    /// reap could never fire and the UI would stay stuck on a dead session.
    #[test]
    fn failed_open_posts_disconnected_then_thread_finishes() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (log_tx, _log_rx) = std::sync::mpsc::channel();
        let hub = Hub::new(tx, log_tx, crate::bus::Repaint::headless());
        let cfg = SerialCfg {
            port: "COM_NO_SUCH_PORT_9999".to_string(),
            ..SerialCfg::default()
        };
        let h = spawn(
            cfg,
            hub,
            Arc::new(CorrQueue::new(4)),
            Arc::new(RwLock::new(None)),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut reason = None;
        while reason.is_none() && Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(AppEvent::Serial(SerialStatus::Disconnected { reason: r })) => {
                    reason = Some(r);
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
        let reason = reason.expect("failed open must post Disconnected");
        assert!(reason.contains("COM_NO_SUCH_PORT_9999"), "{reason}");
        while !h.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(h.is_finished(), "thread must exit after its final post");
        assert!(h.join(Duration::from_secs(1)));
    }

    fn gga(
        quality: u8,
        sats: u8,
        hdop: Option<f32>,
        age: Option<f32>,
        station: Option<u16>,
    ) -> Gga {
        Gga {
            hms: None,
            lat_deg: None,
            lon_deg: None,
            quality,
            sats,
            hdop,
            alt_m: None,
            geoid_sep_m: None,
            age_s: age,
            station_id: station,
            raw: String::new(),
        }
    }

    #[test]
    fn quality_transitions_named_like_original() {
        let mut w = NmeaWatch::default();
        // Baseline is Invalid (0): a first fix logs the transition.
        let lines = w.on_gga(&gga(5, 10, None, None, None));
        assert!(
            lines.contains(&"Invalid -> RTK Float".to_string()),
            "{lines:?}"
        );
        let lines = w.on_gga(&gga(4, 10, None, None, None));
        assert!(
            lines.contains(&"RTK Float -> RTK Fixed".to_string()),
            "{lines:?}"
        );
        // No change, no line.
        assert!(w.on_gga(&gga(4, 10, None, None, None)).is_empty());
    }

    #[test]
    fn rtk_degrade_warns_with_correction_age() {
        let mut w = NmeaWatch::default();
        w.on_gga(&gga(4, 10, None, Some(1.0), None));
        let lines = w.on_gga(&gga(1, 10, None, Some(11.5), None));
        assert!(lines.contains(&"RTK Fixed -> GPS".to_string()), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("correction data age was 11.5 s")),
            "{lines:?}"
        );
        // Fixed -> Float also warns; Float -> Fixed must not.
        let mut w = NmeaWatch::default();
        w.on_gga(&gga(4, 10, None, Some(1.0), None));
        let lines = w.on_gga(&gga(5, 10, None, Some(9.0), None));
        assert!(
            lines.iter().any(|l| l.contains("RTK degraded")),
            "{lines:?}"
        );
        let lines = w.on_gga(&gga(4, 10, None, Some(1.0), None));
        assert!(
            !lines.iter().any(|l| l.contains("degraded")),
            "upgrade must not warn: {lines:?}"
        );
    }

    #[test]
    fn satellite_and_hdop_and_station_changes() {
        let mut w = NmeaWatch::default();
        let first = w.on_gga(&gga(1, 8, Some(1.21), None, Some(0)));
        // First observation: only quality transition and station announce.
        assert!(first.contains(&"Invalid -> GPS".to_string()));
        assert!(first.contains(&"Base station ID: 0".to_string()));
        assert!(
            !first.iter().any(|l| l.starts_with("Satellites")),
            "{first:?}"
        );

        let lines = w.on_gga(&gga(1, 10, Some(1.24), None, Some(0)));
        assert!(
            lines.contains(&"Satellites: 8 -> 10".to_string()),
            "{lines:?}"
        );
        // 1.21 and 1.24 both display as 1.2: no HDOP line.
        assert!(!lines.iter().any(|l| l.starts_with("HDOP")), "{lines:?}");

        let lines = w.on_gga(&gga(1, 10, Some(0.9), None, Some(451)));
        assert!(lines.contains(&"HDOP: 1.2 -> 0.9".to_string()), "{lines:?}");
        assert!(
            lines.contains(&"Base station ID: 0 -> 451".to_string()),
            "{lines:?}"
        );
    }

    #[test]
    fn novatel_command_sequence_exact() {
        assert_eq!(
            novatel_commands(NovatelFormat::Rtcmv3, 5),
            vec![
                "unlogall thisport",
                "log thisport gpggalong ontime 0.2",
                "log thisport gprmc ontime 0.2",
                "interfacemode thisport rtcmv3 novatel",
            ]
        );
        assert_eq!(
            novatel_commands(NovatelFormat::Cmr, 1)[1],
            "log thisport gpggalong ontime 1.0"
        );
        assert_eq!(
            novatel_commands(NovatelFormat::Omnistar, 10)[2],
            "log thisport gprmc ontime 0.1"
        );
        assert_eq!(
            novatel_commands(NovatelFormat::Novatel, 10)[3],
            "interfacemode thisport novatel novatel"
        );
    }

    #[test]
    fn port_summary_format() {
        let cfg = SerialCfg::default();
        assert_eq!(port_summary(&cfg), "115200 8-N-1");
        let cfg = SerialCfg {
            baud: 9600,
            data_bits: 7,
            stop_bits: 2,
            parity: ParityCfg::Even,
            ..SerialCfg::default()
        };
        assert_eq!(port_summary(&cfg), "9600 7-E-2");
    }

    #[test]
    fn line_assembly_updates_last_gga_slot() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (log_tx, _log_rx) = std::sync::mpsc::channel();
        let hub = Hub::new(tx, log_tx, crate::bus::Repaint::headless());
        let last: RwLock<Option<String>> = RwLock::new(None);
        let mut watch = NmeaWatch::default();
        let mut line_buf = Vec::new();

        let body = "GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,";
        let ck = body.bytes().fold(0u8, |a, b| a ^ b);
        let sentence = format!("${body}*{ck:02X}");
        let wire = format!("noise\r\n{sentence}\r\n$GPXTE,bad*00\r\n");
        // Split across arbitrary boundaries to prove reassembly.
        for chunk in wire.as_bytes().chunks(7) {
            on_rx_bytes(chunk, &mut line_buf, &mut watch, &hub, &last);
        }
        assert_eq!(last.read().unwrap().as_deref(), Some(sentence.as_str()));
        // A parsed GGA came through the bus (plus transition events).
        let mut saw_gga = false;
        while let Ok(ev) = rx.try_recv() {
            if let AppEvent::Nmea(Sentence::Gga(g)) = ev {
                saw_gga = true;
                assert_eq!(g.sats, 8);
            }
        }
        assert!(saw_gga);
    }
}
