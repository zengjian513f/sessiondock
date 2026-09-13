use super::*;
use std::io::Cursor;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
const UID: &str = "claude:0123456789abcdef";

fn span(data: &[u8], offset: usize, mime: &str) -> NativeSpan {
    NativeSpan {
        root: std::env::temp_dir().join("synthetic-native-not-opened"),
        path: std::env::temp_dir().join("synthetic-native-not-opened/fixture.jsonl"),
        file_identity: "synthetic-stamp".into(),
        record_start: 0,
        record_end: data.len() as u64 + 100,
        start: 20,
        end: data.len() as u64 + 20,
        plan: None,
        decoded_len: data.len() as u64,
        decoded_sha1: Sha1::digest(data).into(),
        encoded_offset: offset as u64,
        payload_sha1: Sha1::digest(&data[offset..]).into(),
        mime: mime.into(),
    }
}
fn native(data: &[u8]) -> NativeImage {
    NativeImage::from_native_span(span(data, 0, "image/png")).unwrap()
}
fn register(store: &MediaStore, image: &NativeImage, agent: &str) -> String {
    let prepared = PreparedImage::native_span(image, UID, agent).unwrap();
    let token = prepared.token.clone();
    assert_eq!(
        store.register_prepared(&[prepared]).unwrap(),
        [json!({"src":format!("/api/media/{token}"),"alt":"会话图片","lazy":true})]
    );
    token
}
fn get(store: &MediaStore, token: &str, data: &[u8]) -> Result<Arc<MediaBlob>, FileError> {
    store.materialize_native(store.ticket(token).unwrap(), data)
}

#[test]
fn native_base64_accepts_python_complete_group_padding() {
    for encoded in [b"Zm9v=".as_slice(), b"Zm9v==", b"Zm9v====="] {
        let store = MediaStore::new();
        let token = register(&store, &native(encoded), "");
        assert_eq!(get(&store, &token, encoded).unwrap().bytes(), b"foo");
    }
}

#[test]
fn native_data_url_accepts_mime_parameters_without_a_header_quota() {
    let header = format!("data:image/png;name={};base64,", "x".repeat(300));
    let data = format!("{header}Zm9v");
    let image =
        NativeImage::from_native_span(span(data.as_bytes(), header.len(), "image/png")).unwrap();
    let store = MediaStore::new();
    let token = register(&store, &image, "");
    assert_eq!(
        get(&store, &token, data.as_bytes()).unwrap().bytes(),
        b"foo"
    );
}
fn used(store: &MediaStore) -> usize {
    store.budget.used.load(Ordering::Acquire)
}

fn nested_span(data: &str, encoded_offset: usize) -> (NativeSpan, String) {
    use crate::native_replay::StringRange;
    let outer = json!({"content":[{"type":"image","data":data}]}).to_string();
    let physical = serde_json::to_string(&outer).unwrap();
    let inner_start = outer.find(data).unwrap() as u64;
    let mut meta = span(data.as_bytes(), encoded_offset, "image/png");
    meta.end = meta.start + physical.len() as u64 - 2;
    meta.record_end = meta.end + 20;
    meta.plan = Some(
        DecodePlan::new(vec![
            StringRange {
                start: meta.start,
                end: meta.end,
                decoded_len: outer.len() as u64,
                decoded_sha1: Sha1::digest(outer.as_bytes()).into(),
            },
            StringRange {
                start: inner_start,
                end: inner_start + data.len() as u64,
                decoded_len: data.len() as u64,
                decoded_sha1: Sha1::digest(data.as_bytes()).into(),
            },
        ])
        .unwrap(),
    );
    (meta, outer)
}

#[test]
fn nested_plan_binds_outer_coordinates_and_final_image_hash_not_outer_length() {
    let (meta, _) = nested_span(PNG, 0);
    let plan = meta.plan.as_ref().unwrap();
    assert!(plan.first().decoded_len > meta.decoded_len);
    assert_eq!(plan.last().decoded_len, meta.decoded_len);
    assert!(NativeImage::from_native_span(meta.clone()).is_ok());

    let mut cases = Vec::new();
    let mut bad = meta.clone();
    bad.start += 1;
    cases.push(bad);
    let mut bad = meta.clone();
    bad.end -= 1;
    cases.push(bad);
    let mut bad = meta.clone();
    bad.decoded_len -= 1;
    cases.push(bad);
    let mut bad = meta.clone();
    bad.decoded_sha1[0] ^= 1;
    bad.payload_sha1 = bad.decoded_sha1;
    cases.push(bad);
    let mut bad = meta.clone();
    bad.plan = Some(plan.with_outer_offset(1).unwrap());
    cases.push(bad);
    let mut bad = meta.clone();
    let mut ranges = plan.ranges().to_vec();
    ranges.last_mut().unwrap().decoded_sha1[0] ^= 1;
    bad.plan = Some(DecodePlan::new(ranges).unwrap());
    cases.push(bad);
    for bad in cases {
        assert!(matches!(
            NativeImage::from_native_span(bad),
            Err(MediaError::Invalid)
        ));
    }
}

