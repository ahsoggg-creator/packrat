//! Names in old ZIP archives: no UTF-8 flag, no Unicode Path field, just bytes in the code page of whoever made
//! the archive. This module decodes them with a chosen code page and guesses which one is right.
//!
//! `encoding_rs` has no CP437 (the ZIP specification's default), so its table lives here.

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};

const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', //
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', //
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', //
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', //
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', //
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', //
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', //
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

pub fn decode_cp437(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                b as char
            } else {
                CP437_HIGH[(b - 0x80) as usize]
            }
        })
        .collect()
}

/// The code pages a ZIP name can reasonably be in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameEncoding {
    Utf8,
    Cp437,
    Cp866,
    Win1251,
    ShiftJis,
    Gbk,
    Big5,
    EucKr,
}

impl NameEncoding {
    pub const ALL: [NameEncoding; 8] = [
        NameEncoding::Utf8,
        NameEncoding::Cp437,
        NameEncoding::Cp866,
        NameEncoding::Win1251,
        NameEncoding::ShiftJis,
        NameEncoding::Gbk,
        NameEncoding::Big5,
        NameEncoding::EucKr,
    ];

    /// Legacy code pages only (what detection chooses between).
    pub const LEGACY: [NameEncoding; 7] = [
        NameEncoding::Cp437,
        NameEncoding::Cp866,
        NameEncoding::Win1251,
        NameEncoding::ShiftJis,
        NameEncoding::Gbk,
        NameEncoding::Big5,
        NameEncoding::EucKr,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NameEncoding::Utf8 => "UTF-8",
            NameEncoding::Cp437 => "CP437",
            NameEncoding::Cp866 => "CP866",
            NameEncoding::Win1251 => "Windows-1251",
            NameEncoding::ShiftJis => "Shift-JIS",
            NameEncoding::Gbk => "GBK",
            NameEncoding::Big5 => "Big5",
            NameEncoding::EucKr => "EUC-KR",
        }
    }

    /// Accepts the usual spellings: `cp866`, `866`, `ibm866`, `windows-1251`, `sjis`, `euc-kr`...
    pub fn from_name(name: &str) -> Option<NameEncoding> {
        let n = name.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        Some(match n.as_str() {
            "utf-8" | "utf8" => NameEncoding::Utf8,
            "cp437" | "437" | "ibm437" | "oem-us" => NameEncoding::Cp437,
            "cp866" | "866" | "ibm866" | "oem-ru" => NameEncoding::Cp866,
            "cp1251" | "1251" | "windows-1251" | "win1251" => NameEncoding::Win1251,
            "shift-jis" | "shiftjis" | "sjis" | "cp932" => NameEncoding::ShiftJis,
            "gbk" | "gb2312" | "cp936" | "gb18030" => NameEncoding::Gbk,
            "big5" | "cp950" => NameEncoding::Big5,
            "euc-kr" | "euckr" | "cp949" => NameEncoding::EucKr,
            _ => return None,
        })
    }

    fn rs(self) -> Option<&'static encoding_rs::Encoding> {
        Some(match self {
            NameEncoding::Cp866 => encoding_rs::IBM866,
            NameEncoding::Win1251 => encoding_rs::WINDOWS_1251,
            NameEncoding::ShiftJis => encoding_rs::SHIFT_JIS,
            NameEncoding::Gbk => encoding_rs::GBK,
            NameEncoding::Big5 => encoding_rs::BIG5,
            NameEncoding::EucKr => encoding_rs::EUC_KR,
            NameEncoding::Utf8 | NameEncoding::Cp437 => return None,
        })
    }

    /// Decodes `bytes`; the flag says whether anything was malformed (and replaced by U+FFFD).
    pub fn decode(self, bytes: &[u8]) -> (String, bool) {
        match self {
            NameEncoding::Utf8 => match std::str::from_utf8(bytes) {
                Ok(s) => (s.to_owned(), false),
                Err(_) => (String::from_utf8_lossy(bytes).into_owned(), true),
            },
            NameEncoding::Cp437 => (decode_cp437(bytes), false),
            other => {
                let (s, _, bad) = other.rs().expect("legacy code page").decode(bytes);
                (s.into_owned(), bad)
            }
        }
    }
}

