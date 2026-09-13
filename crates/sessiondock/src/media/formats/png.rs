//! PNG/APNG container inspection. Not a zlib or pixel decoder.
//! Animation chunks are validated first so error precedence is preserved; the
//! base check then borrows the same buffer, never a filtered body copy. Frame
//! rectangles, sequence numbers, and dispose/blend values are checked here.
use super::super::crc32;
use super::{MediaError, dimensions, frames, rectangle, take};
fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes[..4].try_into().expect("checked field"))
}

pub(super) fn inspect(bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    if validate_animation(bytes)? {
        super::super::png_base(bytes, true)
    } else {
        super::super::png(bytes)
    }
}

// Preserve the original animation pass and its error precedence. The base
// validator subsequently borrows the same buffer, never a filtered body copy.
fn validate_animation(bytes: &[u8]) -> Result<bool, MediaError> {
    if bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n") {
        return Err(MediaError::Invalid);
    }
    let mut pos = 8;
    let mut canvas = None;
    let mut declared = None;
    let mut count = 0;
    let mut sequence = 0;
    let mut idat = false;
    let mut default_frame = false;
    let mut frame_data = false;
    while pos < bytes.len() {
        let start = pos;
        let h = take(bytes, &mut pos, 8)?;
        let kind = &h[4..];
        let data = take(bytes, &mut pos, be32(h) as usize)?;
        let crc = take(bytes, &mut pos, 4)?;
        if crc32(&bytes[start + 4..pos - 4]) != be32(crc) {
            return Err(MediaError::Invalid);
        }
        match kind {
            b"IHDR" => {
                if start != 8 || data.len() != 13 {
                    return Err(MediaError::Invalid);
                }
                canvas = Some(dimensions(be32(data), be32(&data[4..]))?);
            }
            b"acTL" => {
                if declared.is_some() || idat || data.len() != 8 {
                    return Err(MediaError::Invalid);
                }
                let n = be32(data);
                frames(canvas.ok_or(MediaError::Invalid)?, n)?;
                declared = Some(n);
            }
            b"fcTL" => {
                if declared.is_none()
                    || data.len() != 26
                    || be32(data) != sequence
                    || (count > 0 && !frame_data)
                {
                    return Err(MediaError::Invalid);
                }
                sequence += 1;
                let size = canvas.ok_or(MediaError::Invalid)?;
                let w = be32(&data[4..]);
                let h = be32(&data[8..]);
                let x = be32(&data[12..]);
                let y = be32(&data[16..]);
                rectangle(size, x, y, w, h)?;
                if data[24] > 2 || data[25] > 1 {
                    return Err(MediaError::Invalid);
                }
                if !idat {
                    if count != 0 || (w, h) != size || x != 0 || y != 0 {
                        return Err(MediaError::Invalid);
                    }
                    default_frame = true;
                }
                count += 1;
                frames(size, count + u32::from(!default_frame))?;
                frame_data = false;
            }
            b"fdAT" => {
                if !idat
                    || count == 0
                    || (default_frame && count == 1)
                    || data.len() < 5
                    || be32(data) != sequence
                {
                    return Err(MediaError::Invalid);
                }
                sequence += 1;
                frame_data = true;
            }
            b"IDAT" => {
                if count > u32::from(default_frame) {
                    return Err(MediaError::Invalid);
                }
                idat = true;
                if default_frame && !data.is_empty() {
                    frame_data = true;
                }
            }
            b"IEND" if declared.is_some() && (declared != Some(count) || !frame_data) => {
                return Err(MediaError::Invalid);
            }
            _ => {}
        }
    }
    Ok(declared.is_some())
}

#[cfg(test)]
mod tests;
