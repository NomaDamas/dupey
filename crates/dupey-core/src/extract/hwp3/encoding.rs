//! Strict HWP 3.0 hchar decoding (body words are little-endian, not UTF-16).
//!
//! HWP-specific symbol mappings and araea handling adapted from rhwp's
//! `src/parser/hwp3/johab.rs`, Copyright (c) 2025-2026 Edward Kim (MIT).
//! Reference: https://github.com/edwardkim/rhwp/tree/680111ec7bea2fe11110de18c3676ba5a1cf7847
//! See LICENSE-rhwp.txt. Standard KS X 1001 mappings use encoding_rs;
//! no platform codec or external process is required.

const VOWELS: [i32; 32] = [
    -1, -1, -1, 0, 1, 2, 3, 4, -1, -1, 5, 6, 7, 8, 9, 10, -1, -1, 11, 12, 13, 14, 15, 16, -1, -1,
    17, 18, 19, 20, -1, -1,
];
const FINALS: [i32; 32] = [
    -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, -1, 17, 18, 19, 20, 21, 22, 23,
    24, 25, 26, 27, -1, -1,
];

pub(super) fn decode_hchar(code: u16) -> Result<String, String> {
    let decoded = if code < 0x80 {
        Some(char::from(code as u8).to_string())
    } else if code < 0x8000 {
        decode_hwp_symbol(code).map(|c| c.to_string())
    } else {
        decode_hangul(code).or_else(|| decode_standard_symbol(code).map(|c| c.to_string()))
    };
    // A placeholder would make distinct unsupported text hash identically.
    decoded.ok_or_else(|| format!("unsupported HWP 3.0 character 0x{code:04X}"))
}

fn decode_hangul(code: u16) -> Option<String> {
    let initial = usize::from((code >> 10) & 31);
    let vowel = usize::from((code >> 5) & 31);
    let final_index = usize::from(code & 31);
    let trailing = FINALS[final_index];
    if !(1..=20).contains(&initial) || trailing < 0 {
        return None;
    }

    // Araea has no precomposed Unicode syllable; retain all its conjoining jamo.
    if vowel == 30 {
        let mut text = String::new();
        if initial != 1 {
            text.push(char::from_u32(0x1100 + initial as u32 - 2)?);
        }
        text.push('\u{119E}');
        if trailing != 0 {
            text.push(char::from_u32(0x11A7 + trailing as u32)?);
        }
        return Some(text);
    }

    let medial = VOWELS[vowel];
    if initial >= 2 && medial >= 0 {
        return char::from_u32(
            0xAC00 + (initial as u32 - 2) * 588 + medial as u32 * 28 + trailing as u32,
        )
        .map(|c| c.to_string());
    }

    // Filled components represent standalone compatibility jamo.
    let character = match (initial, vowel, trailing) {
        (1, 2, 0) => Some('\u{3000}'),
        (1, 2, _) => "ㄱㄲㄳㄴㄵㄶㄷㄹㄺㄻㄼㄽㄾㄿㅀㅁㅂㅄㅅㅆㅇㅈㅊㅋㅌㅍㅎ"
            .chars()
            .nth(trailing as usize - 1),
        (1, _, 0) if medial >= 0 => char::from_u32(0x314F + medial as u32),
        (_, 2, 0) => "ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ"
            .chars()
            .nth(initial - 2),
        _ => None,
    };
    character.map(|c| c.to_string())
}

fn ksc_character(row: u16, cell: u16) -> Option<char> {
    if !(0xA1..=0xFE).contains(&row) || !(0xA1..=0xFE).contains(&cell) {
        return None;
    }
    let bytes = [row as u8, cell as u8];
    let text = encoding_rs::EUC_KR.decode_without_bom_handling_and_without_replacement(&bytes)?;
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(c)
}

