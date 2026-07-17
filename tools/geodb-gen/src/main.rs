//! Generates `crates/geodb/data/geo.bin` from GeoNames and US Census TSVs.
//!
//! ```text
//! geodb-gen --cities cities5000.txt --admin1 admin1CodesASCII.txt
//!           --countries countryInfo.txt --zips 2024_Gaz_zcta_national.txt
//!           --out geo.bin
//! ```
//!
//! See README.md next to this crate for where to download the inputs.
//! Malformed rows are skipped and counted rather than aborting: upstream
//! dumps occasionally carry a stray row, and a refresh must not break on one.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use geodb::format::{self, Admin1In, CityIn, CountryIn, ZipIn};

const USAGE: &str = "usage: geodb-gen --cities <cities5000.txt> \
    --admin1 <admin1CodesASCII.txt> --countries <countryInfo.txt> \
    --zips <census_zcta_gazetteer.txt> --out <geo.bin>";

fn main() -> ExitCode {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("geodb-gen: {e}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("geodb-gen: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    cities: PathBuf,
    admin1: PathBuf,
    countries: PathBuf,
    zips: PathBuf,
    out: PathBuf,
}

impl Args {
    fn parse(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
        let mut cities = None;
        let mut admin1 = None;
        let mut countries = None;
        let mut zips = None;
        let mut out = None;
        while let Some(flag) = it.next() {
            let slot = match flag.as_str() {
                "--cities" => &mut cities,
                "--admin1" => &mut admin1,
                "--countries" => &mut countries,
                "--zips" => &mut zips,
                "--out" => &mut out,
                other => return Err(format!("unknown argument '{other}'")),
            };
            let value = it
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            if slot.replace(PathBuf::from(value)).is_some() {
                return Err(format!("duplicate {flag}"));
            }
        }
        Ok(Args {
            cities: cities.ok_or("missing --cities")?,
            admin1: admin1.ok_or("missing --admin1")?,
            countries: countries.ok_or("missing --countries")?,
            zips: zips.ok_or("missing --zips")?,
            out: out.ok_or("missing --out")?,
        })
    }
}

fn run(args: &Args) -> Result<(), String> {
    let countries = load_countries(&args.countries)?;
    let admin1 = load_admin1(&args.admin1)?;
    let mut cities = load_cities(&args.cities)?;
    let zips = load_zips(&args.zips)?;

    // encode() drops cities whose country is unknown; do it here instead so
    // the loss is visible in the run log.
    let known: HashSet<String> = countries
        .iter()
        .map(|c| c.iso2.to_ascii_uppercase())
        .collect();
    let before = cities.len();
    cities.retain(|c| known.contains(&c.country_iso2.to_ascii_uppercase()));
    if cities.len() < before {
        eprintln!(
            "warning: dropped {} cities with country codes absent from countryInfo",
            before - cities.len()
        );
    }

    let bytes = format::encode(&cities, &admin1, &countries, &zips);
    std::fs::write(&args.out, &bytes).map_err(|e| format!("write {}: {e}", args.out.display()))?;

    println!(
        "wrote {}: {} cities, {} admin1, {} countries, {} zips",
        args.out.display(),
        cities.len(),
        admin1.len(),
        countries.len(),
        zips.len()
    );
    print_sections(&bytes);
    Ok(())
}

/// Reports section sizes by reading the header back out of the image just
/// written - the format itself is the source of truth, not a parallel tally.
fn print_sections(bytes: &[u8]) {
    const NAMES: [&str; format::SECTION_COUNT] = [
        "name_index",
        "city_records",
        "string_pool",
        "admin1_table",
        "country_table",
        "zip_table",
    ];
    for (i, name) in NAMES.iter().enumerate() {
        let at = 20 + i * 8;
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("header present"));
        println!("  {name:<14}{len:>10} bytes");
    }
    println!("  {:<14}{:>10} bytes", "total", bytes.len());
}

