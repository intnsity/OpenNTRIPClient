//! The application object and main-window layout. Every mutation of
//! `AppState` happens at the top of `update` by draining the event bus;
//! everything below that is rendering plus user-action methods.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use ntrip_core::sourcetable::SourceTable;
use ntrip_core::{NtripVersion, Transport};

use crate::bus::{AppEvent, Hub, NtripStatus, Repaint};
use crate::logging::{Logger, days_from_civil};
use crate::settings::{CheckUpdates, ProtocolCfg, Settings};
use crate::state::AppState;
use crate::workers::ntrip::{NtripHandle, NtripJob};
use crate::workers::serial::SerialHandle;
use crate::workers::{CorrQueue, ntrip, serial};
use crate::{RELEASES_URL, paths, settings, user_agent};

pub struct App {
    pub base: PathBuf,
    pub settings: Settings,
    pub state: AppState,
    pub hub: Hub,
    rx: Receiver<AppEvent>,
    logger: Option<Logger>,
    pub ntrip: Option<NtripHandle>,
    pub serial: Option<SerialHandle>,
    pub corr: Arc<CorrQueue>,
    pub last_gga: Arc<RwLock<Option<String>>>,
    /// (port_name, human label) pairs from the last enumeration.
    pub ports: Vec<(String, String)>,

    pub show_options: bool,
    pub show_about: bool,
    pub reveal_password: bool,
    pub mount_popup_open: bool,

    pub show_profiles: bool,
    pub srctbl_view: super::sourcetable_browser::ViewState,
    pub connlog_view: super::connlog_window::ViewState,
    pub gga_view: super::gga_section::ViewState,
    pub profiles_view: super::profiles_dialog::ViewState,
    /// Generation of the last unclassified caster response the user has SEEN
    /// by opening the Connection tab. While `state.ntrip.unknown_response_gen`
    /// runs ahead of it the Conn tab wears an attention dot; `bottom_tabs`
    /// stamps this to the current generation on every frame that tab is up.
    /// (One Surface folded the old Connection Log window into a tab, so the
    /// "something unusual arrived" signal moved from a window that popped to a
    /// badge on an always-present strip.)
    pub conn_unknown_ack: u64,
    /// Embedded offline geocoder; None only if the compiled-in database
    /// fails validation (logged at boot).
    pub geodb: Option<geodb::GeoDb>,
    /// True while the current NTRIP worker was started with the
    /// accept-any-certificate override; drives the red warning banner.
    pub insecure_tls_active: bool,

    /// RX-rate meter: sampled at most once per second.
    rate_sample: (Instant, u64),
    pub rate_kbps: f64,
    geometry_rescued: bool,