fn decode_standard_symbol(code: u16) -> Option<char> {
    // Standard Johab has two KS X 1001 rows per lead byte, with a trail gap.
    let high = code >> 8;
    let low = code & 0xFF;
    let row = match high {
        0xD9..=0xDE => 0xA1 + (high - 0xD9) * 2,
        0xE0..=0xF9 => 0xCA + (high - 0xE0) * 2,
        _ => return None,
    };
    let index = match low {
        0x31..=0x7E => low - 0x31,
        0x91..=0xFE => low - 0x91 + 78,
        _ => return None,
    };
    // Compatibility jamo are encoded by the Hangul bitfields, not this block.
    if high == 0xDA && (0xA1..=0xD3).contains(&low) {
        return None;
    }
    ksc_character(row + index / 94, 0xA1 + index % 94)
}

fn decode_hwp_symbol(code: u16) -> Option<char> {
    if (0x00A0..=0x00FF).contains(&code) {
        return char::from_u32(u32::from(code));
    }
    if (0x37C0..=0x37C5).contains(&code) {
        return char::from_u32(0xF03EF + u32::from(code - 0x37C0));
    }
    private_symbol(code).or_else(|| {
        // HWP's own symbol and Hanja coordinates differ from standard Johab.
        let character = match code {
            0x3401..=0x387E => {
                let index = code - 0x3401;
                ksc_character(0xA1 + index / 96, 0xA1 + index % 96)
            }
            0x4000..=0x5317 => {
                let index = code - 0x4000;
                ksc_character(0xCA + index / 94, 0xA1 + index % 94)
            }
            _ => None,
        }?;
        // HWP's glyph choices, established by rhwp's Hancom conversion pairs.
        Some(match character {
            '\u{223C}' => '\u{FF5E}',
            '\u{2299}' => '\u{25C9}',
            other => other,
        })
    })
}

