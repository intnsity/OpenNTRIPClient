//! UI-thread application state.
//!
//! Mutated in exactly one place: `AppState::apply`, called while draining the
//! event channel at the top of each frame. Workers own no shared UI state -
//! the bus is the only way in - so every field here is plain data with no
//! locks.

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gnss::rtcm::decode::Decoded;
use ntrip_core::sourcetable::SourceTable;

use crate::bus::{AppEvent, NtripStatus, SerialStatus};

/// Fixed-capacity line ring: pushing past capacity drops the oldest line.
/// Backs both log panes; the caps guarantee bounded memory on day-long runs.
pub struct Ring {
    buf: VecDeque<String>,
    cap: usize,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Ring {
            buf: VecDeque::with_capacity(cap.min(1024)),
            cap,
        }
    }

    pub fn push(&mut self, line: String) {
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(line);
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<&String> {
        self.buf.get(i)
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    pub fn join(&self) -> String {
        let mut out = String::new();
        for line in &self.buf {
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}

/// Exponentially weighted moving average, alpha fixed at construction.
/// First sample seeds the average so the display never ramps up from zero.
pub struct Ewma {
    alpha: f32,
    value: Option<f32>,
}

impl Ewma {
    pub fn new(alpha: f32) -> Self {
        Ewma { alpha, value: None }
    }

    pub fn update(&mut self, x: f32) -> f32 {
        let v = match self.value {
            None => x,
            Some(prev) => prev + self.alpha * (x - prev),
        };
        self.value = Some(v);
        v
    }

    pub fn get(&self) -> Option<f32> {
        self.value
    }
}

/// Message-rate meter: counts are folded into an EWMA of Hz once per second.
/// Bucketing (rather than per-arrival deltas) makes the reading independent
/// of how the TCP stream batches frames, and folding on `tick` even without
/// arrivals lets a stalled type's rate decay instead of freezing.
pub struct RateMeter {
    window_start: Option<Instant>,
    window_count: u32,
    ewma: Ewma,
}

const RATE_WINDOW: Duration = Duration::from_secs(1);

impl RateMeter {
    pub fn new() -> Self {
        RateMeter {
            window_start: None,
            window_count: 0,
            ewma: Ewma::new(0.2),
        }
    }

    pub fn on_frames(&mut self, n: u32, now: Instant) {
        if self.window_start.is_none() {
            self.window_start = Some(now);
        }
        self.window_count += n;
        self.fold_if_due(now);
    }

    /// Periodic fold; call about once a frame so silence decays the rate.
    pub fn tick(&mut self, now: Instant) {
        self.fold_if_due(now);
    }

    fn fold_if_due(&mut self, now: Instant) {
        let Some(t0) = self.window_start else { return };
        let elapsed = now.duration_since(t0);
        if elapsed >= RATE_WINDOW {
            self.ewma
                .update(self.window_count as f32 / elapsed.as_secs_f32());
            self.window_start = Some(now);
            self.window_count = 0;
        }
    }

    /// Smoothed Hz; None until the first full window has elapsed.
    pub fn hz(&self) -> Option<f32> {
        self.ewma.get()
    }
}

impl Default for RateMeter {
    fn default() -> Self {
        Self::new()
    }
}

/// Live per-message-type stream statistics for the RTCM inspector.
pub struct RtcmTypeStat {
    pub count: u64,
    /// Wire size in bytes of the most recent complete frame.
    pub last_frame_len: u32,
    pub last_seen: Instant,
    pub rate: RateMeter,
}

/// Elevation samples for the strip chart. Bounded at `cap` points by halving:
/// when full, every other retained point is dropped and the keep-stride
/// doubles, so an arbitrarily long session stays <= cap points while the
/// visual shape survives. Min/max track ALL samples, not just retained ones.
pub struct PlotSeries {
    points: Vec<[f64; 2]>,
    cap: usize,
    stride: u32,
    skip: u32,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub current: Option<f64>,
}

impl PlotSeries {
    pub fn new(cap: usize) -> Self {
        PlotSeries {
            points: Vec::new(),
            cap: cap.max(2),
            stride: 1,
            skip: 0,
            min: None,
            max: None,
            current: None,
        }
    }

    pub fn push(&mut self, t: f64, v: f64) {
        self.current = Some(v);
        self.min = Some(self.min.map_or(v, |m| m.min(v)));
        self.max = Some(self.max.map_or(v, |m| m.max(v)));
        if self.skip > 0 {
            self.skip -= 1;
            return;
        }
        self.points.push([t, v]);
        if self.points.len() >= self.cap {
            let mut keep = 0;
            self.points.retain(|_| {
                keep += 1;
                keep % 2 == 1
            });
            self.stride *= 2;
        }
        self.skip = self.stride - 1;
    }

    pub fn points(&self) -> &[[f64; 2]] {
        &self.points
    }

    pub fn range(&self) -> Option<f64> {
        Some(self.max? - self.min?)
    }

    pub fn clear(&mut self) {
        self.points.clear();
        self.stride = 1;
        self.skip = 0;
        self.min = None;
        self.max = None;
        self.current = None;
    }
}

/// Latest values parsed from the receiver's NMEA stream.
pub struct GnssState {
    pub quality: u8,
    pub sats: u8,
    pub hdop: Option<f32>,
    pub vdop: Option<f32>,
    pub pdop: Option<f32>,
    pub alt_m: Option<f32>,
    pub age_s: Option<f32>,
    pub station_id: Option<u16>,
    pub speed_knots: Option<f32>,
    pub heading_deg: Option<f32>,
    pub mph_smooth: Ewma,
    pub kmh_smooth: Ewma,
    /// Last position with a value, kept through fix loss: the location
    /// picker's "use receiver position" and the inspector's baseline want
    /// the last known point, not a blank the moment the sky disappears.
    pub lat_deg: Option<f64>,
    pub lon_deg: Option<f64>,
    /// Set once the first GGA arrives; the fix label shows a neutral
    /// placeholder until then rather than claiming "Invalid".
    pub has_fix_data: bool,
    /// Any NMEA sentence at all has arrived (not just GGA: an RMC-only
    /// receiver still counts as present). While false, the receiver-derived
    /// readouts can explain themselves as "needs a GPS receiver" instead of
    /// showing a bare dash forever.
    pub nmea_seen: bool,
}

impl GnssState {
    fn new() -> Self {
        GnssState {
            quality: 0,
            sats: 0,
            hdop: None,
            vdop: None,
            pdop: None,
            alt_m: None,
            age_s: None,
            station_id: None,
            speed_knots: None,
            heading_deg: None,
            mph_smooth: Ewma::new(0.2),
            kmh_smooth: Ewma::new(0.2),
            lat_deg: None,
            lon_deg: None,
            has_fix_data: false,
            nmea_seen: false,
        }
    }

    fn apply(&mut self, sentence: &gnss::nmea::Sentence) {
        self.nmea_seen = true;
        match sentence {
            gnss::nmea::Sentence::Gga(g) => {
                self.has_fix_data = true;
                self.quality = g.quality;
                self.sats = g.sats;
                self.hdop = g.hdop;
                self.alt_m = g.alt_m;
                self.age_s = g.age_s;
                self.station_id = g.station_id;
                if let (Some(lat), Some(lon)) = (g.lat_deg, g.lon_deg) {
                    self.lat_deg = Some(lat);
                    self.lon_deg = Some(lon);
                }
            }
            gnss::nmea::Sentence::Rmc(r) => {
                self.speed_knots = r.speed_knots;
                self.heading_deg = r.track_deg;
                if let Some(k) = r.speed_knots {
                    self.mph_smooth.update(gnss::nmea::knots_to_mph(k));
                    self.kmh_smooth.update(gnss::nmea::knots_to_kmh(k));
                }
            }
            gnss::nmea::Sentence::Gsa(g) => {
                self.pdop = g.pdop;
                self.vdop = g.vdop;
            }
            gnss::nmea::Sentence::Other { .. } => {}
        }
    }
}

/// Two missed 1 Hz RTCM epochs: the stream is officially quiet. Drives the
/// "no data for N s" stall affordance so "Streaming" cannot read as healthy
/// while the caster starves the client (the pre-kick window on GGA-hungry
/// casters).
pub const STALL_AFTER: Duration = Duration::from_secs(2);

pub struct NtripUi {
    pub status: NtripStatus,
    pub total_bytes: u64,
    /// When correction bytes last arrived; None until this session's first
    /// data. Feeds the activity indicator and the stall predicate.
    pub last_rx: Option<Instant>,
    /// When the current status was applied - the stall reference before the
    /// first byte (WaitingForData with no data yet is itself a stall).
    pub status_since: Instant,
    /// (host, port, parsed table) for the caster it came from.
    pub sourcetable: Option<(String, u16, Arc<SourceTable>)>,
    /// Inspector: live per-type statistics, keyed by message type.
    pub rtcm: BTreeMap<u16, RtcmTypeStat>,
    pub rtcm_crc_failures: u64,
    pub rtcm_garbage_bytes: u64,
    /// Latest diagnostic decodes; each slot holds only its own variant.
    pub base: Option<Decoded>,
    pub antenna: Option<Decoded>,
    /// Latest 1029 text with its local receive time ("HH:MM:SS").
    pub text_1029: Option<(String, Decoded)>,
    pub biases_1230: Option<Decoded>,
    /// Raw bytes of the most recent UnknownResponse close, for the hex view.
    /// Deliberately survives reconnects and new sessions: it is forensic
    /// evidence the user may want after the retry succeeded.
    pub last_unknown_response: Option<Vec<u8>>,
    /// Generation counter for UnknownResponse captures: bumps once per
    /// capture and, like the raw bytes above, is deliberately NOT reset by
    /// `reset_stream_stats` - the evidence outlives sessions. The Conn tab
    /// acks the generation it has shown (`App.conn_unknown_ack`); the tab's
    /// attention badge fires while gen is ahead of the ack.
    pub unknown_response_gen: u64,
}

impl NtripUi {
    /// Seconds since correction bytes last arrived, measured from the start
    /// of the current status when no data has come yet. None outside the
    /// data-bearing states - nothing can stall while disconnected.
    pub fn rx_age(&self, now: Instant) -> Option<Duration> {
        matches!(
            self.status,
            NtripStatus::Streaming | NtripStatus::WaitingForData
        )
        .then(|| now.duration_since(self.last_rx.unwrap_or(self.status_since)))
    }

    /// True when a live connection has gone quiet past `STALL_AFTER`.
    pub fn stalled(&self, now: Instant) -> bool {
        self.rx_age(now).is_some_and(|age| age > STALL_AFTER)
    }

    /// Wipe per-session stream state at the start of a new user-initiated
    /// connection. The worker's counters restart from zero, so stale UI
    /// numbers would otherwise mix two sessions.
    pub fn reset_stream_stats(&mut self) {
        self.total_bytes = 0;
        self.last_rx = None;
        self.rtcm.clear();
        self.rtcm_crc_failures = 0;
        self.rtcm_garbage_bytes = 0;
        self.base = None;
        self.antenna = None;
        self.text_1029 = None;
        self.biases_1230 = None;
    }
}

pub struct SerialUi {
    pub status: SerialStatus,
    pub overruns: u64,
}

pub struct ElevationChart {
    pub series: PlotSeries,
    pub recording: bool,
    /// Time origin for the x axis; fixed at first use.
    pub t0: Instant,
}

pub struct AppState {
    pub events: Ring,
    pub conn: Ring,
    pub gnss: GnssState,
    pub ntrip: NtripUi,
    pub serial: SerialUi,
    pub chart: ElevationChart,
}

pub const EVENT_RING_CAP: usize = 10_000;
pub const CONN_RING_CAP: usize = 5_000;
pub const PLOT_POINT_CAP: usize = 50_000;

impl AppState {
    pub fn new(now: Instant) -> Self {
        AppState {
            events: Ring::new(EVENT_RING_CAP),
            conn: Ring::new(CONN_RING_CAP),
            gnss: GnssState::new(),
            ntrip: NtripUi {
                status: NtripStatus::Idle,
                total_bytes: 0,
                last_rx: None,
                status_since: now,
                sourcetable: None,
                rtcm: BTreeMap::new(),
                rtcm_crc_failures: 0,
                rtcm_garbage_bytes: 0,
                base: None,
                antenna: None,
                text_1029: None,
                biases_1230: None,
                last_unknown_response: None,
                unknown_response_gen: 0,
            },
            serial: SerialUi {
                status: SerialStatus::Disconnected {
                    reason: String::new(),
                },
                overruns: 0,
            },
            chart: ElevationChart {
                series: PlotSeries::new(PLOT_POINT_CAP),
                recording: true,
                t0: now,
            },
        }
    }

    pub fn apply(&mut self, ev: AppEvent, now: Instant) {
        match ev {
            AppEvent::EventLine(line) => self.events.push(line),
            AppEvent::ConnLine(line) => self.conn.push(line),
            AppEvent::Ntrip(status) => {
                self.ntrip.status = status;
                self.ntrip.status_since = now;
            }
            AppEvent::RxBytes { total } => {
                // The counter only ever grows within a session; a growth tick
                // IS the arrival signal the activity indicator pulses on.
                if total > self.ntrip.total_bytes {
                    self.ntrip.last_rx = Some(now);
                }
                self.ntrip.total_bytes = total;
            }
            AppEvent::Rtcm(batch) => {
                for (ty, n, last_len) in batch.frames {
                    let stat = self.ntrip.rtcm.entry(ty).or_insert_with(|| RtcmTypeStat {
                        count: 0,
                        last_frame_len: 0,
                        last_seen: now,
                        rate: RateMeter::new(),
                    });
                    stat.count += u64::from(n);
                    stat.last_frame_len = last_len;
                    stat.last_seen = now;
                    stat.rate.on_frames(n, now);
                }
                self.ntrip.rtcm_crc_failures = batch.crc_failures;
                self.ntrip.rtcm_garbage_bytes = batch.garbage_bytes;
                for d in batch.decoded {
                    match &d {
                        Decoded::BasePosition { .. } => self.ntrip.base = Some(d),
                        Decoded::AntennaInfo { .. } => self.ntrip.antenna = Some(d),
                        Decoded::TextMessage { .. } => {
                            let t = gnss::clock::now_local();
                            let at = format!("{:02}:{:02}:{:02}", t.hour, t.min, t.sec);
                            self.ntrip.text_1029 = Some((at, d));
                        }
                        Decoded::GlonassBiases { .. } => self.ntrip.biases_1230 = Some(d),
                        Decoded::MsmHeader { .. } => {}
                    }
                }
            }
            AppEvent::SourcetableReady { host, port, table } => {
                self.ntrip.sourcetable = Some((host, port, table));
            }
            AppEvent::UnknownResponse { raw } => {
                self.ntrip.last_unknown_response = Some(raw);
                self.ntrip.unknown_response_gen += 1;
            }
            AppEvent::Nmea(sentence) => {
                self.gnss.apply(&sentence);
                if self.chart.recording
                    && let gnss::nmea::Sentence::Gga(g) = &sentence
                    && let Some(alt) = g.alt_m
                {
                    let t = now.duration_since(self.chart.t0).as_secs_f64();
                    self.chart.series.push(t, f64::from(alt));
                }
            }
            AppEvent::Serial(status) => self.serial.status = status,
            AppEvent::Overruns(n) => self.serial.overruns = n,
        }
    }

    /// Per-frame heartbeat work: fold rate-meter windows so per-type rates
    /// decay while a type (or the whole stream) is silent.
    pub fn tick(&mut self, now: Instant) {
        for stat in self.ntrip.rtcm.values_mut() {
            stat.rate.tick(now);
        }
    }

    /// STR record from the cached table matching a host/port/mountpoint, if
    /// the cached table is for that caster at all. Exact mountpoint match
    /// wins; a case-insensitive fallback covers casters (NTRIP Caster 1.0,
    /// verified live) that serve mounts case-insensitively - without it a
    /// profile typed as "rtcm32" streams fine but loses the row's nmea flag,
    /// silently disabling the GGA warnings and hints.
    pub fn str_record(
        &self,
        host: &str,
        port: u16,
        mountpoint: &str,
    ) -> Option<&ntrip_core::sourcetable::StrRecord> {
        let (h, p, table) = self.ntrip.sourcetable.as_ref()?;
        if h != host || *p != port {
            return None;
        }
        table
            .strs
            .iter()
            .find(|s| s.mountpoint == mountpoint)
            .or_else(|| {
                table
                    .strs
                    .iter()
                    .find(|s| s.mountpoint.eq_ignore_ascii_case(mountpoint))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_caps_and_drops_oldest() {
        let mut r = Ring::new(3);
        for i in 0..5 {
            r.push(format!("line {i}"));
        }
        assert_eq!(r.len(), 3);
        assert_eq!(r.get(0).unwrap(), "line 2");
        assert_eq!(r.get(2).unwrap(), "line 4");
        assert_eq!(r.join(), "line 2\nline 3\nline 4\n");
        r.clear();
        assert!(r.is_empty());
    }

    #[test]
    fn event_and_conn_ring_caps_match_spec() {
        assert_eq!(EVENT_RING_CAP, 10_000);
        assert_eq!(CONN_RING_CAP, 5_000);
        let mut state = AppState::new(Instant::now());
        for i in 0..EVENT_RING_CAP + 10 {
            state.apply(AppEvent::EventLine(format!("e{i}")), Instant::now());
        }
        assert_eq!(state.events.len(), EVENT_RING_CAP);
        assert_eq!(state.events.get(0).unwrap(), "e10");
    }

    #[test]
    fn ewma_alpha_point_two() {
        let mut e = Ewma::new(0.2);
        assert_eq!(e.get(), None);
        assert!((e.update(10.0) - 10.0).abs() < 1e-6, "first sample seeds");
        // 10 + 0.2 * (20 - 10) = 12
        assert!((e.update(20.0) - 12.0).abs() < 1e-6);
        // 12 + 0.2 * (20 - 12) = 13.6
        assert!((e.update(20.0) - 13.6).abs() < 1e-6);
        assert!((e.get().unwrap() - 13.6).abs() < 1e-6);
    }

    #[test]
    fn ewma_converges_to_constant_input() {
        let mut e = Ewma::new(0.2);
        for _ in 0..100 {
            e.update(42.0);
        }
        assert!((e.get().unwrap() - 42.0).abs() < 1e-3);
    }

    #[test]
    fn plot_series_caps_by_decimation_and_keeps_stats() {
        let mut s = PlotSeries::new(1000);
        for i in 0..10_000 {
            s.push(f64::from(i), f64::from(i % 100));
        }
        assert!(s.points().len() <= 1000, "len {}", s.points().len());
        assert!(s.points().len() > 400, "decimation must not empty the plot");
        assert_eq!(s.min, Some(0.0));
        assert_eq!(s.max, Some(99.0));
        assert_eq!(s.current, Some(f64::from(9_999 % 100)));
        assert_eq!(s.range(), Some(99.0));
        // Time still spans the whole run after decimation.
        let last_t = s.points().last().unwrap()[0];
        assert!(last_t > 9_000.0, "last t {last_t}");
        s.clear();
        assert!(s.points().is_empty());
        assert_eq!(s.range(), None);
    }

    #[test]
    fn rate_meter_folds_one_second_windows() {
        let t0 = Instant::now();
        let mut r = RateMeter::new();
        assert_eq!(r.hz(), None, "no reading before the first full window");
        // 10 frames over the first second.
        for i in 0..10 {
            r.on_frames(1, t0 + Duration::from_millis(i * 100));
        }
        assert_eq!(r.hz(), None, "window not yet elapsed");
        r.on_frames(1, t0 + Duration::from_millis(1000));
        let hz = r.hz().expect("first window folded");
        assert!((10.0..=11.5).contains(&hz), "seeded near 11/1.0 Hz: {hz}");
        // Silence: ticks fold empty windows, rate decays toward zero.
        for k in 1..=20u64 {
            r.tick(t0 + Duration::from_millis(1000 + k * 1000));
        }
        assert!(r.hz().unwrap() < 0.5, "silence must decay: {:?}", r.hz());
    }

    #[test]
    fn rtcm_batch_updates_stats_and_decoded_slots() {
        let mut state = AppState::new(Instant::now());
        let now = Instant::now();
        let batch = crate::bus::RtcmBatch {
            frames: vec![(1005, 1, 25), (1074, 3, 120)],
            crc_failures: 2,
            garbage_bytes: 7,
            decoded: vec![
                Decoded::TextMessage {
                    station_id: 9,
                    mjd: 60_000,
                    seconds_of_day: 1,
                    text: "base moved".to_string(),
                },
                Decoded::GlonassBiases {
                    station_id: 9,
                    bias_indicator: false,
                    biases_m: Vec::new(),
                },
            ],
        };
        state.apply(AppEvent::Rtcm(batch), now);
        let s1005 = &state.ntrip.rtcm[&1005];
        assert_eq!((s1005.count, s1005.last_frame_len), (1, 25));
        assert_eq!(state.ntrip.rtcm[&1074].count, 3);
        assert_eq!(state.ntrip.rtcm_crc_failures, 2);
        assert_eq!(state.ntrip.rtcm_garbage_bytes, 7);
        let (at, text) = state.ntrip.text_1029.as_ref().expect("1029 kept");
        assert_eq!(at.len(), "HH:MM:SS".len());
        assert!(matches!(text, Decoded::TextMessage { text, .. } if text == "base moved"));
        assert!(state.ntrip.biases_1230.is_some());
        assert!(state.ntrip.base.is_none());

        // A later batch accumulates counts and replaces last-frame size.
        state.apply(
            AppEvent::Rtcm(crate::bus::RtcmBatch {
                frames: vec![(1005, 2, 27)],
                crc_failures: 2,
                garbage_bytes: 7,
                decoded: Vec::new(),
            }),
            now,
        );
        let s1005 = &state.ntrip.rtcm[&1005];
        assert_eq!((s1005.count, s1005.last_frame_len), (3, 27));

        state.ntrip.reset_stream_stats();
        assert!(state.ntrip.rtcm.is_empty());
        assert!(state.ntrip.text_1029.is_none());
        assert_eq!(state.ntrip.rtcm_crc_failures, 0);
    }

    #[test]
    fn unknown_response_raw_is_kept_for_hex_view() {
        let mut state = AppState::new(Instant::now());
        state.apply(
            AppEvent::UnknownResponse {
                raw: b"HTTP/1.1 302 Found\r\n".to_vec(),
            },
            Instant::now(),
        );
        assert_eq!(
            state.ntrip.last_unknown_response.as_deref(),
            Some(b"HTTP/1.1 302 Found\r\n".as_slice())
        );
    }

    /// The Conn tab's badge contract: each capture bumps the generation, and
    /// a new session must NOT clear it - unseen forensic evidence stays
    /// flagged until the tab is actually shown (the ack lives in App).
    #[test]
    fn unknown_response_gen_counts_and_survives_stream_reset() {
        let mut state = AppState::new(Instant::now());
        assert_eq!(state.ntrip.unknown_response_gen, 0);
        state.apply(
            AppEvent::UnknownResponse {
                raw: b"HTTP/1.1 302 Found\r\n".to_vec(),
            },
            Instant::now(),
        );
        state.apply(
            AppEvent::UnknownResponse {
                raw: b"totally not http".to_vec(),
            },
            Instant::now(),
        );
        assert_eq!(state.ntrip.unknown_response_gen, 2);
        state.ntrip.reset_stream_stats();
        assert_eq!(
            state.ntrip.unknown_response_gen, 2,
            "generation must survive a new session like the raw bytes do"
        );
        assert!(state.ntrip.last_unknown_response.is_some());
    }

    #[test]
    fn gnss_state_keeps_last_position_through_fix_loss() {
        let mut state = AppState::new(Instant::now());
        let body = "GPGGA,123519,4807.038,N,01131.000,E,4,08,0.9,545.4,M,46.9,M,2.0,42";
        let ck = body.bytes().fold(0u8, |a, b| a ^ b);
        let with_fix = gnss::nmea::parse(&format!("${body}*{ck:02X}")).unwrap();
        state.apply(AppEvent::Nmea(with_fix), Instant::now());
        let lat = state.gnss.lat_deg.expect("lat parsed");
        assert!((lat - 48.1173).abs() < 1e-3, "{lat}");
        // Fix lost: empty position fields must NOT wipe the last known point.
        let body = "GPGGA,123520,,,,,0,00,,,M,,M,,";
        let ck = body.bytes().fold(0u8, |a, b| a ^ b);
        let no_fix = gnss::nmea::parse(&format!("${body}*{ck:02X}")).unwrap();
        state.apply(AppEvent::Nmea(no_fix), Instant::now());
        assert_eq!(state.gnss.quality, 0);
        assert_eq!(state.gnss.lat_deg, Some(lat), "last position survives");
    }

    /// The stall/activity contract for the UI's data indicator: last_rx is
    /// stamped by growing byte totals, rx_age falls back to the status
    /// change, and only the data-bearing states can stall.
    #[test]
    fn ntrip_rx_age_and_stall_predicate() {
        let t0 = Instant::now();
        let mut state = AppState::new(t0);
        assert_eq!(state.ntrip.rx_age(t0), None, "Idle cannot stall");
        assert!(!state.ntrip.stalled(t0 + Duration::from_secs(60)));

        // Connected, waiting: the status change is the stall reference.
        state.apply(AppEvent::Ntrip(NtripStatus::WaitingForData), t0);
        let waited = state.ntrip.rx_age(t0 + Duration::from_secs(3)).unwrap();
        assert_eq!(waited, Duration::from_secs(3));
        assert!(state.ntrip.stalled(t0 + Duration::from_secs(3)));
        assert!(!state.ntrip.stalled(t0 + Duration::from_secs(1)));

        // Data arrives: last_rx takes over.
        state.apply(
            AppEvent::Ntrip(NtripStatus::Streaming),
            t0 + Duration::from_secs(4),
        );
        state.apply(
            AppEvent::RxBytes { total: 512 },
            t0 + Duration::from_secs(5),
        );
        assert_eq!(
            state.ntrip.rx_age(t0 + Duration::from_secs(6)),
            Some(Duration::from_secs(1))
        );
        assert!(!state.ntrip.stalled(t0 + Duration::from_secs(6)));
        assert!(state.ntrip.stalled(t0 + Duration::from_secs(8)));

        // A duplicate total is not an arrival: the stall clock keeps running.
        state.apply(
            AppEvent::RxBytes { total: 512 },
            t0 + Duration::from_secs(9),
        );
        assert!(state.ntrip.stalled(t0 + Duration::from_secs(9)));

        // New session wipes the arrival stamp with the other stream stats.
        state.ntrip.reset_stream_stats();
        assert_eq!(state.ntrip.last_rx, None);
        assert_eq!(state.ntrip.total_bytes, 0);
    }

    /// The STR lookup behind the GGA machinery: exact match beats the
    /// case-insensitive fallback, the fallback finds case-different mounts
    /// (casters match mountpoints case-insensitively on the wire), and a
    /// table cached for another caster never answers.
    #[test]
    fn str_record_prefers_exact_then_ignores_ascii_case() {
        let raw = b"STR;RTCM32;Boulder;RTCM 3.2;1074;2;GPS;NET;USA;40.0;-105.0;1;0;GEN;none;B;N;9600;\r\n\
                    STR;rtcm32;Lower;RTCM 3.2;1074;2;GPS;NET;USA;40.0;-105.0;0;0;GEN;none;B;N;9600;\r\n\
                    ENDSOURCETABLE\r\n";
        let table = ntrip_core::sourcetable::parse(raw);
        assert_eq!(table.strs.len(), 2, "fixture must parse both rows");
        let mut state = AppState::new(Instant::now());
        state.apply(
            AppEvent::SourcetableReady {
                host: "caster.example".to_string(),
                port: 2101,
                table: Arc::new(table),
            },
            Instant::now(),
        );
        // Exact casing wins even with a case-different sibling present.
        let exact = state.str_record("caster.example", 2101, "RTCM32").unwrap();
        assert!(exact.nmea_required, "must be the uppercase row");
        let exact = state.str_record("caster.example", 2101, "rtcm32").unwrap();
        assert!(!exact.nmea_required, "must be the lowercase row");
        // Case-different query falls back instead of reporting unlisted.
        let fallback = state.str_record("caster.example", 2101, "Rtcm32").unwrap();
        assert_eq!(fallback.mountpoint, "RTCM32", "first case-insensitive hit");
        assert!(
            state
                .str_record("caster.example", 2101, "MISSING")
                .is_none()
        );
        // Wrong caster: the cached table must not answer at all.
        assert!(state.str_record("other.example", 2101, "RTCM32").is_none());
        assert!(state.str_record("caster.example", 9999, "RTCM32").is_none());
    }

    #[test]
    fn nmea_seen_latches_on_any_sentence_kind() {
        let mut state = AppState::new(Instant::now());
        assert!(!state.gnss.nmea_seen);
        // An RMC-only receiver (no GGA, so has_fix_data stays false) must
        // still count as present.
        let body = "GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W";
        let ck = body.bytes().fold(0u8, |a, b| a ^ b);
        let rmc = gnss::nmea::parse(&format!("${body}*{ck:02X}")).unwrap();
        state.apply(AppEvent::Nmea(rmc), Instant::now());
        assert!(state.gnss.nmea_seen);
        assert!(!state.gnss.has_fix_data, "RMC alone is not fix data");
    }

    #[test]
    fn gnss_state_tracks_sentences() {
        let mut state = AppState::new(Instant::now());
        let body = "GPGGA,123519,4807.038,N,01131.000,E,4,08,0.9,545.4,M,46.9,M,2.0,42";
        let ck = body.bytes().fold(0u8, |a, b| a ^ b);
        let gga = format!("${body}*{ck:02X}");
        let parsed = gnss::nmea::parse(&gga);
        let Ok(sentence) = parsed else {
            panic!("bad fixture: {parsed:?}");
        };
        state.apply(AppEvent::Nmea(sentence), Instant::now());
        assert!(state.gnss.has_fix_data);
        assert_eq!(state.gnss.quality, 4);
        assert_eq!(state.gnss.sats, 8);
        assert_eq!(state.gnss.station_id, Some(42));
        assert_eq!(state.chart.series.points().len(), 1, "chart records alt");
    }
}
