use super::*;

type Chunk = ([u8; 4], Vec<u8>);

fn chunks(frames: u32, fallback: bool) -> Vec<Chunk> {
    let mut result = vec![(*b"IHDR", vec![0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0])];
    let data = vec![
        0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0, 5, 0, 1, 0xff,
    ];
    if frames == 0 {
        result.push((*b"IDAT", data));
    } else {
        let mut control = frames.to_be_bytes().to_vec();
        control.extend([0; 4]);
        result.push((*b"acTL", control));
        if fallback {
            result.push((*b"IDAT", data.clone()));
        }
        let mut sequence = 0u32;
        for frame in 0..frames {
            let mut control = sequence.to_be_bytes().to_vec();
            sequence += 1;
            control.extend(1u32.to_be_bytes());
            control.extend(1u32.to_be_bytes());
            control.extend([0; 8]);
            control.extend([0, 1, 0, 10, 0, 0]);
            result.push((*b"fcTL", control));
            if frame == 0 && !fallback {
                result.push((*b"IDAT", data.clone()));
            } else {
                let mut payload = sequence.to_be_bytes().to_vec();
                sequence += 1;
                payload.extend(&data);
                result.push((*b"fdAT", payload));
            }
        }
    }
    result.push((*b"IEND", Vec::new()));
    result
}

fn encode(chunks: &[Chunk]) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in chunks {
        bytes.extend((data.len() as u32).to_be_bytes());
        let start = bytes.len();
        bytes.extend(kind);
        bytes.extend(data);
        let crc = crc32(&bytes[start..]);
        bytes.extend(crc.to_be_bytes());
    }
    bytes
}

/// Old production algorithm, retained only as a small-fixture test oracle.
/// Animation validation is unchanged; previously its loop also copied every
/// non-animation chunk before invoking the strict baseline validator.
fn filtered_before_change(bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    validate_animation(bytes)?;
    let mut base = bytes[..8].to_vec();
    let mut pos = 8;
    while pos < bytes.len() {
        let start = pos;
        let header = take(bytes, &mut pos, 8)?;
        take(bytes, &mut pos, be32(header) as usize)?;
        take(bytes, &mut pos, 4)?;
        if !matches!(&header[4..], b"acTL" | b"fcTL" | b"fdAT") {
            base.extend_from_slice(&bytes[start..pos]);
        }
    }
    super::super::super::png(&base)
}

fn equivalent(bytes: &[u8]) {
    assert_eq!(inspect(bytes), filtered_before_change(bytes));
}

#[test]
fn borrowed_validation_matches_filtered_oracle_on_static_apng_truncations_and_damage() {
    for (frames, fallback) in [(0, false), (1, false), (2, false), (2, true)] {
        let bytes = encode(&chunks(frames, fallback));
        assert_eq!(inspect(&bytes), Ok((1, 1)));
        equivalent(&bytes);
        for end in 0..bytes.len() {
            equivalent(&bytes[..end]);
        }
        for index in 0..bytes.len() {
            let mut changed = bytes.clone();
            changed[index] ^= 0xff;
            equivalent(&changed);
        }
    }
}

#[test]
fn valid_crc_order_and_header_errors_retain_filtered_error_precedence() {
    let valid = chunks(2, false);
    let mut cases = Vec::new();
    let mut bad = valid.clone();
    bad[0].1[8] = 3;
    cases.push(bad);
    let mut bad = valid.clone();
    bad[0].1[9] = 3;
    cases.push(bad); // Indexed without palette.
    let mut bad = valid.clone();
    bad[0].1[12] = 2;
    cases.push(bad);
    let mut bad = valid.clone();
    bad[1].1[..4].copy_from_slice(&129u32.to_be_bytes());
    cases.push(bad);
    let mut bad = valid.clone();
    bad[2].1[..4].copy_from_slice(&99u32.to_be_bytes());
    cases.push(bad);
    let mut bad = valid.clone();
    bad[2].1[24] = 3;
    cases.push(bad);
    let mut bad = valid.clone();
    bad[2].1[25] = 2;
    cases.push(bad);
    let mut bad = valid.clone();
    bad[2].1[4..8].copy_from_slice(&2u32.to_be_bytes());
    cases.push(bad);
    let mut bad = valid.clone();
    bad[5].1.truncate(4);
    cases.push(bad); // No frame body.
    let mut bad = valid.clone();
    bad[6].1.push(1);
    cases.push(bad); // IEND payload.
    let mut bad = valid.clone();
    bad.pop();
    cases.push(bad);
    for kind in [*b"UNKN", *b"te1t", *b"test"] {
        let mut bad = valid.clone();
        bad.insert(4, (kind, vec![1]));
        cases.push(bad);
    }
    let mut bad = valid.clone();
    bad.insert(4, (*b"PLTE", vec![0, 0, 0]));
    cases.push(bad);
    let mut bad = valid.clone();
    bad.insert(1, valid[0].clone());
    cases.push(bad);
    let mut bad = valid.clone();
    bad.push((*b"IDAT", vec![1]));
    cases.push(bad);
    let mut bad = valid.clone();
    bad.push((*b"IEND", Vec::new()));
    cases.push(bad);
    for case in cases {
        let bytes = encode(&case);
        assert!(inspect(&bytes).is_err());
        equivalent(&bytes);
    }
    // The animation pass still wins over malformed base-header bit depth.
    let mut both = valid;
    both[0].1[8] = 3;
    both[1].1[..4].copy_from_slice(&129u32.to_be_bytes());
    let bytes = encode(&both);
    assert_eq!(inspect(&bytes), Err(MediaError::Limit));
    equivalent(&bytes);
}

