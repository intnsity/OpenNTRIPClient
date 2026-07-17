//! WGS-84 geodesy: ECEF <-> geodetic conversion and the two distance
//! measures the client displays (3-D distance between base positions,
//! great-circle distance from rover to base).

/// WGS-84 semi-major axis, meters.
const A: f64 = 6_378_137.0;
/// WGS-84 flattening.
const F: f64 = 1.0 / 298.257_223_563;
/// First eccentricity squared, e^2 = f (2 - f).
const E2: f64 = F * (2.0 - F);
/// Semi-minor axis, b = a (1 - f).
const B: f64 = A * (1.0 - F);
/// Second eccentricity squared, e'^2 = (a^2 - b^2) / b^2.
const EP2: f64 = (A * A - B * B) / (B * B);

/// Geodetic (lat deg, lon deg, ellipsoidal height m) -> ECEF meters.
pub fn lla_to_ecef(lat_deg: f64, lon_deg: f64, alt_m: f64) -> (f64, f64, f64) {
    let lat = lat_deg.to_radians();
    let lon = lon_deg.to_radians();
    let (sin_lat, cos_lat) = lat.sin_cos();
    let (sin_lon, cos_lon) = lon.sin_cos();
    // Prime vertical radius of curvature.
    let n = A / (1.0 - E2 * sin_lat * sin_lat).sqrt();
    (
        (n + alt_m) * cos_lat * cos_lon,
        (n + alt_m) * cos_lat * sin_lon,
        (n * (1.0 - E2) + alt_m) * sin_lat,
    )
}

/// ECEF meters -> geodetic (lat deg, lon deg, ellipsoidal height m).
///
/// Bowring's closed-form parametric-latitude solution: for terrestrial
/// altitudes its latitude error is below 1e-11 rad, orders of magnitude
/// inside this crate's 1e-6 degree contract, with no iteration.
pub fn ecef_to_lla(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
    // Bowring degenerates at the ECEF origin (lat comes out as 180 deg).
    // Return the exact inverse of lla_to_ecef(0, 0, -A) instead; callers that
    // treat all-zero ECEF as "position not set" should check before calling.
    if x == 0.0 && y == 0.0 && z == 0.0 {
        return (0.0, 0.0, -A);
    }
    let p = (x * x + y * y).sqrt();
    let theta = (z * A).atan2(p * B);
    let (st, ct) = theta.sin_cos();
    let lat = (z + EP2 * B * st * st * st).atan2(p - E2 * A * ct * ct * ct);
    let lon = y.atan2(x);
    let sin_lat = lat.sin();
    let n = A / (1.0 - E2 * sin_lat * sin_lat).sqrt();
    // h = p cos(lat) + z sin(lat) - N (1 - e^2 sin^2(lat)): algebraically
    // equal to p / cos(lat) - N but stable at the poles where cos -> 0.
    let alt = p * lat.cos() + z * sin_lat - n * (1.0 - E2 * sin_lat * sin_lat);
    (lat.to_degrees(), lon.to_degrees(), alt)
}

/// Straight-line (chord) distance between two ECEF points, meters.
pub fn ecef_distance_m(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    let dz = a.2 - b.2;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Great-circle distance on the mean-radius sphere (R = 6 371 000 m), meters.
/// Good to ~0.5% versus the ellipsoid - plenty for "how far is the base".
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let p1 = lat1.to_radians();
    let p2 = lat2.to_radians();
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let sp = (dp / 2.0).sin();
    let sl = (dl / 2.0).sin();
    let h = sp * sp + p1.cos() * p2.cos() * sl * sl;
    // asin form is exact for antipodes where the atan2 form's h can round
    // slightly above 1; clamp guards that same rounding here.
    2.0 * R * h.sqrt().min(1.0).asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_front_range_vector() {
        // lat 40 N, lon 105 W, 1600 m: reference ECEF computed independently
        // (PowerShell IEEE-754 doubles, same WGS-84 constants).
        let (x, y, z) = lla_to_ecef(40.0, -105.0, 1600.0);
        assert!((x - -1_266_643.136_0).abs() < 1e-3, "x {x}");
        assert!((y - -4_727_176.538_8).abs() < 1e-3, "y {y}");
        assert!((z - 4_079_014.032_4).abs() < 1e-3, "z {z}");

        let (lat, lon, alt) = ecef_to_lla(x, y, z);
        assert!((lat - 40.0).abs() < 1e-8, "lat {lat}");
        assert!((lon - -105.0).abs() < 1e-8, "lon {lon}");
        assert!((alt - 1600.0).abs() < 1e-3, "alt {alt}");
    }

    #[test]
    fn roundtrip_property_100_points() {
        // Fixed-seed LCG; unit floats from the top 53 bits.
        let mut s: u64 = 0xDEAD_BEEF_CAFE_F00D;
        let mut unit = move || {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (s >> 11) as f64 / (1u64 << 53) as f64
        };
        for i in 0..100 {
            let lat = unit() * 179.8 - 89.9;
            let lon = unit() * 360.0 - 180.0;
            let alt = unit() * 9_500.0 - 500.0;
            let (x, y, z) = lla_to_ecef(lat, lon, alt);
            let (lat2, lon2, alt2) = ecef_to_lla(x, y, z);
            assert!((lat - lat2).abs() < 1e-6, "case {i}: lat {lat} vs {lat2}");
            assert!((lon - lon2).abs() < 1e-6, "case {i}: lon {lon} vs {lon2}");
            assert!((alt - alt2).abs() < 1e-3, "case {i}: alt {alt} vs {alt2}");
        }
    }

    #[test]
    fn poles_and_equator_altitude_is_stable() {
        for &(lat, lon) in &[(90.0, 0.0), (-90.0, 0.0), (0.0, 0.0), (0.0, 180.0)] {
            let (x, y, z) = lla_to_ecef(lat, lon, 123.456);
            let (lat2, _, alt2) = ecef_to_lla(x, y, z);
            assert!((lat - lat2).abs() < 1e-6, "lat at {lat},{lon}");
            assert!((alt2 - 123.456).abs() < 1e-3, "alt at {lat},{lon}: {alt2}");
        }
    }

    #[test]
    fn ecef_distance_euclidean() {
        assert_eq!(ecef_distance_m((0.0, 0.0, 0.0), (3.0, 4.0, 0.0)), 5.0);
        let p = lla_to_ecef(40.0, -105.0, 1600.0);
        assert!(ecef_distance_m(p, p) == 0.0);
    }

    #[test]
    fn haversine_pinned_values() {
        // One degree of longitude on the equator: R * pi / 180.
        let d = haversine_m(0.0, 0.0, 0.0, 1.0);
        assert!((d - 111_194.926_6).abs() < 0.01, "{d}");
        // Warsaw -> Rome, independently computed with the same sphere.
        let d = haversine_m(52.229_675_6, 21.012_228_7, 41.891_930_0, 12.511_330_0);
        assert!((d - 1_315_510.156).abs() < 0.5, "{d}");
        assert_eq!(haversine_m(45.0, 7.0, 45.0, 7.0), 0.0);
        // Antipodal points: half the circumference, no NaN from rounding.
        let d = haversine_m(0.0, 0.0, 0.0, 180.0);
        assert!((d - std::f64::consts::PI * 6_371_000.0).abs() < 1e-3, "{d}");
    }
}
