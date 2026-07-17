//! Shared key normalization for the geocoding database.
//!
//! `fold` is THE canonical normalization applied to every searchable name by
//! the encoder (`format::encode`) and to every query by [`crate::GeoDb::resolve`].
//! Both sides must agree byte-for-byte on the folded form or lookups silently
//! miss, which is why the function lives in this crate instead of the
//! generator tool.
//!
//! The transform: ASCII is lowercased; letters in the Latin-1 Supplement
//! (U+00C0..=U+00FF) and Latin Extended-A (U+0100..=U+017F) blocks are
//! lowercased and stripped of diacritics, with the conventional
//! multi-character expansions (ae, oe, ss, th, ij); every other character
//! passes through Unicode-lowercased but otherwise untouched, so non-Latin
//! scripts remain searchable verbatim.

/// Appends the folded form of `input` to `out`.
///
/// Appends rather than returns so hot callers can reuse one buffer across
/// many keys; `out` is never cleared here.
pub fn fold(input: &str, out: &mut String) {
    out.reserve(input.len());
    for c in input.chars() {
        // ASCII fast path: the overwhelming majority of keys and queries.
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
        } else {
            fold_non_ascii(c, out);
        }
    }
}

/// Folds one non-ASCII character. Split out of `fold` to keep the hot ASCII
/// loop small. Source files are ASCII-only, hence the escapes; each arm names
/// the letters it covers.
fn fold_non_ascii(c: char, out: &mut String) {
    let mapped: &str = match c {
        // Latin-1 Supplement. U+00D7 (multiplication sign) and U+00F7
        // (division sign) fall through to the pass-through arm on purpose.
        '\u{00C0}'..='\u{00C5}' | '\u{00E0}'..='\u{00E5}' => "a", // A-grave..A-ring
        '\u{00C6}' | '\u{00E6}' => "ae",                          // AE ligature
        '\u{00C7}' | '\u{00E7}' => "c",                           // C-cedilla
        '\u{00C8}'..='\u{00CB}' | '\u{00E8}'..='\u{00EB}' => "e", // E-grave..E-diaeresis
        '\u{00CC}'..='\u{00CF}' | '\u{00EC}'..='\u{00EF}' => "i", // I-grave..I-diaeresis
        '\u{00D0}' | '\u{00F0}' => "d",                           // Eth
        '\u{00D1}' | '\u{00F1}' => "n",                           // N-tilde
        '\u{00D2}'..='\u{00D6}' | '\u{00F2}'..='\u{00F6}' => "o", // O-grave..O-diaeresis
        '\u{00D8}' | '\u{00F8}' => "o",                           // O-stroke
        '\u{00D9}'..='\u{00DC}' | '\u{00F9}'..='\u{00FC}' => "u", // U-grave..U-diaeresis
        '\u{00DD}' | '\u{00FD}' | '\u{00FF}' => "y",              // Y-acute, y-diaeresis
        '\u{00DE}' | '\u{00FE}' => "th",                          // Thorn
        '\u{00DF}' => "ss",                                       // Eszett
        // Latin Extended-A. Upper/lower case variants alternate codepoints,
        // so most letters collapse into one contiguous range.
        '\u{0100}'..='\u{0105}' => "a", // A-macron, A-breve, A-ogonek
        '\u{0106}'..='\u{010D}' => "c", // C-acute..C-caron
        '\u{010E}'..='\u{0111}' => "d", // D-caron, D-stroke
        '\u{0112}'..='\u{011B}' => "e", // E-macron..E-caron
        '\u{011C}'..='\u{0123}' => "g", // G-circumflex..G-cedilla
        '\u{0124}'..='\u{0127}' => "h", // H-circumflex, H-stroke
        '\u{0128}'..='\u{0131}' => "i", // I-tilde..dotless i (incl. dotted I)
        '\u{0132}' | '\u{0133}' => "ij", // IJ ligature
        '\u{0134}' | '\u{0135}' => "j", // J-circumflex
        '\u{0136}'..='\u{0138}' => "k", // K-cedilla, kra
        '\u{0139}'..='\u{0142}' => "l", // L-acute..L-stroke
        '\u{0143}'..='\u{014B}' => "n", // N-acute..Eng
        '\u{014C}'..='\u{0151}' => "o", // O-macron..O-double-acute
        '\u{0152}' | '\u{0153}' => "oe", // OE ligature
        '\u{0154}'..='\u{0159}' => "r", // R-acute..R-caron
        '\u{015A}'..='\u{0161}' => "s", // S-acute..S-caron
        '\u{0162}'..='\u{0167}' => "t", // T-cedilla..T-stroke
        '\u{0168}'..='\u{0173}' => "u", // U-tilde..U-ogonek
        '\u{0174}' | '\u{0175}' => "w", // W-circumflex
        '\u{0176}'..='\u{0178}' => "y", // Y-circumflex, Y-diaeresis
        '\u{0179}'..='\u{017E}' => "z", // Z-acute..Z-caron
        '\u{017F}' => "s",              // long s
        _ => {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            return;
        }
    };
    out.push_str(mapped);
}