#[test]
fn idat_continuity_and_apng_frame_order_are_not_relaxed_by_skipping_chunks() {
    let mut multi = chunks(1, false);
    let idat = multi.iter().position(|(kind, _)| kind == b"IDAT").unwrap();
    multi.insert(idat + 1, (*b"IDAT", vec![1, 2, 3]));
    let valid = encode(&multi);
    assert_eq!(inspect(&valid), Ok((1, 1)));
    equivalent(&valid);
    multi.insert(idat + 1, (*b"npAD", vec![0]));
    let interrupted = encode(&multi);
    assert_eq!(inspect(&interrupted), Err(MediaError::Invalid));
    equivalent(&interrupted);

    let mut interleaved = chunks(2, false);
    let second_frame = interleaved
        .iter()
        .rposition(|(kind, _)| kind == b"fcTL")
        .unwrap();
    interleaved.insert(second_frame + 1, (*b"IDAT", vec![1, 2, 3]));
    let bytes = encode(&interleaved);
    assert_eq!(inspect(&bytes), Err(MediaError::Invalid));
    equivalent(&bytes);
    let mut bad_crc = encode(&chunks(2, false));
    let marker = bad_crc
        .windows(4)
        .position(|chunk| chunk == b"fdAT")
        .unwrap();
    bad_crc[marker + 5] ^= 1;
    assert_eq!(inspect(&bad_crc), Err(MediaError::Invalid));
    equivalent(&bad_crc);
    // Direct strict PNG validation still rejects APNG without its wrapper.
    assert_eq!(
        super::super::super::png(&valid),
        Err(MediaError::Unsupported)
    );
}

#[test]
fn animation_after_iend_is_an_explicit_security_delta_from_old_filtering() {
    let mut trailing = chunks(2, false);
    let mut extra = 3u32.to_be_bytes().to_vec();
    extra.push(1);
    trailing.push((*b"fdAT", extra));
    let bytes = encode(&trailing);
    // Precisely reproduce the old bug: stripping the trailing valid-sequence
    // fdAT made IEND appear to terminate the file. Do not normalize this delta.
    assert_eq!(filtered_before_change(&bytes), Ok((1, 1)));
    assert_eq!(inspect(&bytes), Err(MediaError::Invalid));
    for suffix in [vec![0], encode(&[(*b"npAD", vec![0])])[8..].to_vec()] {
        let mut bytes = encode(&chunks(2, false));
        bytes.extend(suffix);
        assert_eq!(inspect(&bytes), Err(MediaError::Invalid));
        equivalent(&bytes);
    }
}

#[test]
fn large_ancillary_bodies_validate_directly_from_the_unchanged_borrowed_input() {
    use sha1::{Digest, Sha1};
    for frames in [0, 2] {
        let mut image = chunks(frames, false);
        image.insert(1, (*b"npAD", vec![b'p'; 3 * 1024 * 1024]));
        let bytes = encode(&image);
        drop(image);
        let pointer = bytes.as_ptr();
        let digest = Sha1::digest(&bytes);
        // The production call graph is borrowed slices plus scalar state:
        // validate_animation -> png_base. The allocating oracle is deliberately
        // not used here, and neither production pass allocates a filtered body.
        assert_eq!(inspect(&bytes), Ok((1, 1)));
        assert_eq!(bytes.as_ptr(), pointer);
        assert_eq!(Sha1::digest(&bytes), digest);
    }
}