/// How plausible a decoded string is as human text: letters of the script score, junk (box drawing, C1 controls,
/// odd symbols, capitals in the middle of Cyrillic words) costs.
fn text_score(text: &str) -> f64 {
    const COMMON_RU: &str = "оеаинтсрвлкмдпуяыьгзбчйхжшюцщэфё";
    let mut score = 0.0;
    let mut prev_letter = false;
    for c in text.chars() {
        let (s, letter) = match c {
            ' '..='~' => (0.0, c.is_ascii_alphabetic()),
            'а'..='я' | 'ё' => (if COMMON_RU.contains(c) { 1.3 } else { 0.7 }, true),
            'А'..='Я' | 'Ё' => (if prev_letter { -0.8 } else { 0.5 }, true),
            '\u{0400}'..='\u{04FF}' => (-1.2, false),
            '\u{2500}'..='\u{259F}' => (-3.0, false),
            '\u{3040}'..='\u{30FF}' => (1.0, true),
            '\u{4E00}'..='\u{9FFF}' => (0.8, true),
            '\u{AC00}'..='\u{D7AF}' => (1.0, true),
            '\u{FF61}'..='\u{FF9F}' => (-1.0, false),
            '\u{00C0}'..='\u{024F}' => (0.5, true),
            '\u{0080}'..='\u{009F}' => (-3.0, false),
            '\u{FFFD}' => (-4.0, false),
            '\u{0370}'..='\u{03FF}' => (-0.8, false),
            '\u{00A0}'..='\u{00BF}' | '\u{2000}'..='\u{2BFF}' => (-1.0, false),
            _ => (-0.5, false),
        };
        score += s;
        prev_letter = letter;
    }
    score
}

#[derive(Clone, Debug)]
pub struct Detection {
    pub best: NameEncoding,
    /// Every legacy code page with its raw score, best first (valid decodings before invalid ones).
    pub ranked: Vec<(NameEncoding, f64)>,
}

