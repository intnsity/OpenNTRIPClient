# geodb-gen

Dev-only tool that builds `crates/geodb/data/geo.bin` (format "GEO1") from
GeoNames and US Census sources. The generated `geo.bin` IS committed, so
contributors and CI build the workspace without downloading anything; this
tool is only needed to refresh the database.

## Inputs

| File | Source | What it provides |
| --- | --- | --- |
| `cities5000.txt` | <https://download.geonames.org/export/dump/cities5000.zip> | World cities with population >= 5000 (name, ASCII name, lat/lon, country, admin1, population) |
| `admin1CodesASCII.txt` | <https://download.geonames.org/export/dump/admin1CodesASCII.txt> | First-level admin division names ("US.OR" -> "Oregon") |
| `countryInfo.txt` | <https://download.geonames.org/export/dump/countryInfo.txt> | ISO2 -> country name |
| `2024_Gaz_zcta_national.txt` | <https://www2.census.gov/geo/docs/maps-data/data/gazetteer/2024_Gazetteer/2024_Gaz_zcta_national.zip> | US ZIP (ZCTA) centroids |

If the 2024 gazetteer path 404s, use the previous year's directory
(`.../2023_Gazetteer/2023_Gaz_zcta_national.zip`) - the header layout is the
same and the loader locates columns by name (GEOID / INTPTLAT / INTPTLONG).

## Regenerating geo.bin

From the repository root, in PowerShell. GeoNames rejects blank user agents,
so pass a normal one. Downloads land in `tools/geodb-gen/raw/`, which is
gitignored.

```powershell
$raw = "tools\geodb-gen\raw"
New-Item -ItemType Directory -Force $raw | Out-Null
$ua = @{ UserAgent = "Mozilla/5.0 (open-ntrip-client geodb-gen)" }

Invoke-WebRequest @ua https://download.geonames.org/export/dump/cities5000.zip -OutFile $raw\cities5000.zip
Expand-Archive -Force $raw\cities5000.zip $raw
Invoke-WebRequest @ua https://download.geonames.org/export/dump/admin1CodesASCII.txt -OutFile $raw\admin1CodesASCII.txt
Invoke-WebRequest @ua https://download.geonames.org/export/dump/countryInfo.txt -OutFile $raw\countryInfo.txt
Invoke-WebRequest @ua https://www2.census.gov/geo/docs/maps-data/data/gazetteer/2024_Gazetteer/2024_Gaz_zcta_national.zip -OutFile $raw\zcta.zip
Expand-Archive -Force $raw\zcta.zip $raw

cargo run -p geodb-gen --release -- `
    --cities $raw\cities5000.txt `
    --admin1 $raw\admin1CodesASCII.txt `
    --countries $raw\countryInfo.txt `
    --zips $raw\2024_Gaz_zcta_national.txt `
    --out crates\geodb\data\geo.bin
```

The tool prints per-section and total byte sizes. Expect roughly 3-5 MB;
`cargo test -p geodb` enforces a hard 6 MiB ceiling and re-checks known
landmarks (Nairobi, Portland OR, ZIP 97201, Zurich), so run it after any
refresh. Malformed source rows are skipped and counted, never fatal; large
skip counts mean the upstream format shifted and the loader needs a look.

## Data licenses

- GeoNames dumps (`cities5000`, `admin1CodesASCII`, `countryInfo`) are
  licensed [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/),
  attribution: "GeoNames (geonames.org)". Keep the attribution in the
  application's credits.
- US Census Bureau gazetteer files are a work of the United States
  government: public domain.

The compiled `geo.bin` embeds data from both sources; the GeoNames CC BY 4.0
attribution requirement follows it into any distribution.
