use super::*;
use sha1::{Digest, Sha1};
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    path: PathBuf,
    stamp: FileStamp,
}
impl Fixture {
    fn new(bytes: &[u8]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("native")).unwrap();
        let path = root.join("native/synthetic.jsonl");
        std::fs::write(&path, bytes).unwrap();
        let stamp = stamp(&path).unwrap();
        Self {
            _temp: temp,
            root,
            path,
            stamp,
        }
    }
    fn open(&self) -> CheckedNative {
        CheckedNative::open(&self.root, &self.path, &self.stamp).unwrap()
    }
}
fn advance_modified(fixture: &Fixture) {
    let seconds = (fixture.stamp.modified / 1_000_000_000) as u64 + 2;
    std::fs::File::options()
        .write(true)
        .open(&fixture.path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
        .unwrap();
}
fn scan(bytes: &[u8]) -> RawIndex {
    RawIndex::scan(bytes).unwrap()
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}
fn assert_index(index: &RawIndex, bytes: &[u8]) {
    assert_eq!(index.length(), bytes.len() as u64);
    assert_eq!(
        index.committed(),
        bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index as u64 + 1)
    );
    assert_eq!(index.head_bytes(), &bytes[..bytes.len().min(HEAD)]);
    assert_eq!(index.digest(), hash(bytes));
    assert!(index.is_checkpoint(0));
    assert_eq!(index.prefix_hash(0), Some(hash(b"")));
    for end in 1..=bytes.len() {
        assert_eq!(index.is_checkpoint(end as u64), bytes[end - 1] == b'\n');
        if bytes[end - 1] == b'\n' {
            assert_eq!(index.prefix_hash(end as u64), Some(hash(&bytes[..end])));
        } else {
            assert!(index.prefix_hash(end as u64).is_none());
        }
    }
    assert!(!index.is_checkpoint(bytes.len() as u64 + 1));
}

#[test]
fn checked_ranges_seek_once_and_never_expose_prefix_or_suffix_bytes() {
    let fixture = Fixture::new(b"PREFIXpayloadSUFFIX");
    for (start, end) in [(0, 0), (6, 6), (6, 13), (19, 19), (0, 19)] {
        let mut reader =
            CheckedNative::open_range(&fixture.root, &fixture.path, &fixture.stamp, start, end)
                .unwrap();
        assert_eq!(reader.file.stream_position().unwrap(), start);
        let mut output = Vec::new();
        reader.read_to_end(&mut output).unwrap();
        assert_eq!(
            output,
            &b"PREFIXpayloadSUFFIX"[start as usize..end as usize]
        );
        assert_eq!(reader.file.stream_position().unwrap(), end);
        reader.finish().unwrap();
    }
    assert!(
        CheckedNative::open_range(&fixture.root, &fixture.path, &fixture.stamp, 6, 13)
            .unwrap()
            .finish()
            .is_err()
    );
    for (start, end) in [(8, 7), (0, 20), (u64::MAX, u64::MAX)] {
        assert_eq!(
            CheckedNative::open_range(&fixture.root, &fixture.path, &fixture.stamp, start, end)
                .err()
                .unwrap()
                .status,
            409
        );
    }
}

#[test]
fn large_sparse_ranges_remain_bounded_by_actual_file_size() {
    let fixture = Fixture::new(b"x");
    let offset = 6 * 1024 * 1024 * 1024;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&fixture.path)
        .unwrap();
    file.set_len(offset + 3).unwrap();
    let expected = stamp(&fixture.path).unwrap();
    assert!(
        CheckedNative::open_range(&fixture.root, &fixture.path, &expected, 0, offset + 1).is_ok()
    );
    let mut range =
        CheckedNative::open_range(&fixture.root, &fixture.path, &expected, offset, offset + 3)
            .unwrap();
    let mut output = Vec::new();
    range.read_to_end(&mut output).unwrap();
    assert_eq!(output, [0; 3]);
    range.finish().unwrap();
    // A wide range can be authorized without materializing the sparse fixture.
    assert!(
        CheckedNative::open_range(&fixture.root, &fixture.path, &expected, 1, offset + 1).is_ok()
    );
}

#[test]
fn changes_outside_range_still_invalidate_finish_and_stale_reopen() {
    for offset in [0, 18] {
        let fixture = Fixture::new(b"PREFIXpayloadSUFFIX");
        let mut range =
            CheckedNative::open_range(&fixture.root, &fixture.path, &fixture.stamp, 6, 13).unwrap();
        range.read_exact(&mut [0; 7]).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&fixture.path)
            .unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        file.write_all(b"Z").unwrap();
        drop(file);
        advance_modified(&fixture);
        assert!(range.finish().is_err());
        assert!(
            CheckedNative::open_range(&fixture.root, &fixture.path, &fixture.stamp, 6, 13).is_err()
        );
    }
}

