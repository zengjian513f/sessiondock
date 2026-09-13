use super::*;
use crate::files::{FileScope, FileService};
use std::fs;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

fn image(data: &str) -> NativeImage {
    NativeImage::from_block(
        &json!({"type":"image","source":{"media_type":"image/png","data":data}}),
    )
    .unwrap()
    .unwrap()
}
fn register(store: &MediaStore, native: &NativeImage) -> String {
    let prepared = PreparedImage::embedded(native).unwrap();
    let token = prepared.token.clone();
    let values = store.register_prepared(&[prepared]).unwrap();
    assert_eq!(
        values,
        vec![json!({"src":format!("/api/media/{token}"),"alt":"会话图片","lazy":true})]
    );
    token
}
fn used(store: &MediaStore) -> usize {
    store.budget.used.load(Ordering::Acquire)
}
fn encoded(store: &MediaStore) -> usize {
    store.encoded_budget.used.load(Ordering::Acquire)
}
fn materialize(store: &MediaStore, token: &str) -> Result<Arc<MediaBlob>, FileError> {
    store.materialize(store.ticket(token).unwrap(), None)
}
fn limits(descriptors: usize, encoded_bytes: usize, blobs: usize, bytes: usize) -> MediaStore {
    let mut store = MediaStore::with_limits(blobs, bytes);
    store.descriptors = Mutex::new(Descriptors::new(descriptors));
    store.encoded_budget = Arc::new(Budget {
        used: AtomicUsize::new(0),
        maximum: encoded_bytes,
    });
    store
}

#[test]
fn registration_does_not_decode_and_first_get_survives_original_source_drop() {
    let store = MediaStore::new();
    let native = image(PNG);
    let source = Arc::downgrade(&native.source);
    let token = register(&store, &native);
    assert_eq!(used(&store), 0);
    assert!(store.cache.lock().unwrap().entries.is_empty());
    assert_eq!(encoded(&store), PNG.len());
    drop(native); // A normal append replaces provider events and their sources.
    assert!(source.upgrade().is_some());
    let ticket = store.ticket(&token).unwrap();
    assert_eq!(ticket.scope(), None);
    // GET must not reacquire the descriptor table during decode/publication.
    let table = store.descriptors.lock().unwrap();
    let first = store.materialize(ticket, None).unwrap();
    drop(table);
    assert_eq!(first.bytes(), STANDARD.decode(PNG).unwrap());
    assert_eq!(first.mime(), "image/png");
    assert_eq!(used(&store), first.bytes().len());
    let second = materialize(&store, &token).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(used(&store), first.bytes().len());
}

#[test]
fn invalid_encoding_is_registered_but_get_releases_reservation_and_flight() {
    let store = MediaStore::new();
    for payload in ["!!!!", "AAAA"] {
        let token = register(&store, &image(payload));
        assert_eq!(used(&store), 0);
        for _ in 0..2 {
            assert_eq!(materialize(&store, &token).err().unwrap().status, 422);
            assert_eq!(used(&store), 0);
            let cache = store.cache.lock().unwrap();
            assert!(cache.entries.is_empty());
            assert!(cache.in_flight.is_empty());
        }
    }
}

#[test]
fn descriptor_eviction_is_independent_from_blob_cache_and_held_ticket_charge() {
    let store = limits(1, PNG.len() * 3, 256, MAX_CACHE_BYTES);
    let token = register(&store, &image(PNG));
    let held = store.ticket(&token).unwrap();
    let replacement = register(&store, &image(PNG));
    assert_ne!(token, replacement);
    assert!(store.ticket(&token).is_none());
    assert_eq!(encoded(&store), PNG.len() * 2);
    let blob = store.materialize(held, None).unwrap();
    assert_eq!(encoded(&store), PNG.len());
    assert_eq!(blob.bytes(), STANDARD.decode(PNG).unwrap());
    assert!(store.ticket(&token).is_none()); // An old blob does not revive a grant.
}

#[test]
fn held_encoded_source_blocks_replacement_until_ticket_cancelled() {
    let store = limits(1, PNG.len(), 256, MAX_CACHE_BYTES);
    let token = register(&store, &image(PNG));
    let held = store.ticket(&token).unwrap();
    let replacement = PreparedImage::embedded(&image(PNG)).unwrap();
    assert_eq!(
        store
            .register_prepared(std::slice::from_ref(&replacement))
            .unwrap_err(),
        MediaError::Busy
    );
    assert!(store.ticket(&token).is_none());
    assert_eq!(encoded(&store), PNG.len());
    drop(held); // Cancellation before decoding releases retained encoded bytes.
    assert_eq!(encoded(&store), 0);
    assert!(store.register_prepared(&[replacement]).is_ok());
}

