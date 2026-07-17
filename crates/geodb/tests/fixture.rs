//! Round-trip tests over a tiny in-test database: `format::encode` produces
//! the bytes, `GeoDb::from_bytes` consumes them. Also proves the reader's
//! corruption discipline: damaged headers error, damaged bodies never panic.

use geodb::format::{self, Admin1In, CityIn, CountryIn, ZipIn};
use geodb::{FormatError, GeoDb};

fn leak(v: Vec<u8>) -> &'static [u8] {
    Box::leak(v.into_boxed_slice())
}

fn city(
    name: &str,
    ascii: &str,
    iso2: &str,
    admin1: &str,
    lat: f64,
    lon: f64,
    population: u32,
) -> CityIn {
    CityIn {
        name: name.to_owned(),
        ascii_name: ascii.to_owned(),
        country_iso2: iso2.to_owned(),
        admin1_code: admin1.to_owned(),
        lat,
        lon,
        population,
    }
}

/// Two Portlands (same name, different states), a diacritic city whose ASCII
/// variant folds identically (Zurich), one whose variants differ (Munich),
/// a city without admin1 (Nairobi), and two zips including a leading-zero one.
fn fixture_bytes() -> Vec<u8> {
    let cities = vec![
        city(
            "Portland", "Portland", "US", "OR", 45.5202, -122.6742, 652_503,
        ),
        city(
            "Portland", "Portland", "US", "ME", 43.6615, -70.2553, 68_408,
        ),
        city(
            "Z\u{00FC}rich",
            "Zurich",
            "CH",
            "ZH",
            47.3769,
            8.5417,
            341_730,
        ),
        city("Nairobi", "Nairobi", "KE", "", -1.2833, 36.8167, 2_750_547),
        city(
            "M\u{00FC}nchen",
            "Munich",
            "DE",
            "BY",
            48.1374,
            11.5755,
            1_260_391,
        ),
    ];
    let admin1 = vec![
        Admin1In {
            code: "US.OR".to_owned(),
            name: "Oregon".to_owned(),
        },
        Admin1In {
            code: "US.ME".to_owned(),
            name: "Maine".to_owned(),
        },
        Admin1In {
            code: "CH.ZH".to_owned(),
            name: "Z\u{00FC}rich".to_owned(),
        },
        Admin1In {
            code: "DE.BY".to_owned(),
            name: "Bavaria".to_owned(),
        },
    ];
    let countries = vec![
        CountryIn {
            iso2: "US".to_owned(),
            name: "United States".to_owned(),
        },
        CountryIn {
            iso2: "CH".to_owned(),
            name: "Switzerland".to_owned(),
        },
        CountryIn {
            iso2: "KE".to_owned(),
            name: "Kenya".to_owned(),
        },
        CountryIn {
            iso2: "DE".to_owned(),
            name: "Germany".to_owned(),
        },
    ];
    let zips = vec![
        ZipIn {
            zip: 97201,
            lat: 45.4977,
            lon: -122.6867,
        },
        ZipIn {
            zip: 4101,
            lat: 43.6606,
            lon: -70.2590,
        },
    ];
    format::encode(&cities, &admin1, &countries, &zips)
}

fn fixture_db() -> GeoDb {
    GeoDb::from_bytes(leak(fixture_bytes())).expect("fixture must validate")
}

fn displays(hits: &[geodb::GeoHit]) -> Vec<&str> {
    hits.iter().map(|h| h.display.as_str()).collect()
}

#[test]
fn encode_is_deterministic() {
    assert_eq!(fixture_bytes(), fixture_bytes());
}

#[test]
fn prefix_search_finds_all_matches_population_descending() {
    let db = fixture_db();
    let hits = db.resolve("port", 10);
    assert_eq!(
        displays(&hits),
        ["Portland, OR, US", "Portland, ME, US"],
        "bigger Portland first"
    );
    assert!(hits[0].population > hits[1].population);
    assert!((hits[0].lat - 45.5202).abs() < 1e-3);
    assert!((hits[0].lon - -122.6742).abs() < 1e-3);
}

#[test]
fn full_name_and_case_insensitive_queries_match() {
    let db = fixture_db();
    assert_eq!(displays(&db.resolve("PORTLAND", 10)).len(), 2);
    assert_eq!(displays(&db.resolve("Nairobi", 10)), ["Nairobi, KE"]);
}

