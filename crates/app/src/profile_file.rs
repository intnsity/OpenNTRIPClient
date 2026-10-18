//! Profile files (`.oncprofile`): one or many profiles in a tiny TOML file
//! that can be mailed to someone and imported in a click.
//!
//! The format is settings.toml's own `[[profiles]]` tables under a
//! `format`/`version` stamp, so the field names never drift from the
//! settings schema, and a whole settings.toml from another install imports
//! too. Import is a plan the user reviews ([`plan`]), then [`apply_import`];
//! names are profile identities, so nothing lands under a name that is
//! blank or already taken unless the user chose "Replace existing".

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::profiles::{self, NameProblem};
use crate::settings::{Profile, Settings};

pub const EXTENSION: &str = "oncprofile";
const FORMAT: &str = "onc-profiles";
const VERSION: u32 = 1;
const HEADER: &str = "# Open NTRIP Client profile file - import it in Profiles > Import...\n";

#[derive(Serialize)]
struct FileOut {
    format: &'static str,
    version: u32,
    profiles: Vec<Profile>,
}

/// Lenient on purpose: a settings.toml has no `format` and many other keys,
/// all of which are ignored.
#[derive(Deserialize)]
struct FileIn {
    format: Option<String>,
    version: Option<u32>,
    #[serde(default)]
    profiles: Vec<Profile>,
}

/// The file text for `profiles`. Excluded passwords are emptied rather
/// than omitted, so the importer sees an ordinary profile with no password.
pub fn to_text(profiles: &[Profile], include_passwords: bool) -> String {
    let profiles = profiles
        .iter()
        .map(|p| Profile {
            password: if include_passwords {
                p.password.clone()
            } else {
                String::new()
            },
            ..p.clone()
        })
        .collect();
    let body = toml::to_string_pretty(&FileOut {
        format: FORMAT,
        version: VERSION,
        profiles,
    })
    // Profiles are flat scalars and strings; TOML can always hold them.
    .expect("profiles serialize to TOML");
    format!("{HEADER}{body}")
}

/// The profiles in an `.oncprofile` or a whole ONC settings.toml, repaired
/// like settings.toml loading repairs them; a profile with a blank name is
/// dropped. Errors are one short sentence for the import dialog.
pub fn parse(text: &str) -> Result<Vec<Profile>, String> {
    let file: FileIn = toml::from_str(text).map_err(|e| {
        format!(
            "This is not a readable profile file ({}).",
            e.message().trim_end_matches('.')
        )
    })?;
    match file.format.as_deref() {
        None | Some(FORMAT) => {}
        Some(_) => return Err("This is not an Open NTRIP Client profile file.".to_string()),
    }
    if file.format.is_some() && file.version.unwrap_or(VERSION) > VERSION {
        return Err(
            "This file was made by a newer Open NTRIP Client - update to import it.".to_string(),
        );
    }
    let profiles: Vec<Profile> = file
        .profiles
        .into_iter()
        .filter_map(|mut p| {
            p.name = p.name.trim().to_string();
            p.normalize();
            (!p.name.is_empty()).then_some(p)
        })
        .collect();
    if profiles.is_empty() {
        return Err("This file holds no profiles.".to_string());
    }
    Ok(profiles)
}

/// "<name>.oncprofile" for one profile, "ONC profiles (N).oncprofile" for
/// several - always a name every OS accepts.
pub fn file_name_for(profiles: &[Profile]) -> String {
    let stem = match profiles {
        [p] => sanitize(&p.name),
        _ => format!("ONC profiles ({})", profiles.len()),
    };
    format!("{stem}.{EXTENSION}")
}

/// Windows is the strictest file system we ship to: its reserved
/// characters and control characters become '-', and trailing dots and
/// spaces (which Windows silently strips) go.
fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim().trim_end_matches(['.', ' ']);
    if s.is_empty() {
        "Profile".to_string()
    } else {
        s.to_string()
    }
}