#[test]
fn nested_plan_preserves_image_semantics_but_is_part_of_the_exact_private_grant() {
    let (meta, outer) = nested_span(PNG, 0);
    let image = NativeImage::from_native_span(meta.clone()).unwrap();
    let direct = native(PNG.as_bytes());
    let inline =
        NativeImage::from_block(&json!({"type":"image","mime_type":"image/png","data":PNG}))
            .unwrap()
            .unwrap();
    let url = format!("data:image/png;base64,{PNG}");
    let (url_meta, _) = nested_span(&url, 22);
    let url_image = NativeImage::from_native_span(url_meta).unwrap();
    for other in [&direct, &inline, &url_image] {
        assert_eq!(image.semantic_key(), other.semantic_key());
        assert_eq!(other.encoded_len(), PNG.len());
    }
    let mut altered = meta.clone();
    let mut ranges = altered.plan.as_ref().unwrap().ranges().to_vec();
    ranges[0].decoded_sha1[0] ^= 1;
    altered.plan = Some(DecodePlan::new(ranges).unwrap());
    let different_origin = NativeImage::from_native_span(altered).unwrap();
    assert_eq!(image.semantic_key(), different_origin.semantic_key());
    assert!(image.native_span() != different_origin.native_span());

    let store = MediaStore::new();
    let main = register(&store, &image, "");
    let agent = register(&store, &image, "worker");
    assert_ne!(main, agent);
    assert_eq!(register(&store, &image, ""), main);
    let ticket = store.ticket(&main).unwrap();
    let (scope_uid, scope_agent, grant) = ticket.native_grant().unwrap();
    assert_eq!((scope_uid, scope_agent), (UID, ""));
    assert!(grant == &meta);
    assert!(store.materialize(ticket, None).is_err());
    // Media does not replay arbitrary outer JSON. Its caller must provide the
    // final unescaped image stream and finish the checked replay independently.
    assert_eq!(
        get(&store, &main, outer.as_bytes()).err().unwrap().status,
        409
    );
    let first = get(&store, &main, PNG.as_bytes()).unwrap();
    let warm = get(&store, &main, PNG.as_bytes()).unwrap();
    assert!(Arc::ptr_eq(&first, &warm));
    assert_eq!(first.bytes(), STANDARD.decode(PNG).unwrap());
    let url_token = register(&store, &url_image, "");
    assert_eq!(
        get(&store, &url_token, url.as_bytes()).unwrap().bytes(),
        first.bytes()
    );
}

#[test]
fn nested_plan_boxed_ranges_are_charged_once_in_descriptor_residency() {
    let (mut meta, _) = nested_span(PNG, 0);
    let with_plan = meta.resident_len();
    let plan = meta.plan.take().unwrap();
    let plan_heap = plan
        .resident_len()
        .saturating_sub(std::mem::size_of::<DecodePlan>());
    assert_eq!(with_plan, meta.resident_len() + plan_heap);
    meta.plan = Some(plan);
    let image = NativeImage::from_native_span(meta).unwrap();
    let store = MediaStore::new();
    let token = register(&store, &image, "worker");
    assert_eq!(
        store.encoded_budget.used.load(Ordering::Acquire),
        image.resident_len() + UID.len() + "worker".len()
    );
    assert_eq!(used(&store), 0);
    let descriptor = store.ticket(&token).unwrap();
    drop(image);
    assert_eq!(
        descriptor
            .native_grant()
            .unwrap()
            .2
            .plan
            .as_ref()
            .unwrap()
            .ranges()
            .len(),
        2
    );
}