#[test]
fn qualifier_filters_by_state_abbrev() {
    let db = fixture_db();
    assert_eq!(
        displays(&db.resolve("Portland, OR", 10)),
        ["Portland, OR, US"]
    );
    assert_eq!(
        displays(&db.resolve("portland,me", 10)),
        ["Portland, ME, US"]
    );
}

#[test]
fn qualifier_filters_by_admin1_name_prefix() {
    let db = fixture_db();
    assert_eq!(
        displays(&db.resolve("Portland, Maine", 10)),
        ["Portland, ME, US"]
    );
    assert_eq!(
        displays(&db.resolve("Portland, oreg", 10)),
        ["Portland, OR, US"]
    );
}

#[test]
fn qualifier_matches_country_iso2_and_name() {
    let db = fixture_db();
    // Country-name prefix keeps both US Portlands.
    assert_eq!(db.resolve("portland, united", 10).len(), 2);
    assert_eq!(displays(&db.resolve("nairobi, KE", 10)), ["Nairobi, KE"]);
    assert_eq!(displays(&db.resolve("nairobi, keny", 10)), ["Nairobi, KE"]);
}

#[test]
fn qualifier_with_no_match_yields_empty() {
    let db = fixture_db();
    assert!(db.resolve("Portland, ZZ", 10).is_empty());
    // Qualifier must match the start of the field, not a substring.
    assert!(db.resolve("Portland, regon", 10).is_empty());
}

#[test]
fn trailing_comma_or_blank_qualifier_is_ignored() {
    let db = fixture_db();
    assert_eq!(db.resolve("portland,", 10).len(), 2);
    assert_eq!(db.resolve("portland,   ", 10).len(), 2);
}

#[test]
fn diacritics_fold_in_both_data_and_query() {
    let db = fixture_db();
    let expected = "Z\u{00FC}rich, Z\u{00FC}rich, CH";
    assert_eq!(displays(&db.resolve("zurich", 10)), [expected]);
    assert_eq!(displays(&db.resolve("Z\u{00FC}rich", 10)), [expected]);
    // Qualifier folds too: "zu" matches the admin1 name "Zurich" (folded),
    // and the non-US abbrev path matches "zh".
    assert_eq!(displays(&db.resolve("zurich, z\u{00FC}", 10)), [expected]);
    assert_eq!(displays(&db.resolve("zurich, zh", 10)), [expected]);
}

#[test]
fn both_name_variants_hit_the_same_record_once() {
    let db = fixture_db();
    // Prefix "m" reaches Munich via both folded "munchen" and "munich" keys;
    // the record must appear exactly once.
    let hits = db.resolve("m", 10);
    assert_eq!(displays(&hits), ["M\u{00FC}nchen, Bavaria, DE"]);
    // And each variant works spelled out.
    assert_eq!(db.resolve("m\u{00FC}nchen", 10).len(), 1);
    assert_eq!(db.resolve("munich", 10).len(), 1);
}

#[test]
fn zip_lookup_exact_match_only() {
    let db = fixture_db();
    let hits = db.resolve("97201", 10);
    assert_eq!(displays(&hits), ["ZIP 97201"]);
    assert_eq!(hits[0].population, 0);
    assert!((hits[0].lat - 45.4977).abs() < 1e-3);
    assert!((hits[0].lon - -122.6867).abs() < 1e-3);
    // Surrounding whitespace is trimmed before classification.
    assert_eq!(db.resolve("  97201  ", 10).len(), 1);
    assert!(db.resolve("99999", 10).is_empty());
}

#[test]
fn zip_display_preserves_leading_zeros() {
    let db = fixture_db();
    assert_eq!(displays(&db.resolve("04101", 10)), ["ZIP 04101"]);
}

#[test]
fn four_or_six_digit_queries_are_not_zips() {
    let db = fixture_db();
    assert!(db.resolve("9720", 10).is_empty());
    assert!(db.resolve("972010", 10).is_empty());
}

