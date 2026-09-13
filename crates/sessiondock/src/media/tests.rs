use super::*;

const RED_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

#[test]
fn empty_projection_never_waits_for_a_busy_media_cache() {
    let store = Arc::new(MediaStore::new());
    let held = store.cache.lock().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let worker_store = store.clone();
    let worker = std::thread::spawn(move || {
        let _ = send.send(worker_store.project(&[]));
    });
    let response = receive.recv_timeout(std::time::Duration::from_secs(2));
    drop(held);
    worker.join().unwrap();
    assert!(response.unwrap().unwrap().is_empty());
}
fn image(bytes: &[u8], mime: &str) -> NativeImage {
    NativeImage::from_block(&json!({"type":"image","source":{"type":"base64","media_type":mime,"data":STANDARD.encode(bytes)}})).unwrap().unwrap()
}
fn red() -> NativeImage {
    NativeImage::from_block(&json!({"type":"image","mime_type":"image/png","data":RED_PNG}))
        .unwrap()
        .unwrap()
}
fn token(image: &NativeImage) -> &str {
    &image.source.token
}
fn png_bytes() -> Vec<u8> {
    STANDARD.decode(RED_PNG).unwrap()
}
fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut result = (payload.len() as u32).to_be_bytes().to_vec();
    result.extend(kind);
    result.extend(payload);
    let checksum = crc32(&result[4..]);
    result.extend(checksum.to_be_bytes());
    result
}
fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut result = vec![0xff, marker];
    result.extend(((payload.len() + 2) as u16).to_be_bytes());
    result.extend(payload);
    result
}
fn jpeg_bytes() -> Vec<u8> {
    // One grayscale pixel, baseline 8-bit, one-code DC/AC Huffman tables.
    let mut data = vec![0xff, 0xd8];
    let mut quant = vec![1u8; 65];
    quant[0] = 0;
    data.extend(segment(0xdb, &quant));
    let mut table = vec![0u8; 18];
    table[1] = 1;
    data.extend(segment(0xc4, &table));
    table[0] = 0x10;
    data.extend(segment(0xc4, &table));
    data.extend(segment(0xc0, &[8, 0, 1, 0, 1, 1, 1, 0x11, 0]));
    data.extend(segment(0xda, &[1, 1, 0, 0, 63, 0]));
    data.extend([0x3f, 0xff, 0xd9]);
    data
}