#[test]
fn pushed_index_matches_read_scan_at_every_boundary_and_probe() {
    let bytes = "\n\r\n中文😀\nlast\npartial".as_bytes();
    for probe in (0..=bytes.len() as u64 + 1).map(Some).chain([None]) {
        let scanned = RawIndex::scan_with_probe(bytes, probe).unwrap();
        for chunk in [1, 2, 3, 7, CHUNK] {
            let mut builder = RawIndexBuilder::new(probe).unwrap();
            builder.push(&[]).unwrap();
            for piece in bytes.chunks(chunk) {
                builder.push(piece).unwrap();
            }
            let index = builder.finish().unwrap();
            assert_index(&index, bytes);
            assert_eq!(index.committed_digest(), scanned.committed_digest());
            assert_eq!(index.probe_digest(), scanned.probe_digest());
        }
    }
}

#[test]
fn checked_reader_caps_each_read_and_finishes_only_complete_verified_input() {
    let bytes = vec![b'x'; CHUNK * 3 + 19];
    let fixture = Fixture::new(&bytes);
    let mut reader = fixture.open();
    let mut buffer = vec![0u8; CHUNK * 2];
    let mut received = Vec::new();
    let mut calls = 0;
    loop {
        let count = reader.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        assert!(count <= CHUNK);
        calls += 1;
        received.extend_from_slice(&buffer[..count]);
    }
    assert_eq!(calls, 4);
    assert_eq!(received, bytes);
    reader.finish().unwrap();
    assert_eq!(std::fs::read(&fixture.path).unwrap(), bytes);
    assert!(
        fixture.open().finish().is_err(),
        "drop/partial read is not completion"
    );
}

#[test]
fn checked_open_accepts_empty_input() {
    let empty = Fixture::new(b"");
    let mut reader = empty.open();
    assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
    reader.finish().unwrap();
}

#[test]
fn checked_prefix_reads_only_its_range_and_finish_requires_that_range() {
    use std::io::Seek;
    let content = b"head\nunread suffix\n";
    let fixture = Fixture::new(content);
    for end in [0, 3, 5, content.len() as u64] {
        let mut reader =
            CheckedNative::open_prefix(&fixture.root, &fixture.path, &fixture.stamp, end).unwrap();
        let mut output = [0u8; 128];
        let count = reader.read(&mut output).unwrap();
        assert_eq!(count as u64, end);
        assert_eq!(&output[..count], &content[..end as usize]);
        assert_eq!(reader.read(&mut output).unwrap(), 0);
        // Position inspection is test-only; CheckedNative implements no Seek.
        assert_eq!(reader.file.stream_position().unwrap(), end);
        reader.finish().unwrap();
    }
    let mut incomplete =
        CheckedNative::open_prefix(&fixture.root, &fixture.path, &fixture.stamp, 5).unwrap();
    incomplete.read_exact(&mut [0; 2]).unwrap();
    assert!(incomplete.finish().is_err());
    assert_eq!(
        CheckedNative::open_prefix(
            &fixture.root,
            &fixture.path,
            &fixture.stamp,
            content.len() as u64 + 1
        )
        .err()
        .unwrap()
        .status,
        409
    );
    assert_eq!(std::fs::read(&fixture.path).unwrap(), content);
}

#[test]
fn prefix_verification_still_covers_current_identity_of_the_unread_suffix() {
    for (end, rewrite) in [(0, false), (5, false), (0, true), (5, true)] {
        let fixture = Fixture::new(b"head\nold suffix\n");
        let mut reader =
            CheckedNative::open_prefix(&fixture.root, &fixture.path, &fixture.stamp, end).unwrap();
        if end > 0 {
            reader.read_exact(&mut [0; 5]).unwrap();
        }
        if rewrite {
            // The authorized bytes remain identical, but the expected stamp
            // still covers a same-length change in the unread remainder.
            std::fs::write(&fixture.path, b"head\nnew suffix\n").unwrap();
        } else {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&fixture.path)
                .unwrap()
                .write_all(b"later\n")
                .unwrap();
        }
        advance_modified(&fixture);
        assert!(reader.verify().is_err());
        assert!(reader.finish().is_err());
        assert_eq!(
            CheckedNative::open_prefix(&fixture.root, &fixture.path, &fixture.stamp, end)
                .err()
                .unwrap()
                .status,
            503
        );
    }
}

