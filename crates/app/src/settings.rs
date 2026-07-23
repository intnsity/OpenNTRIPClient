//! Settings model, TOML persistence, and first-run legacy import.
//!
//! The settings.toml schema is frozen at `schema = 1`: fields for M3 features
//! (TLS, capture) already exist so the file format never has to migrate.
//! Loading is maximally tolerant - unknown keys are ignored, unknown enum
//! values fall back to defaults - and saving is atomic (tmp + rename) so a
//! crash mid-save can never destroy the previous good file.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::paths;

/// A closed string vocabulary stored as a TOML string. Unknown values
/// deserialize to the default instead of failing the whole file: a settings
/// file must never brick the app.
macro_rules! string_enum {
    ($(#[$meta:meta])* $vis:vis enum $name:ident { $($variant:ident => $text:literal),+ $(,)? } default $def:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis enum $name {
            $($variant),+
        }

        impl $name {
            $vis const ALL: &'static [$name] = &[$($name::$variant),+];

            $vis fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }

            $vis fn from_id(s: &str) -> Option<Self> {
                let s = s.trim();
                $(if s.eq_ignore_ascii_case($text) { return Some($name::$variant); })+
                None
            }
        }

        impl Default for $name {
            fn default() -> Self { $name::$def }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Ok($name::from_id(&s).unwrap_or_default())
            }
        }
    };
}

string_enum! {
    /// Update-check cadence. Weekly is the original client's default
    /// (original parity); a due check only ever PROMPTS in the event log -
    /// it never opens a browser by itself (see main_window).
    pub enum CheckUpdates {
        Off => "off",
        Startup => "startup",
        Weekly => "weekly",
    } default Weekly
}

string_enum! {
    pub enum ParityCfg {
        None => "none",
        Even => "even",
        Odd => "odd",
    } default None
}

string_enum! {
    pub enum NovatelFormat {
        Rtcm => "rtcm",
        Rtcmv3 => "rtcmv3",
        Cmr => "cmr",
        Rtca => "rtca",
        Omnistar => "omnistar",
        Novatel => "novatel",
    } default Rtcmv3
}

string_enum! {
    pub enum ProtocolCfg {
        Ntrip => "ntrip",
        Tcp => "tcp",
    } default Ntrip
}

string_enum! {
    pub enum GgaMode {
        Off => "off",
        WhenRequired => "when_required",
        Always => "always",
    } default WhenRequired
}

string_enum! {
    pub enum GgaSource {
        Manual => "manual",
        Receiver => "receiver",
    } default Receiver
}

string_enum! {
    /// Active bottom-pane tab (One Surface layout). UI-geometry class like
    /// window size: persisted by the unconditional exit save, ignored by
    /// [`Settings::persistable_eq`] so tab churn never lights the Save
    /// button. Unknown values fall back to Events like every string_enum.
    pub enum BottomTab {
        Events => "events",
        Conn => "conn",
        Stream => "stream",
        Sourcetable => "sourcetable",
    } default Events
}

string_enum! {
    /// The original client's display-slot identifiers, verbatim - they round
    /// trip through Settings.txt imports and the [display] table - plus two
    /// stream-side deltas (data-age, data-rate) fed by the caster stream
    /// rather than the receiver, so they work with no serial port at all.
    pub enum DisplayId {
        Age => "age",
        Hdop => "hdop",
        Vdop => "vdop",
        Pdop => "pdop",
        ElevationFeet => "elevation-feet",
        ElevationMeters => "elevation-meters",
        SpeedMph => "speed-mph",
        SpeedMphSmoothed => "speed-mph-smoothed",
        SpeedKmh => "speed-kmh",
        SpeedKmhSmoothed => "speed-kmh-smoothed",
        Heading => "heading",
        DataAge => "data-age",
        DataRate => "data-rate",
        Nothing => "nothing",
    } default Nothing
}

impl DisplayId {
    /// Human name for combo boxes; the id string stays the storage format.
    pub fn label(self) -> &'static str {
        match self {
            DisplayId::Age => "Correction Age",
            DisplayId::Hdop => "HDOP",
            DisplayId::Vdop => "VDOP",
            DisplayId::Pdop => "PDOP",
            DisplayId::ElevationFeet => "Elevation (ft)",
            DisplayId::ElevationMeters => "Elevation (m)",
            DisplayId::SpeedMph => "Speed (mph)",
            DisplayId::SpeedMphSmoothed => "Speed (mph, smoothed)",
            DisplayId::SpeedKmh => "Speed (km/h)",
            DisplayId::SpeedKmhSmoothed => "Speed (km/h, smoothed)",
            DisplayId::Heading => "Heading",
            DisplayId::DataAge => "Data Age (stream)",
            DisplayId::DataRate => "Data Rate (stream)",
            DisplayId::Nothing => "Nothing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema: u32,
    pub active_profile: String,
    pub app: AppCfg,
    pub window: WindowCfg,
    pub display: DisplayCfg,
    pub serial: SerialCfg,
    pub state: StateCfg,
    pub profiles: Vec<Profile>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            schema: 1,
            active_profile: "Default".to_string(),
            app: AppCfg::default(),
            window: WindowCfg::default(),
            display: DisplayCfg::default(),
            serial: SerialCfg::default(),
            state: StateCfg::default(),
            profiles: vec![Profile::default()],
        }
    }
}

impl Settings {
    pub fn active_index(&self) -> usize {
        self.profiles
            .iter()
            .position(|p| p.name == self.active_profile)
            .unwrap_or(0)
    }

