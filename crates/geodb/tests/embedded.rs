//! Sanity checks against the real committed database (data/geo.bin). These
//! pin behavior a support tech relies on, with loose coordinate tolerances so
//! routine GeoNames/Census refreshes do not break the build.

use geodb::{GeoDb, GeoHit};

const EMBEDDED: &[u8] = include_bytes!("../data/geo.bin");

fn db() -> GeoDb {
    GeoDb::embedded().expect("embedded geo.bin must validate")
}

fn assert_near(hit: &GeoHit, lat: f64, lon: f64, tol: f64) {
    assert!(
        (hit.lat - lat).abs() < tol && (hit.lon - lon).abs() < tol,
        "{} at ({}, {}) not within {tol} deg of ({lat}, {lon})",
        hit.display,
        hit.lat,
        hit.lon
    );
}

#[test]
fn embedded_db_is_a_sane_size() {
    assert!(
        EMBEDDED.len() < 6 * 1024 * 1024,
        "geo.bin is {} bytes; over the 6 MiB budget",
        EMBEDDED.len()
    );
    // A plausibly complete world db cannot be tiny; catches a stale
    // placeholder file sneaking into a release.
    assert!(
        EMBEDDED.len() > 1024 * 1024,
        "geo.bin is only {} bytes; looks like a placeholder",
        EMBEDDED.len()
    );
}

#[test]
fn nairobi_resolves_to_kenya() {
    let hits = db().resolve("Nairobi", 5);
    assert!(!hits.is_empty(), "no hits for Nairobi");
    assert_near(&hits[0], -1.2864, 36.8172, 0.1);
    assert!(hits[0].display.ends_with("KE"), "got {}", hits[0].display);
}

#[test]
fn portland_oregon_beats_portland_maine() {
    let hits = db().resolve("Portland, OR", 5);
    assert!(!hits.is_empty(), "no hits for Portland, OR");
    let top = &hits[0];
    assert_near(top, 45.52, -122.68, 0.2);
    assert_eq!(top.display, "Portland, OR, US");
    assert!(
        !top.display.contains(", ME"),
        "top hit is in Maine: {}",
        top.display
    );
}

#[test]
fn displayed_labels_round_trip_through_resolve() {
    // A support tech pastes a previously shown hit label back into the
    // picker; every comma token acts as an independent qualifier, so the
    // full label must find the same city.
    let hits = db().resolve("Portland, OR, US", 5);
    assert!(!hits.is_empty(), "label did not round-trip");
    assert_eq!(hits[0].display, "Portland, OR, US");
    assert_near(&hits[0], 45.52, -122.68, 0.2);
}

#[test]
fn zip_97201_is_portland_oregon() {
    let hits = db().resolve("97201", 5);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].display, "ZIP 97201");
    assert_near(&hits[0], 45.52, -122.68, 0.2);
}

#[test]
fn zurich_found_without_typing_the_umlaut() {
    let hits = db().resolve("zurich", 5);
    assert!(!hits.is_empty(), "no hits for zurich");
    let top = &hits[0];
    assert_near(top, 47.3769, 8.5417, 0.2);
    assert!(top.display.ends_with("CH"), "got {}", top.display);
}

#[test]
fn springfield_is_ambiguous_and_population_sorted() {
    let hits = db().resolve("springfield", 10);
    assert!(
        hits.len() >= 3,
        "expected many Springfields, got {}",
        hits.len()
    );
    assert!(hits[0].population > 0);
    for pair in hits.windows(2) {
        assert!(
            pair[0].population >= pair[1].population,
            "not population-sorted: {} ({}) before {} ({})",
            pair[0].display,
            pair[0].population,
            pair[1].display,
            pair[1].population
        );
    }
}