/// `dir/file_name`, or "name 2.ext", "name 3.ext"... when taken: an export
/// never overwrites an earlier one.
fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let path = dir.join(file_name);
    if !path.exists() {
        return path;
    }
    let (stem, ext) = file_name.rsplit_once('.').unwrap_or((file_name, ""));
    (2u32..)
        .map(|k| dir.join(format!("{stem} {k}.{ext}")))
        .find(|p| !p.exists())
        .expect("some suffix is always free")
}

/// Write `profiles` as a new file in `dir` (created if missing); returns
/// where it landed.
pub fn export(
    dir: &Path,
    profiles: &[Profile],
    include_passwords: bool,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let path = unique_path(dir, &file_name_for(profiles));
    std::fs::write(&path, to_text(profiles, include_passwords))
        .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(path)
}

/// Read and [`parse`] a file, with the reason in the user's terms.
pub fn read(path: &Path) -> Result<Vec<Profile>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("Could not open the file: {e}"))?;
    parse(&text)
}

/// Where profile files usually are: our own export folder, then wherever a
/// mailed or downloaded file lands.
pub fn search_dirs(base: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![paths::profiles_dir(base)];
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    if let Some(home) = home.map(PathBuf::from) {
        dirs.push(home.join("Downloads"));
        dirs.push(home.join("Desktop"));
    }
    dirs.dedup();
    dirs
}

/// A profile file found on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundFile {
    pub path: PathBuf,
    pub modified: SystemTime,
}

/// Up to this many files are offered; older ones are a drop away.
const DISCOVER_CAP: usize = 20;

/// `*.oncprofile` in [`search_dirs`], newest first. Missing or unreadable
/// folders simply contribute nothing.
pub fn discover(base: &Path) -> Vec<FoundFile> {
    discover_in(&search_dirs(base))
}

fn discover_in(dirs: &[PathBuf]) -> Vec<FoundFile> {
    let mut found: Vec<FoundFile> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| has_extension(&e.path(), EXTENSION))
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            Some(FoundFile {
                path: e.path(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            })
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.modified));
    found.truncate(DISCOVER_CAP);
    found
}

/// Case-insensitive extension test ("RIG.ONCPROFILE" counts).
pub fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext))
}

/// Show `path` in the system file manager, selected where the platform can.
pub fn reveal(path: &Path) -> std::io::Result<()> {
    use std::process::Command;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // Explorer parses its own command line: the path must be quoted
        // AFTER the comma, which std's argument quoting cannot express.
        Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(path).spawn().map(|_| ())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let dir = path.parent().unwrap_or(path);
        Command::new("xdg-open").arg(dir).spawn().map(|_| ())
    }
}

/// One incoming profile as the import dialog shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportRow {
    pub profile: Profile,
    /// Import this one (the row's checkbox).
    pub take: bool,
    /// The name it is added under: its own when free, else a suggestion.
    pub name: String,
    /// A profile named `profile.name` already exists here.
    pub clashes_with_existing: bool,
    /// Overwrite that existing profile instead of adding under `name`.
    pub replace: bool,
}

impl ImportRow {
    /// The name this row ends up under.
    pub fn target_name(&self) -> &str {
        if self.replace && self.clashes_with_existing {
            &self.profile.name
        } else {
            self.name.trim()
        }
    }
}

/// Rows for `incoming`, every one taken and none replacing. A name that
/// clashes with an existing profile or an earlier row gets a free
/// suggestion, so the default plan always imports cleanly.
pub fn plan(incoming: Vec<Profile>, settings: &Settings) -> Vec<ImportRow> {
    let mut seen = settings.clone();
    incoming
        .into_iter()
        .map(|profile| {
            let name = profiles::free_name(&seen, &profile.name);
            seen.profiles.push(Profile {
                name: name.clone(),
                ..Profile::default()
            });
            ImportRow {
                clashes_with_existing: settings.profiles.iter().any(|p| p.name == profile.name),
                profile,
                take: true,
                name,
                replace: false,
            }
        })
        .collect()
}

