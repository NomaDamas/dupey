//! HWP 3.0 flat binary documents, per Hancom's HWP 3.0 revision 1.2 spec.
//! Counts in paragraph headers are logical hchars; nested control payloads
//! have independent byte lengths. Never substitute partial extraction on error.

mod drawing;
mod encoding;

use std::path::Path;

use super::{CanonicalText, DocMeta, Format};
use crate::error::{Error, Result};

type ParseResult<T> = std::result::Result<T, String>;
const MAX_DEPTH: usize = 128;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> ParseResult<&'a [u8]> {
        let bytes = self
            .data
            .get(self.pos..)
            .and_then(|tail| tail.get(..n))
            .ok_or_else(|| format!("truncated data at byte {} (need {n} bytes)", self.pos))?;
        self.pos += n;
        Ok(bytes)
    }
    fn u8(&mut self) -> ParseResult<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> ParseResult<u16> {
        Ok(le16(self.take(2)?, 0))
    }
    fn u32(&mut self) -> ParseResult<usize> {
        Ok(le32(self.take(4)?, 0))
    }
    fn close(&mut self, code: u16) -> ParseResult<()> {
        let actual = self.u16()?;
        if actual != code {
            return Err(format!("control {code} ends with {actual}"));
        }
        Ok(())
    }
    fn common(&mut self, code: u16) -> ParseResult<usize> {
        let n = self.u32()?;
        self.close(code)?;
        Ok(n)
    }
}

fn le16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}
fn le32(data: &[u8], offset: usize) -> usize {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize
}

pub(super) fn extract_hwp3(path: &Path, bytes: &[u8]) -> Result<CanonicalText> {
    let parse = || -> ParseResult<String> {
        let mut r = Reader::new(bytes);
        let signature = r.take(30)?;
        if !signature.starts_with(b"HWP Document File V3.00") {
            return Err("unsupported legacy HWP version (expected V3.00)".into());
        }
        let info = r.take(128)?;
        if le16(info, 96) != 0 {
            return Err("password-encrypted HWP 3.0 is not supported".into());
        }
        r.take(1008)?; // Summary fields are metadata, not document content.
        r.take(usize::from(le16(info, 126)))?;
        let raw = &bytes[r.pos..];
        let inflated;
        let body = if info[124] != 0 {
            inflated = super::hwp::inflate_raw(path, raw).map_err(|e| e.to_string())?;
            &inflated
        } else {
            raw
        };
        let mut r = Reader::new(body);
        for _ in 0..7 {
            let count = usize::from(r.u16()?);
            r.take(count * 40)?;
        }
        let styles = usize::from(r.u16()?);
        r.take(styles * 238)?;
        paragraph_list(&mut r, 0)
        // Remaining additional-information blocks contain embedded resources.
    };
    let text = parse().map_err(|message| Error::Extract {
        path: path.to_path_buf(),
        message: format!("HWP 3.0: {message}"),
    })?;
    Ok(CanonicalText {
        path: path.to_path_buf(),
        format: Format::Hwp,
        text,
        meta: DocMeta::default(),
    })
}

fn paragraph_list(r: &mut Reader<'_>, depth: usize) -> ParseResult<String> {
    if depth >= MAX_DEPTH {
        return Err("paragraph nesting limit exceeded".into());
    }
    let mut out = String::new();
    loop {
        let header = r.take(43)?;
        let count = usize::from(le16(header, 1));
        if count == 0 {
            return Ok(out);
        }
        if header[0] == 0 {
            r.take(187)?;
        }
        r.take(usize::from(le16(header, 3)) * 14)?;
        if header[5] != 0 {
            for _ in 0..count {
                if r.u8()? != 1 {
                    r.take(31)?;
                }
            }
        }
        let mut used = 0;
        let mut ended = false;
        while used < count {
            let code = r.u16()?;
            if code == 13 {
                if used + 1 != count {
                    return Err("premature paragraph terminator".into());
                }
                out.push('\n');
                ended = true;
                break;
            }
            let width = if code >= 32 {
                out.push_str(&encoding::decode_hchar(code)?);
                1
            } else {
                control(r, code, depth, &mut out)?
            };
            used += width;
            if used >= count {
                return Err("control exceeds paragraph character count".into());
            }
        }
        if !ended {
            return Err("missing paragraph terminator".into());
        }
    }
}

