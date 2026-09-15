//! Text in HWP 3.x drawing objects (file format specification, section 11).

const MAX_OBJECTS: usize = 4096;
const MAX_DEPTH: usize = 128;
const MAX_TEXT_BYTES: usize = 64 * 1024 * 1024;

struct Reader<'a> {
    remaining: &'a [u8],
}

#[derive(Default)]
struct Counts {
    nodes: usize,
    objects: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        if length > self.remaining.len() {
            return Err("truncated HWP 3.x drawing data".into());
        }
        let (bytes, remaining) = self.remaining.split_at(length);
        self.remaining = remaining;
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<usize, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()) as usize)
    }

    fn block(&mut self) -> Result<&'a [u8], String> {
        let length = self.u32()?;
        self.take(length)
    }
}

pub(super) fn extract_text(
    data: &[u8],
    parse_list: &mut impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<String, String> {
    let mut reader = Reader { remaining: data };
    // Frame and object header lengths exclude their own four-byte length field.
    let frame = reader.block()?;
    if frame.len() < 24 {
        return Err("invalid HWP 3.x drawing frame header".into());
    }
    let count = u32::from_le_bytes(frame[4..8].try_into().unwrap()) as usize;
    if count > MAX_OBJECTS {
        return Err("too many HWP 3.x drawing objects".into());
    }
    let mut text = String::new();
    let mut seen = Counts::default();
    if count != 0 {
        walk(&mut reader, 0, &mut seen, &mut text, parse_list)?;
    }
    // Group containers describe the tree but are excluded from the frame's
    // declared object count. Both groups and shapes count toward our node cap.
    if seen.objects != count || !reader.remaining.is_empty() {
        return Err("HWP 3.x drawing object count or data length mismatch".into());
    }
    Ok(text)
}

fn walk(
    reader: &mut Reader<'_>,
    depth: usize,
    seen: &mut Counts,
    text: &mut String,
    parse_list: &mut impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<(), String> {
    if depth >= MAX_DEPTH {
        return Err("HWP 3.x drawing nesting limit exceeded".into());
    }
    loop {
        if seen.nodes >= MAX_OBJECTS {
            return Err("too many HWP 3.x drawing nodes".into());
        }
        seen.nodes += 1;
        let (kind, links) = read_object(reader, text, parse_list)?;
        if kind != 0 {
            seen.objects += 1;
        }
        if links & 2 != 0 {
            walk(reader, depth + 1, seen, text, parse_list)?;
        }
        if links & 1 == 0 {
            return Ok(());
        }
    }
}

fn read_object(
    reader: &mut Reader<'_>,
    text: &mut String,
    parse_list: &mut impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<(u16, u16), String> {
    let header = reader.block()?;
    if header.len() < 88 {
        return Err("invalid HWP 3.x drawing object header".into());
    }
    let kind = u16::from_le_bytes(header[..2].try_into().unwrap());
    let links = u16::from_le_bytes(header[2..4].try_into().unwrap());
    let options = u32::from_le_bytes(header[84..88].try_into().unwrap());
    if kind > 11 || links & !3 != 0 {
        return Err(format!(
            "unsupported HWP 3.x drawing type {kind} or links {links}"
        ));
    }
    let mut attributes = Reader {
        remaining: &header[88..],
    };
    let other_attributes = usize::from(options & (1 << 16) != 0) * 28
        + usize::from(options & (1 << 18) != 0) * 278
        + usize::from(options & (1 << 20) != 0) * 3;
    // The specification explicitly permits the rotation flag without stored
    // rotation attributes. Only omit them when the declared header lacks room.
    if options & (1 << 17) != 0 && attributes.remaining.len() >= other_attributes + 32 {
        attributes.take(32)?;
    }
    attributes.take(other_attributes)?;
    let mut had_textbox = false;
    if !attributes.remaining.is_empty() {
        // Some writers include optional textbox data in the declared header;
        // others put it immediately after the header, as section 11.3 describes.
        read_textbox(&mut attributes, text, parse_list)?;
        if !attributes.remaining.is_empty() {
            return Err("unrecognized HWP 3.x drawing header extension".into());
        }
        had_textbox = true;
    }
    if options & (1 << 19) != 0 && kind != 6 && !had_textbox {
        read_textbox(reader, text, parse_list)?;
    }
    if kind == 6 {
        read_textbox(reader, text, parse_list)?;
        return Ok((kind, links));
    }
    if kind == 9 {
        // Rotated arcs use their rotation attributes and have no detail blocks.
        return Ok((kind, links));
    }
    let info1 = reader.block()?;
    let info2 = reader.block()?;
    let valid = match kind {
        0 | 2 | 3 => info1.is_empty() && info2.is_empty(),
        1 | 4 => info1.len() == 4 && info2.is_empty(),
        8 => info1.len() == 16 && info2.is_empty(),
        5 | 7 | 10 | 11 => {
            if info1.len() != 4 {
                false
            } else {
                let points = u32::from_le_bytes(info1.try_into().unwrap()) as usize;
                let matches = points.checked_mul(8) == Some(info2.len());
                if matches && matches!(kind, 10 | 11) {
                    reader.take(points)?;
                }
                matches
            }
        }
        _ => false,
    };
    if !valid {
        return Err(format!(
            "invalid HWP 3.x drawing detail blocks for type {kind}"
        ));
    }
    Ok((kind, links))
}

fn read_textbox(
    reader: &mut Reader<'_>,
    text: &mut String,
    parse_list: &mut impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<(), String> {
    if !reader.block()?.is_empty() {
        return Err("unrecognized HWP 3.x drawing textbox information".into());
    }
    let paragraphs = reader.block()?;
    if !paragraphs.is_empty() {
        let value = parse_list(paragraphs)?;
        if value.len() >= MAX_TEXT_BYTES.saturating_sub(text.len()) {
            return Err("HWP 3.x drawing text size limit exceeded".into());
        }
        text.push_str(&value);
        text.push('\n');
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(count: u32, objects: &[u8]) -> Vec<u8> {
        let mut data = vec![0; 28];
        data[..4].copy_from_slice(&24u32.to_le_bytes());
        data[8..12].copy_from_slice(&count.to_le_bytes());
        data.extend_from_slice(objects);
        data
    }

    fn textbox_info(text: &[u8]) -> Vec<u8> {
        let mut data = vec![0; 4];
        data.extend_from_slice(&(text.len() as u32).to_le_bytes());
        data.extend_from_slice(text);
        data
    }

    fn object(kind: u16, links: u16, options: u32, extra: &[u8], details: &[u8]) -> Vec<u8> {
        let mut data = vec![0; 92];
        data[..4].copy_from_slice(&(88u32 + extra.len() as u32).to_le_bytes());
        data[4..6].copy_from_slice(&kind.to_le_bytes());
        data[6..8].copy_from_slice(&links.to_le_bytes());
        data[88..92].copy_from_slice(&options.to_le_bytes());
        data.extend_from_slice(extra);
        data.extend_from_slice(details);
        data
    }

    fn extract(data: &[u8]) -> Result<String, String> {
        extract_text(data, &mut |bytes| {
            String::from_utf8(bytes.to_vec()).map_err(|error| error.to_string())
        })
    }

    #[test]
    fn drawing_textbox_values_are_kept_distinct() {
        let first = frame(1, &object(6, 0, 0, &[], &textbox_info(b"2026-09-15: 100")));
        let second = frame(1, &object(6, 0, 0, &[], &textbox_info(b"2026-09-16: 101")));
        assert_eq!(extract(&first).unwrap().trim(), "2026-09-15: 100");
        assert_ne!(extract(&first).unwrap(), extract(&second).unwrap());
    }

    #[test]
    fn traverses_children_and_siblings_with_text_boundaries() {
        let mut objects = object(0, 3, 0, &[], &[0; 8]);
        objects.extend(object(6, 1, 0, &[], &textbox_info(b"first")));
        objects.extend(object(6, 0, 0, &[], &textbox_info(b"second")));
        objects.extend(object(6, 0, 0, &[], &textbox_info(b"third")));
        assert_eq!(
            extract(&frame(3, &objects)).unwrap(),
            "first\nsecond\nthird\n"
        );
    }

    #[test]
    fn declared_object_count_excludes_nested_group_containers() {
        // The real issue1892_hwp3_drawing_group_roundtrip document declares
        // three objects for this five-node structure: two grouping containers,
        // two lines, and one textbox. sample11 likewise declares 67 for 116
        // nodes, including 49 containers. Grouping must not hide the text node.
        let mut objects = object(0, 2, 0, &[], &[0; 8]);
        objects.extend(object(0, 3, 0, &[], &[0; 8]));
        let line = [4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        objects.extend(object(1, 1, 0, &[], &line));
        objects.extend(object(1, 0, 0, &[], &line));
        objects.extend(object(6, 0, 0, &[], &textbox_info(b"grouped value 101")));
        assert_eq!(extract(&frame(3, &objects)).unwrap(), "grouped value 101\n");
        assert!(extract(&frame(2, &objects)).is_err());
        assert!(extract(&frame(4, &objects)).is_err());
    }

    #[test]
    fn recovers_textboxes_inside_and_after_shape_headers() {
        let info = textbox_info(b"inside");
        let first = object(2, 1, 1 << 19, &info, &[0; 8]);
        let mut details = textbox_info(b"after");
        details.extend_from_slice(&[0; 8]);
        let second = object(2, 0, 1 << 19, &[], &details);
        let objects = [first, second].concat();
        assert_eq!(extract(&frame(2, &objects)).unwrap(), "inside\nafter\n");
    }

    #[test]
    fn nested_paragraph_errors_invalidate_the_whole_drawing() {
        let mut objects = object(6, 1, 0, &[], &textbox_info(b"valid"));
        objects.extend(object(6, 0, 0, &[], &textbox_info(&[0xff])));
        assert!(extract(&frame(2, &objects)).is_err());
    }

    #[test]
    fn rejects_truncated_lengths_unknown_shapes_and_unparsed_bytes() {
        let good = frame(1, &object(6, 0, 0, &[], &textbox_info(b"text")));
        for length in 0..good.len() {
            assert!(
                extract(&good[..length]).is_err(),
                "accepted truncation at {length}"
            );
        }
        assert!(extract(&frame(1, &object(99, 0, 0, &[], &[0; 8]))).is_err());
        assert!(extract(&frame(1, &object(2, 0, 0, &[1; 5], &[0; 8]))).is_err());
        let mut trailing = good;
        trailing.push(1);
        assert!(extract(&trailing).is_err());
    }

    #[test]
    fn rejects_incorrect_node_count_and_excessive_nesting() {
        assert!(extract(&frame(2, &object(2, 0, 0, &[], &[0; 8]))).is_err());
        let mut objects = Vec::new();
        for _ in 0..300 {
            objects.extend(object(0, 2, 0, &[], &[0; 8]));
        }
        objects.extend(object(2, 0, 0, &[], &[0; 8]));
        assert!(extract(&frame(1, &objects)).is_err());
    }

    #[test]
    fn group_containers_still_consume_the_node_budget() {
        let mut objects = Vec::new();
        for _ in 0..MAX_OBJECTS {
            objects.extend(object(0, 1, 0, &[], &[0; 8]));
        }
        objects.extend(object(6, 0, 0, &[], &textbox_info(b"last")));
        let error = extract(&frame(1, &objects)).unwrap_err();
        assert!(error.contains("too many"));
    }

    #[test]
    fn skips_geometry_before_a_textbox() {
        let mut points = 4u32.to_le_bytes().to_vec();
        points.extend_from_slice(&2u32.to_le_bytes());
        points.extend_from_slice(&16u32.to_le_bytes());
        points.extend_from_slice(&[0; 16]);
        points.extend_from_slice(&[0, 1]);
        let mut objects = object(10, 1, 0, &[], &points);
        objects.extend(object(6, 0, 0, &[], &textbox_info(b"label")));
        assert_eq!(extract(&frame(2, &objects)).unwrap(), "label\n");
    }

    #[test]
    fn accepts_all_documented_nontext_shapes_and_optional_attributes() {
        let mut objects = Vec::new();
        for kind in [0, 1, 2, 3, 4, 5, 7, 8, 9, 10, 11] {
            let mut details = Vec::new();
            if kind != 9 {
                let info1 = match kind {
                    1 | 4 | 5 | 7 | 10 | 11 => vec![0; 4],
                    8 => vec![0; 16],
                    _ => vec![],
                };
                details.extend_from_slice(&(info1.len() as u32).to_le_bytes());
                details.extend_from_slice(&info1);
                details.extend_from_slice(&[0; 4]);
            }
            objects.extend(object(kind, 1, 0, &[], &details));
        }
        let options = (1 << 16) | (1 << 17) | (1 << 18) | (1 << 20);
        objects.extend(object(
            6,
            0,
            options,
            &[0; 32 + 28 + 278 + 3],
            &textbox_info(b"end"),
        ));
        assert_eq!(extract(&frame(11, &objects)).unwrap(), "end\n");
    }

    #[test]
    fn honors_rotation_flag_with_omitted_rotation_attributes() {
        let obj = object(6, 0, 1 << 17, &[], &textbox_info(b"text"));
        assert_eq!(extract(&frame(1, &obj)).unwrap(), "text\n");
    }

    #[test]
    fn rejects_oversized_frame_header_and_detail_lengths() {
        let mut data = frame(1, &object(6, 0, 0, &[], &textbox_info(b"text")));
        data[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(extract(&data).is_err());
        let mut info = textbox_info(b"text");
        info[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(extract(&frame(1, &object(6, 0, 0, &[], &info))).is_err());
    }
}
