use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};

const AVIF: &str = "AAAAIGZ0eXBhdmlmAAAAAGF2aWZtaWYxbWlhZk1BMUIAAADrbWV0YQAAAAAAAAAhaGRscgAAAAAAAAAAcGljdAAAAAAAAAAAAAAAAAAAAAAOcGl0bQAAAAAAAQAAAB5pbG9jAAAAAEQAAAEAAQAAAAEAAAETAAAAKAAAAChpaW5mAAAAAAABAAAAGmluZmUCAAAAAAEAAGF2MDFDb2xvcgAAAABqaXBycAAAAEtpcGNvAAAAFGlzcGUAAAAAAAAAAwAAAAIAAAAQcGl4aQAAAAADCAgIAAAADGF2MUOBAAwAAAAAE2NvbHJuY2x4AAEADQAGgAAAABdpcG1hAAAAAAAAAAEAAQQBAoMEAAAAMG1kYXQSAAoIGAQrRAQ0GhAyGhTHh4ZlAgggnkAAAJBLsrmsYuXxFjb2VyIt";
fn gif_fixture(count: usize, w: u16, h: u16) -> Vec<u8> {
    let mut b = b"GIF89a".to_vec();
    b.extend(w.to_le_bytes());
    b.extend(h.to_le_bytes());
    b.extend([0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
    for _ in 0..count {
        b.extend([0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0]);
    }
    b.push(0x3b);
    b
}
#[test]
fn gif_frames_bounds_and_trailer() {
    assert_eq!(inspect("image/gif", &gif_fixture(2, 1, 1)), Ok((1, 1)));
    assert_eq!(gif(&gif_fixture(129, 1, 1)), Err(MediaError::Limit));
    assert_eq!(gif(&gif_fixture(65, 1024, 1024)), Err(MediaError::Limit));
    let mut b = gif_fixture(1, 1, 1);
    b.pop();
    assert_eq!(gif(&b), Err(MediaError::Invalid));
}
#[test]
fn avif_primary_and_damage() {
    let b = STANDARD.decode(AVIF).unwrap();
    assert_eq!(inspect("image/avif", &b), Ok((3, 2)));
    for n in 0..b.len() {
        assert!(inspect("image/avif", &b[..n]).is_err(), "prefix {n}");
    }
    let mut wrong = b;
    wrong[8..12].copy_from_slice(b"avis");
    assert_eq!(inspect("image/avif", &wrong), Err(MediaError::Unsupported));
}
#[test]
fn bmp_rows_are_present() {
    let mut b = vec![0u8; 58];
    b[..2].copy_from_slice(b"BM");
    b[2..6].copy_from_slice(&58u32.to_le_bytes());
    b[10..14].copy_from_slice(&54u32.to_le_bytes());
    b[14..18].copy_from_slice(&40u32.to_le_bytes());
    b[18..22].copy_from_slice(&1u32.to_le_bytes());
    b[22..26].copy_from_slice(&1u32.to_le_bytes());
    b[26] = 1;
    b[28] = 24;
    assert_eq!(bmp(&b), Ok((1, 1)));
    b.truncate(57);
    b[2..6].copy_from_slice(&57u32.to_le_bytes());
    assert_eq!(bmp(&b), Err(MediaError::Invalid));
}

#[test]
fn avif_mutations_cannot_unwind_through_the_cache() {
    let original = STANDARD.decode(AVIF).unwrap();
    for i in 0..original.len() {
        let mut b = original.clone();
        b[i] ^= 0xff;
        assert!(
            std::panic::catch_unwind(|| inspect("image/avif", &b)).is_ok(),
            "byte {i}"
        );
    }
    let mut absent = original;
    let p = absent.windows(4).position(|v| v == b"pitm").unwrap();
    absent[p + 9] = 9;
    assert_eq!(inspect("image/avif", &absent), Err(MediaError::Invalid));
}

fn riff_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut b = kind.to_vec();
    b.extend((data.len() as u32).to_le_bytes());
    b.extend(data);
    if !data.len().is_multiple_of(2) {
        b.push(0);
    }
    b
}
fn riff(data: &[u8]) -> Vec<u8> {
    let mut b = b"RIFF".to_vec();
    b.extend((data.len() as u32 + 4).to_le_bytes());
    b.extend(b"WEBP");
    b.extend(data);
    b
}
fn webp_fixture(count: usize, canvas: u32) -> Vec<u8> {
    let mut x = vec![2, 0, 0, 0];
    let side = (canvas - 1).to_le_bytes();
    x.extend(&side[..3]);
    x.extend(&side[..3]);
    let mut chunks = riff_chunk(b"VP8X", &x);
    chunks.extend(riff_chunk(b"ANIM", &[0; 6]));
    let mut frame = vec![0; 16];
    frame[12] = 10;
    frame.extend(riff_chunk(b"VP8L", &[0x2f, 0, 0, 0, 0, 0]));
    for _ in 0..count {
        chunks.extend(riff_chunk(b"ANMF", &frame));
    }
    riff(&chunks)
}
#[test]
fn webp_static_animation_and_budgets() {
    assert_eq!(
        inspect(
            "image/webp",
            &riff(&riff_chunk(b"VP8L", &[0x2f, 0, 0, 0, 0, 0]))
        ),
        Ok((1, 1))
    );
    assert_eq!(inspect("image/webp", &webp_fixture(2, 1)), Ok((1, 1)));
    assert_eq!(
        inspect("image/webp", &webp_fixture(129, 1)),
        Err(MediaError::Limit)
    );
    assert_eq!(
        inspect("image/webp", &webp_fixture(65, 1024)),
        Err(MediaError::Limit)
    );
    let mut mismatch = webp_fixture(1, 1);
    let p = mismatch.windows(4).position(|v| v == b"VP8L").unwrap();
    mismatch[p + 9] = 1;
    assert_eq!(inspect("image/webp", &mismatch), Err(MediaError::Invalid));
}
#[test]
fn webp_padding_flags_and_truncation() {
    let good = webp_fixture(2, 1);
    for n in 0..good.len() {
        assert!(inspect("image/webp", &good[..n]).is_err(), "prefix {n}");
    }
    let mut bad = good;
    bad[20] |= 0x80;
    assert_eq!(inspect("image/webp", &bad), Err(MediaError::Invalid));
    let mut chunks = riff_chunk(b"VP8L", &[0x2f, 0, 0, 0, 0, 0, 0]);
    *chunks.last_mut().unwrap() = 1;
    assert_eq!(
        inspect("image/webp", &riff(&chunks)),
        Err(MediaError::Invalid)
    );
}
fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut b = (data.len() as u32).to_be_bytes().to_vec();
    b.extend(kind);
    b.extend(data);
    let crc = super::super::crc32(&b[4..]);
    b.extend(crc.to_be_bytes());
    b
}
fn apng_fixture(count: u32, fallback: bool, canvas: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut h = canvas.to_be_bytes().to_vec();
    h.extend(canvas.to_be_bytes());
    h.extend([8, 6, 0, 0, 0]);
    b.extend(png_chunk(b"IHDR", &h));
    let mut ctl = count.to_be_bytes().to_vec();
    ctl.extend([0; 4]);
    b.extend(png_chunk(b"acTL", &ctl));
    let data = [
        0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0, 5, 0, 1, 0xff,
    ];
    if fallback {
        b.extend(png_chunk(b"IDAT", &data));
    }
    let mut sequence = 0u32;
    for i in 0..count {
        let mut frame = sequence.to_be_bytes().to_vec();
        sequence += 1;
        frame.extend(canvas.to_be_bytes());
        frame.extend(canvas.to_be_bytes());
        frame.extend([0; 8]);
        frame.extend([0, 1, 0, 10, 0, 0]);
        b.extend(png_chunk(b"fcTL", &frame));
        if i == 0 && !fallback {
            b.extend(png_chunk(b"IDAT", &data));
        } else {
            let mut fd = sequence.to_be_bytes().to_vec();
            sequence += 1;
            fd.extend(data);
            b.extend(png_chunk(b"fdAT", &fd));
        }
    }
    b.extend(png_chunk(b"IEND", &[]));
    b
}
#[test]
fn apng_default_and_independent_fallback() {
    for fallback in [false, true] {
        assert_eq!(
            inspect("image/png", &apng_fixture(2, fallback, 1)),
            Ok((1, 1))
        );
    }
    assert_eq!(
        inspect("image/png", &apng_fixture(129, false, 1)),
        Err(MediaError::Limit)
    );
    assert_eq!(
        inspect("image/png", &apng_fixture(65, false, 1024)),
        Err(MediaError::Limit)
    );
    assert_eq!(
        inspect("image/png", &apng_fixture(64, true, 1024)),
        Err(MediaError::Limit)
    );
}
#[test]
fn apng_crc_sequence_and_missing_frame() {
    let good = apng_fixture(2, false, 1);
    for n in 0..good.len() {
        assert!(inspect("image/png", &good[..n]).is_err(), "prefix {n}");
    }
    let mut bad = good.clone();
    let pos = bad.windows(4).position(|v| v == b"fdAT").unwrap();
    bad[pos + 7] = 7;
    let crc = super::super::crc32(&bad[pos..pos + 21]);
    bad[pos + 21..pos + 25].copy_from_slice(&crc.to_be_bytes());
    assert_eq!(inspect("image/png", &bad), Err(MediaError::Invalid));
    let mut bad = good;
    bad[20] ^= 1;
    assert_eq!(inspect("image/png", &bad), Err(MediaError::Invalid));
}
#[test]
fn file_png_can_exceed_embedded_budget_but_not_file_budget() {
    let original=STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==").unwrap();
    let mut large = original[..original.len() - 12].to_vec();
    large.extend(png_chunk(b"ruSt", &vec![0; super::super::MAX_IMAGE_BYTES]));
    large.extend(&original[original.len() - 12..]);
    assert_eq!(inspect("image/png", &large), Ok((1, 1)));
    let too_large = vec![0; crate::files::MAX_RAW_BYTES as usize + 1];
    assert_eq!(inspect("image/png", &too_large), Err(MediaError::Limit));
    assert_eq!(inspect("image/avif", &large), Err(MediaError::Limit));
}
#[test]
fn signatures_are_not_interchangeable() {
    let avif = STANDARD.decode(AVIF).unwrap();
    let images = [
        ("image/avif", avif),
        ("image/gif", gif_fixture(1, 1, 1)),
        ("image/webp", webp_fixture(1, 1)),
        ("image/png", apng_fixture(1, false, 1)),
    ];
    for (mime, bytes) in &images {
        for (other, _) in &images {
            if mime != other {
                assert!(inspect(other, bytes).is_err());
            }
        }
    }
}
