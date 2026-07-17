//! Profile CRUD invariants, kept pure so the Profiles Manager window is a
//! thin render layer. Invariants maintained by every operation:
//!
//! - at least one profile always exists (delete refuses the last one);
//! - profile names stay unique (collisions get a " 2"/" 3"... suffix,
//!   because `active_profile` and the UI identify profiles by name);
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

/// Append a fresh default profile; returns its index.
pub fn add(settings: &mut Settings) -> usize {
    let name = unique_name(settings, "New profile", None);
    settings.profiles.push(Profile {
        name,
        ..Profile::default()
    });
    settings.profiles.len() - 1
}

/// Clone profile `i` (credentials, caster, GGA setup - everything but the
/// name); returns the copy's index, or None for an out-of-range `i`.
pub fn duplicate(settings: &mut Settings, i: usize) -> Option<usize> {
    let src = settings.profiles.get(i)?.clone();
    let name = unique_name(settings, &format!("{} copy", src.name), None);
    settings.profiles.push(Profile { name, ..src });
    Some(settings.profiles.len() - 1)
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
    fn add_uses_unique_names() {
        let mut s = settings_with(&["Default"]);
        let a = add(&mut s);
        let b = add(&mut s);
        assert_eq!(s.profiles[a].name, "New profile");
        assert_eq!(s.profiles[b].name, "New profile 2");
        assert_eq!(add(&mut s), 3);
        assert_eq!(s.profiles[3].name, "New profile 3");
        assert_eq!(s.active_profile, "Default", "adding never steals focus");
    }

    #[test]
    fn add_starts_from_connectable_defaults() {
        let mut s = settings_with(&["Default"]);
        let i = add(&mut s);
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
    fn duplicate_copies_everything_but_the_name() {
        let mut s = settings_with(&["Rig"]);
        s.profiles[0].host = "caster.example".to_string();
        s.profiles[0].password = "secret".to_string();
        s.profiles[0].tls = true;
        let i = duplicate(&mut s, 0).unwrap();
        assert_eq!(s.profiles[i].name, "Rig copy");
        assert_eq!(s.profiles[i].host, "caster.example");
        assert_eq!(s.profiles[i].password, "secret");
        assert!(s.profiles[i].tls);
        let j = duplicate(&mut s, 0).unwrap();
        assert_eq!(s.profiles[j].name, "Rig copy 2");
        assert_eq!(duplicate(&mut s, 99), None);
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