    pub fn active(&self) -> &Profile {
        &self.profiles[self.active_index()]
    }

    pub fn active_mut(&mut self) -> &mut Profile {
        let i = self.active_index();
        &mut self.profiles[i]
    }

    /// Equality over the fields a user deliberately edits - the active
    /// profile pointer, the profiles themselves, and the app/display/serial
    /// config - ignoring `schema` and the app-managed `[window]`/`[state]`
    /// tables. Drives the profile strip's stateful Save button: window
    /// geometry, tab and disclosure churn, and resume-intent flags persist
    /// via the unconditional exit save and must never light "unsaved".
    pub fn persistable_eq(&self, other: &Settings) -> bool {
        self.active_profile == other.active_profile
            && self.app == other.app
            && self.display == other.display
            && self.serial == other.serial
            && self.profiles == other.profiles
    }

    /// Re-establish invariants a hand-edited or partial file may break.
    /// Every accessor above may then index without checking.
    fn normalize(&mut self) {
        self.schema = 1;
        if self.profiles.is_empty() {
            self.profiles.push(Profile::default());
        }
        if !self.profiles.iter().any(|p| p.name == self.active_profile) {
            self.active_profile = self.profiles[0].name.clone();
        }
        for p in &mut self.profiles {
            if !(p.ntrip_version == 1 || p.ntrip_version == 2) {
                p.ntrip_version = 1;
            }
        }
        if !(self.serial.data_bits == 7 || self.serial.data_bits == 8) {
            self.serial.data_bits = 8;
        }
        if !(self.serial.stop_bits == 1 || self.serial.stop_bits == 2) {
            self.serial.stop_bits = 1;
        }
        if !matches!(self.serial.novatel_rate_hz, 1 | 5 | 10) {
            self.serial.novatel_rate_hz = 5;
        }
        let [w, h] = self.window.size;
        // Floor tracks the One Surface layout's real minimum (main.rs
        // additionally clamps the startup size to its own layout floor);
        // the ceiling merely rejects absurd hand edits.
        self.window.size = [w.clamp(700.0, 4000.0), h.clamp(480.0, 3000.0)];
    }
}

/// The frozen seven-key `[app]` table - the M2 contract enumerates exactly
/// these user-facing keys, pinned by a test. App-managed bookkeeping goes in
/// [`StateCfg`], never here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppCfg {
    pub check_updates: CheckUpdates,
    pub audio_alert_file: String,
    pub write_event_log: bool,
    pub write_nmea_log: bool,
    /// M3 raw-capture wiring; the field exists now so the schema never moves.
    pub capture_corrections: bool,
    pub auto_reconnect: bool,
    pub max_reconnect_attempts: u32,
}

impl Default for AppCfg {
    fn default() -> Self {
        AppCfg {
            check_updates: CheckUpdates::Weekly,
            audio_alert_file: String::new(),
            // Deliberate delta from the original, which shipped with file
            // logging off: a diagnostic tool that writes no diagnostics by
            // default failed in the field tonight - the session that needed
            // evidence left none behind. Both daily file logs default ON;
            // the Options toggles remain for turning them off.
            write_event_log: true,
            write_nmea_log: true,
            capture_corrections: false,
            auto_reconnect: true,
            max_reconnect_attempts: 10_000,
        }
    }
}

/// Bookkeeping the app writes for itself - kept OUTSIDE the frozen `[app]`
/// config table so app-managed state never grows the contracted surface.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StateCfg {
    /// "YYYY-MM-DD" of the last update check; empty = never. Backs the
    /// weekly cadence and the Options dialog's "last checked" note.
    pub last_update_check: String,
    /// Resume intent, stamped by the exit save: the app closed with a live
    /// serial session, so the next boot may pick the session back up.
    /// Consumed by the profile-strip wave; ignored by `persistable_eq`.
    pub serial_connected: bool,
    /// Resume intent for the NTRIP side; see `serial_connected`.
    pub ntrip_connected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowCfg {
    /// Outer position in points; None until the first clean exit records one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pos: Option<[i32; 2]>,
    pub size: [f32; 2],
    pub graph_open: bool,
    /// Active bottom-pane tab (One Surface layout).
    pub tab: BottomTab,
    /// GGA section disclosure state in the NTRIP block.
    pub gga_open: bool,
}