#[test]
fn limit_truncates_and_zero_means_none() {
    let db = fixture_db();
    assert_eq!(db.resolve("portland", 1).len(), 1);
    assert!(db.resolve("portland", 0).is_empty());
    assert!(db.resolve("97201", 0).is_empty());
}

#[test]
fn empty_and_junk_queries_yield_empty() {
    let db = fixture_db();
    assert!(db.resolve("", 10).is_empty());
    assert!(db.resolve("   ", 10).is_empty());
    assert!(db.resolve(",OR", 10).is_empty());
    assert!(db.resolve("xyzzy", 10).is_empty());
}

#[test]
fn empty_database_resolves_nothing() {
    let db = GeoDb::from_bytes(leak(format::encode(&[], &[], &[], &[]))).expect("empty db valid");
    assert!(db.resolve("portland", 10).is_empty());
    assert!(db.resolve("97201", 10).is_empty());
}

#[test]
fn scan_cap_bounds_pathological_prefixes() {
    // 600 cities sharing a prefix: the scan must stop at 500 examined
    // entries, so at most 500 records come back no matter the limit.
    let cities: Vec<CityIn> = (0..600)
        .map(|i| {
            city(
                &format!("Alphaville{i:03}"),
                &format!("Alphaville{i:03}"),
                "US",
                "",
                40.0,
                -100.0,
                i,
            )
        })
        .collect();
    let countries = vec![CountryIn {
        iso2: "US".to_owned(),
        name: "United States".to_owned(),
    }];
    let bytes = format::encode(&cities, &[], &countries, &[]);
    let db = GeoDb::from_bytes(leak(bytes)).expect("valid");
    let hits = db.resolve("alphaville", 1000);
    assert_eq!(hits.len(), 500);
}

// --- corruption discipline ---------------------------------------------

#[test]
fn every_truncation_is_rejected() {
    // Sections run contiguously to EOF, so any truncation must leave some
    // section out of bounds (or the header short). No length may panic.
    let bytes = fixture_bytes();
    for len in 0..bytes.len() {
        let cut = leak(bytes[..len].to_vec());
        assert!(
            GeoDb::from_bytes(cut).is_err(),
            "truncation to {len} bytes was accepted"
        );
    }
}

#[test]
fn header_field_corruption_is_rejected_with_specific_errors() {
    let good = fixture_bytes();

    let mut bad_magic = good.clone();
    bad_magic[0] ^= 0xFF;
    assert_eq!(
        GeoDb::from_bytes(leak(bad_magic)).err(),
        Some(FormatError::BadMagic)
    );

    let mut bad_version = good.clone();
    bad_version[8] = 9;
    assert_eq!(
        GeoDb::from_bytes(leak(bad_version)).err(),
        Some(FormatError::BadVersion { found: 9 })
    );

    let mut bad_count = good.clone();
    bad_count[12] = 5;
    assert_eq!(
        GeoDb::from_bytes(leak(bad_count)).err(),
        Some(FormatError::BadSectionCount { found: 5 })
    );

    // Section 0 length inflated far past EOF.
    let mut oob = good.clone();
    oob[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        GeoDb::from_bytes(leak(oob)).err(),
        Some(FormatError::SectionOutOfBounds { section: 0 })
    );

    // Section 0 (name_index, 10-byte entries) shrunk by one byte: still in
    // bounds, no longer a whole entry count.
    let mut misaligned = good.clone();
    let len0 = u32::from_le_bytes(good[20..24].try_into().unwrap());
    misaligned[20..24].copy_from_slice(&(len0 - 1).to_le_bytes());
    assert_eq!(
        GeoDb::from_bytes(leak(misaligned)).err(),
        Some(FormatError::SectionMisaligned { section: 0 })
    );
}

#[test]
fn body_corruption_never_panics() {
    // Flip every body byte in turn; the header stays intact so from_bytes
    // accepts the db, and resolve must degrade gracefully on every mutant.
    let good = fixture_bytes();
    for pos in geodb::format::HEADER_LEN..good.len() {
        let mut mutant = good.clone();
        mutant[pos] ^= 0xFF;
        let db = GeoDb::from_bytes(leak(mutant)).expect("body damage passes header checks");
        for q in ["port", "portland, or", "zurich", "97201", "04101", "zzz"] {
            let _ = db.resolve(q, 5);
        }
    }
}