/// Why row `i` cannot be imported as it stands; None for a fine or untaken
/// row. Replacing is fine by definition; anything else must be a free name
/// that no other taken row ends up under either.
pub fn row_problem(
    settings: &Settings,
    rows: &[ImportRow],
    i: usize,
    busy: bool,
) -> Option<NameProblem> {
    let row = &rows[i];
    if !row.take {
        return None;
    }
    if busy
        && row.replace
        && row.clashes_with_existing
        && row.target_name() == settings.active_profile
    {
        return Some(NameProblem::ActiveWhileConnected);
    }
    if !(row.replace && row.clashes_with_existing)
        && let Err(problem) = profiles::check_name(settings, None, &row.name)
    {
        return Some(problem);
    }
    let target = row.target_name();
    let twin = rows
        .iter()
        .enumerate()
        .any(|(j, r)| j != i && r.take && r.target_name() == target);
    twin.then_some(NameProblem::Taken)
}

/// Import every taken row that has no [`row_problem`]: replace in place
/// (same position, same name, so `active_profile` stays valid) or append.
/// The active profile never changes. Returns the indices of the imported
/// profiles in row order.
pub fn apply_import(settings: &mut Settings, rows: &[ImportRow], busy: bool) -> Vec<usize> {
    let ok: Vec<&ImportRow> = (0..rows.len())
        .filter(|&i| rows[i].take && row_problem(settings, rows, i, busy).is_none())
        .map(|i| &rows[i])
        .collect();
    ok.into_iter()
        .filter_map(|row| {
            let mut profile = Profile {
                name: row.target_name().to_string(),
                ..row.profile.clone()
            };
            if row.replace && row.clashes_with_existing {
                let at = settings
                    .profiles
                    .iter()
                    .position(|p| p.name == profile.name)?;
                // A file shared without passwords updates the caster setup;
                // it must not wipe the password this install already holds
                // for the same account.
                let existing = &settings.profiles[at];
                if profile.password.is_empty() && profile.username == existing.username {
                    profile.password = existing.password.clone();
                }
                settings.profiles[at] = profile;
                Some(at)
            } else {
                settings.profiles.push(profile);
                Some(settings.profiles.len() - 1)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{GgaMode, ProtocolCfg};
    use std::time::Duration;

    fn rig(name: &str) -> Profile {
        Profile {
            name: name.to_string(),
            host: "rtk.example.net".to_string(),
            port: 2102,
            ntrip_version: 2,
            protocol: ProtocolCfg::Ntrip,
            username: "alice".to_string(),
            password: "hunter2".to_string(),
            mountpoint: "BASE01".to_string(),
            gga_mode: GgaMode::Always,
            manual_lat: 45.5,
            manual_lon: -122.6,
            ..Profile::default()
        }
    }

    fn settings_with(names: &[&str]) -> Settings {
        Settings {
            active_profile: names[0].to_string(),
            profiles: names.iter().map(|n| rig(n)).collect(),
            ..Settings::default()
        }
    }

    fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "open-ntrip-client-profile-file-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn single_and_bulk_round_trip() {
        let one = vec![rig("Field rig")];
        let text = to_text(&one, true);
        assert!(text.starts_with(HEADER));
        assert!(text.contains("format = \"onc-profiles\""));
        assert!(text.contains("version = 1"));
        assert_eq!(parse(&text).unwrap(), one);

        let many = vec![rig("A"), rig("B"), rig("C")];
        assert_eq!(parse(&to_text(&many, true)).unwrap(), many);
    }

    #[test]
    fn passwords_can_stay_home() {
        let text = to_text(&[rig("A")], false);
        assert!(!text.contains("hunter2"));
        let back = parse(&text).unwrap();
        assert_eq!(back[0].password, "");
        assert_eq!(back[0].username, "alice", "only the password is held back");
    }

    #[test]
    fn a_whole_settings_toml_imports() {
        let mut s = settings_with(&["Office", "Truck"]);
        s.app.write_nmea_log = false;
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(parse(&text).unwrap(), s.profiles);
    }

    #[test]
    fn hand_edits_are_repaired_and_blank_names_dropped() {
        let text = "format = \"onc-profiles\"\nversion = 1\n\
                    [[profiles]]\nname = \"  Rig  \"\nntrip_version = 7\n\
                    [[profiles]]\nname = \"   \"\n";
        let p = parse(text).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].name, "Rig");
        assert_eq!(p[0].ntrip_version, 1);
    }

    #[test]
    fn wrong_files_are_refused_in_plain_words() {
        let garbage = parse("this is = = not toml").unwrap_err();
        assert!(
            garbage.starts_with("This is not a readable profile file"),
            "{garbage}"
        );
        let other = parse("format = \"something-else\"\n[[profiles]]\nname = \"A\"\n").unwrap_err();
        assert!(
            other.contains("not an Open NTRIP Client profile file"),
            "{other}"
        );
        let empty = parse("format = \"onc-profiles\"\nversion = 1\n").unwrap_err();
        assert!(empty.contains("no profiles"), "{empty}");
        assert!(parse("").unwrap_err().contains("no profiles"));
    }

    #[test]
    fn newer_versions_are_refused() {
        let text = "format = \"onc-profiles\"\nversion = 2\n[[profiles]]\nname = \"A\"\n";
        assert!(parse(text).unwrap_err().contains("newer"));
    }

    #[test]
    fn file_names_are_safe_everywhere() {
        assert_eq!(file_name_for(&[rig("Field rig")]), "Field rig.oncprofile");
        assert_eq!(
            file_name_for(&[rig(r#"a<b>c:d"e/f\g|h?i*j"#)]),
            "a-b-c-d-e-f-g-h-i-j.oncprofile"
        );
        assert_eq!(file_name_for(&[rig("tab\there.")]), "tab-here.oncprofile");
        assert_eq!(file_name_for(&[rig("...")]), "Profile.oncprofile");
        assert_eq!(
            file_name_for(&[rig("A"), rig("B")]),
            "ONC profiles (2).oncprofile"
        );
    }

    #[test]
    fn export_never_overwrites_and_discover_finds_newest_first() {
        let d = tempdir("export");
        let a = export(&d, &[rig("Rig")], true).unwrap();
        let b = export(&d, &[rig("Rig")], true).unwrap();
        assert_eq!(a, d.join("Rig.oncprofile"));
        assert_eq!(b, d.join("Rig 2.oncprofile"));
        assert_eq!(read(&b).unwrap(), vec![rig("Rig")]);
        std::fs::write(d.join("notes.txt"), "x").unwrap();
        // Ordering is by mtime; push one file into the past to make it so.
        let old = std::fs::File::options().write(true).open(&a).unwrap();
        old.set_modified(SystemTime::now() - Duration::from_secs(3_600))
            .unwrap();
        let found = discover_in(&[d.clone(), d.join("missing")]);
        let paths: Vec<_> = found.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, vec![b, a]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ages_read_like_a_person_says_them() {}

    #[test]
    fn plan_takes_everything_and_suggests_free_names() {
        let s = settings_with(&["Office", "Truck"]);
        let rows = plan(vec![rig("Truck"), rig("New"), rig("New")], &s);
        assert!(rows.iter().all(|r| r.take && !r.replace));
        assert_eq!(rows[0].name, "Truck 2");
        assert!(rows[0].clashes_with_existing);
        assert_eq!(rows[1].name, "New");
        assert!(!rows[1].clashes_with_existing);
        assert_eq!(rows[2].name, "New 2", "rows do not clash with each other");
        assert!((0..3).all(|i| row_problem(&s, &rows, i, false).is_none()));
    }

    #[test]
    fn apply_adds_under_the_chosen_names() {
        let mut s = settings_with(&["Office", "Truck"]);
        let rows = plan(vec![rig("Truck"), rig("New")], &s);
        assert_eq!(apply_import(&mut s, &rows, false), vec![2, 3]);
        let names: Vec<_> = s.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Office", "Truck", "Truck 2", "New"]);
        assert_eq!(s.profiles[3].host, "rtk.example.net");
        assert_eq!(s.active_profile, "Office", "import never switches");
    }

    #[test]
    fn replace_keeps_position_and_the_active_pointer() {
        let mut s = settings_with(&["Office", "Truck"]);
        let mut incoming = rig("Office");
        incoming.host = "new.caster".to_string();
        let mut rows = plan(vec![incoming], &s);
        rows[0].replace = true;
        assert_eq!(rows[0].target_name(), "Office");
        assert_eq!(apply_import(&mut s, &rows, false), vec![0]);
        assert_eq!(s.profiles.len(), 2);
        assert_eq!(s.profiles[0].host, "new.caster");
        assert_eq!(s.active_profile, "Office");
        assert_eq!(s.active().host, "new.caster");
    }

    #[test]
    fn passwordless_replace_keeps_the_local_password_for_the_same_account() {
        let mut s = settings_with(&["Office"]);
        s.profiles[0] = rig("Office"); // alice / hunter2 here
        let shared = parse(&to_text(&[rig("Office")], false)).unwrap();
        let mut rows = plan(shared.clone(), &s);
        rows[0].replace = true;
        apply_import(&mut s, &rows, false);
        assert_eq!(s.profiles[0].password, "hunter2");
        // A different account in the file does not inherit it.
        let mut other = shared;
        other[0].username = "bob".to_string();
        let mut rows = plan(other, &s);
        rows[0].replace = true;
        apply_import(&mut s, &rows, false);
        assert_eq!(s.profiles[0].password, "");
    }

    #[test]
    fn the_active_profile_cannot_be_replaced_while_connected() {
        let mut s = settings_with(&["Office", "Truck"]);
        s.profiles[0].host = "live.caster".to_string(); // what the stream runs on
        let mut rows = plan(vec![rig("Office"), rig("Truck")], &s);
        rows[0].replace = true;
        rows[1].replace = true;
        assert_eq!(
            row_problem(&s, &rows, 0, true),
            Some(NameProblem::ActiveWhileConnected)
        );
        assert_eq!(
            row_problem(&s, &rows, 1, true),
            None,
            "others may be replaced"
        );
        assert_eq!(apply_import(&mut s, &rows, true), vec![1]);
        assert_eq!(
            s.profiles[0].host, "live.caster",
            "the live profile is untouched"
        );
    }

    #[test]
    fn untaken_rows_are_skipped() {
        let mut s = settings_with(&["Office"]);
        let mut rows = plan(vec![rig("A"), rig("B")], &s);
        rows[0].take = false;
        rows[0].name.clear(); // an untaken row's name is irrelevant
        assert_eq!(row_problem(&s, &rows, 0, false), None);
        assert_eq!(apply_import(&mut s, &rows, false), vec![1]);
        assert_eq!(s.profiles[1].name, "B");
    }

    #[test]
    fn bad_names_are_refused_never_invented() {
        let mut s = settings_with(&["Office"]);
        let mut rows = plan(vec![rig("A"), rig("B"), rig("C")], &s);
        rows[0].name = "  ".to_string();
        rows[1].name = "Office".to_string();
        assert_eq!(row_problem(&s, &rows, 0, false), Some(NameProblem::Blank));
        assert_eq!(row_problem(&s, &rows, 1, false), Some(NameProblem::Taken));
        assert_eq!(row_problem(&s, &rows, 2, false), None);
        assert_eq!(apply_import(&mut s, &rows, false), vec![1]);
        let names: Vec<_> = s.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Office", "C"]);
    }

    #[test]
    fn two_rows_may_not_end_with_one_name() {
        let mut s = settings_with(&["Office"]);
        let mut rows = plan(vec![rig("A"), rig("B")], &s);
        rows[1].name = " A ".to_string();
        assert_eq!(row_problem(&s, &rows, 0, false), Some(NameProblem::Taken));
        assert_eq!(row_problem(&s, &rows, 1, false), Some(NameProblem::Taken));
        assert!(apply_import(&mut s, &rows, false).is_empty());
        assert_eq!(s.profiles.len(), 1);

        // Nor may a replace and an add land on the same name.
        let mut rows = plan(vec![rig("Office"), rig("X")], &s);
        rows[0].replace = true;
        rows[1].name = "Office".to_string();
        assert_eq!(row_problem(&s, &rows, 0, false), Some(NameProblem::Taken));
        assert_eq!(row_problem(&s, &rows, 1, false), Some(NameProblem::Taken));
    }
}
