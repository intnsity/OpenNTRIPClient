//! On-disk layout of the "GEO1" database, plus its encoder.
//!
//! Encoder and reader live in the same crate so the format cannot drift:
//! `encode` here produces exactly the bytes [`crate::GeoDb`] consumes, and the
//! fixture tests round-trip through both.
//!
//! The file layout IS the runtime structure: the reader keeps raw byte slices
//! and binary-searches them in place, so there is zero parse work on load.
//!
//! All integers are little-endian. Section-table offsets are absolute file
//! offsets; every `*_off` string field is an offset into the string_pool
//! section.
//!
//! ```text
//! header (64 bytes):
//!   0   [u8; 8]   magic "ONCGEO1\0"
//!   8   u32       version = 1
//!   12  u32       section_count = 6
//!   16  6 pairs   (u32 offset, u32 len), one per section, in SEC_* order
//! sections (contiguous, in this order):
//!   name_index    10-byte entries: key_off u32, key_len u8, pad u8 = 0,
//!                 city_rec u32 (record index into city_records).
//!                 Sorted by folded key bytes. One entry per searchable
//!                 variant per city: folded(name), plus folded(ascii_name)
//!                 when that differs.
//!   city_records  21-byte entries: disp_off u32, disp_len u8,
//!                 country_idx u16, admin1_idx u16 (0xFFFF = none),
//!                 lat_e4 i32, lon_e4 i32, pop u32.
//!                 disp_* is the original-case city name; lat/lon are degrees
//!                 in 1e-4 units (~11 m resolution).
//!   string_pool   concatenated UTF-8; exact-duplicate strings stored once.
//!   admin1_table  8-byte entries: name_off u32, name_len u8,
//!                 abbrev [u8; 3] uppercase, right-padded with spaces.
//!                 The abbrev is the GeoNames admin1 code after the country
//!                 prefix ("US.OR" -> "OR "), truncated to 3 bytes; for the
//!                 US that code is the 2-letter state abbreviation.
//!   country_table 7-byte entries: name_off u32, name_len u8,
//!                 iso2 [u8; 2] uppercase.
//!   zip_table     12-byte entries: zip u32, lat_e4 i32, lon_e4 i32,
//!                 sorted ascending by zip, unique.
//! ```

use std::collections::HashMap;

use crate::fold::fold;

pub const MAGIC: [u8; 8] = *b"ONCGEO1\0";
pub const VERSION: u32 = 1;
pub const SECTION_COUNT: usize = 6;
/// Magic + version + section_count + section table.
pub const HEADER_LEN: usize = 8 + 4 + 4 + SECTION_COUNT * 8;

/// Section indices into the header's section table.
pub const SEC_NAME_INDEX: usize = 0;
pub const SEC_CITY_RECORDS: usize = 1;
pub const SEC_STRING_POOL: usize = 2;
pub const SEC_ADMIN1_TABLE: usize = 3;
pub const SEC_COUNTRY_TABLE: usize = 4;
pub const SEC_ZIP_TABLE: usize = 5;

pub const NAME_INDEX_ENTRY_LEN: usize = 10;
pub const CITY_RECORD_LEN: usize = 21;
pub const ADMIN1_ENTRY_LEN: usize = 8;
pub const COUNTRY_ENTRY_LEN: usize = 7;
pub const ZIP_ENTRY_LEN: usize = 12;

/// `admin1_idx` sentinel: the city has no admin1 subdivision.
pub const ADMIN1_NONE: u16 = 0xFFFF;

/// Entry size per section (string_pool has byte granularity). Lets the
/// reader reject sections whose length is not a whole number of entries.
pub const SECTION_ENTRY_LEN: [usize; SECTION_COUNT] = [
    NAME_INDEX_ENTRY_LEN,
    CITY_RECORD_LEN,
    1,
    ADMIN1_ENTRY_LEN,
    COUNTRY_ENTRY_LEN,
    ZIP_ENTRY_LEN,
];

/// One city, as parsed from the GeoNames `cities5000.txt` dump.
pub struct CityIn {
    /// Display name, original case and diacritics (GeoNames column 1).
    pub name: String,
    /// ASCII transliteration (GeoNames column 2); may equal `name`.
    pub ascii_name: String,
    /// ISO-3166 alpha-2 country code (GeoNames column 8).
    pub country_iso2: String,
    /// Raw GeoNames admin1 code, e.g. "OR" or "05" (column 10); empty = none.
    /// `encode` resolves it against `Admin1In` codes as "{ISO2}.{code}".
    pub admin1_code: String,
    pub lat: f64,
    pub lon: f64,
    pub population: u32,
}

