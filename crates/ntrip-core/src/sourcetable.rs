//! NTRIP sourcetable parser.
//!
//! Deliberately lossless: any line that is not an STR/CAS/NET record lands
//! verbatim in `unparsed` because full verbosity is a product requirement
//! (the original client silently dropped CAS and NET rows entirely). Short
//! or malformed records fill defaults and never error: nonconforming casters
//! are the norm and a diagnostic tool must show whatever it can.

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceTable {
    pub strs: Vec<StrRecord>,
    pub casters: Vec<CasRecord>,
    pub networks: Vec<NetRecord>,
    /// Every non-empty line that is not an STR/CAS/NET record (comments,
    /// stray HTTP headers casters leak into v1 bodies, vendor extensions),
    /// excluding the ENDSOURCETABLE terminator.
    pub unparsed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrRecord {
    pub mountpoint: String,
    pub identifier: String,
    pub format: String,
    pub format_details: String,
    pub carrier: u8,
    pub nav_system: String,
    pub network: String,
    pub country: String,
    pub lat: f32,
    pub lon: f32,
    pub nmea_required: bool,
    pub solution: u8,
    pub generator: String,
    pub compression: String,
    /// Authentication flag as published (N/B/D); ' ' when absent.
    pub auth: char,
    pub fee: bool,
    pub bitrate: u32,
    /// Fields beyond the 18 standard ones, rejoined with ';' verbatim.
    pub misc: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CasRecord {
    pub host: String,
    pub port: u16,
    pub identifier: String,
    pub operator: String,
    pub nmea: String,
    pub country: String,
    pub lat: f32,
    pub lon: f32,
    pub misc: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NetRecord {
    pub identifier: String,
    pub operator: String,
    pub auth: String,
    pub fee: String,
    pub web_net: String,
    pub web_str: String,
    pub web_reg: String,
    pub misc: String,
}

/// Parse a raw table body (lossy UTF-8). Never fails: garbage degrades into
/// defaults and `unparsed` lines, never into an error.
pub fn parse(raw: &[u8]) -> SourceTable {
    let text = String::from_utf8_lossy(raw);
    let mut table = SourceTable::default();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let trimmed = line.trim_ascii();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("ENDSOURCETABLE") {
            continue;
        }
        let fields: Vec<&str> = line.split(';').collect();
        match fields[0] {
            "STR" => table.strs.push(parse_str(&fields)),
            "CAS" => table.casters.push(parse_cas(&fields)),
            "NET" => table.networks.push(parse_net(&fields)),
            _ => table.unparsed.push(line.to_string()),
        }
    }
    table
}

/// Field accessor: absent fields read as "". String fields stay verbatim
/// (no trimming) so nothing the caster published is altered; trimming only
/// happens on the way into numeric/flag conversions.
fn field<'a>(fields: &[&'a str], i: usize) -> &'a str {
    fields.get(i).copied().unwrap_or("")
}

fn num<T: std::str::FromStr + Default>(fields: &[&str], i: usize) -> T {
    field(fields, i).trim().parse().unwrap_or_default()
}

fn misc_from(fields: &[&str], start: usize) -> String {
    if fields.len() > start {
        fields[start..].join(";")
    } else {
        String::new()
    }
}

fn parse_str(fields: &[&str]) -> StrRecord {
    StrRecord {
        mountpoint: field(fields, 1).to_string(),
        identifier: field(fields, 2).to_string(),
        format: field(fields, 3).to_string(),
        format_details: field(fields, 4).to_string(),
        carrier: num(fields, 5),
        nav_system: field(fields, 6).to_string(),
        network: field(fields, 7).to_string(),
        country: field(fields, 8).to_string(),
        lat: num(fields, 9),
        lon: num(fields, 10),
        nmea_required: field(fields, 11).trim() == "1",
        solution: num(fields, 12),
        generator: field(fields, 13).to_string(),
        compression: field(fields, 14).to_string(),
        auth: field(fields, 15).trim().chars().next().unwrap_or(' '),
        fee: field(fields, 16).trim().eq_ignore_ascii_case("Y"),
        bitrate: num(fields, 17),
        misc: misc_from(fields, 18),
    }
}

fn parse_cas(fields: &[&str]) -> CasRecord {
    CasRecord {
        host: field(fields, 1).to_string(),
        port: num(fields, 2),
        identifier: field(fields, 3).to_string(),
        operator: field(fields, 4).to_string(),
        nmea: field(fields, 5).to_string(),
        country: field(fields, 6).to_string(),
        lat: num(fields, 7),
        lon: num(fields, 8),
        misc: misc_from(fields, 9),
    }
}