#[test]
fn held_blob_cannot_be_evicted_out_of_budget_and_retry_can_materialize() {
    let bytes = STANDARD.decode(PNG).unwrap().len();
    let store = limits(10, PNG.len() * 10, 1, bytes);
    let first = register(&store, &image(PNG));
    let second = register(&store, &image(PNG));
    assert_eq!(used(&store), 0);
    let held = materialize(&store, &first).unwrap();
    assert_eq!(materialize(&store, &second).err().unwrap().status, 503);
    assert_eq!(used(&store), bytes);
    assert!(store.cache.lock().unwrap().entries.is_empty());
    drop(held);
    assert_eq!(used(&store), 0);
    assert!(materialize(&store, &second).is_ok());
    assert!(store.ticket(&first).is_some()); // Descriptor lifetime is separate.
}

#[test]
fn same_token_single_flight_is_busy_without_additional_allocation() {
    let store = MediaStore::new();
    let token = register(&store, &image(PNG));
    store.cache.lock().unwrap().in_flight.insert(token.clone());
    let flight = Flight {
        store: &store,
        token: &token,
    };
    assert_eq!(materialize(&store, &token).err().unwrap().status, 503);
    assert_eq!(used(&store), 0);
    drop(flight);
    assert!(materialize(&store, &token).is_ok());
}

#[test]
fn concurrent_gets_share_one_blob_or_report_busy_without_duplicate_charges() {
    let store = Arc::new(MediaStore::new());
    let token = register(&store, &image(PNG));
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let threads = (0..8)
        .map(|_| {
            let store = store.clone();
            let token = token.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let ticket = store.ticket(&token).unwrap();
                barrier.wait();
                store.materialize(ticket, None)
            })
        })
        .collect::<Vec<_>>();
    let blobs = threads
        .into_iter()
        .filter_map(|thread| match thread.join().unwrap() {
            Ok(blob) => Some(blob),
            Err(error) => {
                assert_eq!(error.status, 503);
                None
            }
        })
        .collect::<Vec<_>>();
    assert!(!blobs.is_empty());
    assert!(blobs.iter().all(|blob| Arc::ptr_eq(blob, &blobs[0])));
    assert_eq!(used(&store), STANDARD.decode(PNG).unwrap().len());
    assert!(store.cache.lock().unwrap().in_flight.is_empty());
}

#[test]
fn oversized_descriptor_batch_is_atomic_and_zero_decode() {
    let store = limits(10, PNG.len(), 256, MAX_CACHE_BYTES);
    let token = register(&store, &image(PNG));
    let images = [
        PreparedImage::embedded(&image(PNG)).unwrap(),
        PreparedImage::embedded(&image(PNG)).unwrap(),
    ];
    assert_eq!(
        store.register_prepared(&images).unwrap_err(),
        MediaError::Limit
    );
    assert!(store.ticket(&token).is_some());
    assert!(
        images
            .iter()
            .all(|image| store.ticket(&image.token).is_none())
    );
    assert_eq!(encoded(&store), PNG.len());
    assert_eq!(used(&store), 0);
}

#[test]
fn descriptors_are_bounded_deduplicated_private_and_store_specific() {
    let store = limits(2, PNG.len() * 2, 256, MAX_CACHE_BYTES);
    let native = image(PNG);
    let a = PreparedImage::embedded(&native).unwrap();
    let b = PreparedImage::embedded(&native).unwrap();
    store.register_prepared(&[a, b]).unwrap();
    assert_eq!(encoded(&store), PNG.len());
    let token = native.source.token.clone();
    let other = MediaStore::new();
    assert_eq!(
        other
            .materialize(store.ticket(&token).unwrap(), None)
            .err()
            .unwrap()
            .status,
        403
    );
    for bad in ["../source", "", "0123456789ABCDEF0123456789ABCDEF00"] {
        assert!(store.ticket(bad).is_none());
    }
    let mut forged = PreparedImage::embedded(&image(PNG)).unwrap();
    forged.token = token.clone();
    assert_eq!(
        store.register_prepared(&[forged]).unwrap_err(),
        MediaError::Unavailable
    );
    assert_eq!(encoded(&store), PNG.len());
    let too_many = (0..257)
        .map(|_| PreparedImage::embedded(&native).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        store.register_prepared(&too_many).unwrap_err(),
        MediaError::Limit
    );
    assert_eq!(used(&store), 0);
}

