//! Logger thread: the only place file IO happens for logs, so a stalled UNC
//! share or slow disk can never block the network, serial, or UI threads.
//!
//! Sinks: `Logs\YYYYMMDD.txt` (event lines, when enabled), `NMEA\YYYYMMDD.txt`
//! (raw GGA record, when enabled), and `Captures\YYYYMMDD_HHMMSS_{mount}.rtcm`
//! (raw correction bytes, per connection, when armed). Daily filenames use
//! the LOCAL date and roll at local midnight; the rollover itself is logged.
//! Writers are buffered and flushed once per second and on shutdown.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gnss::clock::LocalStamp;

use crate::bus::AppEvent;

/// Where a correction capture lands: a directory (file named
/// `YYYYMMDD_HHMMSS_{mount}.rtcm` at first bytes - the GUI path) or an
/// explicit file (the selftest's `--capture <file>`).
#[derive(Debug, Clone)]
pub enum CaptureTarget {
    Dir(PathBuf),
    File(PathBuf),
}

pub enum LogCmd {
    /// Timestamped event line for the daily event log.
    Event(String),
    /// Raw NMEA sentence for the daily NMEA record.
    Nmea(String),
    SetEventLog(bool),
    SetNmeaLog(bool),
    /// Arm the capture sink for one connection. The file opens lazily on the
    /// first CaptureData so an idle connection never leaves an empty file.
    CaptureBegin {
        target: CaptureTarget,
        mount: String,
    },
    /// Raw correction bytes, exactly as received.
    CaptureData(Vec<u8>),
    /// Connection over: close the capture file and report its byte count.
    CaptureEnd,
    /// Flush and exit the thread.
    Shutdown,
}

/// Daily log filename for a local date: "YYYYMMDD.txt".
pub fn daily_filename(t: &LocalStamp) -> String {
    format!("{:04}{:02}{:02}.txt", t.year, t.month, t.day)
}

/// Crash report filename: "crash-YYYYMMDD-HHMMSS.txt".
pub fn crash_filename(t: &LocalStamp) -> String {
    format!(
        "crash-{:04}{:02}{:02}-{:02}{:02}{:02}.txt",
        t.year, t.month, t.day, t.hour, t.min, t.sec
    )
}

/// Capture filename for a connection: "YYYYMMDD_HHMMSS_{mount}.rtcm", the
/// timestamp being when the first correction bytes arrived. Mountpoints are
/// caster-controlled strings, so path-hostile characters fold to '_' the
/// same way sourcetable cache names do; an empty mount (raw TCP) becomes
/// "stream".
pub fn capture_filename(t: &LocalStamp, mount: &str) -> String {
    let cleaned: String = mount
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mount = if cleaned.is_empty() {
        "stream".to_string()
    } else {
        cleaned
    };
    format!(
        "{:04}{:02}{:02}_{:02}{:02}{:02}_{mount}.rtcm",
        t.year, t.month, t.day, t.hour, t.min, t.sec
    )
}

/// Days since 1970-01-01 for a civil date (Hinnant's days_from_civil).
/// Used for the weekly update-check cadence; exact for all Gregorian dates.
pub fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// One daily-rolled buffered file sink. The file opens lazily on the first
/// line so disabled sinks never create directories or empty files.
struct DailySink {
    dir: PathBuf,
    enabled: bool,
    current: Option<(String, BufWriter<File>)>,
    dirty: bool,
    /// Suppresses repeated open-failure lines (a dead share would otherwise
    /// spam one error per event).
    failed: bool,
}

enum SinkEvent {
    None,
    RolledTo(String),
    Failed(String),
}

impl DailySink {
    fn new(dir: PathBuf, enabled: bool) -> Self {
        DailySink {
            dir,
            enabled,
            current: None,
            dirty: false,
            failed: false,
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.flush();
            self.current = None;
        }
        self.failed = false;
    }