    /// Stream-activity envelope (0..=1) behind the status strip's bar: any
    /// received correction bytes snap it to 1.0 and silence decays it with a
    /// 400 ms half-life, reproducing the original client's per-burst pulse
    /// independent of the stream's byte rate. (The stall/arrival clock the
    /// bar's caption reads lives in `state.ntrip.last_rx`/`status_since`;
    /// only this render-side animation state belongs to App.)
    pub activity: f32,
    /// Byte total the envelope last saw; a positive delta is a burst.
    activity_total: u64,
    /// Anchor for the envelope decay.
    activity_at: Instant,
    /// The corrections-need-a-receiver explainer has been logged for the
    /// current NTRIP session; reset per connect so each session gets it at
    /// most once instead of once per Streaming transition.
    receiver_hint_logged: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        base: PathBuf,
        settings: Settings,
        boot_log: Vec<String>,
    ) -> Self {
        // Butter Paper visuals before the first frame: every window and
        // dialog derives its colors from the theme tokens, never ad hoc.
        super::theme::apply(&cc.egui_ctx);
        let (tx, rx) = std::sync::mpsc::channel::<AppEvent>();
        let logger = Logger::start(
            paths::logs_dir(&base),
            paths::nmea_dir(&base),
            settings.app.write_event_log,
            settings.app.write_nmea_log,
            tx.clone(),
        );
        let hub = Hub::new(tx, logger.sender(), Repaint::ui(cc.egui_ctx.clone()));

        let mut app = App {
            base,
            settings,
            state: AppState::new(Instant::now()),
            hub,
            rx,
            logger: Some(logger),
            ntrip: None,
            serial: None,
            corr: Arc::new(CorrQueue::new(256)),
            last_gga: Arc::new(RwLock::new(None)),
            ports: serial::list_ports(),
            show_options: false,
            show_about: false,
            reveal_password: false,
            mount_popup_open: false,
            show_profiles: false,
            srctbl_view: Default::default(),
            connlog_view: Default::default(),
            gga_view: Default::default(),
            profiles_view: Default::default(),
            conn_unknown_ack: 0,
            geodb: None,
            insecure_tls_active: false,
            rate_sample: (Instant::now(), 0),
            rate_kbps: 0.0,
            geometry_rescued: false,
            activity: 0.0,
            activity_total: 0,
            activity_at: Instant::now(),
            receiver_hint_logged: false,
        };
        app.state.chart.recording = true;
        app.hub.event(format!(
            "Open NTRIP Client v{} started",
            env!("CARGO_PKG_VERSION")
        ));
        match geodb::GeoDb::embedded() {
            Ok(db) => app.geodb = Some(db),
            // The picker degrades to manual entry; say why, once, at boot.
            Err(e) => app
                .hub
                .event(format!("Offline location database unavailable: {e}")),
        }
        for line in boot_log {
            app.hub.event(line);
        }
        // Announce a previous run's crash exactly once: the panic hook can
        // only write a file, and a client that vanished mid-survey must not
        // restart looking like nothing happened.
        if let Some(path) = crate::take_crash_notice(&app.base) {
            app.hub
                .event("The previous run crashed; a crash report was written");
            app.hub.event(format!("Crash report: {path}"));
        }
        app.startup_update_check();
        app
    }

    // ------------------------------------------------------------------
    // Actions
    // ------------------------------------------------------------------

    pub fn ntrip_busy(&self) -> bool {
        self.ntrip.is_some()
    }

    /// Connect using the active profile. `table_only` forces a sourcetable
    /// request (empty mountpoint) regardless of the profile's mountpoint.
    pub fn connect_ntrip(&mut self, table_only: bool) {
        if self.ntrip.is_some() {
            return;
        }
        let p = self.settings.active().clone();
        let host = p.host.trim().to_string();
        if host.is_empty() {
            self.hub.event("No caster host configured");
            return;
        }
        let transport = if table_only {
            Transport::Ntrip
        } else {
            match p.protocol {
                ProtocolCfg::Ntrip => Transport::Ntrip,
                ProtocolCfg::Tcp => Transport::RawTcp,
            }
        };
        let mountpoint = if table_only {
            String::new()
        } else {
            p.mountpoint.trim().to_string()
        };
        let is_table = matches!(transport, Transport::Ntrip) && mountpoint.is_empty();
        let stream_requires = self
            .state
            .str_record(&host, p.port, &mountpoint)
            .map(|s| s.nmea_required);
        let job = NtripJob {
            host,
            port: p.port,
            mountpoint,
            username: p.username.clone(),
            password: p.password.clone(),
            version: if p.ntrip_version == 2 {
                NtripVersion::V2
            } else {
                NtripVersion::V1
            },
            transport,
            tls: p.tls,
            allow_invalid_certs: p.allow_invalid_certs,
            gga_mode: p.gga_mode,
            gga_source: p.gga_source,
            manual_lat: p.manual_lat,
            manual_lon: p.manual_lon,
            stream_requires_gga: stream_requires,
            // A table fetch is one-shot BY DESIGN; only stream sessions
            // consult the Options auto-reconnect setting (the distinction
            // decides how the no-reconnect line explains itself).
            reconnect: if is_table {
                ntrip::ReconnectPolicy::OneShot
            } else if self.settings.app.auto_reconnect {
                ntrip::ReconnectPolicy::Auto
            } else {
                ntrip::ReconnectPolicy::OptionsOff
            },
            max_attempts: self.settings.app.max_reconnect_attempts,
            audio_alert: self.settings.app.audio_alert_file.clone(),
            // Streams only: capturing a sourcetable body would be noise.
            capture: (self.settings.app.capture_corrections && !is_table)
                .then(|| crate::logging::CaptureTarget::Dir(paths::captures_dir(&self.base))),
            user_agent: user_agent(),
        };
        self.insecure_tls_active = job.tls && job.allow_invalid_certs;
        self.state.ntrip.reset_stream_stats();
        self.rate_sample = (Instant::now(), 0);
        self.rate_kbps = 0.0;
        // The worker restarts its byte counter from zero, so the envelope's
        // last-seen total must restart with it or the first burst of the new
        // session would compare against the old session's total.
        self.activity = 0.0;
        self.activity_total = 0;
        self.receiver_hint_logged = false;
        self.ntrip = Some(ntrip::spawn(
            job,
            self.hub.clone(),
            self.corr.clone(),
            self.last_gga.clone(),
        ));
    }

    pub fn disconnect_ntrip(&mut self) {
        // Cancel only; the Stopped event joins and drops the handle so the
        // UI never blocks on a worker that is still winding down.
        if let Some(h) = &self.ntrip {
            h.cancel();
        }
    }

    pub fn toggle_serial(&mut self) {
        match self.serial.take() {
            Some(h) => {
                h.cancel_and_join(Duration::from_secs(2));
            }
            None => {
                if self.settings.serial.port.trim().is_empty() {
                    self.hub.event("No serial port selected");
                    return;
                }
                self.serial = Some(serial::spawn(
                    self.settings.serial.clone(),
                    self.hub.clone(),
                    self.corr.clone(),
                    self.last_gga.clone(),
                ));
            }
        }
    }

    pub fn refresh_ports(&mut self) {
        self.ports = serial::list_ports();
    }

    pub fn save_settings(&mut self) {
        match settings::save(&self.settings, &paths::settings_file(&self.base)) {
            Ok(()) => self.hub.event("Settings saved"),
            Err(e) => self.hub.event(format!("Could not save settings: {e}")),
        }
    }

    /// Called after the active profile changes: per-caster UI state resets.
    /// The in-memory sourcetable belonged to the previous caster, so it is
    /// dropped; the new caster's table is fetched fresh on demand.
    pub fn on_profile_switched(&mut self) {
        self.reveal_password = false;
        self.mount_popup_open = false;
        self.state.ntrip.sourcetable = None;
    }

    /// Open the releases page and record today as the last check. Only the
    /// explicit user gestures (Options "Check now", the About link) reach
    /// this - never a startup cadence.
    pub fn check_updates_now(&mut self) {
        match crate::audio::open_url(RELEASES_URL) {
            Ok(()) => self.hub.event("Opened the releases page in the browser"),
            Err(e) => self.hub.event(format!("Could not open browser: {e}")),
        }
        let t = gnss::clock::now_local();
        self.settings.state.last_update_check =
            format!("{:04}-{:02}-{:02}", t.year, t.month, t.day);
    }

    /// Startup cadence: a due check PROMPTS in the event log. It must never
    /// launch the browser itself - an unrequested tab on boot reads as
    /// adware and steals focus from the diagnostic task; browser-opening
    /// stays attributed to the explicit "Check now" gesture.
    fn startup_update_check(&mut self) {
        let now = gnss::clock::now_local();
        let due = update_check_due(
            self.settings.app.check_updates,
            &self.settings.state.last_update_check,
            (now.year, now.month, now.day),
        );
        if due {
            self.hub
                .event("Update check due - use Options > Check now to open the releases page");
        }
    }

    // ------------------------------------------------------------------
    // Frame plumbing
    // ------------------------------------------------------------------

    fn drain_events(&mut self) {
        let now = Instant::now();
        while let Ok(ev) = self.rx.try_recv() {
            match &ev {
                AppEvent::Ntrip(NtripStatus::Stopped { .. }) => {
                    // Stopped is the worker's final post; the join is
                    // effectively immediate.
                    if let Some(h) = self.ntrip.take() {
                        h.join(Duration::from_secs(2));
                    }
                    self.insecure_tls_active = false;
                }
                // Corrections without a receiver leave the age/DOP readouts
                // empty; say why once per session, at the moment of success
                // when the user is watching the log.
                AppEvent::Ntrip(NtripStatus::Streaming) => {
                    if self.serial.is_none() && !self.receiver_hint_logged {
                        self.receiver_hint_logged = true;
                        // Two short lines: a single long sentence clips at
                        // the default window width, losing exactly the
                        // remedy half this explainer exists to deliver.
                        self.hub.event(
                            "Corrections are flowing; age, DOPs, elevation and \
                             speed come from the receiver",
                        );
                        self.hub.event(
                            "Connect a GPS receiver on the Serial side to \
                             populate those readouts",
                        );
                    }
                }
                AppEvent::SourcetableReady { host, port, table } => {
                    // The worker cannot log this: table-only jobs blank the
                    // mountpoint, so only the UI still knows which mount the
                    // profile actually targets.
                    self.log_mount_nmea_flag(host, *port, table);
                }
                _ => {}
            }
            self.state.apply(ev, now);
        }
        self.state.tick(now);
        // Advance the activity envelope. Done here - the only place
        // total_bytes mutates - so every burst registers exactly once.
        // (state.apply stamps last_rx from the same byte-total growth, so
        // caption and bar agree by construction.)
        self.activity = super::status_strip::advance_activity(
            self.activity,
            now.duration_since(self.activity_at),
            self.activity_total,
            self.state.ntrip.total_bytes,
        );
        self.activity_at = now;
        self.activity_total = self.state.ntrip.total_bytes;
        // Reap a self-terminated serial worker (failed open, USB unplug).
        // Checked every frame rather than on the Disconnected event: the
        // event can drain a beat before the OS finishes ending the thread,
        // and no later event would re-run a one-shot check - the UI would
        // stay stuck on a dead session until a manual Disconnect click.
        // A handle taken by toggle_serial never reaches here, so any
        // finished handle in the slot is a worker that ended itself.
        if let Some(h) = self.serial.take_if(|h| h.is_finished()) {
            h.join(Duration::from_millis(100));
        }
    }

    /// When a sourcetable lands for the caster the ACTIVE profile points at,
    /// log what it says about the profile's mountpoint - above all the NMEA
    /// flag, which decides whether this caster will silently starve a client
    /// that never sends GGA. Skipped for table-only fetches with no mount
    /// configured: there is nothing to check the table against.
    fn log_mount_nmea_flag(&mut self, host: &str, port: u16, table: &SourceTable) {
        let p = self.settings.active();
        let mount = p.mountpoint.trim();
        if mount.is_empty() || p.host.trim() != host || p.port != port {
            return;
        }
        // Case-insensitive fallback, exact match first: NTRIP Caster 1.0
        // serves mounts case-insensitively, so "rtcm32" against a listed
        // "RTCM32" streams fine and must not be reported as unlisted.
        let record = table
            .strs
            .iter()
            .find(|s| s.mountpoint == mount)
            .or_else(|| {
                table
                    .strs
                    .iter()
                    .find(|s| s.mountpoint.eq_ignore_ascii_case(mount))
            });
        let line = match record {
            Some(s) => {
                let format = if s.format.is_empty() {
                    "unknown"
                } else {
                    &s.format
                };
                if s.nmea_required {
                    format!(
                        "Sourcetable: mount {mount} - format {format}, \
                         nmea=1 (this stream requests NMEA GGA)"
                    )
                } else {
                    format!(
                        "Sourcetable: mount {mount} - format {format}, \
                         nmea=0 (no GGA needed)"
                    )
                }
            }
            None => format!("Sourcetable: mount {mount} is not listed by this caster"),
        };
        self.hub.event(line);
    }

    fn update_rate_meter(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.rate_sample.0).as_secs_f64();
        if elapsed >= 1.0 {
            let delta = self
                .state
                .ntrip
                .total_bytes
                .saturating_sub(self.rate_sample.1);
            self.rate_kbps = delta as f64 / elapsed / 1000.0;
            self.rate_sample = (now, self.state.ntrip.total_bytes);
        }
    }

    /// Track live geometry for the exit save, and - once, at boot - rescue a
    /// restored position that is no longer visible on ANY monitor (its
    /// monitor was unplugged). Positions that are merely negative or beyond
    /// the current monitor's size are normal multi-monitor coordinates and
    /// must be left alone, or the window could never live on a secondary
    /// monitor across restarts.
    fn track_geometry(&mut self, ctx: &egui::Context) {
        let (outer, inner, ppp) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.native_pixels_per_point)
        });
        if let Some(r) = outer
            && r.min.x > -20_000.0
            && r.min.y > -20_000.0
        {
            self.settings.window.pos = Some([r.min.x.round() as i32, r.min.y.round() as i32]);
        }
        if let Some(r) = inner
            && r.width() > 50.0
            && r.height() > 50.0
        {
            self.settings.window.size = [r.width(), r.height()];
        }
        if !self.geometry_rescued
            && let Some(r) = outer
        {
            self.geometry_rescued = true;
            // egui rects are logical points; the desktop metrics come back
            // in physical pixels, so compare in pixels.
            let scale = f64::from(ppp.unwrap_or(1.0)).max(0.1);
            let win_px = (
                (f64::from(r.min.x) * scale).round() as i32,
                (f64::from(r.min.y) * scale).round() as i32,
                (f64::from(r.width()) * scale).round() as i32,
                (f64::from(r.height()) * scale).round() as i32,
            );
            if let Some((screen, primary)) = desktop_px()
                && let Some((x, y)) = rescue_position(win_px, screen, primary)
            {
                let pos = egui::pos2((f64::from(x) / scale) as f32, (f64::from(y) / scale) as f32);
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
            }
        }
    }
}