/// Guesses the code page of `samples` (raw names that are not UTF-8). The ranking is meant to be shown to the user
/// together with a preview of the decoded names.
pub fn detect(samples: &[&[u8]]) -> Detection {
    let mut joined: Vec<u8> = Vec::new();
    for s in samples {
        if joined.len() > 64 * 1024 {
            break;
        }
        joined.extend_from_slice(s);
        joined.push(b'\n');
    }

    let mut ranked: Vec<(NameEncoding, f64)> = NameEncoding::LEGACY
        .iter()
        .map(|&e| {
            let (text, bad) = e.decode(&joined);
            let non_ascii = joined.iter().filter(|&&b| b >= 0x80).count().max(1) as f64;
            let score = if bad { -1000.0 } else { text_score(&text) / non_ascii };
            (e, score)
        })
        .collect();

    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(&joined, true);
    let guess = detector.guess(None, Utf8Detection::Deny);

    let cjk = if guess == encoding_rs::SHIFT_JIS || guess == encoding_rs::EUC_JP {
        Some(NameEncoding::ShiftJis)
    } else if guess == encoding_rs::GBK || guess == encoding_rs::GB18030 {
        Some(NameEncoding::Gbk)
    } else if guess == encoding_rs::BIG5 {
        Some(NameEncoding::Big5)
    } else if guess == encoding_rs::EUC_KR {
        Some(NameEncoding::EucKr)
    } else {
        None
    };
    let valid = |e: NameEncoding, r: &[(NameEncoding, f64)]| r.iter().any(|(x, s)| *x == e && *s > -999.0);

    let best = match cjk {
        Some(e) if valid(e, &ranked) => e,
        _ => {
            // not a CJK page: Russian pages are told apart by letter statistics, anything else is CP437 (the ZIP default)
            let ru = |e| ranked.iter().find(|(x, _)| *x == e).map_or(-1000.0, |(_, s)| *s);
            let (s866, s1251, s437) = (
                ru(NameEncoding::Cp866),
                ru(NameEncoding::Win1251),
                ru(NameEncoding::Cp437),
            );
            let cyrillic = s866.max(s1251);
            if cyrillic > 0.3 && cyrillic > s437 {
                if s866 >= s1251 {
                    NameEncoding::Cp866
                } else {
                    NameEncoding::Win1251
                }
            } else {
                NameEncoding::Cp437
            }
        }
    };
    ranked.sort_by(|a, b| (b.0 == best).cmp(&(a.0 == best)).then(b.1.total_cmp(&a.1)));
    Detection { best, ranked }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(enc: &'static encoding_rs::Encoding, names: &[&str]) -> Vec<Vec<u8>> {
        names.iter().map(|n| enc.encode(n).0.into_owned()).collect()
    }

    fn cp866(s: &str) -> Vec<u8> {
        encode(encoding_rs::IBM866, &[s]).remove(0)
    }

    fn detect_owned(names: &[Vec<u8>]) -> NameEncoding {
        let refs: Vec<&[u8]> = names.iter().map(|n| n.as_slice()).collect();
        detect(&refs).best
    }

    const RU: &[&str] = &[
        "Документы/Отчёт за 2024 год.docx",
        "Фото/Отпуск на море/IMG_0001.jpg",
        "Музыка/Любимые песни.mp3",
        "Привет мир.txt",
    ];

    #[test]
    fn cp437_basics() {
        assert_eq!(decode_cp437(b"abc"), "abc");
        assert_eq!(decode_cp437(&[0x81, 0x82, 0xe1, 0xfe]), "üéß■");
        assert_eq!(CP437_HIGH.len(), 128);
    }

    #[test]
    fn russian_names_in_cp866_and_cp1251_are_told_apart() {
        assert_eq!(detect_owned(&encode(encoding_rs::IBM866, RU)), NameEncoding::Cp866);
        assert_eq!(
            detect_owned(&encode(encoding_rs::WINDOWS_1251, RU)),
            NameEncoding::Win1251
        );
    }

    #[test]
    fn short_russian_names_are_still_right() {
        for name in ["Привет.txt", "Новая папка", "Инструкция.pdf", "Договор №5.doc"]
        {
            assert_eq!(detect_owned(&[cp866(name)]), NameEncoding::Cp866, "{name} in cp866");
            assert_eq!(
                detect_owned(&encode(encoding_rs::WINDOWS_1251, &[name])),
                NameEncoding::Win1251,
                "{name} in 1251"
            );
        }
    }

    #[test]
    fn cjk_pages_are_recognised() {
        let jp = [
            "日本語のファイル名.txt",
            "写真/ドキュメント/報告書です.docx",
            "音楽/お気に入りの歌.mp3",
        ];
        let zh = [
            "项目文档/使用说明.txt",
            "图片/风景照片集合.jpg",
            "软件/安装程序说明书.doc",
        ];
        let tw = [
            "專案文件/使用說明.txt",
            "圖片/風景照片集合.jpg",
            "軟體/安裝程式說明書.doc",
        ];
        let kr = [
            "한국어 문서/사진 모음.jpg",
            "음악/좋아하는 노래.mp3",
            "프로그램/설치 안내서.txt",
        ];
        assert_eq!(
            detect_owned(&encode(encoding_rs::SHIFT_JIS, &jp)),
            NameEncoding::ShiftJis
        );
        assert_eq!(detect_owned(&encode(encoding_rs::GBK, &zh)), NameEncoding::Gbk);
        assert_eq!(detect_owned(&encode(encoding_rs::BIG5, &tw)), NameEncoding::Big5);
        assert_eq!(detect_owned(&encode(encoding_rs::EUC_KR, &kr)), NameEncoding::EucKr);
    }

    #[test]
    fn western_names_fall_back_to_cp437() {
        // "Café/Überraschung.txt" in CP437
        let name: Vec<u8> = vec![
            b'C', b'a', b'f', 0x82, b'/', 0x9a, b'b', b'e', b'r', b'r', b'a', b's', b'c', b'h', b'u', b'n', b'g', b'.',
            b't', b'x', b't',
        ];
        assert_eq!(detect_owned(std::slice::from_ref(&name)), NameEncoding::Cp437);
        assert_eq!(NameEncoding::Cp437.decode(&name).0, "Café/Überraschung.txt");
    }

    #[test]
    fn ranking_lists_every_legacy_page_and_decode_roundtrips() {
        let names = encode(encoding_rs::IBM866, RU);
        let refs: Vec<&[u8]> = names.iter().map(|n| n.as_slice()).collect();
        let d = detect(&refs);
        assert_eq!(d.ranked.len(), NameEncoding::LEGACY.len());
        assert_eq!(d.ranked[0].0, NameEncoding::Cp866);
        assert_eq!(NameEncoding::Cp866.decode(&names[0]).0, RU[0]);
    }

    #[test]
    fn names_can_be_parsed_from_user_text() {
        assert_eq!(NameEncoding::from_name("CP866"), Some(NameEncoding::Cp866));
        assert_eq!(NameEncoding::from_name("windows-1251"), Some(NameEncoding::Win1251));
        assert_eq!(NameEncoding::from_name("Shift_JIS"), Some(NameEncoding::ShiftJis));
        assert_eq!(NameEncoding::from_name("euc-kr"), Some(NameEncoding::EucKr));
        assert_eq!(NameEncoding::from_name("klingon"), None);
    }
}