    fn write_line(&mut self, now: &LocalStamp, line: &str) -> SinkEvent {
        if !self.enabled {
            return SinkEvent::None;
        }
        let name = daily_filename(now);
        let mut event = SinkEvent::None;
        let need_open = match &self.current {
            Some((cur, _)) => *cur != name,
            None => true,
        };
        if need_open {
            let rolled = self.current.is_some();
            self.flush();
            self.current = None;
            match std::fs::create_dir_all(&self.dir).and_then(|()| {
                OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(self.dir.join(&name))
            }) {
                Ok(f) => {
                    self.current = Some((name.clone(), BufWriter::new(f)));
                    self.failed = false;
                    if rolled {
                        event = SinkEvent::RolledTo(name);
                    }
                }
                Err(e) => {
                    if !self.failed {
                        self.failed = true;
                        return SinkEvent::Failed(format!(
                            "Could not open {}: {e}",
                            self.dir.join(&name).display()
                        ));
                    }
                    return SinkEvent::None;
                }
            }
        }
        if let Some((_, w)) = &mut self.current {
            let _ = w.write_all(line.as_bytes());
            let _ = w.write_all(b"\r\n");
            self.dirty = true;
        }
        event
    }

    fn flush(&mut self) {
        if self.dirty
            && let Some((_, w)) = &mut self.current
        {
            let _ = w.flush();
        }
        self.dirty = false;
    }
}

/// Raw-corrections capture sink. Armed per connection; the file opens on the
/// first data so the timestamp in the name matches the first received byte
/// and idle connections leave nothing behind. Methods return event-log lines
/// (start/close/error notices) for the caller to fan out.
struct CaptureSink {
    armed: Option<(CaptureTarget, String)>,
    open: Option<(PathBuf, BufWriter<File>, u64)>,
    dirty: bool,
    /// One error line per armed connection, not one per data block.
    failed: bool,
}

impl CaptureSink {
    fn new() -> Self {
        CaptureSink {
            armed: None,
            open: None,
            dirty: false,
            failed: false,
        }
    }

    fn begin(&mut self, target: CaptureTarget, mount: String) -> Option<String> {
        // A dangling open capture means the previous End never arrived
        // (worker died hard); close it out so its byte count still surfaces.
        let leftover = self.end();
        self.armed = Some((target, mount));
        self.failed = false;
        leftover
    }

    fn data(&mut self, bytes: &[u8]) -> Option<String> {
        let mut notice = None;
        if self.open.is_none() {
            let (target, mount) = self.armed.as_ref()?;
            if self.failed {
                return None;
            }
            let path = match target {
                CaptureTarget::File(p) => p.clone(),
                CaptureTarget::Dir(dir) => {
                    dir.join(capture_filename(&gnss::clock::now_local(), mount))
                }
            };
            let opened = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| File::create(&path));
            match opened {
                Ok(f) => {
                    notice = Some(format!("Capturing corrections to {}", path.display()));
                    self.open = Some((path, BufWriter::new(f), 0));
                }
                Err(e) => {
                    self.failed = true;
                    return Some(format!(
                        "Could not open capture file {}: {e}",
                        path.display()
                    ));
                }
            }
        }
        if let Some((_, w, count)) = &mut self.open {
            let _ = w.write_all(bytes);
            *count += bytes.len() as u64;
            self.dirty = true;
        }
        notice
    }

    /// Close the current file (if any) and disarm. Returns the close notice
    /// with the byte count - the connection's capture receipt.
    fn end(&mut self) -> Option<String> {
        self.armed = None;
        self.dirty = false;
        let (path, mut w, count) = self.open.take()?;
        let _ = w.flush();
        Some(format!(
            "Correction capture closed: {} ({count} bytes)",
            path.display()
        ))
    }

    fn flush(&mut self) {
        if self.dirty
            && let Some((_, w, _)) = &mut self.open
        {
            let _ = w.flush();
        }
        self.dirty = false;
    }
}

pub struct Logger {
    tx: Sender<LogCmd>,
    join: Option<JoinHandle<()>>,
}