fn private_symbol(code: u16) -> Option<char> {
    let codepoint: u32 = match code {
        // Matched by adjacent text against SO-SUEOP.hwpx's Hancom conversion:
        // 13 opening / 14 closing quotes, 18 arrows, 1 dash, 4 dots, 3 bracket pairs.
        0x3062 => 0x201C,
        0x3063 => 0x201D,
        0x21B3 => 0x21B3,
        0x2014 => 0x2014,
        0x3066 => 0x2027,
        0x30BB => 0xF0854,
        0x30BD => 0xF0855,
        // Identity inferred from hwp3-empty-cell ticket dimension diagrams:
        // 62 horizontal bars bounded by left/right arrows; 2 upward arrows
        // opposite downward arrows. No conversion pair was available for this
        // fixture; these individual mappings do not imply range-wide identity.
        0x2015 => 0x2015,
        0x2191 => 0x2191,
        // Six matching diagram dividers in SO-SUEOP.hwp / SO-SUEOP.hwpx.
        0x2223 => 0x2223,
        // Hollow square bullet: HWP 2F00 -> KS X 1001 A1E0 -> U+25A1.
        // Confirmed by LibreOffice hwpfilter/source/hcode.cxx, hcharconv.
        0x2F00 => 0x25A1,
        0x0081 => 0x201C,  // 왼쪽 큰따옴표
        0x0082 => 0x201D,  // 오른쪽 큰따옴표
        0x0083 => 0x2018,  // ‘
        0x0084 => 0x2019,  // ’
        0x301E => 0xF0811, // 한컴 PUA - 관계도 가지 선문자
        0x301C => 0xF080F, // 한컴 PUA - 굵은 가로선 (94.5% 발생)
        0x3024 => 0xF0817, // 한컴 PUA - 관계도 하단 가지 선문자
        0x3027 => 0xF081A, // 한컴 PUA - 관계도 가로 선문자
        0x3404 => 0x2024,  // 한 점 리더
        0x3446 => 0x2192,  // 오른쪽 화살표
        0x35E1 => 0x2500,  // 상자 그리기 가로선
        0x303D => 0xF0827, // 한컴 PUA
        0x3479 => 0x25B7,  // ▷ WHITE RIGHT-POINTING TRIANGLE
        0x347A => 0x25B6,  // ▶ BLACK RIGHT-POINTING TRIANGLE
        0x3441 => 0x25A0,  // ■ BLACK SQUARE
        0x2F67 => 0x25B8,  // ▸ BLACK RIGHT-POINTING SMALL TRIANGLE
        0x3366 => 0xF03C5,
        0x203B => 0x203B,
        0x2010 => 0x2010, // ‐  07615(1)
        0x2013 => 0x2013, // –  07270(78)
        0x2103 => 0x2103, // ℃  07270(9)
        0x2113 => 0x2113, // ℓ  05434(5)
        0x2190 => 0x2190, // ←  07270(5)
        0x2192 => 0x2192, // →  07270(3)
        0x2193 => 0x2193, // ↓  07270(7)
        0x2219 => 0x2219, // ∙  07270(57)
        0x22C5 => 0x22C5, // ⋅  07615(3)
        0x2024 => 0x30FB, // ・  07270(4)
        0x2058 => 0x25B3, // △  05434(1)
        0x205A => 0x25CB, // ○  04405(2)·04566(3)·05555(5)
        0x2F08 => 0x25AA, // ▪  07270(16)
        0x2F11 => 0x25E6, // ◦  06109(3)
        0x2F14 => 0x25E6, // ◦  07270(49)·07615(60)
        0x3157 => 0x2027, // ‧  04640(1)
        0x0480 => 0x02D0, // ː  07270(71)
        0x1F2E => 0x306E, // の  07270(1)
        0x32B0 => 0xFF70, // ｰ (반각)  07270(1)
        0x3067 => 0x2018, // ‘  07615(2)
        0x3068 => 0x2019, // ’  07615(2)
        0x309B => 0xFF62, // ｢  07615(7)
        0x309D => 0xFF63, // ｣  07615(7)
        0x3013 => 0xF0806,
        0x3014 => 0xF0807,
        0x3015 => 0xF0808,
        0x3019 => 0xF080C,
        0x301B => 0xF080E,
        0x301D => 0xF0810,
        0x37ED => 0x2550,
        0x2E01 => 0x2460,
        0x2E02 => 0x2461,
        0x2E03 => 0x2462,
        0x2E04 => 0x2463,
        0x2E05 => 0x2464,
        0x2E06 => 0x2465,
        0x2E07 => 0x2466,
        0x2E00 => 0xF0288,
        0x2E0A => 0xF0289,
        0x2E0B => 0xF028A,
        0x2E0D => 0xF028C,
        0x2E0E => 0xF028D,
        0x2E0F => 0xF028E,
        0x2E10 => 0xF028F,
        0x2E11 => 0xF0290,
        0x2E12 => 0xF0291,
        0x2C21 => 0x24D0, // ⓐ  sample11 개수 1·1·2·2·2·3 ↔ 변환본 ⓐ~ⓕ 동수
        0x2C22 => 0x24D1,
        0x2C23 => 0x24D2,
        0x2C24 => 0x24D3,
        0x2C25 => 0x24D4,
        0x2C26 => 0x24D5,
        0x2C40 => 0x3260, // ㉠  sample11 각 1건
        0x2C41 => 0x3261,
        0x2C42 => 0x3262,
        0x2022 => 0x2022,  // •  sample(4) — 유니코드 값 그대로 담은 자리
        0x2F17 => 0x2022,  // •  sample10(3)
        0x2F06 => 0x25A0,  // ■  sample10 "제목차례" 좌우 장식(2)
        0x25F5 => 0xF0099, // 04759(1)
        0x2BCE => 0xF012B,
        0x3048 => 0xF0832, // 06109(61)
        _ => return None,
    };
    char::from_u32(codepoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_korean_report_and_keeps_date_and_amount_digits() {
        // Standard Johab values, with ASCII stored as full little-endian hchars.
        let codes = [0xB7A9, 0xB7A9, 0x20, 0xA5A1, 0x89A1, 0xACE1];
        let text: String = codes
            .into_iter()
            .map(|c| decode_hchar(c).unwrap())
            .collect();
        assert_eq!(text, "일일 보고서");
        for text in ["2026-09-15 100", "2026-09-16 101"] {
            let decoded: String = text
                .bytes()
                .map(|c| decode_hchar(u16::from(c)).unwrap())
                .collect();
            assert_eq!(decoded, text);
        }
        assert_eq!(decode_hchar(0x8861).unwrap(), "가");
        assert_eq!(decode_hchar(0xD3BD).unwrap(), "힣");
        assert_eq!(decode_hchar(0x003F).unwrap(), "?");
    }

    #[test]
    fn decodes_hwp_hanja_symbols_and_distinguishes_private_codes() {
        for (code, expected) in [
            (0x4F5D, "債"),
            (0x4222, "權"),
            (0x3438, "※"),
            (0x343B, "○"),
            (0x3440, "□"),
            (0x3446, "→"),
            (0x3590, "Ⅰ"),
            (0x36E7, "①"),
            (0x205A, "○"),
            (0x2024, "・"),
            (0x0081, "“"),
            (0x00FC, "ü"),
            (0x37C1, "\u{F03F0}"),
            (0x2F67, "▸"),
            (0xD932, "、"),
            (0xF9FE, "詰"),
        ] {
            assert_eq!(decode_hchar(code).unwrap(), expected, "code 0x{code:04X}");
        }
    }

    #[test]
    fn decodes_legacy_diagram_dividers_and_hollow_square_bullets() {
        // SO-SUEOP.hwp and its Hancom HWPX conversion contain six matching
        // dividers; these are individual identity codes, not a Unicode range.
        assert_eq!(decode_hchar(0x2223).unwrap(), "∣");
        // hwp3-empty-cell.hwp: "□ 입 장 권". The same bullet appears before
        // institution names in hwp3-table-caption.hwp.
        assert_eq!(decode_hchar(0x2F00).unwrap(), "□");
        assert_ne!(decode_hchar(0x2F00).unwrap(), decode_hchar(0x2F06).unwrap());
    }

    #[test]
    fn decodes_legacy_quotation_marks_and_diagram_annotations() {
        // SO-SUEOP's HWPX conversion retains these exact values, including PUA
        // corner brackets; do not replace the brackets with guessed glyphs.
        for (code, expected) in [
            (0x3062, "“"),
            (0x3063, "”"),
            (0x21B3, "↳"),
            (0x2014, "—"),
            (0x3066, "‧"),
            (0x30BB, "\u{F0854}"),
            (0x30BD, "\u{F0855}"),
        ] {
            assert_eq!(decode_hchar(code).unwrap(), expected);
        }
    }

    #[test]
    fn preserves_dimension_line_characters_in_ticket_diagrams() {
        // hwp3-empty-cell.hwp uses repeated horizontal bars between arrows.
        let line = [0x2190, 0x2015, 0x2015, 0x2192];
        let decoded: String = line.into_iter().map(|c| decode_hchar(c).unwrap()).collect();
        assert_eq!(decoded, "←――→");
        assert_eq!(decode_hchar(0x2191).unwrap(), "↑");
        assert_eq!(decode_hchar(0x2193).unwrap(), "↓");
    }

    #[test]
    fn decodes_standalone_jamo_and_archaic_araea_without_losing_letters() {
        for (code, expected) in [
            (0x8841, "ㄱ"),
            (0x8444, "ㄳ"),
            (0xA841, "ㅃ"),
            (0x8461, "ㅏ"),
            (0x87A1, "ㅣ"),
            (0x8441, "　"),
            (0xD3C5, "ᄒᆞᆫ"),
            (0x87C1, "ᆞ"),
        ] {
            assert_eq!(decode_hchar(code).unwrap(), expected, "code 0x{code:04X}");
        }
    }

    #[test]
    fn unknown_and_reserved_codes_are_errors_instead_of_collapsing_text() {
        for code in [
            0x0085, 0x2059, 0x2F09, 0x8860, 0x8872, 0x887E, 0x887F, 0xFFFF,
        ] {
            let error = decode_hchar(code).unwrap_err();
            assert!(error.contains(&format!("0x{code:04X}")), "{error}");
        }
    }
}