#[test]
fn default_descriptor_limit_is_1024_and_blob_limit_stays_256() {
    let store = MediaStore::new();
    assert_eq!(store.maximum_items, 256);
    assert_eq!(store.budget.maximum, 32 * 1024 * 1024);
    assert_eq!(store.encoded_budget.maximum, 32 * 1024 * 1024);
    let oldest = register(&store, &image(PNG));
    for _ in 1..1024 {
        register(&store, &image(PNG));
    }
    assert!(store.ticket(&oldest).is_some());
    // Refreshing lookup touched the oldest; insert enough to prove finite LRU.
    let early = store
        .descriptors
        .lock()
        .unwrap()
        .entries
        .iter()
        .min_by_key(|(_, e)| e.used)
        .unwrap()
        .0
        .clone();
    register(&store, &image(PNG));
    assert!(store.ticket(&early).is_none());
    assert!(store.ticket(&oldest).is_some());
    assert_eq!(store.descriptors.lock().unwrap().entries.len(), 1024);
    assert_eq!(used(&store), 0);
}

#[test]
fn file_first_get_and_warm_hit_require_original_version_and_exact_current_scope() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let bytes = STANDARD.decode(PNG).unwrap();
    fs::write(root.join("private.bin"), &bytes).unwrap();
    let files = FileService::open(vec![root.clone()]).unwrap();
    let spec = FileScope {
        uid: "claude:owner",
        agent: Some("child"),
        cwd: root.to_str().unwrap(),
        messages: &[],
    };
    let scope = files.scoped_media(&spec, &["private.bin"]).unwrap();
    let store = MediaStore::new();
    let prepared = PreparedImage::file(&scope, "private.bin").unwrap();
    let token = prepared.token.clone();
    let values = store.register_prepared(&[prepared]).unwrap();
    assert!(!values[0].to_string().contains("private"));
    assert_eq!(used(&store), 0);
    assert_eq!(encoded(&store), 0);
    assert_eq!(
        store.ticket(&token).unwrap().scope(),
        Some(("claude:owner", "child"))
    );
    assert_eq!(materialize(&store, &token).err().unwrap().status, 403);
    let wrong_spec = FileScope {
        agent: None,
        ..spec
    };
    let wrong = files.scoped_media(&wrong_spec, &["private.bin"]).unwrap();
    assert_eq!(
        store
            .materialize(store.ticket(&token).unwrap(), Some(&wrong))
            .err()
            .unwrap()
            .status,
        403
    );
    fs::rename(root.join("private.bin"), root.join("old.bin")).unwrap();
    fs::write(root.join("private.bin"), &bytes).unwrap();
    assert_eq!(
        store
            .materialize(store.ticket(&token).unwrap(), Some(&scope))
            .err()
            .unwrap()
            .status,
        409
    );
    assert_eq!(used(&store), 0);
    let new = PreparedImage::file(&scope, "private.bin").unwrap();
    let new_token = new.token.clone();
    store.register_prepared(&[new]).unwrap();
    let held = store
        .materialize(store.ticket(&new_token).unwrap(), Some(&scope))
        .unwrap();
    let warm = store
        .materialize(store.ticket(&new_token).unwrap(), Some(&scope))
        .unwrap();
    assert!(Arc::ptr_eq(&held, &warm));
    let revoked = files.scoped_media(&spec, &[]).unwrap();
    assert_eq!(
        store
            .materialize(store.ticket(&new_token).unwrap(), Some(&revoked))
            .err()
            .unwrap()
            .code,
        "file_not_referenced"
    );
    fs::rename(root.join("private.bin"), root.join("second.bin")).unwrap();
    fs::write(root.join("private.bin"), &bytes).unwrap();
    assert_eq!(
        store
            .materialize(store.ticket(&new_token).unwrap(), Some(&scope))
            .err()
            .unwrap()
            .status,
        409
    );
    assert_eq!(held.bytes(), bytes); // Previously handed-off responses remain charged.
}

#[test]
fn bad_file_registers_without_reading_and_does_not_prevent_embedded_materialization() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("bad.png"), b"not an image").unwrap();
    let files = FileService::open(vec![root.clone()]).unwrap();
    let spec = FileScope {
        uid: "codex:owner",
        agent: None,
        cwd: root.to_str().unwrap(),
        messages: &[],
    };
    let scope = files.scoped_media(&spec, &["bad.png"]).unwrap();
    let store = MediaStore::new();
    let bad = PreparedImage::file(&scope, "bad.png").unwrap();
    let bad_token = bad.token.clone();
    let good = PreparedImage::embedded(&image(PNG)).unwrap();
    let good_token = good.token.clone();
    let values = store.register_prepared(&[bad, good]).unwrap();
    assert!(values.iter().all(|value| value["lazy"] == true));
    assert_eq!(used(&store), 0);
    assert_eq!(
        store
            .materialize(store.ticket(&bad_token).unwrap(), Some(&scope))
            .err()
            .unwrap()
            .status,
        422
    );
    assert_eq!(used(&store), 0);
    assert!(materialize(&store, &good_token).is_ok());
}