fn parse_net(fields: &[&str]) -> NetRecord {
    NetRecord {
        identifier: field(fields, 1).to_string(),
        operator: field(fields, 2).to_string(),
        auth: field(fields, 3).to_string(),
        fee: field(fields, 4).to_string(),
        web_net: field(fields, 5).to_string(),
        web_str: field(fields, 6).to_string(),
        web_reg: field(fields, 7).to_string(),
        misc: misc_from(fields, 8),
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    const TABLE: &[u8] = b"CAS;rtk2go.com;2101;RTK2go;SNIP;0;USA;37.20;-95.00;http://rtk2go.com;extra\r\n\
NET;SNIP;RTK2go;B;N;http://rtk2go.com;none;support@example.com;trailing;bits\r\n\
STR;MOUNT1;Frankfurt;RTCM 3.2;1005(1),1074(1);2;GPS+GLO;SNIP;DEU;50.09;8.66;1;1;SNIP;none;B;Y;3120;extra1;extra2\r\n\
# a comment line casters sometimes emit\r\n\
Server: NTRIP Caster 2.0\r\n\
\r\n\
ENDSOURCETABLE\r\n";

    #[test]
    fn full_records_parse() {
        let t = parse(TABLE);
        assert_eq!(t.strs.len(), 1);
        assert_eq!(t.casters.len(), 1);
        assert_eq!(t.networks.len(), 1);

        let s = &t.strs[0];
        assert_eq!(s.mountpoint, "MOUNT1");
        assert_eq!(s.identifier, "Frankfurt");
        assert_eq!(s.format, "RTCM 3.2");
        assert_eq!(s.format_details, "1005(1),1074(1)");
        assert_eq!(s.carrier, 2);
        assert_eq!(s.nav_system, "GPS+GLO");
        assert_eq!(s.network, "SNIP");
        assert_eq!(s.country, "DEU");
        assert!((s.lat - 50.09).abs() < 1e-4);
        assert!((s.lon - 8.66).abs() < 1e-4);
        assert!(s.nmea_required);
        assert_eq!(s.solution, 1);
        assert_eq!(s.generator, "SNIP");
        assert_eq!(s.compression, "none");
        assert_eq!(s.auth, 'B');
        assert!(s.fee);
        assert_eq!(s.bitrate, 3120);
        assert_eq!(s.misc, "extra1;extra2");

        let c = &t.casters[0];
        assert_eq!(c.host, "rtk2go.com");
        assert_eq!(c.port, 2101);
        assert_eq!(c.identifier, "RTK2go");
        assert_eq!(c.operator, "SNIP");
        assert_eq!(c.nmea, "0");
        assert_eq!(c.country, "USA");
        assert!((c.lat - 37.20).abs() < 1e-4);
        assert!((c.lon - -95.00).abs() < 1e-4);
        assert_eq!(c.misc, "http://rtk2go.com;extra");

        let n = &t.networks[0];
        assert_eq!(n.identifier, "SNIP");
        assert_eq!(n.operator, "RTK2go");
        assert_eq!(n.auth, "B");
        assert_eq!(n.fee, "N");
        assert_eq!(n.web_net, "http://rtk2go.com");
        assert_eq!(n.web_str, "none");
        assert_eq!(n.web_reg, "support@example.com");
        assert_eq!(n.misc, "trailing;bits");
    }

    #[test]
    fn unparsed_kept_verbatim_endsourcetable_and_blanks_dropped() {
        let t = parse(TABLE);
        assert_eq!(
            t.unparsed,
            vec![
                "# a comment line casters sometimes emit".to_string(),
                "Server: NTRIP Caster 2.0".to_string(),
            ]
        );
    }

    #[test]
    fn short_lines_fill_defaults() {
        let t = parse(b"STR;ONLY\nCAS\nNET;X\n");
        let s = &t.strs[0];
        assert_eq!(s.mountpoint, "ONLY");
        assert_eq!(s.identifier, "");
        assert_eq!(s.carrier, 0);
        assert_eq!(s.lat, 0.0);
        assert!(!s.nmea_required);
        assert_eq!(s.auth, ' ');
        assert!(!s.fee);
        assert_eq!(s.bitrate, 0);
        assert_eq!(s.misc, "");
        assert_eq!(t.casters[0].host, "");
        assert_eq!(t.casters[0].port, 0);
        assert_eq!(t.networks[0].identifier, "X");
        assert_eq!(t.networks[0].misc, "");
    }

    #[test]
    fn garbage_numerics_default_not_error() {
        let t = parse(b"STR;M;;RTCM;;banana;;;;north;west;maybe;lots;;;ZZ;$$;fast\n");
        let s = &t.strs[0];
        assert_eq!(s.carrier, 0);
        assert_eq!(s.lat, 0.0);
        assert_eq!(s.lon, 0.0);
        assert!(!s.nmea_required);
        assert_eq!(s.solution, 0);
        assert_eq!(s.auth, 'Z');
        assert!(!s.fee);
        assert_eq!(s.bitrate, 0);
    }

    #[test]
    fn lossy_utf8_still_parses() {
        let t = parse(b"STR;M\xFF1;Ident\nENDSOURCETABLE\n");
        assert_eq!(t.strs.len(), 1);
        assert!(t.strs[0].mountpoint.contains('\u{FFFD}'));
        assert_eq!(t.strs[0].identifier, "Ident");
    }

    #[test]
    fn lowercase_tag_goes_to_unparsed() {
        let t = parse(b"str;not-a-record\n");
        assert!(t.strs.is_empty());
        assert_eq!(t.unparsed, vec!["str;not-a-record".to_string()]);
    }
}