#[cfg(unix)]
#[test]
fn prefix_rejects_replacement_even_after_all_authorized_bytes_were_consumed() {
    let fixture = Fixture::new(b"head\nold suffix\n");
    let mut reader =
        CheckedNative::open_prefix(&fixture.root, &fixture.path, &fixture.stamp, 5).unwrap();
    reader.read_exact(&mut [0; 5]).unwrap();
    let replacement = fixture.root.join("replacement");
    std::fs::write(&replacement, b"head\nnew suffix\n").unwrap();
    std::fs::rename(replacement, &fixture.path).unwrap();
    assert!(reader.finish().is_err());
}

#[test]
fn stale_stamp_rejects_changed_or_replaced_file_before_open() {
    for replacement in [false, true] {
        let fixture = Fixture::new(b"old\n");
        if replacement {
            let next = fixture.root.join("replacement");
            std::fs::write(&next, b"new\n").unwrap();
            std::fs::rename(next, &fixture.path).unwrap();
        } else {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&fixture.path)
                .unwrap()
                .write_all(b"appended\n")
                .unwrap();
        }
        let error = CheckedNative::open(&fixture.root, &fixture.path, &fixture.stamp)
            .err()
            .unwrap();
        assert_eq!(error.status, 503);
        assert!(!error.message.contains(fixture.root.to_str().unwrap()));
    }
}

#[test]
fn modification_during_read_cannot_pass_finish_or_eof_verification() {
    for change in ["append", "rewrite", "truncate"] {
        let fixture = Fixture::new(b"old initial complete content\n");
        let mut reader = fixture.open();
        reader.read_exact(&mut [0; 4]).unwrap();
        match change {
            "append" => std::fs::OpenOptions::new()
                .append(true)
                .open(&fixture.path)
                .unwrap()
                .write_all(b"new tail\n")
                .unwrap(),
            "rewrite" => std::fs::write(&fixture.path, b"NEW initial complete content\n").unwrap(),
            _ => std::fs::write(&fixture.path, b"x").unwrap(),
        }
        advance_modified(&fixture);
        assert!(reader.verify().is_err(), "{change}");
        let mut buffer = [0; CHUNK];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => panic!("{change}: changed input falsely reached successful EOF"),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert!(
            reader.read(&mut buffer).is_err(),
            "read failure must be sticky"
        );
        assert!(reader.finish().is_err());
    }
}

#[cfg(unix)]
#[test]
fn leaf_or_parent_replacement_is_rejected_even_with_retained_open_handle() {
    let fixture = Fixture::new(b"old bytes\n");
    let mut reader = fixture.open();
    reader.read_exact(&mut [0; 2]).unwrap();
    let replacement = fixture.root.join("replacement");
    std::fs::write(&replacement, b"new bytes\n").unwrap();
    std::fs::rename(replacement, &fixture.path).unwrap();
    assert!(reader.verify().is_err());
    assert!(reader.finish().is_err());

    let fixture = Fixture::new(b"retained leaf\n");
    let reader = fixture.open();
    let moved = fixture.root.join("moved");
    std::fs::rename(fixture.root.join("native"), &moved).unwrap();
    std::fs::create_dir(fixture.root.join("native")).unwrap();
    std::fs::rename(moved.join("synthetic.jsonl"), &fixture.path).unwrap();
    // Even moving the same leaf inode into a new parent cannot authorize it.
    assert_eq!(
        stamp(&fixture.path).unwrap().file_identity,
        fixture.stamp.file_identity
    );
    assert!(reader.verify().is_err());
}

#[cfg(unix)]
#[test]
fn links_follow_python_open_semantics_but_special_and_unindexed_paths_fail() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    let fixture = Fixture::new(b"private native bytes\n");
    for path in [
        fixture.root.join("leaf-link"),
        fixture.root.join("parent-link/synthetic.jsonl"),
    ] {
        if path.file_name().unwrap() == "leaf-link" {
            symlink(&fixture.path, &path).unwrap();
        } else {
            symlink(
                fixture.root.join("native"),
                fixture.root.join("parent-link"),
            )
            .unwrap();
        }
        let mut reader = CheckedNative::open(&fixture.root, &path, &fixture.stamp).unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        reader.finish().unwrap();
        assert_eq!(bytes, b"private native bytes\n");
    }
    let hard = fixture.root.join("hard");
    std::fs::hard_link(&fixture.path, &hard).unwrap();
    let updated = stamp(&fixture.path).unwrap();
    let mut reader = CheckedNative::open(&fixture.root, &fixture.path, &updated).unwrap();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    reader.finish().unwrap();
    assert_eq!(bytes, b"private native bytes\n");
    let socket = fixture.root.join("socket");
    let _listener = UnixListener::bind(&socket).unwrap();
    assert!(CheckedNative::open(&fixture.root, &socket, &fixture.stamp).is_err());
    let other = Fixture::new(b"outside\n");
    assert_eq!(
        CheckedNative::open(&fixture.root, &other.path, &other.stamp)
            .err()
            .unwrap()
            .status,
        403
    );
}

