//! Embedded offline geocoder: resolves "Nairobi", "Portland, OR", or "97201"
//! to coordinates from a compact database compiled into the binary.
//!
//! The database (format "GEO1", see [`format`]) is generated offline by
//! `tools/geodb-gen` from GeoNames and US Census data and committed as
//! `data/geo.bin`. [`GeoDb::embedded`] validates it once; [`GeoDb::resolve`]
//! then answers queries by binary-searching the raw bytes in place.
//!
//! The bytes are treated as untrusted: `from_bytes` rejects structurally
//! invalid files, and every per-entry access during `resolve` is
//! bounds-checked, so a corrupt body degrades to missing results, never a
//! panic.

pub mod fold;
pub mod format;

use std::fmt;

use format::{
    ADMIN1_ENTRY_LEN, ADMIN1_NONE, CITY_RECORD_LEN, COUNTRY_ENTRY_LEN, HEADER_LEN, MAGIC,
    NAME_INDEX_ENTRY_LEN, SECTION_COUNT, SECTION_ENTRY_LEN, VERSION, ZIP_ENTRY_LEN,
};

/// Hard cap on index entries examined per query. Bounds worst-case latency on
/// pathological prefixes ("a") while never truncating realistic result sets:
/// the UI shows a handful of hits and no real prefix a user would type has
/// hundreds of thousands of matches.
const SCAN_CAP: usize = 500;

/// One geocoding result.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoHit {
    /// Human-readable label. Exact format, by record kind:
    /// - ZIP hit: `"ZIP 97201"` (always five digits, zero-padded);
    /// - US city with a state abbrev: `"Portland, OR, US"`;
    /// - other city with an admin1 name: `"Zurich, Zurich, CH"`;
    /// - city without admin1: `"Nairobi, KE"`.
    pub display: String,
    pub lat: f64,
    pub lon: f64,
    /// 0 for ZIP hits (the gazetteer has no population).
    pub population: u32,
}