/// Reads a whole file, tolerating stray non-UTF-8 bytes (replaced, and the
/// affected row then fails its field checks rather than killing the run).
fn read_lossy(path: &Path) -> Result<String, String> {
    let raw = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// Tally of skipped rows; remembers the first few 1-based line numbers so a
/// bad dump is diagnosable without flooding the log.
#[derive(Default)]
struct Skipped {
    count: usize,
    first_lines: Vec<usize>,
}

impl Skipped {
    fn note(&mut self, line_no: usize) {
        if self.first_lines.len() < 5 {
            self.first_lines.push(line_no);
        }
        self.count += 1;
    }

    fn report(&self, what: &str) {
        if self.count > 0 {
            eprintln!(
                "warning: skipped {} malformed {what} rows (first at lines {:?})",
                self.count, self.first_lines
            );
        }
    }
}

fn load_cities(path: &Path) -> Result<Vec<CityIn>, String> {
    let text = read_lossy(path)?;
    let mut out = Vec::new();
    let mut skipped = Skipped::default();
    for (i, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        match parse_city(line) {
            Some(c) => out.push(c),
            None => skipped.note(i + 1),
        }
    }
    skipped.report("city");
    if out.is_empty() {
        return Err(format!("{}: no usable city rows", path.display()));
    }
    Ok(out)
}

/// GeoNames dump columns (0-based): 1 name, 2 asciiname, 4 lat, 5 lon,
/// 8 country code, 10 admin1 code, 14 population. Column 3 (alternatenames)
/// is enormous and deliberately never touched.
fn parse_city(line: &str) -> Option<CityIn> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 15 {
        return None;
    }
    let name = if f[1].is_empty() { f[2] } else { f[1] };
    if name.is_empty() {
        return None;
    }
    let ascii_name = if f[2].is_empty() { name } else { f[2] };
    let lat: f64 = f[4].trim().parse().ok()?;
    let lon: f64 = f[5].trim().parse().ok()?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    let country = f[8].trim();
    if country.len() != 2 {
        return None;
    }
    let population = match f[14].trim() {
        "" => 0,
        s => u32::try_from(s.parse::<u64>().ok()?).unwrap_or(u32::MAX),
    };
    Some(CityIn {
        name: name.to_owned(),
        ascii_name: ascii_name.to_owned(),
        country_iso2: country.to_owned(),
        admin1_code: f[10].trim().to_owned(),
        lat,
        lon,
        population,
    })
}

/// admin1CodesASCII.txt: col 0 = "CC.ADM1" code, col 1 = display name.
fn load_admin1(path: &Path) -> Result<Vec<Admin1In>, String> {
    let text = read_lossy(path)?;
    let mut out = Vec::new();
    let mut skipped = Skipped::default();
    for (i, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let mut f = line.split('\t');
        match (f.next(), f.next()) {
            (Some(code), Some(name)) if code.contains('.') && !name.is_empty() => {
                out.push(Admin1In {
                    code: code.trim().to_owned(),
                    name: name.trim().to_owned(),
                });
            }
            _ => skipped.note(i + 1),
        }
    }
    skipped.report("admin1");
    if out.is_empty() {
        return Err(format!("{}: no usable admin1 rows", path.display()));
    }
    Ok(out)
}

/// countryInfo.txt: '#' lines are comments; col 0 = ISO2, col 4 = name.
fn load_countries(path: &Path) -> Result<Vec<CountryIn>, String> {
    let text = read_lossy(path)?;
    let mut out = Vec::new();
    let mut skipped = Skipped::default();
    for (i, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() >= 5 && f[0].trim().len() == 2 && !f[4].trim().is_empty() {
            out.push(CountryIn {
                iso2: f[0].trim().to_owned(),
                name: f[4].trim().to_owned(),
            });
        } else {
            skipped.note(i + 1);
        }
    }
    skipped.report("country");
    if out.is_empty() {
        return Err(format!("{}: no usable country rows", path.display()));
    }
    Ok(out)
}

/// Census ZCTA gazetteer: tab-separated WITH a header row. Columns are
/// located by header name (GEOID / INTPTLAT / INTPTLONG) because the file
/// carries extra columns and values (especially the last column) come with
/// trailing whitespace.
fn load_zips(path: &Path) -> Result<Vec<ZipIn>, String> {
    let text = read_lossy(path)?;
    let mut lines = text.lines().enumerate();
    let (_, header) = lines
        .next()
        .ok_or_else(|| format!("{}: empty file", path.display()))?;
    let cols: Vec<&str> = header.split('\t').map(str::trim).collect();
    let find = |name: &str| {
        cols.iter()
            .position(|c| c.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("{}: header lacks column {name}", path.display()))
    };
    let geoid_col = find("GEOID")?;
    let lat_col = find("INTPTLAT")?;
    let lon_col = find("INTPTLONG")?;
    let max_col = geoid_col.max(lat_col).max(lon_col);

    let mut out = Vec::new();
    let mut skipped = Skipped::default();
    for (i, line) in lines {
        if line.trim().is_empty() {
            continue;
        }
        match parse_zip(line, geoid_col, lat_col, lon_col, max_col) {
            Some(z) => out.push(z),
            None => skipped.note(i + 1),
        }
    }
    skipped.report("zip");
    if out.is_empty() {
        return Err(format!("{}: no usable zip rows", path.display()));
    }
    Ok(out)
}