#[test]
fn raw_index_preserves_empty_blank_crlf_utf8_and_partial_physical_offsets() {
    for bytes in [
        b"".as_slice(),
        b"no final LF",
        b"\n\n \t\r\npartial",
        "第一行\n第二行\n半".as_bytes(),
        b"\xff\xfe\n\x80",
    ] {
        assert_index(&scan(bytes), bytes);
    }
    let first = scan(b"complete\npart");
    let finished = scan(b"complete\npartial\n");
    assert_eq!(first.committed(), 9);
    assert_eq!(first.prefix_hash(9), finished.prefix_hash(9));
    assert_ne!(first.digest(), finished.digest());
    assert_eq!(finished.committed(), 17);
}

struct SmallReads<'a> {
    bytes: &'a [u8],
    limit: usize,
}
impl Read for SmallReads<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.bytes.len().min(self.limit).min(output.len());
        output[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}
#[test]
fn arbitrary_chunk_boundaries_produce_identical_full_prefix_hashes() {
    let mut bytes = vec![b'x'; CHUNK + 8];
    for end in [1, HEAD - 1, HEAD, HEAD + 1, CHUNK - 1, CHUNK, CHUNK + 1] {
        bytes[end - 1] = b'\n';
    }
    for limit in [1, 3, 4095, CHUNK - 1, CHUNK] {
        let index = RawIndex::scan(SmallReads {
            bytes: &bytes,
            limit,
        })
        .unwrap();
        assert_index(&index, &bytes);
    }
}

#[test]
fn same_length_rewrite_after_head_changes_full_committed_prefix() {
    let mut bytes = vec![b'a'; 8192];
    bytes[8191] = b'\n';
    let first = scan(&bytes);
    bytes[6000] = b'b';
    let rewritten = scan(&bytes);
    assert_eq!(first.length(), rewritten.length());
    assert_eq!(first.head_bytes(), rewritten.head_bytes());
    assert_ne!(first.prefix_hash(8192), rewritten.prefix_hash(8192));
    assert_ne!(first.digest(), rewritten.digest());
}

#[test]
fn sha256_probe_accepts_only_zero_and_complete_lf_boundaries() {
    for bytes in [
        b"".as_slice(),
        b"unfinished",
        "\n空白\r\n\npartial".as_bytes(),
    ] {
        let committed = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        for end in (0..=bytes.len() as u64 + 1).chain([u64::MAX]) {
            let index = RawIndex::scan_with_probe(bytes, Some(end)).unwrap();
            assert_index(&index, bytes);
            let expected: [u8; 32] = Sha256::digest(&bytes[..committed]).into();
            assert_eq!(index.committed_digest(), expected);
            if end == 0 || (end <= committed as u64 && bytes[end as usize - 1] == b'\n') {
                assert_eq!(
                    index.probe_digest(),
                    Some(Sha256::digest(&bytes[..end as usize]).into())
                );
            } else {
                assert_eq!(index.probe_digest(), None);
            }
        }
        let ordinary = scan(bytes);
        assert_eq!(ordinary.probe_digest(), None);
        assert_eq!(
            ordinary.committed_digest(),
            Sha256::digest(&bytes[..committed]).as_slice()
        );
    }
}

#[test]
fn sha256_probe_and_committed_hash_cross_chunks_without_including_partial_tail() {
    let mut bytes = vec![b'x'; 2 * CHUNK + 99];
    let boundaries = [1, CHUNK - 1, CHUNK, CHUNK + 1, 2 * CHUNK + 2];
    for end in boundaries {
        bytes[end - 1] = b'\n';
    }
    for end in boundaries {
        for limit in [7, CHUNK - 1, CHUNK] {
            let reader = SmallReads {
                bytes: &bytes,
                limit,
            };
            let index = RawIndex::scan_with_probe(reader, Some(end as u64)).unwrap();
            assert_eq!(
                index.probe_digest(),
                Some(Sha256::digest(&bytes[..end]).into())
            );
            let expected: [u8; 32] = Sha256::digest(&bytes[..2 * CHUNK + 2]).into();
            assert_eq!(index.committed_digest(), expected);
            assert_eq!(index.prefix_hash(end as u64), Some(hash(&bytes[..end])));
            assert_eq!(index.digest(), hash(&bytes));
        }
    }
}