impl Logger {
    /// Start the logger thread. `ui` receives rollover/error notices as
    /// event lines (they are also written to the file when possible).
    pub fn start(
        logs_dir: PathBuf,
        nmea_dir: PathBuf,
        event_log_on: bool,
        nmea_log_on: bool,
        ui: Sender<AppEvent>,
    ) -> Logger {
        let (tx, rx) = channel::<LogCmd>();
        let join = std::thread::Builder::new()
            .name("logger".to_string())
            .spawn(move || run(rx, logs_dir, nmea_dir, event_log_on, nmea_log_on, ui))
            .expect("spawn logger thread");
        Logger {
            tx,
            join: Some(join),
        }
    }

    pub fn sender(&self) -> Sender<LogCmd> {
        self.tx.clone()
    }

    /// Flush everything and stop the thread; bounded wait so a hung share
    /// cannot wedge app exit.
    pub fn shutdown(mut self) {
        let _ = self.tx.send(LogCmd::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = crate::workers::join_timeout(join, Duration::from_secs(2));
        }
    }
}

fn run(
    rx: Receiver<LogCmd>,
    logs_dir: PathBuf,
    nmea_dir: PathBuf,
    event_on: bool,
    nmea_on: bool,
    ui: Sender<AppEvent>,
) {
    let mut events = DailySink::new(logs_dir, event_on);
    let mut nmea = DailySink::new(nmea_dir, nmea_on);
    let mut capture = CaptureSink::new();
    let notify = |sink_event: SinkEvent, ui: &Sender<AppEvent>, events: &mut DailySink| {
        match sink_event {
            SinkEvent::None => {}
            SinkEvent::RolledTo(name) => {
                let line = crate::bus::stamp(&format!("Log rolled over to {name}"));
                // The notice lands in the fresh file too, keeping the paper
                // trail continuous across midnight.
                let _ = events.write_line(&gnss::clock::now_local(), &line);
                let _ = ui.send(AppEvent::EventLine(line));
            }
            SinkEvent::Failed(msg) => {
                let _ = ui.send(AppEvent::EventLine(crate::bus::stamp(&msg)));
            }
        }
    };
    // Capture notices become normal event lines: UI ring + daily event file.
    let emit = |text: Option<String>, ui: &Sender<AppEvent>, events: &mut DailySink| {
        if let Some(text) = text {
            let line = crate::bus::stamp(&text);
            let _ = events.write_line(&gnss::clock::now_local(), &line);
            let _ = ui.send(AppEvent::EventLine(line));
        }
    };
    // Flush on a wall-clock deadline, not on idleness: a steady sub-second
    // message stream (NMEA at 1-10 Hz is the normal case) would otherwise
    // starve the recv timeout and defer flushing until the BufWriter fills.
    const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
    let mut next_flush = Instant::now() + FLUSH_INTERVAL;
    loop {
        let wait = next_flush.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(LogCmd::Event(line)) => {
                let ev = events.write_line(&gnss::clock::now_local(), &line);
                notify(ev, &ui, &mut events);
            }
            Ok(LogCmd::Nmea(line)) => {
                let ev = nmea.write_line(&gnss::clock::now_local(), &line);
                notify(ev, &ui, &mut events);
            }
            Ok(LogCmd::SetEventLog(on)) => events.set_enabled(on),
            Ok(LogCmd::SetNmeaLog(on)) => nmea.set_enabled(on),
            Ok(LogCmd::CaptureBegin { target, mount }) => {
                emit(capture.begin(target, mount), &ui, &mut events);
            }
            Ok(LogCmd::CaptureData(bytes)) => emit(capture.data(&bytes), &ui, &mut events),
            Ok(LogCmd::CaptureEnd) => emit(capture.end(), &ui, &mut events),
            Ok(LogCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if Instant::now() >= next_flush {
            events.flush();
            nmea.flush();
            capture.flush();
            next_flush = Instant::now() + FLUSH_INTERVAL;
        }
    }
    // A capture left open (worker never sent End before shutdown) still gets
    // closed and receipted rather than silently truncated.
    emit(capture.end(), &ui, &mut events);
    events.flush();
    nmea.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn stamp(y: i32, mo: u8, d: u8, h: u8, mi: u8, s: u8) -> LocalStamp {
        LocalStamp {
            year: y,
            month: mo,
            day: d,
            hour: h,
            min: mi,
            sec: s,
        }
    }

    #[test]
    fn daily_filename_zero_pads() {
        assert_eq!(daily_filename(&stamp(2026, 7, 15, 0, 0, 0)), "20260715.txt");
        assert_eq!(daily_filename(&stamp(2026, 1, 2, 0, 0, 0)), "20260102.txt");
        assert_eq!(daily_filename(&stamp(999, 12, 31, 0, 0, 0)), "09991231.txt");
    }

    #[test]
    fn crash_filename_shape() {
        assert_eq!(
            crash_filename(&stamp(2026, 7, 15, 9, 5, 3)),
            "crash-20260715-090503.txt"
        );
    }

    #[test]
    fn days_from_civil_known_values() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        // A week apart is exactly 7 (weekly update-check math).
        assert_eq!(
            days_from_civil(2026, 7, 15) - days_from_civil(2026, 7, 8),
            7
        );
        // Month and year rollovers.
        assert_eq!(
            days_from_civil(2026, 3, 1) - days_from_civil(2026, 2, 28),
            1,
            "2026 is not a leap year"
        );
        assert_eq!(
            days_from_civil(2024, 3, 1) - days_from_civil(2024, 2, 28),
            2,
            "2024 is a leap year"
        );
    }

    #[test]
    fn sink_rolls_at_date_change_and_logs_it() {
        let dir =
            std::env::temp_dir().join(format!("open-ntrip-client-logtest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut sink = DailySink::new(dir.clone(), true);
        let d1 = stamp(2026, 7, 15, 23, 59, 59);
        let d2 = stamp(2026, 7, 16, 0, 0, 1);
        assert!(matches!(sink.write_line(&d1, "line a"), SinkEvent::None));
        let rolled = sink.write_line(&d2, "line b");
        let SinkEvent::RolledTo(name) = rolled else {
            panic!("expected rollover event");
        };
        assert_eq!(name, "20260716.txt");
        sink.flush();
        let a = std::fs::read_to_string(dir.join("20260715.txt")).unwrap();
        let b = std::fs::read_to_string(dir.join("20260716.txt")).unwrap();
        assert!(a.contains("line a") && !a.contains("line b"));
        assert!(b.contains("line b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_sink_writes_nothing_and_creates_no_dir() {
        let dir = std::env::temp_dir().join(format!(
            "open-ntrip-client-logtest-off-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut sink = DailySink::new(dir.clone(), false);
        assert!(matches!(
            sink.write_line(&stamp(2026, 7, 15, 1, 2, 3), "x"),
            SinkEvent::None
        ));
        assert!(!dir.exists(), "disabled sink must not create its dir");
    }

    /// Guards the "1 s flush" contract under CONTINUOUS traffic: messages
    /// arriving faster than the recv timeout must not postpone flushing
    /// until the writer's buffer fills. The line has to be readable on disk
    /// while the thread is still running and busy.
    #[test]
    fn continuous_traffic_still_flushes_within_a_second() {
        let base =
            std::env::temp_dir().join(format!("open-ntrip-client-logflush-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (ui_tx, _ui_rx) = channel();
        let logger = Logger::start(base.join("Logs"), base.join("NMEA"), true, false, ui_tx);
        let tx = logger.sender();
        let path = base
            .join("Logs")
            .join(daily_filename(&gnss::clock::now_local()));
        let start = Instant::now();
        let mut seen = false;
        // 100 ms cadence keeps the logger permanently non-idle. Allow 3 s of
        // slack for a loaded CI box; the contract target is ~1 s.
        while start.elapsed() < Duration::from_secs(3) && !seen {
            tx.send(LogCmd::Event("flush probe".to_string())).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            seen = std::fs::read_to_string(&path)
                .map(|t| t.contains("flush probe"))
                .unwrap_or(false);
        }
        logger.shutdown();
        assert!(
            seen,
            "line was not flushed to disk while the logger stayed busy"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn capture_filename_shape_and_sanitization() {
        let t = stamp(2026, 7, 16, 9, 5, 3);
        assert_eq!(
            capture_filename(&t, "OGD1_RTCM3"),
            "20260716_090503_OGD1_RTCM3.rtcm"
        );
        assert_eq!(
            capture_filename(&t, "P401-A.B"),
            "20260716_090503_P401-A.B.rtcm",
            "filename-safe characters pass through"
        );
        assert_eq!(
            capture_filename(&t, "..\\evil/mount:x"),
            "20260716_090503_.._evil_mount_x.rtcm",
            "path separators fold to '_'"
        );
        assert_eq!(
            capture_filename(&t, ""),
            "20260716_090503_stream.rtcm",
            "raw TCP (no mountpoint) still gets a name"
        );
        assert_eq!(
            capture_filename(&stamp(999, 1, 2, 0, 0, 0), "M"),
            "09990102_000000_M.rtcm",
            "zero padding"
        );
    }

    #[test]
    fn capture_sink_is_lazy_and_reports_byte_count() {
        let dir =
            std::env::temp_dir().join(format!("open-ntrip-client-capture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut sink = CaptureSink::new();

        // Armed but no data: no directory, no file, silent end.
        assert!(
            sink.begin(CaptureTarget::Dir(dir.clone()), "M1".to_string())
                .is_none()
        );
        assert!(sink.end().is_none(), "no bytes -> no file -> no receipt");
        assert!(!dir.exists(), "lazy open must not create the directory");

        // Data flows: file appears (named by timestamp+mount), bytes exact.
        sink.begin(CaptureTarget::Dir(dir.clone()), "M1".to_string());
        let start = sink.data(&[0xD3, 0x00, 0x01]).expect("start notice");
        assert!(start.contains("Capturing corrections"), "{start}");
        assert!(sink.data(&[0xAA, 0xBB]).is_none(), "no repeat notice");
        let close = sink.end().expect("close receipt");
        assert!(close.contains("(5 bytes)"), "{close}");
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(files.len(), 1);
        let path = files[0].as_ref().unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.ends_with("_M1.rtcm") && name.len() == "YYYYMMDD_HHMMSS_M1.rtcm".len(),
            "{name}"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            vec![0xD3, 0x00, 0x01, 0xAA, 0xBB]
        );

        // Explicit file target (the selftest path).
        let target = dir.join("explicit.rtcm");
        sink.begin(CaptureTarget::File(target.clone()), String::new());
        sink.data(b"abc");
        let close = sink.end().expect("close receipt");
        assert!(close.contains("(3 bytes)"), "{close}");
        assert_eq!(std::fs::read(&target).unwrap(), b"abc");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_open_failure_reports_once_and_stays_quiet() {
        let dir = std::env::temp_dir().join(format!(
            "open-ntrip-client-capture-fail-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        // A file where the target DIRECTORY should be makes create_dir_all
        // (or the create inside it) fail on every platform.
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let mut sink = CaptureSink::new();
        sink.begin(CaptureTarget::Dir(blocker.clone()), "M".to_string());
        let err = sink.data(b"data").expect("first failure is reported");
        assert!(err.contains("Could not open capture file"), "{err}");
        assert!(sink.data(b"more").is_none(), "failures do not spam");
        assert!(sink.end().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn logger_thread_writes_flushes_and_shuts_down() {
        let base = std::env::temp_dir().join(format!(
            "open-ntrip-client-logthread-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let (ui_tx, _ui_rx) = channel();
        let logger = Logger::start(base.join("Logs"), base.join("NMEA"), true, true, ui_tx);
        let tx = logger.sender();
        tx.send(LogCmd::Event("10:00:00 hello".to_string()))
            .unwrap();
        tx.send(LogCmd::Nmea("$GPGGA,raw".to_string())).unwrap();
        logger.shutdown();
        let name = daily_filename(&gnss::clock::now_local());
        let events = std::fs::read_to_string(base.join("Logs").join(&name)).unwrap();
        let nmea = std::fs::read_to_string(base.join("NMEA").join(&name)).unwrap();
        assert!(events.contains("10:00:00 hello"));
        assert!(nmea.contains("$GPGGA,raw"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
