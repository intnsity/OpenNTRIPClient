//! Profile CRUD invariants, kept pure so the Profiles Manager window is a
//! thin render layer. Invariants maintained by every operation:
//!
//! - at least one profile always exists (delete refuses the last one);
//! - profile names stay unique, because `active_profile` and the UI identify
//!   profiles by name. Every name a profile is CREATED with is the user's:
//!   [`add_named`], [`duplicate_named`] and [`save_as`] refuse a blank or
//!   taken name ([`check_name`]) instead of inventing one. The app only
//!   ever SUGGESTS names ([`suggest_name`], [`free_name`]) for the user to
//!   accept or overwrite. `rename`'s suffixing is a last-ditch safety net
//!   behind the same check, never a UI path;
//! - reordering never touches a name, so `active_profile` is unaffected;
//! - `active_profile` always names an existing profile and follows renames.

use crate::settings::{Profile, Settings};

/// `desired` if free, else "desired 2", "desired 3", ... Ignores the profile
/// at `skip` (the one being renamed) when checking collisions.
fn unique_name(settings: &Settings, desired: &str, skip: Option<usize>) -> String {
    let desired = desired.trim();
    let desired = if desired.is_empty() {
        "Profile"
    } else {
        desired
    };
    let taken = |name: &str| {
        settings
            .profiles
            .iter()
            .enumerate()
            .any(|(j, p)| Some(j) != skip && p.name == name)
    };
    if !taken(desired) {
        return desired.to_string();
    }
    (2u32..)
        .map(|k| format!("{desired} {k}"))
        .find(|c| !taken(c))
        .expect("some suffix is always free")
}

/// A suggestion made free for use as a NEW profile's name ("Rig copy" ->
/// "Rig copy 2" when taken); empty stays empty, so a prompt with nothing
/// sensible to suggest shows its placeholder instead of "Profile".
pub fn free_name(settings: &Settings, suggestion: &str) -> String {
    if suggestion.trim().is_empty() {
        String::new()
    } else {
        unique_name(settings, suggestion, None)
    }
}

/// A name for a profile, drawn from where it points: the sourcetable's
/// identifier for the stream (usually the base's place name) with its
/// mountpoint, else "MOUNT @ host", else the host alone; empty when the
/// profile points nowhere yet. A suggestion only - the user edits it.
pub fn suggest_name(p: &Profile, stream_identifier: Option<&str>) -> String {
    let host = p.host.trim();
    let mount = p.mountpoint.trim();
    let ident = stream_identifier
        .map(str::trim)
        .filter(|id| !id.is_empty() && !id.eq_ignore_ascii_case(mount));
    // A raw TCP port is identified by its port: one host often serves
    // several stations (iGage DIP ports), so "host" alone would clash.
    if p.protocol == crate::settings::ProtocolCfg::Tcp && !host.is_empty() {
        return format!("{host}:{}", p.port);
    }
    match (ident, mount.is_empty(), host.is_empty()) {
        (Some(id), false, _) => format!("{id} ({mount})"),
        (_, false, false) => format!("{mount} @ {host}"),
        (_, true, false) => host.to_string(),
        _ => String::new(),
    }
}

/// Why a typed profile name cannot be used as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameProblem {
    /// Empty after trimming.
    Blank,
    /// Another profile already has this name.
    Taken,
    /// Replacing the active profile while a stream runs on it would
    /// relabel the live session - the same lock as switching.
    ActiveWhileConnected,
}

impl NameProblem {
    /// The inline message the Profiles window shows under the name field.
    pub fn message(self, name: &str) -> String {
        match self {
            NameProblem::Blank => "A profile needs a name.".to_string(),
            NameProblem::ActiveWhileConnected => {
                "Disconnect to replace the active profile.".to_string()
            }
            NameProblem::Taken => {
                format!("\"{}\" is already used by another profile.", name.trim())
            }
        }
    }
}

/// Validate a name the user typed - for profile `skip` (a rename; keeping
/// its own name is fine), or for a new profile when `skip` is None. Returns
/// the trimmed name the CRUD calls store unchanged, or why not.
pub fn check_name(
    settings: &Settings,
    skip: Option<usize>,
    name: &str,
) -> Result<String, NameProblem> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NameProblem::Blank);
    }
    let taken = settings
        .profiles
        .iter()
        .enumerate()
        .any(|(j, p)| Some(j) != skip && p.name == name);
    if taken {
        Err(NameProblem::Taken)
    } else {
        Ok(name.to_string())
    }
}

/// Move profile `from` to position `to` (both in range), shifting the rest.
/// Order is the list's only meaning beyond names, so nothing else changes.
pub fn move_profile(settings: &mut Settings, from: usize, to: usize) -> bool {
    let n = settings.profiles.len();
    if from >= n || to >= n || from == to {
        return false;
    }
    let p = settings.profiles.remove(from);
    settings.profiles.insert(to, p);
    true
}

