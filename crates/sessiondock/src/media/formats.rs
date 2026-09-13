//! Bounded container inspection, not a compressed-pixel decoder.
use super::{MediaError, dimensions};

mod avif;
mod png;
mod webp;

pub(super) const MAX_FRAMES: u32 = 128;
pub(super) const MAX_FRAME_PIXELS: u64 = 64 * 1024 * 1024;

pub(super) fn inspect(mime: &str, bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    if bytes.len() as u64 > crate::files::MAX_RAW_BYTES {
        return Err(MediaError::Limit);
    }
    match mime {
        "image/gif" => gif(bytes),
        "image/bmp" => bmp(bytes),
        "image/webp" => webp::inspect(bytes),
        "image/avif" => avif::inspect(bytes),
        "image/png" => png::inspect(bytes),
        _ => Err(MediaError::Unsupported),
    }
}

fn frames(canvas: (u32, u32), count: u32) -> Result<(), MediaError> {
    if count == 0 {
        return Err(MediaError::Invalid);
    }
    if count > MAX_FRAMES
        || u64::from(canvas.0) * u64::from(canvas.1) * u64::from(count) > MAX_FRAME_PIXELS
    {
        return Err(MediaError::Limit);
    }
    Ok(())
}
fn rectangle(canvas: (u32, u32), x: u32, y: u32, w: u32, h: u32) -> Result<(), MediaError> {
    dimensions(w, h)?;
    if x.checked_add(w).is_none_or(|v| v > canvas.0)
        || y.checked_add(h).is_none_or(|v| v > canvas.1)
    {
        return Err(MediaError::Invalid);
    }
    Ok(())
}
fn le16(b: &[u8]) -> u32 {
    u32::from(u16::from_le_bytes([b[0], b[1]]))
}
fn le24(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], 0])
}
fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().expect("checked field"))
}
fn take<'a>(bytes: &'a [u8], pos: &mut usize, len: usize) -> Result<&'a [u8], MediaError> {
    let end = pos.checked_add(len).ok_or(MediaError::Invalid)?;
    let value = bytes.get(*pos..end).ok_or(MediaError::Invalid)?;
    *pos = end;
    Ok(value)
}
fn subblocks(bytes: &[u8], pos: &mut usize) -> Result<usize, MediaError> {
    let mut total = 0;
    loop {
        let n = take(bytes, pos, 1)?[0] as usize;
        if n == 0 {
            return Ok(total);
        }
        take(bytes, pos, n)?;
        total += n;
    }
}
fn gif(bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    let mut pos = 0;
    if !matches!(take(bytes, &mut pos, 6)?, b"GIF87a" | b"GIF89a") {
        return Err(MediaError::Invalid);
    }
    let screen = take(bytes, &mut pos, 7)?;
    let canvas = dimensions(le16(screen), le16(&screen[2..]))?;
    let global = screen[4] & 0x80 != 0;
    if global {
        take(bytes, &mut pos, 3usize << ((screen[4] & 7) + 1))?;
    }
    let mut count = 0;
    let mut control = false;
    loop {
        match take(bytes, &mut pos, 1)?[0] {
            0x3b => {
                if pos != bytes.len() || control {
                    return Err(MediaError::Invalid);
                }
                frames(canvas, count)?;
                return Ok(canvas);
            }
            0x2c => {
                let d = take(bytes, &mut pos, 9)?;
                rectangle(canvas, le16(d), le16(&d[2..]), le16(&d[4..]), le16(&d[6..]))?;
                if d[8] & 0x18 != 0 {
                    return Err(MediaError::Invalid);
                }
                if d[8] & 0x80 != 0 {
                    take(bytes, &mut pos, 3usize << ((d[8] & 7) + 1))?;
                } else if !global {
                    return Err(MediaError::Invalid);
                }
                if !(2..=8).contains(&take(bytes, &mut pos, 1)?[0])
                    || subblocks(bytes, &mut pos)? == 0
                {
                    return Err(MediaError::Invalid);
                }
                count += 1;
                frames(canvas, count)?;
                control = false;
            }
            0x21 => match take(bytes, &mut pos, 1)?[0] {
                0xf9 => {
                    let c = take(bytes, &mut pos, 6)?;
                    if control || c[0] != 4 || c[1] & 0xe0 != 0 || (c[1] >> 2) & 7 > 3 || c[5] != 0
                    {
                        return Err(MediaError::Invalid);
                    }
                    control = true;
                }
                0xfe => {
                    subblocks(bytes, &mut pos)?;
                }
                0xff => {
                    if take(bytes, &mut pos, 1)?[0] != 11 {
                        return Err(MediaError::Invalid);
                    }
                    take(bytes, &mut pos, 11)?;
                    subblocks(bytes, &mut pos)?;
                }
                // Plain-text is itself a rendering block; do not omit it from budgets.
                _ => return Err(MediaError::Unsupported),
            },
            _ => return Err(MediaError::Invalid),
        }
    }
}