/// True when the configured cadence says an update check is due; `today` is
/// local (year, month, day).
fn update_check_due(mode: CheckUpdates, last_check: &str, today: (i32, u8, u8)) -> bool {
    match mode {
        CheckUpdates::Off => false,
        CheckUpdates::Startup => true,
        CheckUpdates::Weekly => match parse_iso_date(last_check) {
            Some((y, m, d)) => {
                days_from_civil(today.0, today.1, today.2) - days_from_civil(y, m, d) >= 7
            }
            None => true,
        },
    }
}

/// Minimum window overlap with the desktop, physical px, in each axis:
/// enough title bar to grab. Smaller than any real monitor.
const GRAB_PX: i32 = 60;

/// (x, y, w, h) rectangle in physical pixels.
type PxRect = (i32, i32, i32, i32);
/// (w, h) extent in physical pixels.
type PxSize = (i32, i32);

/// Boot-time geometry rescue decision, pure for testing. `win` is the
/// restored outer rect and `screen` the virtual-desktop bounds (union of all
/// monitors), both in physical px; `primary` is the primary monitor's
/// extent, whose origin is always (0, 0) and always visible.
/// Returns the rescue position for a window that no longer meaningfully
/// intersects the desktop, or None for anything visible - including every
/// legitimate secondary-monitor position (negative, or beyond the primary's
/// extent), which MUST survive untouched.
fn rescue_position(win: PxRect, screen: PxRect, primary: PxSize) -> Option<(i32, i32)> {
    let (wx, wy, ww, wh) = win;
    let (sx, sy, sw, sh) = screen;
    let overlap_x = (wx.saturating_add(ww)).min(sx.saturating_add(sw)) - wx.max(sx);
    let overlap_y = (wy.saturating_add(wh)).min(sy.saturating_add(sh)) - wy.max(sy);
    if overlap_x >= GRAB_PX && overlap_y >= GRAB_PX {
        return None;
    }
    let (pw, ph) = primary;
    Some((
        wx.clamp(0, (pw - GRAB_PX).max(0)),
        wy.clamp(0, (ph - GRAB_PX).max(0)),
    ))
}