/// Append a fresh default profile under the user's `name`; returns its
/// index.
pub fn add_named(settings: &mut Settings, name: &str) -> Result<usize, NameProblem> {
    let name = check_name(settings, None, name)?;
    settings.profiles.push(Profile {
        name,
        ..Profile::default()
    });
    Ok(settings.profiles.len() - 1)
}

/// Copy profile `i` (credentials, caster, GGA setup - everything but the
/// name) under the user's `name`, right below the original; returns the
/// copy's index. An out-of-range `i` reads as a blank request.
pub fn duplicate_named(
    settings: &mut Settings,
    i: usize,
    name: &str,
) -> Result<usize, NameProblem> {
    let name = check_name(settings, None, name)?;
    let src = settings.profiles.get(i).ok_or(NameProblem::Blank)?.clone();
    settings.profiles.insert(i + 1, Profile { name, ..src });
    Ok(i + 1)
}

/// "Save As" for connection settings: the ACTIVE profile's current values -
/// typically just edited on the main window and tried against the caster -
/// become a new profile named `name`, which becomes active. The source goes
/// back to its last-saved version from `saved` (what settings.toml holds),
/// exactly as a document's Save As leaves the original file as it was. The
/// caller writes the file afterwards.
///
/// Allowed while connected: the new profile carries the very settings the
/// live session runs on, so nothing relabels a stream with another caster.
pub fn save_as(
    settings: &mut Settings,
    saved: &Settings,
    name: &str,
) -> Result<usize, NameProblem> {
    let name = check_name(settings, None, name)?;
    let src = settings.active_index();
    let current = settings.profiles[src].clone();
    let new = Profile {
        name: name.clone(),
        ..current.clone()
    };
    // Back to the file's version - unless the file does not know it by this
    // name (new, or renamed since the save): then it keeps what it has.
    if let Some(original) = saved.profiles.iter().find(|p| p.name == current.name)
        && *original != current
    {
        settings.profiles[src] = original.clone();
    }
    settings.profiles.insert(src + 1, new);
    settings.active_profile = name;
    Ok(src + 1)
}

/// Rename profile `i`. Empty (after trimming) is rejected; a collision with
/// another profile is resolved by suffixing, so the rename always lands
/// visibly in the list. The active-profile pointer follows.
pub fn rename(settings: &mut Settings, i: usize, new_name: &str) -> bool {
    if i >= settings.profiles.len() || new_name.trim().is_empty() {
        return false;
    }
    let name = unique_name(settings, new_name, Some(i));
    let was_active = settings.profiles[i].name == settings.active_profile;
    settings.profiles[i].name = name.clone();
    if was_active {
        settings.active_profile = name;
    }
    true
}

/// Make profile `i` the active one. Returns true only when this actually
/// changed the active profile - callers use the return to decide whether
/// per-caster UI state (cached sourcetable, revealed password) must reset,
/// and re-activating the current profile must not wipe any of that.
pub fn activate(settings: &mut Settings, i: usize) -> bool {
    let Some(p) = settings.profiles.get(i) else {
        return false;
    };
    if settings.active_profile == p.name {
        return false;
    }
    settings.active_profile = p.name.clone();
    true
}