#[test]
fn span_is_metadata_only_and_semantics_match_inline_and_data_url() {
    let image = native(PNG.as_bytes());
    let inline =
        NativeImage::from_block(&json!({"type":"image","mime_type":"image/png","data":PNG}))
            .unwrap()
            .unwrap();
    let data_url = format!("data:image/png;base64,{PNG}");
    let url = NativeImage::from_native_span(span(data_url.as_bytes(), 22, "image/png")).unwrap();
    assert_eq!(image.semantic_key(), inline.semantic_key());
    assert_eq!(image.semantic_key(), url.semantic_key());
    assert_eq!(image.encoded_len(), PNG.len());
    assert_eq!(url.encoded_len(), PNG.len());
    assert!(image.resident_len() < 1024);
    assert!(PreparedImage::embedded(&image).is_err());
    let store = MediaStore::new();
    let token = register(&store, &image, "");
    assert_eq!(register(&store, &image, ""), token);
    let other = register(&store, &image, "child");
    assert_ne!(token, other);
    let ticket = store.ticket(&token).unwrap();
    assert_eq!(ticket.scope(), Some((UID, "")));
    let (uid, agent, grant) = ticket.native_grant().unwrap();
    assert_eq!((uid, agent), (UID, ""));
    assert!(grant == image.native_span().unwrap());
    assert_eq!(used(&store), 0);
    assert!(store.materialize(ticket, None).is_err());
    assert_eq!(
        MediaStore::new()
            .materialize_native(store.ticket(&token).unwrap(), PNG.as_bytes())
            .err()
            .unwrap()
            .status,
        403
    );
    let url_token = register(&store, &url, "");
    assert_eq!(
        get(&store, &url_token, data_url.as_bytes())
            .unwrap()
            .bytes(),
        STANDARD.decode(PNG).unwrap()
    );
}

#[test]
fn invalid_metadata_and_scope_are_rejected_without_opening_paths() {
    let base = span(PNG.as_bytes(), 0, "image/png");
    let mut cases = Vec::new();
    let mut bad = base.clone();
    bad.path = "/elsewhere/fixture.jsonl".into();
    cases.push(bad);
    let mut bad = base.clone();
    bad.record_end = bad.end;
    cases.push(bad);
    let mut bad = base.clone();
    bad.start = bad.record_start;
    cases.push(bad);
    let mut bad = base.clone();
    bad.decoded_len += 1;
    cases.push(bad);
    let mut bad = base.clone();
    bad.payload_sha1[0] ^= 1;
    cases.push(bad);
    let mut bad = base.clone();
    bad.encoded_offset = bad.decoded_len + 1;
    cases.push(bad);
    let mut bad = base.clone();
    bad.file_identity.clear();
    cases.push(bad);
    for bad in cases {
        assert!(NativeImage::from_native_span(bad).is_err());
    }
    let image = NativeImage::from_native_span(base).unwrap();
    for uid in [
        "claude:short",
        "unknown:0123456789abcdef",
        "codex:0123456789ABCDEF",
    ] {
        assert!(PreparedImage::native_span(&image, uid, "").is_err());
    }
    assert!(PreparedImage::native_span(&image, UID, "bad\nagent").is_ok());
}

#[test]
fn cold_and_warm_reads_require_all_current_bytes_and_never_raw_cache_authorize() {
    let store = MediaStore::new();
    let image = native(PNG.as_bytes());
    let source = Arc::downgrade(&image.source);
    let token = register(&store, &image, "");
    drop(image);
    assert!(source.upgrade().is_some());
    let first = get(&store, &token, PNG.as_bytes()).unwrap();
    assert_eq!(first.bytes(), STANDARD.decode(PNG).unwrap());
    assert_eq!((first.width, first.height), (1, 1));
    assert_eq!(used(&store), first.bytes.capacity());
    let mut input = Cursor::new(PNG.as_bytes());
    let second = store
        .materialize_native(store.ticket(&token).unwrap(), &mut input)
        .unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(input.position(), PNG.len() as u64);
    assert!(store.get(&token).is_none());
    let mut changed = PNG.as_bytes().to_vec();
    changed[8] ^= 1;
    let mut extra = PNG.as_bytes().to_vec();
    extra.push(b'A');
    for input in [
        &changed[..],
        &extra[..],
        &PNG.as_bytes()[..PNG.len() - 1],
        &[],
    ] {
        assert_eq!(get(&store, &token, input).err().unwrap().status, 409);
        let cold = MediaStore::new();
        let fresh = register(&cold, &native(PNG.as_bytes()), "");
        assert_eq!(get(&cold, &fresh, input).err().unwrap().status, 409);
        assert_eq!(used(&cold), 0);
    }
}