fn parse_zip(
    line: &str,
    geoid_col: usize,
    lat_col: usize,
    lon_col: usize,
    max_col: usize,
) -> Option<ZipIn> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() <= max_col {
        return None;
    }
    let zip: u32 = f[geoid_col].trim().parse().ok()?;
    if zip > 99_999 {
        return None;
    }
    let lat: f64 = f[lat_col].trim().parse().ok()?;
    let lon: f64 = f[lon_col].trim().parse().ok()?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    Some(ZipIn { zip, lat, lon })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(v: &[&str]) -> Result<Args, String> {
        Args::parse(v.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn args_all_present() {
        let a = args_of(&[
            "--cities",
            "c.txt",
            "--admin1",
            "a.txt",
            "--countries",
            "n.txt",
            "--zips",
            "z.txt",
            "--out",
            "geo.bin",
        ])
        .expect("valid args");
        assert_eq!(a.out, PathBuf::from("geo.bin"));
        assert_eq!(a.cities, PathBuf::from("c.txt"));
    }

    #[test]
    fn args_missing_unknown_and_duplicate_are_errors() {
        assert!(args_of(&["--cities", "c.txt"]).is_err());
        assert!(args_of(&["--bogus", "x"]).is_err());
        assert!(args_of(&["--cities"]).is_err());
        assert!(
            args_of(&[
                "--cities",
                "a",
                "--cities",
                "b",
                "--admin1",
                "a",
                "--countries",
                "c",
                "--zips",
                "z",
                "--out",
                "o",
            ])
            .is_err()
        );
    }

    /// A real-shaped cities5000 row (19 columns).
    fn city_row() -> String {
        let cols = [
            "5746545",
            "Portland",
            "Portland",
            "Portland,Portlandia",
            "45.52345",
            "-122.67621",
            "P",
            "PPLA2",
            "US",
            "",
            "OR",
            "051",
            "",
            "",
            "652503",
            "15",
            "26",
            "America/Los_Angeles",
            "2024-01-01",
        ];
        cols.join("\t")
    }

    #[test]
    fn city_row_parses() {
        let c = parse_city(&city_row()).expect("parses");
        assert_eq!(c.name, "Portland");
        assert_eq!(c.country_iso2, "US");
        assert_eq!(c.admin1_code, "OR");
        assert_eq!(c.population, 652_503);
        assert!((c.lat - 45.52345).abs() < 1e-9);
    }

    #[test]
    fn short_or_junk_city_rows_are_rejected() {
        assert!(parse_city("just\tthree\tcols").is_none());
        let bad_lat = city_row().replace("45.52345", "not-a-number");
        assert!(parse_city(&bad_lat).is_none());
        let bad_country = city_row().replace("\tUS\t", "\tUSA\t");
        assert!(parse_city(&bad_country).is_none());
    }

    #[test]
    fn zip_rows_locate_columns_by_header_and_tolerate_whitespace() {
        // Header/value shape of 2024_Gaz_zcta_national.txt, including the
        // trailing spaces the Census ships on the last column.
        let geoid_col = 0;
        let lat_col = 5;
        let lon_col = 6;
        let line = "97201\t7038934\t231286\t2.718\t0.089\t45.497675\t-122.686972   ";
        let z = parse_zip(line, geoid_col, lat_col, lon_col, 6).expect("parses");
        assert_eq!(z.zip, 97_201);
        assert!((z.lon - -122.686972).abs() < 1e-9);
        // Row with too few columns.
        assert!(parse_zip("97201\t1\t2", geoid_col, lat_col, lon_col, 6).is_none());
        // Six digits is not a ZCTA.
        let long = line.replace("97201", "972011");
        assert!(parse_zip(&long, geoid_col, lat_col, lon_col, 6).is_none());
    }
}