/// One first-level administrative division (GeoNames `admin1CodesASCII.txt`).
pub struct Admin1In {
    /// Full GeoNames code, "CC.ADM1" form, e.g. "US.OR" or "CH.ZH".
    pub code: String,
    /// Display name, e.g. "Oregon".
    pub name: String,
}

/// One country (GeoNames `countryInfo.txt`).
pub struct CountryIn {
    pub iso2: String,
    pub name: String,
}

/// One ZIP centroid (US Census ZCTA gazetteer).
pub struct ZipIn {
    pub zip: u32,
    pub lat: f64,
    pub lon: f64,
}

/// Deduplicating string-pool builder. Identical strings (after the 255-byte
/// clamp) are stored once; offsets are relative to the pool section start.
#[derive(Default)]
struct Pool {
    bytes: Vec<u8>,
    seen: HashMap<String, (u32, u8)>,
}

impl Pool {
    fn add(&mut self, s: &str) -> (u32, u8) {
        let s = clamp_to_255(s);
        if let Some(&v) = self.seen.get(s) {
            return v;
        }
        let off = u32::try_from(self.bytes.len()).expect("string pool exceeds 4 GiB");
        self.bytes.extend_from_slice(s.as_bytes());
        let v = (off, s.len() as u8);
        self.seen.insert(s.to_owned(), v);
        v
    }
}