#[cfg(test)]
mod tests {
    use super::fold;

    fn folded(s: &str) -> String {
        let mut out = String::new();
        fold(s, &mut out);
        out
    }

    #[test]
    fn ascii_is_lowercased_and_preserved() {
        assert_eq!(folded("Portland, OR 97201!"), "portland, or 97201!");
    }

    #[test]
    fn latin1_diacritics_fold_to_base_letters() {
        assert_eq!(folded("Z\u{00FC}rich"), "zurich"); // u-umlaut
        assert_eq!(folded("S\u{00E3}o Paulo"), "sao paulo"); // a-tilde
        assert_eq!(folded("\u{00C5}rhus"), "arhus"); // A-ring
        assert_eq!(folded("Cura\u{00E7}ao"), "curacao"); // c-cedilla
        assert_eq!(folded("\u{00D6}\u{00F8}"), "oo"); // O-umlaut, o-stroke
    }

    #[test]
    fn multi_char_expansions() {
        assert_eq!(folded("Stra\u{00DF}e"), "strasse"); // eszett
        assert_eq!(folded("\u{00C6}r\u{00F8}"), "aero"); // AE + o-stroke
        assert_eq!(folded("\u{0152}uf"), "oeuf"); // OE ligature
        assert_eq!(folded("\u{00DE}ing"), "thing"); // thorn
        assert_eq!(folded("\u{0132}sselstein"), "ijsselstein"); // IJ ligature
    }

    #[test]
    fn latin_extended_a_folds() {
        assert_eq!(folded("\u{0141}\u{00F3}d\u{017A}"), "lodz"); // L-stroke, o-acute, z-acute
        assert_eq!(folded("Ch\u{0113}b"), "cheb"); // e-macron
        assert_eq!(folded("G\u{0117}l\u{0117}"), "gele"); // e-dot-above
        assert_eq!(folded("\u{010E}\u{0165}"), "dt"); // D-caron, t-caron
        assert_eq!(folded("\u{0130}stanbul"), "istanbul"); // dotted capital I
        assert_eq!(folded("D\u{0131}yar"), "diyar"); // dotless i
    }

    #[test]
    fn non_latin_passes_through_lowercased() {
        assert_eq!(
            folded("\u{041C}\u{043E}\u{0441}\u{043A}\u{0432}\u{0430}"),
            "\u{043C}\u{043E}\u{0441}\u{043A}\u{0432}\u{0430}"
        ); // Moskva, Cyrillic
        assert_eq!(folded("\u{6771}\u{4EAC}"), "\u{6771}\u{4EAC}"); // Tokyo, CJK unchanged
    }

    #[test]
    fn math_signs_in_latin1_are_not_letters() {
        assert_eq!(folded("3\u{00D7}4\u{00F7}2"), "3\u{00D7}4\u{00F7}2");
    }

    #[test]
    fn appends_without_clearing() {
        let mut out = String::from("zip ");
        fold("97201", &mut out);
        assert_eq!(out, "zip 97201");
    }
}