fn nested(r: &mut Reader<'_>, depth: usize, out: &mut String) -> ParseResult<()> {
    out.push('\t'); // Preserve empty cell/object boundaries.
    out.push_str(&paragraph_list(r, depth + 1)?);
    Ok(())
}

fn control(r: &mut Reader<'_>, code: u16, depth: usize, out: &mut String) -> ParseResult<usize> {
    match code {
        5 => {
            let n = r.common(code)?;
            r.take(n)?;
        } // Field metadata; value follows in body.
        6 => {
            r.common(code)?;
            r.take(34)?;
        }
        7 => {
            r.take(80)?;
            r.close(code)?;
        }
        8 => {
            let format = r.take(80)?;
            let date = r.take(12)?;
            r.close(code)?;
            out.push_str(&date_text(format, date)?);
        }
        9 => {
            r.take(4)?;
            r.close(code)?;
            out.push('\t');
        }
        10 => {
            r.common(code)?;
            let info = r.take(84)?;
            let count = usize::from(le16(info, 80)).max(1);
            r.take(count * 27)?;
            for _ in 0..count {
                nested(r, depth, out)?;
            }
            nested(r, depth, out)?; // Caption.
        }
        11 => {
            r.common(code)?;
            let info = r.take(348)?;
            let extension = r.take(le32(info, 0))?;
            if info[74] == 3 {
                let text = drawing::extract_text(extension, &mut |data| {
                    let mut nested_reader = Reader::new(data);
                    let text = paragraph_list(&mut nested_reader, depth + 1)?;
                    if nested_reader.pos != data.len() {
                        return Err("trailing drawing paragraph data".into());
                    }
                    Ok(text)
                })?;
                out.push('\t');
                out.push_str(&text);
            }
            nested(r, depth, out)?;
        }
        14 => {
            r.common(code)?;
            r.take(84)?;
        }
        15..=17 => {
            r.common(code)?;
            r.take(match code {
                15 => 8,
                16 => 10,
                _ => 14,
            })?;
            nested(r, depth, out)?;
        }
        18..=21 => {
            r.u16()?;
            let number = r.u16()?;
            r.close(code)?;
            if code == 18 {
                out.push_str(&number.to_string());
            }
        }
        22 => {
            let field = r.take(20)?;
            r.close(code)?;
            let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
            let name =
                std::str::from_utf8(&field[..end]).map_err(|_| "invalid mail-merge field name")?;
            out.push_str(name);
            return Ok(12);
        }
        23 => {
            for _ in 0..3 {
                let ch = r.u16()?;
                if ch != 0 {
                    out.push_str(&encoding::decode_hchar(ch)?);
                }
            }
            r.close(code)?;
            return Ok(5);
        }
        24 | 25 => {
            r.u16()?;
            r.close(code)?;
            if code == 24 {
                out.push('-');
            }
            return Ok(3);
        }
        26 => {
            r.take(242)?;
            r.close(code)?;
            return Ok(123);
        }
        28 => {
            let data = r.take(60)?;
            r.close(code)?;
            if le16(data, 0) == 1 {
                // Normalize numbering notation rather than implementing page
                // layout. Keep the display selectors and literal decorations:
                // discarding them would collapse different custom bullets.
                let level = usize::from(data[3]);
                if level >= 7 {
                    return Err("invalid outline level".into());
                }
                out.push_str(&format!("⟦outline:{}:", data[2]));
                for i in 0..=level {
                    if i != 0 {
                        out.push('.');
                    }
                    out.push_str(&(u32::from(le16(data, 4 + i * 2)) + 1).to_string());
                }
                if data[2] == 0 || data[2] == 128 {
                    let prefix = le16(data, 32 + level * 4);
                    let suffix = le16(data, 34 + level * 4);
                    let style = le16(data, 18 + level * 2);
                    out.push(':');
                    if prefix != 0 {
                        out.push_str(&encoding::decode_hchar(prefix)?);
                    }
                    match style {
                        0..=12 => out.push_str(&format!("number-style-{style}")),
                        128 => {}
                        ch => out.push_str(&encoding::decode_hchar(ch)?),
                    }
                    if suffix != 0 {
                        out.push_str(&encoding::decode_hchar(suffix)?);
                    }
                }
                out.push_str("⟧ ");
            }
            return Ok(32);
        }
        29 => {
            let n = r.common(code)?;
            let data = r.take(n)?;
            if data.len() < 38 {
                return Err("truncated cross-reference target".into());
            }
            match data[0] {
                0 => {} // A target stores only its name, not reference fields.
                1 => {
                    if data.len() < 46 {
                        return Err("truncated cross-reference".into());
                    }
                    let len = usize::from(le16(data, 40));
                    let text = data
                        .get(46..46 + len)
                        .ok_or("truncated cross-reference text")?;
                    out.push_str(&hchars(text)?);
                }
                kind => return Err(format!("unsupported cross-reference kind {kind}")),
            }
        }
        30 | 31 => {
            r.close(code)?;
            out.push(' ');
            return Ok(2);
        }
        0..=4 | 12 | 27 => {
            let n = r.common(code)?;
            r.take(n)?;
        }
        _ => return Err(format!("unsupported control {code}")),
    }
    Ok(4)
}