/// Delete profile `i`. Refuses to delete the last remaining profile (the
/// app requires one to exist); deleting the active profile activates the
/// first remaining one.
pub fn remove(settings: &mut Settings, i: usize) -> bool {
    if settings.profiles.len() <= 1 || i >= settings.profiles.len() {
        return false;
    }
    let was_active = settings.profiles[i].name == settings.active_profile;
    settings.profiles.remove(i);
    if was_active {
        settings.active_profile = settings.profiles[0].name.clone();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with(names: &[&str]) -> Settings {
        Settings {
            active_profile: names[0].to_string(),
            profiles: names
                .iter()
                .map(|n| Profile {
                    name: n.to_string(),
                    ..Profile::default()
                })
                .collect(),
            ..Settings::default()
        }
    }

    #[test]
    fn add_named_takes_the_users_name_or_refuses() {
        let mut s = settings_with(&["Default"]);
        assert_eq!(add_named(&mut s, " Office base "), Ok(1));
        assert_eq!(s.profiles[1].name, "Office base");
        assert_eq!(add_named(&mut s, "Office base"), Err(NameProblem::Taken));
        assert_eq!(add_named(&mut s, "  "), Err(NameProblem::Blank));
        assert_eq!(s.profiles.len(), 2, "a refused name adds nothing");
        assert_eq!(s.active_profile, "Default", "adding never steals focus");
    }

    #[test]
    fn add_named_starts_from_connectable_defaults() {
        let mut s = settings_with(&["Default"]);
        let i = add_named(&mut s, "Fresh").unwrap();
        let p = &s.profiles[i];
        // The fresh profile must be usable after typing only a host: the
        // standard caster port and parity-shaped GGA behavior come along.
        assert_eq!(p.port, 2101);
        assert_eq!(p.gga_mode, crate::settings::GgaMode::WhenRequired);
        assert_eq!(p.gga_source, crate::settings::GgaSource::Receiver);
        assert!(!p.tls);
        assert_eq!(p.ntrip_version, 1);
        assert!(p.host.is_empty());
        assert!(p.mountpoint.is_empty());
    }

    #[test]
    fn duplicate_named_copies_everything_but_the_name() {
        let mut s = settings_with(&["Rig", "Other"]);
        s.profiles[0].host = "caster.example".to_string();
        s.profiles[0].password = "secret".to_string();
        s.profiles[0].tls = true;
        let i = duplicate_named(&mut s, 0, "Rig north").unwrap();
        assert_eq!(i, 1, "the copy lands right below its original");
        assert_eq!(s.profiles[i].name, "Rig north");
        assert_eq!(s.profiles[i].host, "caster.example");
        assert_eq!(s.profiles[i].password, "secret");
        assert!(s.profiles[i].tls);
        assert_eq!(duplicate_named(&mut s, 0, "Other"), Err(NameProblem::Taken));
        assert_eq!(duplicate_named(&mut s, 99, "X"), Err(NameProblem::Blank));
        assert_eq!(s.profiles.len(), 3);
    }

    #[test]
    fn suggestions_come_from_where_the_profile_points() {
        let p = Profile {
            host: "rtk.example.net".to_string(),
            mountpoint: "ABQ1".to_string(),
            ..Profile::default()
        };
        assert_eq!(suggest_name(&p, Some("Albuquerque")), "Albuquerque (ABQ1)");
        // An identifier that merely repeats the mount adds nothing.
        assert_eq!(suggest_name(&p, Some("abq1")), "ABQ1 @ rtk.example.net");
        assert_eq!(suggest_name(&p, None), "ABQ1 @ rtk.example.net");
        let table_only = Profile {
            mountpoint: String::new(),
            ..p.clone()
        };
        assert_eq!(suggest_name(&table_only, None), "rtk.example.net");
        assert_eq!(suggest_name(&Profile::default(), Some("X")), "");
        let dip = Profile {
            host: "apis1.us".to_string(),
            port: 2013,
            protocol: crate::settings::ProtocolCfg::Tcp,
            ..Profile::default()
        };
        assert_eq!(suggest_name(&dip, None), "apis1.us:2013");

        let s = settings_with(&["Rig copy"]);
        assert_eq!(free_name(&s, "Rig copy"), "Rig copy 2");
        assert_eq!(free_name(&s, "Bench"), "Bench");
        assert_eq!(free_name(&s, "  "), "", "nothing to suggest stays empty");
    }

    #[test]
    fn save_as_moves_edits_to_the_new_profile_and_restores_the_source() {
        let saved = settings_with(&["Field rig", "Bench"]);
        let mut s = saved.clone();
        // Edited on the main window and tried against the caster.
        s.profiles[0].host = "new.caster".to_string();
        s.profiles[0].mountpoint = "VRS".to_string();
        let i = save_as(&mut s, &saved, "VRS north").unwrap();
        assert_eq!(i, 1, "lands right below its source");
        assert_eq!(s.profiles[1].name, "VRS north");
        assert_eq!(s.profiles[1].host, "new.caster");
        assert_eq!(s.active_profile, "VRS north", "the new profile is active");
        assert_eq!(s.profiles[0], saved.profiles[0], "source as last saved");
        assert_eq!(s.profiles.len(), 3);

        // Nothing unsaved: the source is simply left alone.
        let mut s = saved.clone();
        save_as(&mut s, &saved, "Rig copy").unwrap();
        assert_eq!(s.profiles.len(), 3);

        // A source the file does not know by this name (new, or renamed
        // since the last save) is left exactly as it is - Save As never
        // removes anything, so a pending rename cannot cost the saved
        // original.
        let mut s = saved.clone();
        s.profiles[0].name = "Renamed".to_string();
        s.profiles[0].host = "edited".to_string();
        s.active_profile = "Renamed".to_string();
        let i = save_as(&mut s, &saved, "Office").unwrap();
        assert_eq!(i, 1);
        assert_eq!(s.profiles.len(), 3, "added, nothing removed");
        assert_eq!(s.profiles[0].name, "Renamed");
        assert_eq!(s.profiles[0].host, "edited");
        assert_eq!(
            s.profiles[1].host, "edited",
            "the new profile has the edits"
        );
        assert_eq!(s.active().name, "Office");

        // The user's name is checked like any other.
        let mut s = saved.clone();
        assert_eq!(save_as(&mut s, &saved, "Bench"), Err(NameProblem::Taken));
        assert_eq!(save_as(&mut s, &saved, ""), Err(NameProblem::Blank));
        assert_eq!(s, saved, "a refused Save As changes nothing");
    }

    #[test]
    fn rename_follows_active_and_resolves_collisions() {
        let mut s = settings_with(&["A", "B"]);
        assert!(rename(&mut s, 0, "Field rig"));
        assert_eq!(s.profiles[0].name, "Field rig");
        assert_eq!(s.active_profile, "Field rig", "active follows its rename");

        // Renaming the non-active profile leaves the pointer alone.
        assert!(rename(&mut s, 1, "Bench"));
        assert_eq!(s.active_profile, "Field rig");

        // Collision gets suffixed rather than silently merging identities.
        assert!(rename(&mut s, 1, "Field rig"));
        assert_eq!(s.profiles[1].name, "Field rig 2");

        // Renaming to the profile's own current name is a no-op success.
        assert!(rename(&mut s, 0, "Field rig"));
        assert_eq!(s.profiles[0].name, "Field rig");

        assert!(!rename(&mut s, 0, "   "), "blank names rejected");
        assert_eq!(s.profiles[0].name, "Field rig");
        assert!(!rename(&mut s, 9, "X"), "out of range rejected");
    }

    #[test]
    fn check_name_refuses_blank_and_clashes_but_not_self() {
        let s = settings_with(&["Field rig", "Bench"]);
        assert_eq!(
            check_name(&s, Some(1), "  Office  "),
            Ok("Office".to_string())
        );
        assert_eq!(
            check_name(&s, Some(1), "Field rig"),
            Err(NameProblem::Taken)
        );
        assert_eq!(
            check_name(&s, Some(1), " Field rig "),
            Err(NameProblem::Taken)
        );
        // Keeping your own name is not a clash.
        assert_eq!(check_name(&s, Some(1), "Bench"), Ok("Bench".to_string()));
        assert_eq!(check_name(&s, Some(0), "   "), Err(NameProblem::Blank));
        // Names are case-sensitive identities, like `rename`'s check.
        assert_eq!(
            check_name(&s, Some(1), "field rig"),
            Ok("field rig".to_string())
        );
        // What check_name accepts, rename stores verbatim (no suffix).
        let mut s = s;
        let ok = check_name(&s, Some(1), "Office").unwrap();
        assert!(rename(&mut s, 1, &ok));
        assert_eq!(s.profiles[1].name, "Office");
        assert!(
            NameProblem::Taken
                .message(" Bench ")
                .contains("\"Bench\" is already used")
        );
    }

    #[test]
    fn move_profile_reorders_without_touching_identity() {
        let mut s = settings_with(&["A", "B", "C"]);
        s.active_profile = "B".to_string();
        assert!(move_profile(&mut s, 2, 0));
        let names: Vec<_> = s.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["C", "A", "B"]);
        assert_eq!(
            s.active_profile, "B",
            "active follows the name, not the row"
        );
        assert_eq!(s.active().name, "B");
        assert!(move_profile(&mut s, 0, 1));
        let names: Vec<_> = s.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["A", "C", "B"]);
        assert!(!move_profile(&mut s, 1, 1), "no-op move");
        assert!(!move_profile(&mut s, 3, 0), "out of range");
        assert!(!move_profile(&mut s, 0, 3), "out of range");
    }

    #[test]
    fn activate_switches_only_on_real_change() {
        let mut s = settings_with(&["A", "B"]);
        assert!(activate(&mut s, 1), "switching to B is a change");
        assert_eq!(s.active_profile, "B");
        assert!(!activate(&mut s, 1), "re-activating B is a no-op");
        assert_eq!(s.active_profile, "B");
        assert!(!activate(&mut s, 9), "out of range rejected");
        assert_eq!(s.active_profile, "B");
        assert!(activate(&mut s, 0));
        assert_eq!(s.active_profile, "A");
    }

    #[test]
    fn remove_guards_the_last_profile_and_reassigns_active() {
        let mut s = settings_with(&["A", "B", "C"]);
        s.active_profile = "B".to_string();

        assert!(remove(&mut s, 1), "deleting the active profile");
        assert_eq!(s.active_profile, "A", "first remaining becomes active");
        assert_eq!(s.profiles.len(), 2);

        assert!(remove(&mut s, 1), "deleting a non-active profile");
        assert_eq!(s.active_profile, "A");
        assert_eq!(s.profiles.len(), 1);

        assert!(!remove(&mut s, 0), "the last profile is undeletable");
        assert_eq!(s.profiles.len(), 1);
        assert!(!remove(&mut s, 5), "out of range rejected");

        // The surviving state still satisfies the accessors' invariants.
        assert_eq!(s.active().name, "A");
    }
}