#[test]
fn explicit_provider_shapes_share_semantics_but_not_new_random_tokens() {
    let shapes = [
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":RED_PNG}}),
        json!({"type":"image","file":{"base64":RED_PNG,"mimeType":"image/png"}}),
        json!({"type":"file","file":{"base64":RED_PNG,"mimeType":"image/png"}}),
        json!({"type":"input_image","image_url":format!("data:image/png;base64,{RED_PNG}")}),
        json!({"type":"image_url","image_url":{"url":format!("data:image/png;base64,{RED_PNG}")}}),
        json!({"type":"image","mimeType":"image/png","data":RED_PNG}),
    ];
    let images = shapes
        .iter()
        .map(|shape| NativeImage::from_block(shape).unwrap().unwrap())
        .collect::<Vec<_>>();
    assert!(
        images
            .iter()
            .all(|image| image.semantic_key() == images[0].semantic_key())
    );
    assert_eq!(
        images.iter().map(token).collect::<BTreeSet<_>>().len(),
        images.len()
    );
    let cloned = images[0].clone();
    assert_eq!(token(&cloned), token(&images[0]));
    assert_eq!(cloned.encoded_len(), RED_PNG.len());
    let store = MediaStore::new();
    let projected = store.project(&images).unwrap();
    for item in projected {
        assert_eq!(item["width"], 1);
        assert_eq!(item["height"], 1);
        assert_eq!(item["mime"], "image/png");
        let blob = store
            .get(
                item["src"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("/api/media/")
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(blob.bytes(), png_bytes());
    }
}

#[test]
fn only_explicit_images_are_recognized_and_remote_urls_and_svg_are_unsupported() {
    for ordinary in [
        Value::Null,
        json!(3),
        json!({"type":"text","text":"image.png","data":"PRIVATE"}),
        json!({"file":{"base64":RED_PNG,"mimeType":"image/png"}}),
    ] {
        assert!(NativeImage::from_block(&ordinary).unwrap().is_none());
    }
    for block in [
        json!({"type":"image","image_url":"https://example.invalid/PRIVATE"}),
        json!({"type":"image","mime_type":"image/svg+xml","data":"PHN2Zz4="}),
        json!({"type":"image","image_url":"data:image/png,PRIVATE"}),
    ] {
        let error = NativeImage::from_block(&block).err().unwrap();
        assert_eq!(error, MediaError::Unsupported.to_string());
        assert!(!error.contains("PRIVATE"));
    }
    for block in [
        json!({"type":"image","source":{"url":"file:///PRIVATE/image.png"}}),
        json!({"type":"image","image_url":"../../PRIVATE/image.png"}),
        json!({"type":"image","source":{"path":"/PRIVATE/image.png"}}),
    ] {
        let image = NativeImage::from_block(&block).unwrap().unwrap();
        assert!(image.file_ref().is_some());
        let store = MediaStore::new();
        assert_eq!(
            store.project(&[image]).unwrap_err(),
            MediaError::Unsupported
        );
        assert_eq!(store.budget.used.load(Ordering::Acquire), 0);
        assert!(store.cache.lock().unwrap().entries.is_empty());
    }
}

#[test]
fn aliases_must_agree_and_untrusted_dimensions_or_names_are_not_projected() {
    let mut block = json!({"type":"image","data":RED_PNG,"mime_type":"image/png","name":"PRIVATE_PATH","dimensions":{"width":999999999,"height":999999999}});
    let store = MediaStore::new();
    let projected = store
        .project(&[NativeImage::from_block(&block).unwrap().unwrap()])
        .unwrap();
    assert_eq!(projected[0]["width"], 1);
    assert!(!projected[0].to_string().contains("PRIVATE"));
    block["base64"] = json!("AAAA");
    assert_eq!(
        NativeImage::from_block(&block).err(),
        Some(MediaError::Invalid.to_string())
    );
    block.as_object_mut().unwrap().remove("base64");
    block["mimeType"] = json!("image/jpeg");
    assert!(NativeImage::from_block(&block).is_err());
}

#[test]
fn base64_errors_and_declared_mime_disagreement_never_register_a_token() {
    let store = MediaStore::new();
    for invalid in ["!!!!", "AA=A", "Zh==", " Zg=", "====", "", "AAA"] {
        let parsed = NativeImage::from_block(
            &json!({"type":"image","mime_type":"image/png","data":invalid}),
        );
        if let Ok(Some(image)) = parsed {
            assert!(matches!(store.project(&[image]), Err(MediaError::Invalid)));
        } else {
            assert!(parsed.is_err());
        }
        assert_eq!(store.budget.used.load(Ordering::Acquire), 0);
    }
    let forged = image(b"<html>PRIVATE</html>", "image/png");
    assert!(matches!(
        store.project(std::slice::from_ref(&forged)),
        Err(MediaError::Invalid)
    ));
    assert!(store.get(token(&forged)).is_none());
    assert!(matches!(
        store.project(&[image(&png_bytes(), "image/jpeg")]),
        Err(MediaError::Invalid)
    ));
    assert!(matches!(
        store.project(&[image(&jpeg_bytes(), "image/png")]),
        Err(MediaError::Invalid)
    ));
    assert_eq!(store.budget.used.load(Ordering::Acquire), 0);
}

#[test]
fn png_crc_container_boundaries_and_pixel_limits_are_checked() {
    let original = png_bytes();
    let mut bad_crc = original.clone();
    bad_crc[40] ^= 1;
    assert_eq!(png(&bad_crc), Err(MediaError::Invalid));
    for end in 0..original.len() {
        assert!(png(&original[..end]).is_err());
    }
    let mut trailing = original.clone();
    trailing.push(0);
    assert!(png(&trailing).is_err());
    let mut big = original.clone();
    big[16..20].copy_from_slice(&(MAX_DIMENSION + 1).to_be_bytes());
    let checksum = crc32(&big[12..29]);
    big[29..33].copy_from_slice(&checksum.to_be_bytes());
    assert_eq!(png(&big), Err(MediaError::Limit));
    assert_eq!(dimensions(8192, 2048), Ok((8192, 2048)));
    assert_eq!(dimensions(8192, 2049), Err(MediaError::Limit));
    assert_eq!(dimensions(0, 1), Err(MediaError::Invalid));
    let mut animated = original[..33].to_vec();
    animated.extend(chunk(b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]));
    animated.extend(&original[33..]);
    assert_eq!(png(&animated), Err(MediaError::Unsupported));
}

#[test]
fn jpeg_markers_dimensions_and_truncation_are_checked() {
    let original = jpeg_bytes();
    assert_eq!(jpeg(&original), Ok((1, 1)));
    for end in 0..original.len() {
        assert!(jpeg(&original[..end]).is_err());
    }
    let store = MediaStore::new();
    let image = image(&original, "image/jpg");
    let projected = store.project(std::slice::from_ref(&image)).unwrap();
    assert_eq!(projected[0]["mime"], "image/jpeg");
    assert_eq!(store.get(token(&image)).unwrap().bytes(), original);
    let mut big = original.clone();
    let sof = big
        .windows(2)
        .position(|pair| pair == [0xff, 0xc0])
        .unwrap();
    big[sof + 5..sof + 7].copy_from_slice(&((MAX_DIMENSION + 1) as u16).to_be_bytes());
    assert_eq!(jpeg(&big), Err(MediaError::Limit));
    let mut trailing = original.clone();
    trailing.push(0);
    assert!(jpeg(&trailing).is_err());
}

#[test]
fn real_decoded_item_limit_is_one_and_a_half_mib_not_legacy_thirty_two_mib() {
    let original = png_bytes();
    let padding = vec![0u8; MAX_IMAGE_BYTES - original.len() - 12];
    let mut maximum = original[..original.len() - 12].to_vec();
    maximum.extend(chunk(b"ruSt", &padding));
    maximum.extend(&original[original.len() - 12..]);
    assert_eq!(maximum.len(), MAX_IMAGE_BYTES);
    let image = image(&maximum, "image/png");
    assert_eq!(image.encoded_len(), MAX_ENCODED_BYTES);
    MediaStore::new().project(&[image]).unwrap();
    maximum.push(0);
    assert_eq!(
        NativeImage::from_block(
            &json!({"type":"image","mime_type":"image/png","data":STANDARD.encode(maximum)})
        )
        .err(),
        Some(MediaError::Limit.to_string())
    );
}

#[test]
fn batch_members_coexist_and_evicted_tokens_can_be_registered_again() {
    let store = MediaStore::with_limits(2, png_bytes().len() * 2);
    let (a, b, c) = (red(), red(), red());
    store.project(&[a.clone(), b.clone()]).unwrap();
    store.project(&[c.clone(), a.clone()]).unwrap();
    assert!(store.get(token(&a)).is_some());
    assert!(store.get(token(&c)).is_some());
    assert!(store.get(token(&b)).is_none());
    let old_token = token(&b).to_owned();
    store.project(&[b.clone(), c.clone()]).unwrap();
    assert!(store.get(&old_token).is_some());
    assert!(store.get(token(&c)).is_some());
    assert_eq!(
        store.budget.used.load(Ordering::Acquire),
        png_bytes().len() * 2
    );
}

#[test]
fn held_response_arcs_keep_budget_after_eviction_until_the_last_clone_is_dropped() {
    let length = png_bytes().len();
    let store = MediaStore::with_limits(2, length * 2);
    let (a, b, c, d) = (red(), red(), red(), red());
    store.project(&[a.clone(), b]).unwrap();
    let held = store.get(token(&a)).unwrap();
    let held_again = held.clone();
    assert!(matches!(
        store.project(&[c.clone(), d.clone()]),
        Err(MediaError::Busy)
    ));
    assert_eq!(store.budget.used.load(Ordering::Acquire), length);
    assert_eq!(held.bytes(), png_bytes());
    assert!(store.get(token(&a)).is_none());
    drop(held);
    assert!(matches!(
        store.project(&[c.clone(), d.clone()]),
        Err(MediaError::Busy)
    ));
    drop(held_again);
    store.project(&[c, d]).unwrap();
    assert_eq!(store.budget.used.load(Ordering::Acquire), length * 2);
}

#[test]
fn failed_batches_release_reservations_and_publish_no_partial_new_tokens() {
    let store = MediaStore::with_limits(2, png_bytes().len() * 2);
    let good = red();
    let bad = image(b"PRIVATE", "image/png");
    assert!(matches!(
        store.project(&[good.clone(), bad.clone()]),
        Err(MediaError::Invalid)
    ));
    assert!(store.get(token(&good)).is_none());
    assert!(store.get(token(&bad)).is_none());
    assert_eq!(store.budget.used.load(Ordering::Acquire), 0);
    store.project(std::slice::from_ref(&good)).unwrap();
    assert!(matches!(
        store.project(&[red(), red(), red()]),
        Err(MediaError::Limit)
    ));
    assert!(store.get(token(&good)).is_some());
}

#[test]
fn duplicates_are_one_cached_blob_tokens_are_strict_and_store_drop_does_not_revoke_held_bytes() {
    let store = MediaStore::new();
    let image = red();
    let projected = store.project(&[image.clone(), image.clone()]).unwrap();
    assert_eq!(projected[0], projected[1]);
    assert_eq!(store.budget.used.load(Ordering::Acquire), png_bytes().len());
    for bad in [
        "",
        "../private",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "000000000000000000000000000000000",
    ] {
        assert!(store.get(bad).is_none());
    }
    let held = store.get(token(&image)).unwrap();
    let budget = store.budget.clone();
    drop(store);
    assert_eq!(budget.used.load(Ordering::Acquire), held.bytes().len());
    drop(held);
    assert_eq!(budget.used.load(Ordering::Acquire), 0);
    assert_eq!(MediaError::Unsupported.status(), 501);
    assert_eq!(MediaError::Limit.status(), 413);
    assert_eq!(MediaError::Busy.status(), 503);
}