/// Desktop metrics in physical pixels: the virtual-screen bounds (x, y, w, h)
/// spanning all monitors, plus the primary monitor's (w, h). Hand FFI in the
/// project's audio.rs style. None off Windows - no rescue is attempted there
/// (the -20000 save filter already excludes minimized sentinels, and window
/// placement is compositor business on Wayland anyway).
#[cfg(windows)]
fn desktop_px() -> Option<(PxRect, PxSize)> {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetSystemMetrics(index: i32) -> i32;
    }
    const SM_CXSCREEN: i32 = 0;
    const SM_CYSCREEN: i32 = 1;
    const SM_XVIRTUALSCREEN: i32 = 76;
    const SM_YVIRTUALSCREEN: i32 = 77;
    const SM_CXVIRTUALSCREEN: i32 = 78;
    const SM_CYVIRTUALSCREEN: i32 = 79;
    // SAFETY: GetSystemMetrics is a pure integer query; no pointers cross.
    let (screen, primary) = unsafe {
        (
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            ),
            (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)),
        )
    };
    (screen.2 > 0 && screen.3 > 0 && primary.0 > 0 && primary.1 > 0).then_some((screen, primary))
}

#[cfg(not(windows))]
fn desktop_px() -> Option<(PxRect, PxSize)> {
    None
}