impl Default for WindowCfg {
    fn default() -> Self {
        WindowCfg {
            pos: None,
            // Sized for the One Surface layout: two config columns plus the
            // bottom tab pane at rest. Existing users keep their saved size.
            size: [920.0, 680.0],
            graph_open: false,
            tab: BottomTab::Events,
            gga_open: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayCfg {
    pub center: DisplayId,
    pub right: DisplayId,
}

impl Default for DisplayCfg {
    fn default() -> Self {
        // Default readouts: elevation in feet (from the receiver) and the
        // base-stream data age - seconds since the last correction byte, the
        // one freshness signal that reads even with no receiver attached, so a
        // fresh install shows something useful the moment corrections flow. A
        // saved settings.toml overrides both per the user's own preference.
        DisplayCfg {
            center: DisplayId::ElevationFeet,
            right: DisplayId::DataAge,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialCfg {
    pub port: String,
    pub baud: u32,
    pub data_bits: u8,
    pub stop_bits: u8,
    pub parity: ParityCfg,
    pub novatel_autoconfig: bool,
    pub novatel_format: NovatelFormat,
    pub novatel_rate_hz: u8,
}

impl Default for SerialCfg {
    fn default() -> Self {
        SerialCfg {
            port: String::new(),
            baud: 115_200,
            data_bits: 8,
            stop_bits: 1,
            parity: ParityCfg::None,
            novatel_autoconfig: false,
            novatel_format: NovatelFormat::Rtcmv3,
            novatel_rate_hz: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    pub host: String,
    pub port: u16,
    /// M3: TLS transport. Persisted now, surfaced as a disabled checkbox.
    pub tls: bool,
    pub allow_invalid_certs: bool,
    pub ntrip_version: u8,
    pub protocol: ProtocolCfg,
    pub username: String,
    pub password: String,
    pub mountpoint: String,
    pub gga_mode: GgaMode,
    pub gga_source: GgaSource,
    pub manual_lat: f64,
    pub manual_lon: f64,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            name: "Default".to_string(),
            host: String::new(),
            port: 2101,
            tls: false,
            allow_invalid_certs: false,
            ntrip_version: 1,
            protocol: ProtocolCfg::Ntrip,
            username: String::new(),
            password: String::new(),
            mountpoint: String::new(),
            // Defaults revisited after the CHC APIS field failure (0.2.1):
            // APIS casters answer "ICY 200 OK" and then hold the stream
            // until ANY GGA arrives, and their base-SN mounts are absent
            // from their own sourcetable - so when_required's old
            // unknown-requirement fallback of "send nothing" deadlocked the
            // default profile forever ("waiting for data" plus a silence
            // timeout on loop). when_required now assumes "required" when
            // the table cannot answer; only an explicit nmea=0 row
            // suppresses sending. The fabricate-at-0,0 hazard that once
            // justified the silent fallback is closed at the send site
            // instead: an unset manual position (exactly 0,0) sends nothing
            // and says so, rather than inventing a confident fix on Null
            // Island for a VRS to chew on. Receiver source stays the
            // default: field rovers have GPS hardware attached, and its
            // misses are explained in the event log.
            gga_mode: GgaMode::WhenRequired,
            gga_source: GgaSource::Receiver,
            manual_lat: 0.0,
            manual_lon: 0.0,
        }
    }
}

pub fn load(path: &Path) -> Result<Settings, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut settings: Settings =
        toml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?;
    settings.normalize();
    Ok(settings)
}

/// Atomic save: the previous good file survives any crash mid-write because
/// the content lands in `settings.toml.tmp` first and replaces the real file
/// in a single rename.
pub fn save(settings: &Settings, path: &Path) -> Result<(), String> {
    let text = toml::to_string_pretty(settings).map_err(|e| format!("serialize settings: {e}"))?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", tmp.display()))
}

/// Startup entry point: load settings.toml, or - on the very first run only -
/// import the original client's files, or fall back to defaults. The second
/// return value is event-log lines describing what happened.
pub fn load_or_import(base: &Path) -> (Settings, Vec<String>) {
    let path = paths::settings_file(base);
    if path.exists() {
        return match load(&path) {
            Ok(s) => (s, Vec::new()),
            Err(e) => {
                // The GUI saves unconditionally on exit, which would replace
                // the user's hand-edited file (profiles, passwords) with
                // defaults after a single typo. Preserve the evidence FIRST,
                // before any save can clobber it.
                let mut log = vec![format!(
                    "Could not load settings.toml ({e}); using defaults"
                )];
                let bad = path.with_extension("toml.bad");
                match std::fs::copy(&path, &bad) {
                    Ok(_) => log.push(format!(
                        "The unparseable file was preserved as {}; \
fix it and rename it back to settings.toml to recover your profiles",
                        bad.display()
                    )),
                    Err(copy_err) => log.push(format!(
                        "Could not preserve the unparseable file as {}: {copy_err}",
                        bad.display()
                    )),
                }
                (Settings::default(), log)
            }
        };
    }
    if let Some((settings, mut log)) = import_legacy(base) {
        match save(&settings, &path) {
            Ok(()) => log.push("Imported settings saved to settings.toml".to_string()),
            Err(e) => log.push(format!("Could not save imported settings: {e}")),
        }
        return (settings, log);
    }
    (Settings::default(), Vec::new())
}

// ---------------------------------------------------------------------------
// Legacy import (original Lefebure client files)
// ---------------------------------------------------------------------------

/// Split legacy `Key=Value` lines with the original parser's exact rules:
/// trim the line; skip lines shorter than 3 chars; skip lines starting '#';
/// require '=' at index >= 2; split at the FIRST '='. Keys and values are
/// returned verbatim; matching is the caller's (case-insensitive) job.
fn legacy_pairs(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.len() < 3 || line.starts_with('#') {
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        if eq < 2 {
            continue;
        }
        pairs.push((line[..eq].to_string(), line[eq + 1..].to_string()));
    }
    pairs
}

fn legacy_bool(value: &str) -> bool {
    let v = value.trim();
    v.eq_ignore_ascii_case("yes") || v.eq_ignore_ascii_case("true") || v == "1"
}

/// Import `Settings.txt` + `ntripconfig.txt` if either exists. Legacy files
/// are read only, never modified. Returns None when there is nothing to
/// import. A legacy `sourcetable.dat` is deliberately ignored: the client no
/// longer persists sourcetables (a cached table can only give a stale answer
/// to a "is this caster alive now" question), so it is fetched fresh instead.
pub fn import_legacy(base: &Path) -> Option<(Settings, Vec<String>)> {
    let ntripconfig = std::fs::read_to_string(base.join("ntripconfig.txt")).ok();
    let settings_txt = std::fs::read_to_string(base.join("Settings.txt")).ok();
    if ntripconfig.is_none() && settings_txt.is_none() {
        return None;
    }

    let mut s = Settings {
        active_profile: "Imported".to_string(),
        profiles: vec![Profile {
            name: "Imported".to_string(),
            ..Profile::default()
        }],
        ..Settings::default()
    };
    let mut log = Vec::new();

    if let Some(text) = &ntripconfig {
        log.push("Importing legacy ntripconfig.txt".to_string());
        for (key, value) in legacy_pairs(text) {
            import_ntripconfig_key(&mut s, &key, &value, &mut log);
        }
    }
    if let Some(text) = &settings_txt {
        log.push("Importing legacy Settings.txt".to_string());
        for (key, value) in legacy_pairs(text) {
            import_settings_key(&mut s, &key, &value, &mut log);
        }
    }

    s.normalize();
    Some((s, log))
}

fn import_ntripconfig_key(s: &mut Settings, key: &str, value: &str, log: &mut Vec<String>) {
    let p = &mut s.profiles[0];
    match key.to_ascii_lowercase().as_str() {
        "ntrip caster" => p.host = value.trim().to_string(),
        "ntrip caster port" => match value.trim().parse::<u16>() {
            Ok(n) => p.port = n,
            Err(_) => {
                log.push(format!(
                    "Ignored invalid value in ntripconfig.txt: {key}={value}"
                ));
                return;
            }
        },
        "ntrip username" => p.username = value.to_string(),
        "ntrip password" => p.password = value.to_string(),
        "ntrip mountpoint" => p.mountpoint = value.trim().to_string(),
        _ => {
            log.push(format!(
                "Ignored unknown key in ntripconfig.txt: {key}={value}"
            ));
            return;
        }
    }
    // Passwords stay out of the event log; everything else is shown verbatim.
    if key.eq_ignore_ascii_case("ntrip password") {
        log.push("Imported from ntripconfig.txt: NTRIP Password=****".to_string());
    } else {
        log.push(format!("Imported from ntripconfig.txt: {key}={value}"));
    }
}

fn import_settings_key(s: &mut Settings, key: &str, value: &str, log: &mut Vec<String>) {
    let v = value.trim();
    let invalid = || format!("Ignored invalid value in Settings.txt: {key}={value}");
    match key.to_ascii_lowercase().as_str() {
        "serial port number" => match v.parse::<u32>() {
            Ok(n) => s.serial.port = format!("COM{n}"),
            Err(_) => return log.push(invalid()),
        },
        "serial port speed" => match v.parse::<u32>() {
            Ok(n) => s.serial.baud = n,
            Err(_) => return log.push(invalid()),
        },
        "serial port data bits" => match v.parse::<u8>() {
            Ok(n @ (7 | 8)) => s.serial.data_bits = n,
            _ => return log.push(invalid()),
        },
        "serial port stop bits" => match v.parse::<u8>() {
            // The original persisted .NET's StopBits enum, whose None
            // variant (0) drove the port with one stop bit; import it as
            // the 1 it behaved as instead of rejecting the whole line.
            Ok(0) => s.serial.stop_bits = 1,
            Ok(n @ (1 | 2)) => s.serial.stop_bits = n,
            _ => return log.push(invalid()),
        },
        "display center" => match DisplayId::from_id(v) {
            Some(id) => s.display.center = id,
            None => return log.push(invalid()),
        },
        "display right" => match DisplayId::from_id(v) {
            Some(id) => s.display.right = id,
            None => return log.push(invalid()),
        },
        "audio alert file" => s.app.audio_alert_file = v.to_string(),
        "write events to file" => s.app.write_event_log = legacy_bool(v),
        "write nmea to file" => s.app.write_nmea_log = legacy_bool(v),
        "protocol" => {
            s.profiles[0].protocol = if v.eq_ignore_ascii_case("rawtcpip") {
                ProtocolCfg::Tcp
            } else {
                ProtocolCfg::Ntrip
            };
        }
        "ntrip use manual gga" => {
            s.profiles[0].gga_source = if legacy_bool(v) {
                GgaSource::Manual
            } else {
                GgaSource::Receiver
            };
        }
        "ntrip manual latitude" => match v.parse::<f64>() {
            Ok(x) => s.profiles[0].manual_lat = x,
            Err(_) => return log.push(invalid()),
        },
        "ntrip manual longitude" => match v.parse::<f64>() {
            Ok(x) => s.profiles[0].manual_lon = x,
            Err(_) => return log.push(invalid()),
        },
        "ntrip only send gga once" => {
            return log.push(
                "Note: 'NTRIP Only Send GGA Once' is handled automatically now; nothing imported"
                    .to_string(),
            );
        }
        "receiver type" => s.serial.novatel_autoconfig = v.eq_ignore_ascii_case("novatel"),
        "receiver correction format" => match NovatelFormat::from_id(v) {
            Some(f) => s.serial.novatel_format = f,
            None => return log.push(invalid()),
        },
        "receiver message rate" => match v.parse::<u8>() {
            Ok(n @ (1 | 5 | 10)) => s.serial.novatel_rate_hz = n,
            _ => return log.push(invalid()),
        },
        "check for updates" => {
            s.app.check_updates = if legacy_bool(v) {
                CheckUpdates::Startup
            } else {
                CheckUpdates::Off
            };
        }
        "check for updates interval" => {
            // The original's cadence word. Only its weekly cadence carries
            // over; every other value maps to off (we deliberately dropped
            // the self-updater, so there is nothing else to preserve).
            s.app.check_updates = if v.eq_ignore_ascii_case("weekly") {
                CheckUpdates::Weekly
            } else {
                CheckUpdates::Off
            };
        }
        "last checked for updates" => {
            // Imported silently: pure bookkeeping that only feeds the weekly
            // cadence (a non-ISO stamp simply makes the next check due), so
            // an "Imported ..." event line for it would be noise.
            s.state.last_update_check = v.to_string();
            return;
        }
        "serial should be connected" => s.state.serial_connected = legacy_bool(v),
        "ntrip should be connected" => s.state.ntrip_connected = legacy_bool(v),
        _ => {
            return log.push(format!(
                "Ignored unknown key in Settings.txt: {key}={value}"
            ));
        }
    }
    log.push(format!("Imported from Settings.txt: {key}={value}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Fixture dir under the OS tempdir - never the repo.
    fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "open-ntrip-client-settings-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn default_roundtrips_through_toml() {
        let d = tempdir("roundtrip");
        let path = paths::settings_file(&d);
        let mut original = Settings::default();
        original.window.pos = Some([120, 80]);
        original.profiles[0].host = "caster.example.com".to_string();
        original.profiles[0].password = "hunter2".to_string();
        save(&original, &path).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, original);
        assert!(!path.with_extension("toml.tmp").exists(), "tmp cleaned up");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unknown_keys_ignored_on_load_dropped_on_save() {
        let d = tempdir("unknown");
        let path = paths::settings_file(&d);
        let text = r#"
schema = 1
active_profile = "Default"
mystery_future_key = "kept calm"

[app]
write_event_log = true
another_unknown = 42

[[profiles]]
name = "Default"
host = "h.example"
unknown_profile_key = "x"
"#;
        std::fs::write(&path, text).unwrap();
        let s = load(&path).unwrap();
        assert!(s.app.write_event_log);
        assert_eq!(s.profiles[0].host, "h.example");
        save(&s, &path).unwrap();
        let round = std::fs::read_to_string(&path).unwrap();
        assert!(!round.contains("mystery_future_key"));
        assert!(!round.contains("another_unknown"));
        assert!(!round.contains("unknown_profile_key"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unknown_enum_values_fall_back_to_defaults() {
        let s: Settings = toml::from_str(
            r#"
[display]
center = "flux-capacitor"
right = "age"

[serial]
parity = "marks"

[[profiles]]
name = "P"
gga_mode = "sometimes"
"#,
        )
        .unwrap();
        assert_eq!(s.display.center, DisplayId::Nothing);
        assert_eq!(s.display.right, DisplayId::Age);
        assert_eq!(s.serial.parity, ParityCfg::None);
        assert_eq!(s.profiles[0].gga_mode, GgaMode::WhenRequired);
    }

    /// The M2 contract froze [app] at exactly these seven keys; anything the
    /// app tracks for itself must live elsewhere ([state]). A failure here
    /// means the frozen schema drifted.
    #[test]
    fn app_table_stays_the_frozen_seven_keys() {
        let text = toml::to_string_pretty(&Settings::default()).unwrap();
        let value: toml::Value = toml::from_str(&text).unwrap();
        let app = value["app"].as_table().unwrap();
        let mut keys: Vec<&str> = app.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "audio_alert_file",
                "auto_reconnect",
                "capture_corrections",
                "check_updates",
                "max_reconnect_attempts",
                "write_event_log",
                "write_nmea_log",
            ]
        );
        // The moved bookkeeping key lives in [state] now, alongside the
        // resume-intent flags the exit save stamps.
        assert!(value["state"].get("last_update_check").is_some());
        assert!(value["state"].get("serial_connected").is_some());
        assert!(value["state"].get("ntrip_connected").is_some());
    }

    /// A hand-editing typo must never cost the user their profiles: the
    /// unparseable file is preserved BEFORE defaults are handed out, so the
    /// GUI's unconditional exit save cannot destroy the only copy.
    #[test]
    fn unparseable_file_is_preserved_before_defaults_can_clobber_it() {
        let d = tempdir("badfile");
        let path = paths::settings_file(&d);
        let broken = "schema = 1\n[[profiles]]\nname = \"Field rig\"\npassword = oops-no-quotes\n";
        std::fs::write(&path, broken).unwrap();

        let (s, log) = load_or_import(&d);
        assert_eq!(s, Settings::default(), "defaults on parse failure");
        let bad = path.with_extension("toml.bad");
        assert_eq!(
            std::fs::read_to_string(&bad).unwrap(),
            broken,
            "byte-exact preservation of the unparseable file"
        );
        assert!(
            log.iter().any(|l| l.contains("settings.toml.bad")),
            "the preserved path must be surfaced: {log:?}"
        );

        // Simulate the GUI's on_exit save over the original file: the user's
        // data must still be recoverable from the .bad copy.
        save(&s, &path).unwrap();
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), broken);
        assert!(
            std::fs::read_to_string(&path).unwrap().contains("Default"),
            "settings.toml itself now holds defaults"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_file_yields_defaults_with_one_profile() {
        let d = tempdir("missing");
        let (s, log) = load_or_import(&d);
        assert_eq!(s, Settings::default());
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.profiles[0].name, "Default");
        assert!(log.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn normalize_repairs_hostile_values() {
        let mut s: Settings = toml::from_str(
            r#"
active_profile = "gone"
profiles = []

[serial]
data_bits = 9
stop_bits = 3
novatel_rate_hz = 7

[window]
size = [10.0, 9999.0]
"#,
        )
        .unwrap();
        s.normalize();
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.active_profile, "Default");
        assert_eq!(s.serial.data_bits, 8);
        assert_eq!(s.serial.stop_bits, 1);
        assert_eq!(s.serial.novatel_rate_hz, 5);
        assert_eq!(s.window.size, [700.0, 3000.0]);
    }

    #[test]
    fn display_id_parsing_covers_all_legacy_ids() {
        let ids = [
            ("age", DisplayId::Age),
            ("hdop", DisplayId::Hdop),
            ("vdop", DisplayId::Vdop),
            ("pdop", DisplayId::Pdop),
            ("elevation-feet", DisplayId::ElevationFeet),
            ("elevation-meters", DisplayId::ElevationMeters),
            ("speed-mph", DisplayId::SpeedMph),
            ("speed-mph-smoothed", DisplayId::SpeedMphSmoothed),
            ("speed-kmh", DisplayId::SpeedKmh),
            ("speed-kmh-smoothed", DisplayId::SpeedKmhSmoothed),
            ("heading", DisplayId::Heading),
            ("data-age", DisplayId::DataAge),
            ("data-rate", DisplayId::DataRate),
            ("nothing", DisplayId::Nothing),
        ];
        for (text, id) in ids {
            assert_eq!(DisplayId::from_id(text), Some(id), "{text}");
            assert_eq!(id.as_str(), text);
        }
        assert_eq!(DisplayId::from_id("HDOP"), Some(DisplayId::Hdop));
        assert_eq!(DisplayId::from_id(" age "), Some(DisplayId::Age));
        assert_eq!(DisplayId::from_id("altitude"), None);
        assert_eq!(DisplayId::ALL.len(), 14);
    }

    /// The defaults a fresh profile ships with, pinned as a deliberate
    /// decision (see Profile::default's comment): when_required GGA (which
    /// sends on unknown requirements - the APIS fix), receiver source, and
    /// outage-riding reconnect on. If this test fails, someone changed
    /// field behavior for every new user - re-read the APIS root-cause
    /// notes before accepting that.
    #[test]
    fn new_profile_and_reconnect_defaults_are_deliberate() {
        let p = Profile::default();
        assert_eq!(p.gga_mode, GgaMode::WhenRequired);
        assert_eq!(p.gga_source, GgaSource::Receiver);
        let a = AppCfg::default();
        assert!(a.auto_reconnect, "outage riding is the default");
        assert_eq!(a.max_reconnect_attempts, 10_000);
    }

    /// Honest-diagnostics defaults, pinned as a deliberate decision: a
    /// diagnostic tool that writes no diagnostics by default failed in the
    /// field - the session that needed evidence left none - so both daily
    /// file logs default ON. Display slots and the update cadence are
    /// original parity.
    #[test]
    fn defaults_write_diagnostics_and_readouts() {
        let a = AppCfg::default();
        assert!(a.write_event_log, "event log must default ON");
        assert!(a.write_nmea_log, "NMEA log must default ON");
        assert_eq!(a.check_updates, CheckUpdates::Weekly, "original parity");
        // Intentional deviation from the Lefebure defaults: elevation in feet
        // and the base-stream data age (the latter reads with no receiver).
        let d = DisplayCfg::default();
        assert_eq!(
            d.center,
            DisplayId::ElevationFeet,
            "default readout: elevation (ft)"
        );
        assert_eq!(
            d.right,
            DisplayId::DataAge,
            "default readout: base-stream data age"
        );
        let st = StateCfg::default();
        assert!(!st.serial_connected, "no resume intent until an exit save");
        assert!(!st.ntrip_connected);
        assert!(st.last_update_check.is_empty());
    }

    #[test]
    fn window_defaults_match_one_surface_layout() {
        let w = WindowCfg::default();
        assert_eq!(w.size, [920.0, 680.0]);
        assert_eq!(w.tab, BottomTab::Events);
        assert!(!w.gga_open);
        assert!(!w.graph_open);
        assert_eq!(w.pos, None);
    }

    #[test]
    fn bottom_tab_round_trips_and_unknown_falls_back_to_events() {
        let ids = [
            ("events", BottomTab::Events),
            ("conn", BottomTab::Conn),
            ("stream", BottomTab::Stream),
            ("sourcetable", BottomTab::Sourcetable),
        ];
        for (text, tab) in ids {
            assert_eq!(BottomTab::from_id(text), Some(tab), "{text}");
            assert_eq!(tab.as_str(), text);
        }
        assert_eq!(BottomTab::ALL.len(), 4);
        // Through the TOML layer, alongside the disclosure flag.
        let mut s = Settings::default();
        s.window.tab = BottomTab::Stream;
        s.window.gga_open = true;
        let text = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back.window.tab, BottomTab::Stream);
        assert!(back.window.gga_open);
        // A future or hand-edited tab name must not brick the file.
        let s: Settings = toml::from_str("[window]\ntab = \"dashboard\"\n").unwrap();
        assert_eq!(s.window.tab, BottomTab::Events);
    }

    /// persistable_eq drives the profile strip's stateful Save button: only
    /// user-authored config may light it. Geometry, tab/disclosure churn,
    /// [state] bookkeeping and the schema stamp are exit-save territory.
    #[test]
    fn persistable_eq_ignores_window_state_and_schema() {
        let base = Settings::default();
        let mut churn = base.clone();
        churn.schema = 999;
        churn.window.pos = Some([5, 5]);
        churn.window.size = [1000.0, 800.0];
        churn.window.graph_open = true;
        churn.window.tab = BottomTab::Sourcetable;
        churn.window.gga_open = true;
        churn.state.last_update_check = "2026-07-16".to_string();
        churn.state.serial_connected = true;
        churn.state.ntrip_connected = true;
        assert!(
            base.persistable_eq(&churn),
            "UI churn must not read as edits"
        );
        assert!(churn.persistable_eq(&base), "symmetric");

        type Edit = (&'static str, fn(&mut Settings));
        let edits: [Edit; 6] = [
            ("profile field", |s| {
                s.profiles[0].host = "caster.example".to_string();
            }),
            ("profile added", |s| s.profiles.push(Profile::default())),
            ("active pointer", |s| s.active_profile = "Other".to_string()),
            ("app cfg", |s| {
                s.app.write_event_log = !s.app.write_event_log
            }),
            ("display slot", |s| s.display.center = DisplayId::Hdop),
            ("serial cfg", |s| s.serial.baud = 9600),
        ];
        for (name, edit) in edits {
            let mut s = base.clone();
            edit(&mut s);
            assert!(!base.persistable_eq(&s), "{name} must read as an edit");
        }
    }

    #[test]
    fn legacy_parser_rules_exact() {
        let text = "\
# comment line skipped\n\
ab\n\
x=1\n\
ab=cd\n\
Key=first=second\n\
  Padded=value  \n\
no equals sign here\n";
        let pairs = legacy_pairs(text);
        assert_eq!(
            pairs,
            vec![
                ("ab".to_string(), "cd".to_string()),
                ("Key".to_string(), "first=second".to_string()),
                ("Padded".to_string(), "value".to_string()),
            ]
        );
    }

    #[test]
    fn import_happy_path_both_files() {
        let d = tempdir("import-happy");
        std::fs::write(
            d.join("ntripconfig.txt"),
            "NTRIP Caster=rtk2go.com\nNTRIP Caster Port=2101\nNTRIP Username=alice\n\
NTRIP Password=secret\nNTRIP MountPoint=MOUNT1\n",
        )
        .unwrap();
        std::fs::write(
            d.join("Settings.txt"),
            "Serial Port Number=4\nSerial Port Speed=57600\nSerial Port Data Bits=7\n\
Serial Port Stop Bits=2\nDisplay Center=vdop\nDisplay Right=speed-mph-smoothed\n\
Audio Alert File=alert.wav\nWrite Events To File=yes\nWrite NMEA To File=no\n\
Protocol=RawTCPIP\nNTRIP Use Manual GGA=yes\nNTRIP Manual Latitude=45.5\n\
NTRIP Manual Longitude=-122.6\nReceiver Type=NovAtel\nReceiver Correction Format=cmr\n\
Receiver Message Rate=10\n",
        )
        .unwrap();
        std::fs::write(d.join("sourcetable.dat"), "STR;M;\nENDSOURCETABLE\n").unwrap();

        let (s, log) = import_legacy(&d).expect("import should trigger");
        assert_eq!(s.active_profile, "Imported");
        let p = &s.profiles[0];
        assert_eq!(p.name, "Imported");
        assert_eq!(p.host, "rtk2go.com");
        assert_eq!(p.port, 2101);
        assert_eq!(p.username, "alice");
        assert_eq!(p.password, "secret");
        assert_eq!(p.mountpoint, "MOUNT1");
        assert_eq!(p.protocol, ProtocolCfg::Tcp);
        assert_eq!(p.gga_source, GgaSource::Manual);
        assert!((p.manual_lat - 45.5).abs() < 1e-12);
        assert!((p.manual_lon + 122.6).abs() < 1e-12);
        assert_eq!(s.serial.port, "COM4");
        assert_eq!(s.serial.baud, 57_600);
        assert_eq!(s.serial.data_bits, 7);
        assert_eq!(s.serial.stop_bits, 2);
        assert!(s.serial.novatel_autoconfig);
        assert_eq!(s.serial.novatel_format, NovatelFormat::Cmr);
        assert_eq!(s.serial.novatel_rate_hz, 10);
        assert_eq!(s.display.center, DisplayId::Vdop);
        assert_eq!(s.display.right, DisplayId::SpeedMphSmoothed);
        assert_eq!(s.app.audio_alert_file, "alert.wav");
        assert!(s.app.write_event_log);
        assert!(!s.app.write_nmea_log);

        // Every imported line produced exactly one log line, passwords masked.
        assert!(log.iter().any(|l| l.contains("NTRIP Caster=rtk2go.com")));
        assert!(log.iter().any(|l| l.contains("NTRIP Password=****")));
        assert!(!log.iter().any(|l| l.contains("secret")));
        assert!(log.iter().any(|l| l.contains("Serial Port Number=4")));

        // sourcetable.dat is present but deliberately ignored: no disk cache
        // is created and nothing about it reaches the import log.
        assert!(
            !d.join("SourceTables").exists(),
            "no sourcetable disk cache"
        );
        assert!(!log.iter().any(|l| l.contains("sourcetable.dat")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn import_keys_case_insensitive_and_unknown_logged() {
        let d = tempdir("import-case");
        std::fs::write(
            d.join("Settings.txt"),
            "SERIAL PORT NUMBER=7\nwrite events to file=YES\nMystery Key=abc\n\
NTRIP Only Send GGA Once=yes\nSerial Port Speed=oops\n",
        )
        .unwrap();
        let (s, log) = import_legacy(&d).expect("import should trigger");
        assert_eq!(s.serial.port, "COM7");
        assert!(s.app.write_event_log);
        assert_eq!(s.serial.baud, 115_200, "invalid speed keeps default");
        assert!(
            log.iter()
                .any(|l| l.contains("unknown key") && l.contains("Mystery Key=abc"))
        );
        assert!(log.iter().any(|l| l.contains("Only Send GGA Once")));
        assert!(
            log.iter()
                .any(|l| l.contains("invalid value") && l.contains("Serial Port Speed=oops"))
        );
        // No ntripconfig: the imported profile exists with defaults.
        assert_eq!(s.profiles[0].name, "Imported");
        assert_eq!(s.profiles[0].host, "");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The original's real update/resume keys in Settings.txt: the weekly
    /// cadence carries over, the last-checked stamp imports silently, and
    /// the should-be-connected flags land in the new [state] resume intent.
    #[test]
    fn import_update_and_resume_keys() {
        let d = tempdir("import-update-keys");
        std::fs::write(
            d.join("Settings.txt"),
            "Check For Updates Interval=Weekly\nLast Checked For Updates=2017-07-27\n\
Serial Should Be Connected=Yes\nNTRIP Should Be Connected=Yes\n",
        )
        .unwrap();
        let (s, log) = import_legacy(&d).expect("import should trigger");
        assert_eq!(s.app.check_updates, CheckUpdates::Weekly);
        assert_eq!(s.state.last_update_check, "2017-07-27");
        assert!(s.state.serial_connected);
        assert!(s.state.ntrip_connected);
        // Silent import: neither an "Imported ..." line nor an unknown-key
        // complaint for the bookkeeping stamp.
        assert!(!log.iter().any(|l| l.contains("Last Checked")));
        // The user-visible keys still log one line each.
        assert!(
            log.iter()
                .any(|l| l.contains("Serial Should Be Connected=Yes"))
        );
        assert!(
            log.iter()
                .any(|l| l.contains("NTRIP Should Be Connected=Yes"))
        );
        assert!(
            log.iter()
                .any(|l| l.contains("Check For Updates Interval=Weekly"))
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Anything but the weekly cadence maps to off: the original's other
    /// values belonged to the dropped self-updater, and "no" must not
    /// silently inherit our weekly default.
    #[test]
    fn import_non_weekly_update_interval_maps_to_off() {
        for value in ["No", "startup", "Daily", ""] {
            let d = tempdir(&format!("import-interval-{}", value.len()));
            std::fs::write(
                d.join("Settings.txt"),
                format!("Check For Updates Interval={value}\n"),
            )
            .unwrap();
            let (s, _) = import_legacy(&d).expect("import should trigger");
            assert_eq!(s.app.check_updates, CheckUpdates::Off, "value {value:?}");
            let _ = std::fs::remove_dir_all(&d);
        }
        // And no-connection flags default false when the keys are absent.
        let d = tempdir("import-interval-absent");
        std::fs::write(d.join("Settings.txt"), "Serial Port Number=4\n").unwrap();
        let (s, _) = import_legacy(&d).expect("import should trigger");
        assert!(!s.state.serial_connected);
        assert!(!s.state.ntrip_connected);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// .NET's StopBits::None (0) behaved as one stop bit in the original;
    /// importing it must yield 1, while genuinely invalid values are still
    /// rejected with a log line.
    #[test]
    fn import_stop_bits_zero_means_one() {
        let d = tempdir("import-stopbits");
        std::fs::write(
            d.join("Settings.txt"),
            "Serial Port Stop Bits=2\nSerial Port Stop Bits=0\n",
        )
        .unwrap();
        let (s, log) = import_legacy(&d).expect("import should trigger");
        assert_eq!(s.serial.stop_bits, 1, "0 imports as the 1 it behaved as");
        assert!(log.iter().any(|l| l.contains("Serial Port Stop Bits=0")));

        let d2 = tempdir("import-stopbits-bad");
        std::fs::write(d2.join("Settings.txt"), "Serial Port Stop Bits=3\n").unwrap();
        let (s2, log2) = import_legacy(&d2).expect("import should trigger");
        assert_eq!(s2.serial.stop_bits, 1, "invalid keeps the default");
        assert!(
            log2.iter()
                .any(|l| l.contains("invalid value") && l.contains("Stop Bits=3"))
        );
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    #[test]
    fn import_leaves_legacy_files_untouched_and_first_run_saves_toml() {
        let d = tempdir("import-untouched");
        let legacy = "NTRIP Caster=example.com\nNTRIP Caster Port=99\n";
        std::fs::write(d.join("ntripconfig.txt"), legacy).unwrap();
        let (s, log) = load_or_import(&d);
        assert_eq!(s.profiles[0].host, "example.com");
        assert_eq!(s.profiles[0].port, 99);
        assert_eq!(
            std::fs::read_to_string(d.join("ntripconfig.txt")).unwrap(),
            legacy,
            "legacy file must be byte-identical after import"
        );
        assert!(paths::settings_file(&d).exists(), "first run persists toml");
        assert!(log.iter().any(|l| l.contains("settings.toml")));
        // Second run loads the toml and does NOT re-import.
        let (s2, log2) = load_or_import(&d);
        assert_eq!(s2, s);
        assert!(log2.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