/// Structural validation failure in [`GeoDb::from_bytes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatError {
    /// Shorter than the fixed 64-byte header.
    TooShort,
    BadMagic,
    BadVersion {
        found: u32,
    },
    BadSectionCount {
        found: u32,
    },
    /// Section extends past the end of the file.
    SectionOutOfBounds {
        section: usize,
    },
    /// Section length is not a whole number of fixed-size entries.
    SectionMisaligned {
        section: usize,
    },
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            FormatError::TooShort => write!(f, "file shorter than the GEO1 header"),
            FormatError::BadMagic => write!(f, "bad magic (not a GEO1 database)"),
            FormatError::BadVersion { found } => {
                write!(f, "unsupported GEO1 version {found} (expected {VERSION})")
            }
            FormatError::BadSectionCount { found } => {
                write!(f, "bad section count {found} (expected {SECTION_COUNT})")
            }
            FormatError::SectionOutOfBounds { section } => {
                write!(f, "section {section} extends past end of file")
            }
            FormatError::SectionMisaligned { section } => {
                write!(f, "section {section} length is not a whole entry count")
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// A validated, zero-copy view of a GEO1 database.
pub struct GeoDb {
    name_index: &'static [u8],
    city_records: &'static [u8],
    string_pool: &'static [u8],
    admin1_table: &'static [u8],
    country_table: &'static [u8],
    zip_table: &'static [u8],
}

impl GeoDb {
    /// Opens the database compiled into the binary.
    pub fn embedded() -> Result<GeoDb, FormatError> {
        static DATA: &[u8] = include_bytes!("../data/geo.bin");
        GeoDb::from_bytes(DATA)
    }

    /// Validates the header and section table of `data` and returns a view
    /// over it. This is the only load-time work; sections are consumed in
    /// place afterwards.
    ///
    /// Structural validation covers the magic, version, section count, that
    /// every section lies within the file, and that fixed-entry sections hold
    /// a whole number of entries (which is how truncation shows up). Entry
    /// contents stay untrusted and are bounds-checked per access in
    /// [`resolve`](Self::resolve).
    pub fn from_bytes(data: &'static [u8]) -> Result<GeoDb, FormatError> {
        if data.len() < HEADER_LEN {
            return Err(FormatError::TooShort);
        }
        if data[..8] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let version = u32le(data, 8).expect("header length checked");
        if version != VERSION {
            return Err(FormatError::BadVersion { found: version });
        }
        let count = u32le(data, 12).expect("header length checked");
        if count as usize != SECTION_COUNT {
            return Err(FormatError::BadSectionCount { found: count });
        }
        let mut sections = [&data[..0]; SECTION_COUNT];
        for (i, slot) in sections.iter_mut().enumerate() {
            let off = u32le(data, 16 + i * 8).expect("header length checked") as usize;
            let len = u32le(data, 20 + i * 8).expect("header length checked") as usize;
            // u32-to-usize widening means off + len cannot overflow on
            // 64-bit; get() still rejects any range beyond the file.
            let Some(sec) = off.checked_add(len).and_then(|end| data.get(off..end)) else {
                return Err(FormatError::SectionOutOfBounds { section: i });
            };
            if !len.is_multiple_of(SECTION_ENTRY_LEN[i]) {
                return Err(FormatError::SectionMisaligned { section: i });
            }
            *slot = sec;
        }
        let [
            name_index,
            city_records,
            string_pool,
            admin1_table,
            country_table,
            zip_table,
        ] = sections;
        Ok(GeoDb {
            name_index,
            city_records,
            string_pool,
            admin1_table,
            country_table,
            zip_table,
        })
    }

    /// Resolves a location query, returning at most `limit` hits.
    ///
    /// Query grammar, after trimming surrounding whitespace:
    /// - exactly five ASCII digits: ZIP lookup ("97201", "04101");
    /// - otherwise `name[, qualifier[, qualifier...]]`, split at commas. The
    ///   folded name is a prefix search over city names (both native and
    ///   ASCII spellings). Each non-empty folded qualifier independently
    ///   keeps only hits whose admin1 abbreviation, admin1 name, country
    ///   ISO2, or country name starts with it ("or" matches Oregon via "OR",
    ///   "maine" matches Maine, "united s" matches United States). Treating
    ///   every comma-separated token as its own qualifier lets a displayed
    ///   hit label ("Portland, OR, US") round-trip through the search box.
    ///
    /// City hits are sorted by population descending (ties by record order,
    /// so output is deterministic) and deduplicated when both name variants
    /// of one city match the prefix.
    pub fn resolve(&self, query: &str, limit: usize) -> Vec<GeoHit> {
        let q = query.trim();
        if q.len() == 5 && q.bytes().all(|b| b.is_ascii_digit()) {
            // Cannot fail: five digits max out at 99999.
            return self.resolve_zip(q.parse().unwrap_or(0), limit);
        }

        let (name_raw, qual_raw) = match q.split_once(',') {
            Some((n, rest)) => (n, Some(rest)),
            None => (q, None),
        };
        let mut prefix = String::new();
        fold::fold(name_raw.trim(), &mut prefix);
        if prefix.is_empty() {
            return Vec::new();
        }
        let qualifiers: Vec<String> = qual_raw
            .map(|s| {
                s.split(',')
                    .filter_map(|part| {
                        let mut f = String::new();
                        fold::fold(part.trim(), &mut f);
                        (!f.is_empty()).then_some(f)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut hits = self.prefix_scan(prefix.as_bytes());
        if !qualifiers.is_empty() {
            hits.retain(|c| qualifiers.iter().all(|q| c.matches_qualifier(q)));
        }
        hits.sort_by(|a, b| b.pop.cmp(&a.pop).then(a.rec.cmp(&b.rec)));
        hits.truncate(limit);
        hits.iter().map(CityRef::to_hit).collect()
    }

    /// Collects unique city records whose folded key starts with `prefix`.
    fn prefix_scan(&self, prefix: &[u8]) -> Vec<CityRef> {
        let n = self.name_index.len() / NAME_INDEX_ENTRY_LEN;
        // Lower bound: first index entry whose key is >= prefix.
        let mut lo = 0;
        let mut hi = n;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.index_key(mid) < prefix {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut recs: Vec<u32> = Vec::new();
        for i in lo..n.min(lo + SCAN_CAP) {
            if !self.index_key(i).starts_with(prefix) {
                break;
            }
            if let Some(rec) = u32le(self.name_index, i * NAME_INDEX_ENTRY_LEN + 6)
                && !recs.contains(&rec)
            {
                recs.push(rec);
            }
        }
        recs.iter().filter_map(|&r| self.city(r)).collect()
    }

    /// Folded key bytes of index entry `i`; empty for corrupt entries, which
    /// keeps comparisons total (they sort first and match no real prefix).
    fn index_key(&self, i: usize) -> &[u8] {
        let base = i * NAME_INDEX_ENTRY_LEN;
        let Some(off) = u32le(self.name_index, base) else {
            return &[];
        };
        let Some(&len) = self.name_index.get(base + 4) else {
            return &[];
        };
        self.string_pool
            .get(off as usize..off as usize + len as usize)
            .unwrap_or(&[])
    }

    /// Loads city record `rec`, resolving all string references. `None` for
    /// any out-of-bounds or non-UTF-8 reference: corrupt records vanish from
    /// results instead of panicking.
    fn city(&self, rec: u32) -> Option<CityRef> {
        let base = (rec as usize).checked_mul(CITY_RECORD_LEN)?;
        let name = self.pool_str(
            u32le(self.city_records, base)?,
            *self.city_records.get(base + 4)?,
        )?;
        let country_idx = u16le(self.city_records, base + 5)?;
        let admin1_idx = u16le(self.city_records, base + 7)?;
        let lat = f64::from(i32le(self.city_records, base + 9)?) / 1e4;
        let lon = f64::from(i32le(self.city_records, base + 13)?) / 1e4;
        let pop = u32le(self.city_records, base + 17)?;
        let (iso2, country_name) = self.country(country_idx)?;
        // A dangling admin1 index degrades to "no admin1" rather than
        // discarding the whole record.
        let admin1 = if admin1_idx == ADMIN1_NONE {
            None
        } else {
            self.admin1(admin1_idx)
        };
        Some(CityRef {
            rec,
            name,
            iso2,
            country_name,
            admin1,
            lat,
            lon,
            pop,
        })
    }

    fn country(&self, idx: u16) -> Option<(&'static str, &'static str)> {
        let base = usize::from(idx) * COUNTRY_ENTRY_LEN;
        let name = self.pool_str(
            u32le(self.country_table, base)?,
            *self.country_table.get(base + 4)?,
        )?;
        let iso2 = std::str::from_utf8(self.country_table.get(base + 5..base + 7)?).ok()?;
        Some((iso2, name))
    }

    fn admin1(&self, idx: u16) -> Option<Admin1Ref> {
        let base = usize::from(idx) * ADMIN1_ENTRY_LEN;
        let name = self.pool_str(
            u32le(self.admin1_table, base)?,
            *self.admin1_table.get(base + 4)?,
        )?;
        let abbrev = std::str::from_utf8(self.admin1_table.get(base + 5..base + 8)?).ok()?;
        Some(Admin1Ref {
            abbrev: abbrev.trim_end_matches(' '),
            name,
        })
    }

    fn pool_str(&self, off: u32, len: u8) -> Option<&'static str> {
        let off = off as usize;
        std::str::from_utf8(self.string_pool.get(off..off + usize::from(len))?).ok()
    }

    fn resolve_zip(&self, zip: u32, limit: usize) -> Vec<GeoHit> {
        let n = self.zip_table.len() / ZIP_ENTRY_LEN;
        let mut lo = 0;
        let mut hi = n;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            // In-bounds by construction (len is a whole entry count); the
            // fallback only keeps the search total if that ever breaks.
            let z = u32le(self.zip_table, mid * ZIP_ENTRY_LEN).unwrap_or(u32::MAX);
            if z < zip {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut out = Vec::new();
        if lo < n
            && u32le(self.zip_table, lo * ZIP_ENTRY_LEN) == Some(zip)
            && let Some(lat) = i32le(self.zip_table, lo * ZIP_ENTRY_LEN + 4)
            && let Some(lon) = i32le(self.zip_table, lo * ZIP_ENTRY_LEN + 8)
        {
            out.push(GeoHit {
                display: format!("ZIP {zip:05}"),
                lat: f64::from(lat) / 1e4,
                lon: f64::from(lon) / 1e4,
                population: 0,
            });
        }
        out.truncate(limit);
        out
    }
}

/// A city record with all string references resolved.
struct CityRef {
    rec: u32,
    name: &'static str,
    iso2: &'static str,
    country_name: &'static str,
    admin1: Option<Admin1Ref>,
    lat: f64,
    lon: f64,
    pop: u32,
}

struct Admin1Ref {
    /// Stored abbreviation with the space padding stripped; may be empty.
    abbrev: &'static str,
    name: &'static str,
}

impl CityRef {
    fn matches_qualifier(&self, folded_qualifier: &str) -> bool {
        if let Some(a1) = &self.admin1
            && (field_starts_with(a1.abbrev, folded_qualifier)
                || field_starts_with(a1.name, folded_qualifier))
        {
            return true;
        }
        field_starts_with(self.iso2, folded_qualifier)
            || field_starts_with(self.country_name, folded_qualifier)
    }

    fn to_hit(&self) -> GeoHit {
        // Display contract (see GeoHit::display): US cities show the state
        // abbreviation, others the admin1 name, and both fall back to plain
        // "Name, CC" when the admin1 field is missing or empty.
        let display = match &self.admin1 {
            Some(a1) if self.iso2 == "US" && !a1.abbrev.is_empty() => {
                format!("{}, {}, {}", self.name, a1.abbrev, self.iso2)
            }
            Some(a1) if !a1.name.is_empty() => {
                format!("{}, {}, {}", self.name, a1.name, self.iso2)
            }
            _ => format!("{}, {}", self.name, self.iso2),
        };
        GeoHit {
            display,
            lat: self.lat,
            lon: self.lon,
            population: self.pop,
        }
    }
}

/// True when the folded form of `field` starts with the already-folded
/// qualifier. Fields are stored unfolded (they double as display text), so
/// each check folds on the fly; result sets are small enough that this never
/// shows up in a profile.
fn field_starts_with(field: &str, folded_qualifier: &str) -> bool {
    let mut f = String::with_capacity(field.len());
    fold::fold(field, &mut f);
    f.starts_with(folded_qualifier)
}

fn u32le(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

fn i32le(b: &[u8], off: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

fn u16le(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
}