#[test]
fn strong_old_prefix_probe_distinguishes_append_rewrite_and_truncate() {
    let mut old = vec![b'a'; 8192];
    old[8191] = b'\n';
    old.extend_from_slice(b"partial");
    let previous = scan(&old);
    let mut appended = old.clone();
    appended.extend_from_slice(b" completed\nnew line\n");
    let current =
        RawIndex::scan_with_probe(appended.as_slice(), Some(previous.committed())).unwrap();
    assert_eq!(current.probe_digest(), Some(previous.committed_digest()));
    assert_ne!(current.committed_digest(), previous.committed_digest());
    appended[6000] = b'b';
    let rewritten =
        RawIndex::scan_with_probe(appended.as_slice(), Some(previous.committed())).unwrap();
    assert_eq!(rewritten.head_bytes(), previous.head_bytes());
    assert_ne!(rewritten.probe_digest(), Some(previous.committed_digest()));
    let truncated =
        RawIndex::scan_with_probe(&appended[..4000], Some(previous.committed())).unwrap();
    assert_eq!(truncated.probe_digest(), None);
}

#[test]
fn checked_prefix_and_one_pass_probe_hash_the_same_real_file_boundary() {
    let fixture = Fixture::new(b"one\n\nthree\npartial");
    let mut full = fixture.open();
    let indexed = RawIndex::scan_with_probe(&mut full, Some(5)).unwrap();
    full.finish().unwrap();
    let mut prefix =
        CheckedNative::open_prefix(&fixture.root, &fixture.path, &fixture.stamp, 5).unwrap();
    let prefix_index = RawIndex::scan(&mut prefix).unwrap();
    prefix.finish().unwrap();
    assert_eq!(
        indexed.probe_digest(),
        Some(prefix_index.committed_digest())
    );
    assert_eq!(indexed.prefix_hash(5), prefix_index.prefix_hash(5));
}

#[test]
fn large_generated_stream_never_requests_or_retains_a_full_raw_body() {
    struct Generated {
        remaining: u64,
        calls: usize,
    }
    impl Read for Generated {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            assert!(
                output.len() <= CHUNK,
                "scanner asked for an unbounded input buffer"
            );
            self.calls += 1;
            let count = self.remaining.min(output.len() as u64) as usize;
            output[..count].fill(b'x');
            self.remaining -= count as u64;
            Ok(count)
        }
    }
    let mut generated = Generated {
        remaining: 48 * 1024 * 1024,
        calls: 0,
    };
    let index = RawIndex::scan(&mut generated).unwrap();
    assert_eq!(index.length(), 48 * 1024 * 1024);
    assert_eq!(index.committed(), 0);
    assert_eq!(index.head.len(), HEAD);
    assert!(index.checkpoints.is_empty());
    assert!(generated.calls > 700);

    // The same nonresident scan also runs against a real checked sparse file.
    let mut fixture = Fixture::new(b"");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&fixture.path)
        .unwrap()
        .set_len(48 * 1024 * 1024)
        .unwrap();
    fixture.stamp = stamp(&fixture.path).unwrap();
    let mut reader = fixture.open();
    let index = RawIndex::scan(&mut reader).unwrap();
    reader.finish().unwrap();
    assert_eq!(index.length(), 48 * 1024 * 1024);
    assert_eq!(index.committed(), 0);
    assert!(index.checkpoints.is_empty());
    assert_eq!(index.head_bytes(), &[0; HEAD]);
}

#[test]
fn checked_file_scan_and_generic_read_errors_do_not_publish_partial_indices() {
    let fixture = Fixture::new(b"first\n\nlast-partial");
    let mut reader = fixture.open();
    let index = RawIndex::scan(&mut reader).unwrap();
    reader.finish().unwrap();
    assert_index(&index, b"first\n\nlast-partial");
    struct Broken {
        prefix: Cursor<&'static [u8]>,
    }
    impl Read for Broken {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            let count = self.prefix.read(output)?;
            if count == 0 {
                Err(io::Error::other("PRIVATE_READ_DETAILS"))
            } else {
                Ok(count)
            }
        }
    }
    let error = RawIndex::scan(Broken {
        prefix: Cursor::new(b"valid\n"),
    })
    .err()
    .unwrap();
    assert_eq!(error.status, 503);
    assert!(!error.message.contains("PRIVATE_READ_DETAILS"));
}