fn hchars(bytes: &[u8]) -> ParseResult<String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("odd hchar byte count".into());
    }
    let mut text = String::new();
    for pair in bytes.as_chunks::<2>().0 {
        let ch = le16(pair, 0);
        if ch == 0 {
            break;
        }
        text.push_str(&encoding::decode_hchar(ch)?);
    }
    Ok(text)
}

fn date_text(format: &[u8], date: &[u8]) -> ParseResult<String> {
    let [year, month, weekday, day, hour, minute] = std::array::from_fn(|i| le16(date, i * 2));
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || weekday > 6
        || hour > 23
        || minute > 59
    {
        return Err("invalid date control".into());
    }
    let format = hchars(format)?;
    let mut chars = format.chars();
    let mut out = String::new();
    while let Some(mut ch) = chars.next() {
        if ch == '\\' {
            if let Some(literal) = chars.next() {
                out.push(literal);
            }
            continue;
        }
        let mut chinese = false;
        if ch == '~' {
            chinese = true;
            ch = chars.next().ok_or("incomplete date format")?;
        }
        let mut pad = false;
        if ch == '0' {
            pad = true;
            ch = chars.next().ok_or("incomplete date format")?;
        }
        let number = match ch {
            '1' => Some(year),
            '!' => Some(year % 100),
            '2' => Some(month),
            '3' => Some(day),
            '4' => Some(if hour.is_multiple_of(12) {
                12
            } else {
                hour % 12
            }),
            '$' => Some(hour),
            '5' | '%' => Some(minute),
            _ => None,
        };
        if let Some(n) = number {
            if pad {
                out.push_str(&format!("{n:02}"));
            } else {
                out.push_str(&n.to_string());
            }
            continue;
        }
        match ch {
            '6' => out.push(
                if chinese {
                    ['日', '月', '火', '水', '木', '金', '土']
                } else {
                    ['일', '월', '화', '수', '목', '금', '토']
                }[usize::from(weekday)],
            ),
            '7' => out.push_str(if chinese {
                if hour < 12 {
                    "午前"
                } else {
                    "午後"
                }
            } else if hour < 12 {
                "오전"
            } else {
                "오후"
            }),
            '&' => out.push_str(if hour < 12 { "a.m." } else { "p.m." }),
            '+' => out.push_str(if hour < 12 { "A.M." } else { "P.M." }),
            '@' | '*' => {
                let name = [
                    "January",
                    "February",
                    "March",
                    "April",
                    "May",
                    "June",
                    "July",
                    "August",
                    "September",
                    "October",
                    "November",
                    "December",
                ][usize::from(month - 1)];
                if ch == '@' {
                    out.push_str(&name[..3]);
                } else {
                    out.push_str(name);
                }
            }
            '^' | '_' => {
                let name = [
                    "Sunday",
                    "Monday",
                    "Tuesday",
                    "Wednesday",
                    "Thursday",
                    "Friday",
                    "Saturday",
                ][usize::from(weekday)];
                if ch == '^' {
                    out.push_str(&name[..3]);
                } else {
                    out.push_str(name);
                }
            }
            '#' => {
                out.push_str(&day.to_string());
                out.push_str(if (11..=13).contains(&day) {
                    "th"
                } else {
                    match day % 10 {
                        1 => "st",
                        2 => "nd",
                        3 => "rd",
                        _ => "th",
                    }
                });
            }
            _ => out.push(ch),
        }
    }
    if out.is_empty() {
        out = format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}");
    }
    Ok(out)
}
