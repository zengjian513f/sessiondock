//! WebP RIFF container inspection. Not an entropy or pixel decoder.
//! Exact RIFF size, VP8/VP8L headers, VP8X canvas, declared metadata, and
//! ANIM/ANMF frames are checked. Unknown chunks are skipped within bounds;
//! duplicate or invalid ordering of recognized chunks fails.
use super::{MediaError, dimensions, frames, le24, le32, rectangle, take};

fn chunk<'a>(bytes: &'a [u8], pos: &mut usize) -> Result<(&'a [u8], &'a [u8]), MediaError> {
    let header = take(bytes, pos, 8)?;
    let n = le32(&header[4..]) as usize;
    let payload = take(bytes, pos, n)?;
    if !n.is_multiple_of(2) && take(bytes, pos, 1)?[0] != 0 {
        return Err(MediaError::Invalid);
    }
    Ok((&header[..4], payload))
}
fn image(kind: &[u8], data: &[u8]) -> Result<(u32, u32), MediaError> {
    match kind {
        b"VP8 " => {
            if data.len() <= 10
                || data[0] & 1 != 0
                || data[0] & 0x10 == 0
                || &data[3..6] != b"\x9d\x01\x2a"
            {
                return Err(MediaError::Invalid);
            }
            let partition = super::le24(data) >> 5;
            if partition == 0 || partition as usize > data.len() - 10 {
                return Err(MediaError::Invalid);
            }
            dimensions(
                super::le16(&data[6..]) & 0x3fff,
                super::le16(&data[8..]) & 0x3fff,
            )
        }
        b"VP8L" => {
            if data.len() <= 5 || data[0] != 0x2f {
                return Err(MediaError::Invalid);
            }
            let h = le32(&data[1..]);
            if h >> 29 != 0 {
                return Err(MediaError::Invalid);
            }
            dimensions((h & 0x3fff) + 1, ((h >> 14) & 0x3fff) + 1)
        }
        _ => Err(MediaError::Invalid),
    }
}
fn frame_data(bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    let mut pos = 0;
    let mut alpha = false;
    let mut result = None;
    while pos < bytes.len() {
        let (kind, data) = chunk(bytes, &mut pos)?;
        match kind {
            b"ALPH" if !alpha && result.is_none() => {
                if data.len() < 2
                    || data[0] & 0xc0 != 0
                    || data[0] & 3 > 1
                    || (data[0] >> 4) & 3 > 1
                {
                    return Err(MediaError::Invalid);
                }
                alpha = true;
            }
            b"VP8 " | b"VP8L" if result.is_none() => {
                if alpha && kind == b"VP8L" {
                    return Err(MediaError::Invalid);
                }
                result = Some(image(kind, data)?);
            }
            _ => return Err(MediaError::Invalid),
        }
    }
    result.ok_or(MediaError::Invalid)
}
pub(super) fn inspect(bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    let mut pos = 0;
    let h = take(bytes, &mut pos, 12)?;
    if &h[..4] != b"RIFF" || &h[8..] != b"WEBP" || le32(&h[4..]) as u64 + 8 != bytes.len() as u64 {
        return Err(MediaError::Invalid);
    }
    let (first, data) = chunk(bytes, &mut pos)?;
    if first != b"VP8X" {
        let size = image(first, data)?;
        if pos != bytes.len() {
            return Err(MediaError::Invalid);
        }
        return Ok(size);
    }
    if data.len() != 10 || data[0] & 0xc1 != 0 || data[1..4] != [0; 3] {
        return Err(MediaError::Invalid);
    }
    let canvas = dimensions(le24(&data[4..]) + 1, le24(&data[7..]) + 1)?;
    let animated = data[0] & 2 != 0;
    let mut anim = false;
    let mut count = 0;
    let mut image_seen = false;
    let mut alpha = false;
    let mut icc = false;
    let mut exif = false;
    let mut xmp = false;
    while pos < bytes.len() {
        let (kind, payload) = chunk(bytes, &mut pos)?;
        match kind {
            b"ICCP" if !icc && !image_seen && count == 0 => {
                if data[0] & 0x20 == 0 || payload.is_empty() {
                    return Err(MediaError::Invalid);
                }
                icc = true;
            }
            b"EXIF" if !exif => {
                if data[0] & 8 == 0 || payload.is_empty() {
                    return Err(MediaError::Invalid);
                }
                exif = true;
            }
            b"XMP " if !xmp => {
                if data[0] & 4 == 0 || payload.is_empty() {
                    return Err(MediaError::Invalid);
                }
                xmp = true;
            }
            b"ANIM" if animated && !anim && count == 0 => {
                if payload.len() != 6 {
                    return Err(MediaError::Invalid);
                }
                anim = true;
            }
            b"ANMF" if animated && anim => {
                if payload.len() < 16 || payload[15] & 0xfc != 0 {
                    return Err(MediaError::Invalid);
                }
                let w = le24(&payload[6..]) + 1;
                let h = le24(&payload[9..]) + 1;
                rectangle(canvas, le24(payload) * 2, le24(&payload[3..]) * 2, w, h)?;
                if frame_data(&payload[16..])? != (w, h) {
                    return Err(MediaError::Invalid);
                }
                count += 1;
                frames(canvas, count)?;
            }
            b"ALPH" if !animated && !alpha && !image_seen => {
                if data[0] & 0x10 == 0
                    || payload.len() < 2
                    || payload[0] & 0xc0 != 0
                    || payload[0] & 3 > 1
                    || (payload[0] >> 4) & 3 > 1
                {
                    return Err(MediaError::Invalid);
                }
                alpha = true;
            }
            b"VP8 " | b"VP8L" if !animated && !image_seen => {
                if (alpha && kind == b"VP8L") || image(kind, payload)? != canvas {
                    return Err(MediaError::Invalid);
                }
                image_seen = true;
            }
            // Unknown RIFF chunks are forward-compatible; known duplicates/order errors aren't.
            b"VP8X" | b"ICCP" | b"EXIF" | b"XMP " | b"ANIM" | b"ANMF" | b"ALPH" | b"VP8 "
            | b"VP8L" => return Err(MediaError::Invalid),
            _ => {}
        }
    }
    if icc != (data[0] & 0x20 != 0)
        || exif != (data[0] & 8 != 0)
        || xmp != (data[0] & 4 != 0)
        || (!animated && !image_seen)
    {
        return Err(MediaError::Invalid);
    }
    if animated {
        frames(canvas, count)?;
    }
    Ok(canvas)
}
