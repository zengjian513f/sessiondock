use super::*;
use crate::files::{FileScope, FileService};
use std::fs;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

#[test]
fn file_tokens_cannot_bypass_current_scope_and_versions_or_use_embedded_get() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("private.bin"), STANDARD.decode(PNG).unwrap()).unwrap();
    let files = FileService::open(vec![root.clone()]).unwrap();
    let specification = FileScope {
        uid: "codex:owner",
        agent: Some("child"),
        cwd: root.to_str().unwrap(),
        messages: &[],
    };
    let scoped = files
        .scoped_media(&specification, &["private.bin"])
        .unwrap();
    let media = MediaStore::new();
    let image = PreparedImage::file(&scoped, "private.bin").unwrap();
    let token = image.token.clone();
    let projected = media.project_prepared(&[image]).unwrap();
    assert_eq!(projected[0]["mime"], "image/png");
    assert!(!projected[0].to_string().contains("private"));
    assert!(media.get(&token).is_none());
    assert_eq!(
        media
            .file_ticket(&token)
            .unwrap()
            .authorize(&scoped)
            .unwrap()
            .bytes(),
        STANDARD.decode(PNG).unwrap()
    );
    let other = FileScope {
        uid: "codex:other",
        ..specification
    };
    let wrong = files.scoped_media(&other, &["private.bin"]).unwrap();
    assert_eq!(
        media
            .file_ticket(&token)
            .unwrap()
            .authorize(&wrong)
            .err()
            .unwrap()
            .status,
        403
    );
    let revoked = files.scoped_media(&specification, &[]).unwrap();
    assert_eq!(
        media
            .file_ticket(&token)
            .unwrap()
            .authorize(&revoked)
            .err()
            .unwrap()
            .code,
        "file_not_referenced"
    );
    // Replace with byte-identical content: identity, not just bytes/dimensions,
    // must change the grant. Already returned bytes are not retroactively revoked.
    fs::rename(root.join("private.bin"), root.join("old.bin")).unwrap();
    fs::write(root.join("private.bin"), STANDARD.decode(PNG).unwrap()).unwrap();
    assert_eq!(
        media
            .file_ticket(&token)
            .unwrap()
            .authorize(&scoped)
            .err()
            .unwrap()
            .status,
        409
    );
    let replacement = PreparedImage::file(&scoped, "private.bin").unwrap();
    assert_ne!(token, replacement.token);
    assert_ne!(
        media.project_prepared(&[replacement]).unwrap()[0]["src"],
        projected[0]["src"]
    );
}

#[test]
fn replacement_after_prepare_returns_an_item_error_and_releases_reserved_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("image.png"), STANDARD.decode(PNG).unwrap()).unwrap();
    let files = FileService::open(vec![root.clone()]).unwrap();
    let spec = FileScope {
        uid: "codex:owner",
        agent: None,
        cwd: root.to_str().unwrap(),
        messages: &[],
    };
    let scope = files.scoped_media(&spec, &["image.png"]).unwrap();
    let image = PreparedImage::file(&scope, "image.png").unwrap();
    fs::rename(root.join("image.png"), root.join("old.png")).unwrap();
    fs::write(
        root.join("image.png"),
        b"must never replace the prepared handle",
    )
    .unwrap();
    let store = MediaStore::new();
    let projected = store.project_prepared(&[image]).unwrap();
    assert!(projected[0]["error"].is_object());
    assert!(projected[0].get("src").is_none());
    assert_eq!(store.budget.used.load(Ordering::Acquire), 0);
    assert!(store.cache.lock().unwrap().entries.is_empty());
}

#[test]
fn file_and_embedded_buffers_share_the_same_held_response_budget() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let bytes = STANDARD.decode(PNG).unwrap();
    fs::write(root.join("image.png"), &bytes).unwrap();
    let files = FileService::open(vec![root.clone()]).unwrap();
    let spec = FileScope {
        uid: "codex:owner",
        agent: None,
        cwd: root.to_str().unwrap(),
        messages: &[],
    };
    let scope = files.scoped_media(&spec, &["image.png"]).unwrap();
    let store = MediaStore::with_limits(1, bytes.len());
    let prepared = PreparedImage::file(&scope, "image.png").unwrap();
    let token = prepared.token.clone();
    store.project_prepared(&[prepared]).unwrap();
    let held = store
        .file_ticket(&token)
        .unwrap()
        .authorize(&scope)
        .unwrap();
    let embedded =
        NativeImage::from_block(&json!({"type":"image","mime_type":"image/png","data":PNG}))
            .unwrap()
            .unwrap();
    store.project(std::slice::from_ref(&embedded)).unwrap();
    assert_eq!(store.budget.used.load(Ordering::Acquire), bytes.len() * 2);
    drop(held);
    assert_eq!(store.budget.used.load(Ordering::Acquire), bytes.len());
    assert!(store.project(&[embedded]).is_ok());
}

#[test]
fn bad_disk_image_does_not_discard_a_valid_embedded_image_in_the_batch() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("invalid.png"), b"not an image").unwrap();
    let files = FileService::open(vec![root.clone()]).unwrap();
    let spec = FileScope {
        uid: "codex:owner",
        agent: None,
        cwd: root.to_str().unwrap(),
        messages: &[],
    };
    let scope = files.scoped_media(&spec, &["invalid.png"]).unwrap();
    let embedded =
        NativeImage::from_block(&json!({"type":"image","mime_type":"image/png","data":PNG}))
            .unwrap()
            .unwrap();
    let store = MediaStore::new();
    let projected = store
        .project_prepared(&[
            PreparedImage::file(&scope, "invalid.png").unwrap(),
            PreparedImage::embedded(&embedded).unwrap(),
        ])
        .unwrap();
    assert!(projected[0]["src"].is_string());
    assert!(projected[1]["src"].is_string());
    assert_eq!(
        store.budget.used.load(Ordering::Acquire),
        STANDARD.decode(PNG).unwrap().len() + b"not an image".len()
    );
}