#[test]
fn malformed_base64_releases_reservation_for_retry() {
    let store = MediaStore::new();
    for payload in ["!!!!", "AA=A", "=AAA"] {
        let token = register(&store, &native(payload.as_bytes()), "");
        for _ in 0..2 {
            assert_eq!(
                get(&store, &token, payload.as_bytes())
                    .err()
                    .unwrap()
                    .status,
                422
            );
            assert_eq!(used(&store), 0);
        }
    }
    let wrong = format!("data:image/gif;base64,{PNG}");
    let image = NativeImage::from_native_span(span(wrong.as_bytes(), 22, "image/png")).unwrap();
    let token = register(&store, &image, "");
    assert_eq!(
        get(&store, &token, wrong.as_bytes()).err().unwrap().status,
        422
    );
    assert_eq!(used(&store), 0);
}

#[test]
fn native_ticket_retains_metadata_charge_after_eviction_and_does_not_revive_lookup() {
    let mut store = MediaStore::new();
    store.descriptors = Mutex::new(descriptors::Descriptors::new(1));
    let image = native(PNG.as_bytes());
    let token = register(&store, &image, "");
    let held = store.ticket(&token).unwrap();
    let charge = store.encoded_budget.used.load(Ordering::Acquire);
    let replacement = register(&store, &native(PNG.as_bytes()), "");
    assert!(store.ticket(&token).is_none());
    assert!(store.ticket(&replacement).is_some());
    assert_eq!(
        store.encoded_budget.used.load(Ordering::Acquire),
        charge * 2
    );
    let blob = store.materialize_native(held, PNG.as_bytes()).unwrap();
    assert_eq!(blob.bytes(), STANDARD.decode(PNG).unwrap());
    assert_eq!(store.encoded_budget.used.load(Ordering::Acquire), charge);
    assert!(store.ticket(&token).is_none());
    assert!(store.get(&token).is_none());
}

#[test]
fn escaped_source_reader_can_return_tiny_chunks_and_interruptions() {
    struct Chunked<'a> {
        input: &'a [u8],
        size: usize,
        interrupt: bool,
    }
    impl Read for Chunked<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if self.interrupt {
                self.interrupt = false;
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.interrupt = true;
            let size = output.len().min(self.size);
            self.input.read(&mut output[..size])
        }
    }
    // The physical JSON string may be escaped; supplied reader is already
    // unescaped. Media checks its whole content, independent of IO chunking.
    let data = format!("data:image/png;base64,{PNG}");
    let mut meta = span(data.as_bytes(), 22, "image/png");
    meta.end += 40;
    meta.record_end += 40;
    let image = NativeImage::from_native_span(meta).unwrap();
    for size in [1, 2, 7, 8192] {
        let store = MediaStore::new();
        let token = register(&store, &image, "");
        for _ in 0..2 {
            let reader = Chunked {
                input: data.as_bytes(),
                size,
                interrupt: true,
            };
            let blob = store
                .materialize_native(store.ticket(&token).unwrap(), reader)
                .unwrap();
            assert_eq!(blob.bytes(), STANDARD.decode(PNG).unwrap());
        }
    }
}

#[test]
fn io_failure_and_unwind_release_blob_allocation() {
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("synthetic private error"))
        }
    }
    struct Panics;
    impl Read for Panics {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("synthetic reader panic")
        }
    }
    let store = MediaStore::new();
    let token = register(&store, &native(PNG.as_bytes()), "");
    let error = store
        .materialize_native(store.ticket(&token).unwrap(), Broken)
        .err()
        .unwrap();
    assert_eq!(error.status, 409);
    assert!(!error.message.contains("private"));
    assert_eq!(used(&store), 0);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = store.materialize_native(store.ticket(&token).unwrap(), Panics);
        }))
        .is_err()
    );
    assert_eq!(used(&store), 0);
    assert!(get(&store, &token, PNG.as_bytes()).is_ok());
}

#[test]
fn read_holds_no_cache_lock_and_concurrent_get_succeeds() {
    struct Check<'a> {
        store: &'a MediaStore,
        token: &'a str,
        source: &'a [u8],
        checked: bool,
    }
    impl Read for Check<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            assert!(self.store.cache.try_lock().is_ok());
            assert!(self.store.descriptors.try_lock().is_ok());
            if !self.checked {
                self.checked = true;
                assert_eq!(
                    get(self.store, self.token, PNG.as_bytes()).unwrap().bytes(),
                    STANDARD.decode(PNG).unwrap()
                );
            }
            self.source.read(output)
        }
    }
    let store = MediaStore::new();
    let token = register(&store, &native(PNG.as_bytes()), "");
    let reader = Check {
        store: &store,
        token: &token,
        source: PNG.as_bytes(),
        checked: false,
    };
    let blob = store
        .materialize_native(store.ticket(&token).unwrap(), reader)
        .unwrap();
    assert_eq!(blob.bytes(), STANDARD.decode(PNG).unwrap());
}

