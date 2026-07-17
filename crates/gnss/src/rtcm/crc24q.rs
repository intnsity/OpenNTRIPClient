//! CRC-24Q, the RTCM 3.x frame check (also used by GPS CNAV).
//!
//! Parameters: polynomial 0x1864CFB, init 0, no reflection, no final XOR;
//! check value over "123456789" is 0xCDE703. Do not confuse it with the
//! OpenPGP CRC-24 in catalogs, which shares the polynomial but uses init
//! 0xB704CE (check 0x21CF02).

/// Byte-at-a-time table, generated at compile time from the polynomial so
/// the table cannot drift from the definition.
static TABLE: [u32; 256] = build_table();

const POLY: u32 = 0x186_4CFB;

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u32) << 16;
        let mut bit = 0;
        while bit < 8 {
            crc <<= 1;
            if crc & 0x100_0000 != 0 {
                crc ^= POLY;
            }
            bit += 1;
        }
        table[i] = crc & 0xFF_FFFF;
        i += 1;
    }
    table
}

/// CRC-24Q of `data`. The result occupies the low 24 bits.
pub fn crc24q(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in data {
        let idx = ((crc >> 16) ^ u32::from(b)) & 0xFF;
        crc = ((crc << 8) ^ TABLE[idx as usize]) & 0xFF_FFFF;
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent bit-by-bit reference: MSB-first shift register,
    /// conditional XOR with the polynomial. Deliberately shares no code
    /// with the table implementation.
    fn crc24q_naive(data: &[u8]) -> u32 {
        let mut crc = 0u32;
        for &b in data {
            crc ^= u32::from(b) << 16;
            for _ in 0..8 {
                crc <<= 1;
                if crc & 0x100_0000 != 0 {
                    crc ^= 0x186_4CFB;
                }
            }
        }
        crc & 0xFF_FFFF
    }

    #[test]
    fn check_value_is_cde703() {
        // The catalog check value for CRC-24/LTE-A a.k.a. CRC-24Q (init 0).
        // If this fails while the naive cross-check passes, the pinned
        // constant is for the wrong CRC-24 variant - verify against RTCM
        // references before touching anything.
        assert_eq!(crc24q(b"123456789"), 0xCD_E703);
        assert_eq!(crc24q_naive(b"123456789"), 0xCD_E703);
    }

    #[test]
    fn empty_input_is_zero() {
        // Init 0 with no final XOR: an empty message hashes to 0.
        assert_eq!(crc24q(&[]), 0);
    }

    #[test]
    fn single_bytes_match_naive() {
        for b in 0..=255u8 {
            assert_eq!(crc24q(&[b]), crc24q_naive(&[b]), "byte {b:#04x}");
        }
    }

    #[test]
    fn table_matches_naive_over_200_random_buffers() {
        // Fixed-seed LCG (Knuth MMIX constants); no rand dependency.
        let mut s: u64 = 0x5EED_0F0C_24C0_FFEE;
        let mut next = move || {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            s
        };
        for case in 0..200 {
            let len = (next() >> 33) as usize % 65;
            let buf: Vec<u8> = (0..len).map(|_| (next() >> 56) as u8).collect();
            assert_eq!(
                crc24q(&buf),
                crc24q_naive(&buf),
                "case {case}, len {len}, buf {buf:02x?}"
            );
        }
    }

    #[test]
    fn real_frame_crc_verifies() {
        // Canonical 1005 example frame from the RTCM 3 documentation; the
        // trailing three bytes are the transmitted CRC.
        const FRAME: [u8; 25] = [
            0xD3, 0x00, 0x13, 0x3E, 0xD7, 0xD3, 0x02, 0x02, 0x98, 0x0E, 0xDE, 0xEF, 0x34, 0xB4,
            0xBD, 0x62, 0xAC, 0x09, 0x41, 0x98, 0x6F, 0x33, 0x36, 0x0B, 0x98,
        ];
        assert_eq!(crc24q(&FRAME[..22]), 0x36_0B98);
        // Init-0 / no-xorout / non-reflected CRC has the classic residue
        // property: the CRC over frame-plus-appended-CRC is exactly zero.
        // Pinning it documents that the zero-residue validation shortcut IS
        // valid for CRC-24Q, guarding against a false "fix" later.
        assert_eq!(crc24q(&FRAME), 0);
    }
}