/// "YYYY-MM-DD" -> (y, m, d); anything else is None.
fn parse_iso_date(s: &str) -> Option<(i32, u8, u8)> {
    let mut it = s.split('-');
    let y: i32 = it.next()?.parse().ok()?;
    let m: u8 = it.next()?.parse().ok()?;
    let d: u8 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events();
        self.update_rate_meter();
        self.track_geometry(&ctx);
        // Heartbeat: age readouts and the rate meter tick without traffic -
        // the RX figures and stall readout stay live with zero mouse input.
        ctx.request_repaint_after(Duration::from_secs(1));
        // The activity bar's decay animates between data bursts; the finer
        // cadence runs only while there is a live session AND a visible
        // level to drain, so an idle app never burns CPU on it.
        if self.ntrip_busy() && self.activity > 0.01 {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        // Topbar rides one paper step above the base, per the guide.
        let topbar = egui::Frame::side_top_panel(&ctx.global_style()).fill(super::theme::PAPER_2);
        egui::Panel::top(egui::Id::new("top-strip"))
            .frame(topbar)
            .show(ui, |ui| {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    self.profile_strip(ui);
                });
                ui.add_space(2.0);
            });

        // Persistent security banner: crimson on the danger tint for as long
        // as the live connection skips certificate verification. It must be
        // impossible to miss and impossible to dismiss without disconnecting.
        if self.insecure_tls_active {
            egui::Panel::top(egui::Id::new("tls-insecure-banner"))
                .frame(super::theme::danger_banner())
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(
                            "TLS certificate verification is DISABLED - \
the caster's identity is not being checked",
                        )
                        .strong()
                        .color(super::theme::DANGER),
                    );
                });
        }

        egui::Panel::bottom(egui::Id::new("bottom-strip")).show(ui, |ui| {
            super::plot_panel::show(self, ui);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            // Two config columns; the NTRIP column carries the GGA disclosure.
            ui.columns(2, |cols| {
                super::serial_block::show(self, &mut cols[0]);
                super::ntrip_block::show(self, &mut cols[1]);
            });
            ui.add_space(4.0);
            super::status_strip::show(self, ui);
            ui.add_space(4.0);
            // One Surface: a one-line stream summary (renders only when live)
            // sits above the always-present bottom tab pane, which fills the
            // remaining height with the four diagnostic surfaces.
            super::stream_summary::show(self, ui);
            super::bottom_tabs::show(self, ui);
        });

        // Only genuine settings/informational surfaces stay as dialogs; every
        // live-data window folded into the bottom tabs or the GGA disclosure.
        super::options_dialog::show(self, &ctx);
        super::about::show(self, &ctx);
        super::profiles_dialog::show(self, &ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(h) = self.ntrip.take() {
            h.cancel_and_join(Duration::from_secs(2));
        }
        if let Some(h) = self.serial.take() {
            h.cancel_and_join(Duration::from_secs(2));
        }
        self.corr.set_active(false);
        if let Some(logger) = self.logger.take() {
            logger.shutdown();
        }
        if let Err(e) = settings::save(&self.settings, &paths::settings_file(&self.base)) {
            eprintln!("could not save settings on exit: {e}");
        }
    }
}

