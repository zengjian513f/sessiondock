//! The library resolves pitm/item associations and parses the selected AV1 header.
//! This preflight only limits allocations and unsupported presentation features.
use super::super::MAX_IMAGE_BYTES;
use super::{MediaError, dimensions, take};

fn number(bytes: &[u8], pos: &mut usize, n: usize) -> Result<u64, MediaError> {
    if n > 8 {
        return Err(MediaError::Invalid);
    }
    Ok(take(bytes, pos, n)?
        .iter()
        .fold(0, |v, b| (v << 8) | u64::from(*b)))
}
fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes(b[..4].try_into().expect("checked field"))
}
struct Preflight {
    boxes: usize,
    extents: usize,
    extent_bytes: u64,
    input_len: usize,
    associations: usize,
    item_ids: std::collections::BTreeSet<u64>,
    info_ids: std::collections::BTreeSet<u64>,
}
impl Preflight {
    fn iloc(&mut self, b: &[u8]) -> Result<(), MediaError> {
        let mut p = 0;
        let full = take(b, &mut p, 4)?;
        let version = full[0];
        if version > 2 || full[1..] != [0; 3] {
            return Err(MediaError::Invalid);
        }
        let sizes = take(b, &mut p, 2)?;
        let offset = (sizes[0] >> 4) as usize;
        let length = (sizes[0] & 15) as usize;
        let base = (sizes[1] >> 4) as usize;
        let index = if version > 0 {
            (sizes[1] & 15) as usize
        } else {
            0
        };
        if [offset, length, base, index]
            .iter()
            .any(|n| !matches!(n, 0 | 4 | 8))
        {
            return Err(MediaError::Unsupported);
        }
        let count = number(b, &mut p, if version < 2 { 2 } else { 4 })?;
        if count > 128 {
            return Err(MediaError::Limit);
        }
        for _ in 0..count {
            let id = number(b, &mut p, if version < 2 { 2 } else { 4 })?;
            if !self.item_ids.insert(id) {
                return Err(MediaError::Invalid);
            }
            if version > 0 && number(b, &mut p, 2)? > 1 {
                return Err(MediaError::Unsupported);
            }
            if number(b, &mut p, 2)? != 0 {
                return Err(MediaError::Unsupported);
            }
            let origin = number(b, &mut p, base)?;
            let n = number(b, &mut p, 2)? as usize;
            self.extents += n;
            if self.extents > 256 {
                return Err(MediaError::Limit);
            }
            for _ in 0..n {
                number(b, &mut p, index)?;
                let start = number(b, &mut p, offset)?;
                let len = number(b, &mut p, length)?;
                let start = origin.checked_add(start).ok_or(MediaError::Invalid)?;
                if start > self.input_len as u64
                    || (len != 0
                        && start
                            .checked_add(len)
                            .is_none_or(|end| end > self.input_len as u64))
                {
                    return Err(MediaError::Invalid);
                }
                self.extent_bytes = self
                    .extent_bytes
                    .checked_add(if len == 0 { self.input_len as u64 } else { len })
                    .ok_or(MediaError::Limit)?;
                if self.extent_bytes > 2 * MAX_IMAGE_BYTES as u64 {
                    return Err(MediaError::Limit);
                }
            }
        }
        if p != b.len() {
            return Err(MediaError::Invalid);
        }
        Ok(())
    }
    fn scan(&mut self, b: &[u8], depth: u8) -> Result<(), MediaError> {
        self.scan_boxes(b, depth, false)
    }
    fn scan_boxes(&mut self, b: &[u8], depth: u8, properties: bool) -> Result<(), MediaError> {
        if depth > 8 {
            return Err(MediaError::Limit);
        }
        let mut p = 0;
        while p < b.len() {
            self.boxes += 1;
            if self.boxes > 256 {
                return Err(MediaError::Limit);
            }
            let h = take(b, &mut p, 8)?;
            let kind = &h[4..];
            let short = be32(h);
            let (size, header) = if short == 1 {
                (number(b, &mut p, 8)?, 16)
            } else {
                (u64::from(short), 8)
            };
            // Size-to-EOF is deliberately unsupported: every allocation has an exact range.
            if short == 0 {
                return Err(MediaError::Unsupported);
            }
            let len = usize::try_from(size.checked_sub(header).ok_or(MediaError::Invalid)?)
                .map_err(|_| MediaError::Invalid)?;
            let data = take(b, &mut p, len)?;
            // The library intentionally ignores many properties (even essential ones).
            // Do not silently accept an unknown rendering transform on that basis.
            if properties
                && !matches!(
                    kind,
                    b"ispe" | b"av1C" | b"pixi" | b"colr" | b"auxC" | b"clli" | b"mdcv"
                )
            {
                return Err(MediaError::Unsupported);
            }
            match kind {
                b"meta" => {
                    if depth != 0 || data.len() > 64 * 1024 {
                        return Err(MediaError::Limit);
                    }
                    if data.get(..4) != Some(&[0; 4]) {
                        return Err(MediaError::Invalid);
                    }
                    self.scan(&data[4..], depth + 1)?;
                }
                b"iprp" => self.scan(data, depth + 1)?,
                b"ipco" => self.scan_boxes(data, depth + 1, true)?,
                b"iref" => {
                    if data.len() < 4 || data[0] > 1 || data[1..4] != [0; 3] {
                        return Err(MediaError::Unsupported);
                    }
                    self.scan(&data[4..], depth + 1)?;
                }
                b"iloc" => self.iloc(data)?,
                b"infe" => {
                    let mut q = 0;
                    let full = take(data, &mut q, 4)?;
                    let n = match full[0] {
                        2 => 2,
                        3 => 4,
                        _ => return Err(MediaError::Unsupported),
                    };
                    let id = number(data, &mut q, n)?;
                    if !self.info_ids.insert(id) {
                        return Err(MediaError::Invalid);
                    }
                }
                b"iinf" => {
                    let mut q = 0;
                    let f = take(data, &mut q, 4)?;
                    if f[0] > 1 {
                        return Err(MediaError::Unsupported);
                    }
                    let n = number(data, &mut q, if f[0] == 0 { 2 } else { 4 })?;
                    if n > 128 {
                        return Err(MediaError::Limit);
                    }
                    self.scan(&data[q..], depth + 1)?;
                }
                b"ipma" => {
                    let mut q = 0;
                    let f = take(data, &mut q, 4)?;
                    if f[0] > 1 || f[1..3] != [0; 2] || f[3] > 1 {
                        return Err(MediaError::Unsupported);
                    }
                    let n = number(data, &mut q, 4)?;
                    if n > 128 {
                        return Err(MediaError::Limit);
                    }
                    for _ in 0..n {
                        number(data, &mut q, if f[0] == 0 { 2 } else { 4 })?;
                        let n = number(data, &mut q, 1)? as usize;
                        self.associations += n;
                        if self.associations > 256 {
                            return Err(MediaError::Limit);
                        }
                        take(data, &mut q, n * if f[3] & 1 == 0 { 1 } else { 2 })?;
                    }
                    if q != data.len() {
                        return Err(MediaError::Invalid);
                    }
                }
                b"auxC" => {
                    if data.len() > 4096 {
                        return Err(MediaError::Limit);
                    }
                }
                b"ispe" => {
                    if data.len() != 12 || data[..4] != [0; 4] {
                        return Err(MediaError::Invalid);
                    }
                    dimensions(be32(&data[4..]), be32(&data[8..]))?;
                }
                b"irot" | b"imir" | b"clap" | b"moov" => return Err(MediaError::Unsupported),
                _ => {}
            }
        }
        Ok(())
    }
}
pub(super) fn inspect(bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(MediaError::Limit);
    }
    if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
        return Err(MediaError::Invalid);
    }
    if &bytes[8..12] == b"avis" {
        return Err(MediaError::Unsupported);
    }
    if &bytes[8..12] != b"avif" {
        return Err(MediaError::Invalid);
    }
    let ftyp = be32(bytes) as usize;
    if ftyp < 16 || ftyp > bytes.len() || !(ftyp - 16).is_multiple_of(4) {
        return Err(MediaError::Invalid);
    }
    if bytes[16..ftyp]
        .as_chunks::<4>()
        .0
        .iter()
        .any(|b| b == b"avis")
    {
        return Err(MediaError::Unsupported);
    }
    let mut pre = Preflight {
        boxes: 0,
        extents: 0,
        extent_bytes: 0,
        input_len: bytes.len(),
        associations: 0,
        item_ids: std::collections::BTreeSet::new(),
        info_ids: std::collections::BTreeSet::new(),
    };
    pre.scan(bytes, 0)?;
    // Some parser builds use debug assertions for malformed box state. Keep a
    // hostile container from poisoning the shared cache mutex on those builds.
    let (primary, alpha) = std::panic::catch_unwind(|| {
        let parsed = avif_parse::AvifData::from_reader(&mut &bytes[..]).map_err(error)?;
        Ok::<_, MediaError>((
            parsed.primary_item_metadata().map_err(error)?,
            parsed.alpha_item_metadata().map_err(error)?,
        ))
    })
    .map_err(|_| MediaError::Invalid)??;
    if !primary.still_picture {
        return Err(MediaError::Unsupported);
    }
    let size = dimensions(
        primary.max_frame_width.get(),
        primary.max_frame_height.get(),
    )?;
    if let Some(alpha) = alpha
        && (!alpha.still_picture
            || dimensions(alpha.max_frame_width.get(), alpha.max_frame_height.get())? != size)
    {
        return Err(MediaError::Invalid);
    }
    Ok(size)
}
fn error(e: avif_parse::Error) -> MediaError {
    match e {
        avif_parse::Error::Unsupported(_) => MediaError::Unsupported,
        avif_parse::Error::OutOfMemory => MediaError::Limit,
        _ => MediaError::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pre() -> Preflight {
        Preflight {
            boxes: 0,
            extents: 0,
            extent_bytes: 0,
            input_len: MAX_IMAGE_BYTES,
            associations: 0,
            item_ids: std::collections::BTreeSet::new(),
            info_ids: std::collections::BTreeSet::new(),
        }
    }
    fn iloc(n: u16, len: u32) -> Vec<u8> {
        let mut b = vec![0, 0, 0, 0, 0x44, 0, 0, 1, 0, 1, 0, 0];
        b.extend(n.to_be_bytes());
        for _ in 0..n {
            b.extend([0; 4]);
            b.extend(len.to_be_bytes());
        }
        b
    }
    #[test]
    fn repeated_extents_are_charged_before_library_allocation() {
        assert_eq!(pre().iloc(&iloc(2, MAX_IMAGE_BYTES as u32)), Ok(()));
        assert_eq!(
            pre().iloc(&iloc(3, MAX_IMAGE_BYTES as u32)),
            Err(MediaError::Limit)
        );
        assert_eq!(pre().iloc(&iloc(3, 0)), Err(MediaError::Limit));
        assert_eq!(pre().iloc(&iloc(257, 1)), Err(MediaError::Limit));
    }
    #[test]
    fn huge_item_counts_and_extent_ranges_rejected() {
        let mut b = iloc(1, 1);
        b[6..8].copy_from_slice(&129u16.to_be_bytes());
        assert_eq!(pre().iloc(&b), Err(MediaError::Limit));
        let mut b = iloc(1, 1);
        b[14..18].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(pre().iloc(&b), Err(MediaError::Invalid));
        let mut b = iloc(1, 1);
        b[10..12].copy_from_slice(&1u16.to_be_bytes());
        assert_eq!(pre().iloc(&b), Err(MediaError::Unsupported));
        let mut duplicate = pre();
        assert_eq!(duplicate.iloc(&iloc(1, 1)), Ok(()));
        assert_eq!(duplicate.iloc(&iloc(1, 1)), Err(MediaError::Invalid));
    }
    #[test]
    fn boxes_metadata_and_parser_reservations_have_limits() {
        let mut boxes = Vec::new();
        for _ in 0..257 {
            boxes.extend([0, 0, 0, 8, b'f', b'r', b'e', b'e']);
        }
        assert_eq!(pre().scan(&boxes, 0), Err(MediaError::Limit));
        let data = [
            0, 0, 0, 16, b'i', b'i', b'n', b'f', 1, 0, 0, 0, 255, 255, 255, 255,
        ];
        assert_eq!(pre().scan(&data, 0), Err(MediaError::Limit));
        let mut data = vec![0, 1, 0, 9, b'm', b'e', b't', b'a'];
        data.resize(65545, 0);
        assert_eq!(pre().scan(&data, 0), Err(MediaError::Limit));
        let data = [0xff, 0xff, 0xff, 0xff, b'm', b'd', b'a', b't'];
        assert_eq!(pre().scan(&data, 0), Err(MediaError::Invalid));
    }
    #[test]
    fn duplicate_item_descriptions_and_unknown_properties_fail_closed() {
        let info = [0, 0, 0, 14, b'i', b'n', b'f', b'e', 2, 0, 0, 0, 0, 1];
        let mut duplicate = info.to_vec();
        duplicate.extend(info);
        assert_eq!(pre().scan(&duplicate, 0), Err(MediaError::Invalid));
        let unknown = [
            0, 0, 0, 16, b'i', b'p', b'c', b'o', 0, 0, 0, 8, b'x', b'x', b'x', b'x',
        ];
        assert_eq!(pre().scan(&unknown, 0), Err(MediaError::Unsupported));
    }
}