fn bmp(bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    let mut pos = 0;
    let file = take(bytes, &mut pos, 14)?;
    if &file[..2] != b"BM" || le32(&file[2..]) as usize != bytes.len() || file[6..10] != [0; 4] {
        return Err(MediaError::Invalid);
    }
    let offset = le32(&file[10..]) as usize;
    let size = le32(take(bytes, &mut pos, 4)?) as usize;
    if !matches!(size, 12 | 40 | 52 | 56 | 108 | 124) {
        return Err(MediaError::Unsupported);
    }
    let dib = take(bytes, &mut pos, size - 4)?;
    let (w, h, bpp, compression, palette, declared) = if size == 12 {
        if le16(&dib[4..]) != 1 {
            return Err(MediaError::Invalid);
        }
        (le16(dib), le16(&dib[2..]), le16(&dib[6..]), 0, 0, 0)
    } else {
        let w = le32(dib) as i32;
        let h = le32(&dib[4..]) as i32;
        if w <= 0 || h == 0 || h == i32::MIN || le16(&dib[8..]) != 1 {
            return Err(MediaError::Invalid);
        }
        (
            w as u32,
            h.unsigned_abs(),
            le16(&dib[10..]),
            le32(&dib[12..]),
            le32(&dib[28..]),
            le32(&dib[16..]),
        )
    };
    let canvas = dimensions(w, h)?;
    if !matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32) {
        return Err(MediaError::Unsupported);
    }
    if size == 12 && !matches!(bpp, 1 | 4 | 8 | 24) {
        return Err(MediaError::Unsupported);
    }
    if !matches!(compression, 0 | 3 | 6) {
        return Err(MediaError::Unsupported);
    }
    if compression != 0 {
        if !matches!(bpp, 16 | 32) || size == 12 {
            return Err(MediaError::Invalid);
        }
        let n = if compression == 6 { 4 } else { 3 };
        let masks = if size == 40 {
            take(bytes, &mut pos, n * 4)?
        } else {
            dib.get(36..36 + n * 4).ok_or(MediaError::Invalid)?
        };
        let mut used = 0;
        for mask in masks.as_chunks::<4>().0 {
            let m = le32(mask);
            if m == 0
                || m & used != 0
                || (bpp == 16 && m > 0xffff)
                || (m >> m.trailing_zeros()).count_ones()
                    != (m >> m.trailing_zeros()).trailing_ones()
            {
                return Err(MediaError::Invalid);
            }
            used |= m;
        }
    }
    let entries = if bpp <= 8 {
        let max = 1 << bpp;
        if palette > max {
            return Err(MediaError::Invalid);
        }
        if palette == 0 { max } else { palette }
    } else {
        palette
    };
    if entries > 256 {
        return Err(MediaError::Limit);
    }
    let palette_bytes = entries as usize * if size == 12 { 3 } else { 4 };
    take(bytes, &mut pos, palette_bytes)?;
    if offset < pos || offset > bytes.len() {
        return Err(MediaError::Invalid);
    }
    let row = (u64::from(w) * u64::from(bpp)).div_ceil(32) * 4;
    let required = row * u64::from(h);
    if required > (bytes.len() - offset) as u64
        || (declared != 0 && u64::from(declared) != required)
    {
        return Err(MediaError::Invalid);
    }
    // V5 profiles are not followed; embedded/external profiles are unsupported.
    if size == 124 && (le32(&dib[108..]) != 0 || le32(&dib[112..]) != 0) {
        return Err(MediaError::Unsupported);
    }
    Ok(canvas)
}

#[cfg(test)]
#[path = "formats_tests.rs"]
mod tests;