/// Truncates to at most 255 bytes on a char boundary, so the result always
/// fits the u8 length fields. Real place names never get near this.
fn clamp_to_255(s: &str) -> &str {
    if s.len() <= 255 {
        return s;
    }
    let mut end = 255;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn coord_e4(deg: f64) -> i32 {
    // Valid inputs are within +-180 deg (1.8e6 in e4 units); the clamp only
    // guards against garbage rows that slipped through the generator.
    (deg * 1e4).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

fn folded(s: &str) -> String {
    let mut out = String::new();
    fold(s, &mut out);
    out
}

/// Encodes a complete database image.
///
/// Order of the input slices does not affect correctness; the encoder sorts
/// where the format requires it (name_index by folded key, zip_table by zip,
/// admin1/country tables by code for deterministic output). Duplicate country
/// or admin1 codes keep their first entry; duplicate zips keep the first
/// occurrence.
///
/// Cities whose `country_iso2` has no entry in `countries` are dropped -
/// every record must be displayable as "Name, CC". Callers that care should
/// pre-validate (the generator warns and counts).
///
/// # Panics
///
/// Panics if `countries` exceeds 65536 entries or `admin1` exceeds 65535
/// (index fields are u16, with 0xFFFF reserved), or if the total output
/// exceeds u32 addressing. Real-world inputs are orders of magnitude smaller.
pub fn encode(
    cities: &[CityIn],
    admin1: &[Admin1In],
    countries: &[CountryIn],
    zips: &[ZipIn],
) -> Vec<u8> {
    assert!(
        countries.len() <= usize::from(u16::MAX) + 1,
        "too many countries for u16 index"
    );
    assert!(
        admin1.len() < usize::from(ADMIN1_NONE),
        "too many admin1 entries for u16 index"
    );

    let mut pool = Pool::default();

    // Country table, sorted by ISO2 for deterministic output.
    let mut countries_sorted: Vec<&CountryIn> = countries.iter().collect();
    countries_sorted.sort_by(|a, b| a.iso2.cmp(&b.iso2));
    let mut country_idx: HashMap<String, u16> = HashMap::new();
    let mut country_bytes = Vec::with_capacity(countries_sorted.len() * COUNTRY_ENTRY_LEN);
    for (i, c) in countries_sorted.iter().enumerate() {
        let (off, len) = pool.add(&c.name);
        let iso = padded_upper::<2>(&c.iso2);
        country_bytes.extend_from_slice(&off.to_le_bytes());
        country_bytes.push(len);
        country_bytes.extend_from_slice(&iso);
        country_idx
            .entry(c.iso2.to_ascii_uppercase())
            .or_insert(i as u16);
    }

    // Admin1 table, sorted by full code.
    let mut admin1_sorted: Vec<&Admin1In> = admin1.iter().collect();
    admin1_sorted.sort_by(|a, b| a.code.cmp(&b.code));
    let mut admin1_idx: HashMap<String, u16> = HashMap::new();
    let mut admin1_bytes = Vec::with_capacity(admin1_sorted.len() * ADMIN1_ENTRY_LEN);
    for (i, a) in admin1_sorted.iter().enumerate() {
        let (off, len) = pool.add(&a.name);
        // Abbrev = the part after the country prefix; the whole code when no
        // dot is present (defensive - GeoNames codes always carry one).
        let sub = a.code.split_once('.').map_or(a.code.as_str(), |(_, s)| s);
        let abbrev = padded_upper::<3>(sub);
        admin1_bytes.extend_from_slice(&off.to_le_bytes());
        admin1_bytes.push(len);
        admin1_bytes.extend_from_slice(&abbrev);
        admin1_idx
            .entry(a.code.to_ascii_uppercase())
            .or_insert(i as u16);
    }

    // City records in input order, collecting (folded key, record) pairs for
    // the name index as we go.
    let mut city_bytes = Vec::with_capacity(cities.len() * CITY_RECORD_LEN);
    let mut index_entries: Vec<(String, u32)> = Vec::with_capacity(cities.len() * 2);
    let mut rec: u32 = 0;
    for c in cities {
        let iso_up = c.country_iso2.to_ascii_uppercase();
        let Some(&ci) = country_idx.get(&iso_up) else {
            continue; // unknown country: documented drop
        };
        let ai = if c.admin1_code.is_empty() {
            ADMIN1_NONE
        } else {
            let key = format!("{}.{}", iso_up, c.admin1_code.to_ascii_uppercase());
            admin1_idx.get(&key).copied().unwrap_or(ADMIN1_NONE)
        };
        let (doff, dlen) = pool.add(&c.name);
        city_bytes.extend_from_slice(&doff.to_le_bytes());
        city_bytes.push(dlen);
        city_bytes.extend_from_slice(&ci.to_le_bytes());
        city_bytes.extend_from_slice(&ai.to_le_bytes());
        city_bytes.extend_from_slice(&coord_e4(c.lat).to_le_bytes());
        city_bytes.extend_from_slice(&coord_e4(c.lon).to_le_bytes());
        city_bytes.extend_from_slice(&c.population.to_le_bytes());

        // Clamp keys the same way the pool will store them so the index stays
        // sorted by the bytes actually on disk.
        let k1 = clamp_to_255(&folded(&c.name)).to_owned();
        let k2 = clamp_to_255(&folded(&c.ascii_name)).to_owned();
        if !k1.is_empty() {
            index_entries.push((k1.clone(), rec));
        }
        if k2 != k1 && !k2.is_empty() {
            index_entries.push((k2, rec));
        }
        rec += 1;
    }

    // Name index: sorted by key bytes, ties by record for determinism.
    index_entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()).then(a.1.cmp(&b.1)));
    let mut index_bytes = Vec::with_capacity(index_entries.len() * NAME_INDEX_ENTRY_LEN);
    for (key, r) in &index_entries {
        let (koff, klen) = pool.add(key);
        index_bytes.extend_from_slice(&koff.to_le_bytes());
        index_bytes.push(klen);
        index_bytes.push(0);
        index_bytes.extend_from_slice(&r.to_le_bytes());
    }

    // Zip table: sorted, first occurrence wins on duplicates.
    let mut zips_sorted: Vec<&ZipIn> = zips.iter().collect();
    zips_sorted.sort_by_key(|z| z.zip);
    let mut zip_bytes = Vec::with_capacity(zips_sorted.len() * ZIP_ENTRY_LEN);
    let mut prev_zip: Option<u32> = None;
    for z in zips_sorted {
        if prev_zip == Some(z.zip) {
            continue;
        }
        prev_zip = Some(z.zip);
        zip_bytes.extend_from_slice(&z.zip.to_le_bytes());
        zip_bytes.extend_from_slice(&coord_e4(z.lat).to_le_bytes());
        zip_bytes.extend_from_slice(&coord_e4(z.lon).to_le_bytes());
    }

    assemble([
        index_bytes,
        city_bytes,
        pool.bytes,
        admin1_bytes,
        country_bytes,
        zip_bytes,
    ])
}

/// Uppercases and right-pads with spaces into a fixed-width ASCII field,
/// truncating overlong input.
fn padded_upper<const N: usize>(s: &str) -> [u8; N] {
    let mut out = [b' '; N];
    for (d, b) in out.iter_mut().zip(s.bytes()) {
        *d = b.to_ascii_uppercase();
    }
    out
}

fn assemble(sections: [Vec<u8>; SECTION_COUNT]) -> Vec<u8> {
    let total = HEADER_LEN + sections.iter().map(Vec::len).sum::<usize>();
    assert!(
        u32::try_from(total).is_ok(),
        "database exceeds u32 addressing"
    );
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(SECTION_COUNT as u32).to_le_bytes());
    let mut off = HEADER_LEN as u32;
    for s in &sections {
        out.extend_from_slice(&off.to_le_bytes());
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        off += s.len() as u32;
    }
    for s in &sections {
        out.extend_from_slice(s);
    }
    out
}