/// Emits a valid 1x1 BMP plus declared trailing zero bytes for byte-limit tests.
struct BmpBase64 {
    prefix: Vec<u8>,
    position: usize,
    length: usize,
    padding: usize,
}
impl BmpBase64 {
    fn new(raw_len: usize) -> Self {
        let mut header = [0; 54];
        header[..2].copy_from_slice(b"BM");
        header[2..6].copy_from_slice(&(raw_len as u32).to_le_bytes());
        header[10..14].copy_from_slice(&54u32.to_le_bytes());
        header[14..18].copy_from_slice(&40u32.to_le_bytes());
        header[18..22].copy_from_slice(&1u32.to_le_bytes());
        header[22..26].copy_from_slice(&1u32.to_le_bytes());
        header[26..28].copy_from_slice(&1u16.to_le_bytes());
        header[28..30].copy_from_slice(&24u16.to_le_bytes());
        header[34..38].copy_from_slice(&4u32.to_le_bytes());
        Self {
            prefix: STANDARD.encode(header).into_bytes(),
            position: 0,
            length: raw_len.div_ceil(3) * 4,
            padding: (3 - raw_len % 3) % 3,
        }
    }
    fn image(raw_len: usize) -> NativeImage {
        let mut input = Self::new(raw_len);
        let mut hash = Sha1::new();
        let mut bytes = [0; 8192];
        loop {
            let count = input.read(&mut bytes).unwrap();
            if count == 0 {
                break;
            }
            hash.update(&bytes[..count]);
        }
        let digest = hash.finalize().into();
        let mut meta = span(b"AAAA", 0, "image/bmp");
        meta.decoded_len = input.length as u64;
        meta.end = meta.start + meta.decoded_len;
        meta.record_end = meta.end + 1;
        meta.decoded_sha1 = digest;
        meta.payload_sha1 = digest;
        NativeImage::from_native_span(meta).unwrap()
    }
}
impl Read for BmpBase64 {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = output.len().min(self.length - self.position);
        output[..count].fill(b'A');
        if self.position < self.prefix.len() {
            let prefix = count.min(self.prefix.len() - self.position);
            output[..prefix].copy_from_slice(&self.prefix[self.position..self.position + prefix]);
        }
        let padding_start = (self.length - self.padding)
            .saturating_sub(self.position)
            .min(count);
        output[padding_start..count].fill(b'=');
        self.position += count;
        Ok(count)
    }
}

#[test]
fn exact_32_mib_streams_into_one_charged_blob_and_one_extra_byte_is_rejected() {
    let store = MediaStore::new();
    let image = BmpBase64::image(MAX_IMAGE_BYTES);
    assert!(image.encoded_len() > MAX_IMAGE_BYTES);
    assert!(image.resident_len() < 1024);
    let token = register(&store, &image, "");
    assert!(store.encoded_budget.used.load(Ordering::Acquire) < 2048);
    let blob = store
        .materialize_native(
            store.ticket(&token).unwrap(),
            BmpBase64::new(MAX_IMAGE_BYTES),
        )
        .unwrap();
    assert_eq!(blob.bytes().len(), MAX_IMAGE_BYTES);
    assert_eq!(used(&store), MAX_IMAGE_BYTES);
    assert_eq!((blob.width, blob.height, blob.mime()), (0, 0, "image/bmp"));
    let small = register(&store, &native(PNG.as_bytes()), "");
    assert_eq!(
        get(&store, &small, PNG.as_bytes()).unwrap().bytes(),
        STANDARD.decode(PNG).unwrap()
    );
    drop(blob);

    let store = MediaStore::new();
    let overflow = BmpBase64::image(MAX_IMAGE_BYTES + 1);
    assert_eq!(overflow.encoded_len(), image.encoded_len()); // Padding ambiguity needs a decoded-byte probe.
    let token = register(&store, &overflow, "");
    assert_eq!(
        store
            .materialize_native(
                store.ticket(&token).unwrap(),
                BmpBase64::new(MAX_IMAGE_BYTES + 1)
            )
            .err()
            .unwrap()
            .status,
        413
    );
    assert_eq!(used(&store), 0);
}