impl App {
    fn profile_strip(&mut self, ui: &mut egui::Ui) {
        ui.label("Profile:");
        let names: Vec<String> = self
            .settings
            .profiles
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let mut selected = self.settings.active_profile.clone();
        // Same connected-state lock the Profiles Manager enforces: a live
        // NTRIP worker was built from the ACTIVE profile, so switching it
        // mid-session would relabel the running stream with another
        // profile's host/mount and reset per-caster UI state under it.
        let busy = self.ntrip_busy();
        ui.add_enabled_ui(!busy, |ui| {
            egui::ComboBox::from_id_salt("profile-combo")
                .width(170.0)
                .selected_text(selected.clone())
                .show_ui(ui, |ui| {
                    for name in &names {
                        ui.selectable_value(&mut selected, name.clone(), name);
                    }
                });
        })
        .response
        .on_disabled_hover_text("Disconnect before switching profiles");
        if !busy && selected != self.settings.active_profile {
            self.settings.active_profile = selected;
            self.on_profile_switched();
        }
        if ui.button("Save").clicked() {
            self.save_settings();
        }
        if ui
            .button("Manage...")
            .on_hover_text("Add, clone, rename, and delete profiles")
            .clicked()
        {
            self.show_profiles = true;
        }

        // The protocol exchange and RTCM statistics are always-present bottom
        // tabs now, not windows to summon; only settings/about remain here.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("About").clicked() {
                self.show_about = true;
            }
            if ui.button("Options").clicked() {
                self.show_options = true;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{CheckUpdates, parse_iso_date, rescue_position, update_check_due};

    #[test]
    fn iso_date_parsing() {
        assert_eq!(parse_iso_date("2026-07-15"), Some((2026, 7, 15)));
        assert_eq!(parse_iso_date("2026-7-1"), Some((2026, 7, 1)));
        assert_eq!(parse_iso_date(""), None);
        assert_eq!(parse_iso_date("2026-13-01"), None);
        assert_eq!(parse_iso_date("2026-01-32"), None);
        assert_eq!(parse_iso_date("2026-01-01-01"), None);
        assert_eq!(parse_iso_date("yesterday"), None);
    }

    #[test]
    fn update_cadence_decision() {
        let today = (2026, 7, 16);
        assert!(!update_check_due(CheckUpdates::Off, "", today));
        assert!(!update_check_due(CheckUpdates::Off, "2020-01-01", today));
        assert!(update_check_due(CheckUpdates::Startup, "2026-07-16", today));
        // Weekly: never checked, exactly 7 days, and 6 days.
        assert!(update_check_due(CheckUpdates::Weekly, "", today));
        assert!(update_check_due(CheckUpdates::Weekly, "2026-07-09", today));
        assert!(!update_check_due(CheckUpdates::Weekly, "2026-07-10", today));
    }

    /// The parity contract row 20 ("window geometry restored") on multi-
    /// monitor desktops: legitimate secondary-monitor positions - negative
    /// coordinates or beyond the primary's size - must never be moved; only
    /// a window with no meaningful desktop intersection gets rescued.
    #[test]
    fn geometry_rescue_leaves_secondary_monitors_alone() {
        let primary = (1920, 1080);
        // Secondary left of primary: virtual screen spans x=[-1920, 1920).
        let screen = (-1920, 0, 3840, 1080);
        assert_eq!(
            rescue_position((-1500, 200, 760, 560), screen, primary),
            None
        );
        // Secondary right of primary.
        let screen = (0, 0, 3840, 1080);
        assert_eq!(
            rescue_position((2500, 100, 760, 560), screen, primary),
            None
        );
        // Secondary above primary.
        let screen = (0, -1080, 1920, 2160);
        assert_eq!(
            rescue_position((300, -900, 760, 560), screen, primary),
            None
        );
        // Maximized-style overhang (borders off every edge) is visible.
        let screen = (0, 0, 1920, 1080);
        assert_eq!(rescue_position((-8, -8, 1936, 1096), screen, primary), None);
    }

    #[test]
    fn geometry_rescue_recovers_invisible_windows() {
        let primary = (1920, 1080);
        let screen = (0, 0, 1920, 1080); // the secondary was unplugged
        // Window saved on a right-hand secondary that is gone.
        assert_eq!(
            rescue_position((2500, 100, 760, 560), screen, primary),
            Some((1860, 100))
        );
        // Window saved on a left-hand secondary that is gone.
        assert_eq!(
            rescue_position((-1500, 200, 760, 560), screen, primary),
            Some((0, 200))
        );
        // A sliver under the grab threshold still counts as lost.
        assert_eq!(
            rescue_position((-710, 200, 760, 560), screen, primary),
            Some((0, 200))
        );
        // Below every monitor.
        assert_eq!(
            rescue_position((400, 2000, 760, 560), screen, primary),
            Some((400, 1020))
        );
    }
}
