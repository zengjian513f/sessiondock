//! 录制目录的只追加写入器。
//!
//! 一个 [`Store`] 对应一个录制目录：分段文件由本进程独占写入，读者可并发打开同一目录，
//! 并容忍末尾尚未写完的半帧。本模块不缓冲（每次追加一次 `write_all`）、不启线程。
//!
//! 现有分段在 [`Store::open`] 之后一律视为已关闭；必须先 [`Store::begin_segment`]
//! 才能 [`Store::append`]。新分段的第一帧是 [`Frame::Checkpoint`]，因此按
//! [`StoreConfig::total_bytes`] 淘汰最旧分段后，剩下最旧的分段仍能独立重建画面。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::format::{encode_frame, encoded_len, parse_segment_header, write_segment_header};
use crate::{
    FRAME_HEADER_LEN, FRAME_TRAILER_LEN, Frame, MAX_PAYLOAD, Position, SEGMENT_HEADER_LEN,
    SegmentInfo, parse_segment_file_name, segment_file_name,
};

/// 写入器的分段大小与目录总容量上限。
///
/// 低于 4096 的值在 [`Store::open`] 时被抬到 4096；本结构本身不做钳制。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreConfig {
    /// 单个分段文件（含 16 字节段头）达到该长度后，[`Store::needs_segment`] 为真。
    pub segment_bytes: u64,
    /// 目录内所有分段 `len` 之和的目标上限；超出且至少还有两段时淘汰最旧段。
    pub total_bytes: u64,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            segment_bytes: 8 << 20,
            total_bytes: 256 << 20,
        }
    }
}

/// 一个录制目录的只追加写入器。
///
/// 字段私有。打开后没有任何分段处于可追加状态；[`Store::position`] 为 `None`，
/// [`Store::needs_segment`] 为 `true`。
pub struct Store {
    dir: PathBuf,
    config: StoreConfig,
    /// 按 `index` 升序。打开着的分段（若有）一定是最后一个。
    segments: Vec<SegmentInfo>,
    /// 扫描或创建过的最高段序号（含头解析失败、内容过短但仍名为 `NNNNNNNN.seg` 的文件）。
    /// 空目录为 0，因此下一段序号永远是 `max_index + 1`（空目录从 1 起）。
    max_index: u32,
    open: Option<File>,
    dirty: bool,
}

impl Store {
    /// 打开（或创建）`dir` 作为录制目录。
    ///
    /// 会 `create_dir_all`；在 unix 上把目录权限设为 `0o700`（chmod 失败忽略）。
    /// 扫描能被 [`parse_segment_file_name`] 识别的文件，读取 16 字节段头并用
    /// [`parse_segment_header`] 构造 [`SegmentInfo`]。头解析失败或短于段头的文件
    /// 跳过且不删除，但它们的序号仍计入下一段编号。
    ///
    /// 已有分段一律视为已关闭：返回的 store 没有打开着的写入文件。
    /// `segment_bytes` 与 `total_bytes` 低于 4096 时抬到 4096。
    pub fn open(dir: &Path, config: StoreConfig) -> io::Result<Store> {
        let config = StoreConfig {
            segment_bytes: config.segment_bytes.max(4096),
            total_bytes: config.total_bytes.max(4096),
        };

        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }

        let mut segments = Vec::new();
        let mut max_index = 0u32;

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(index) = parse_segment_file_name(name) else {
                continue;
            };
            max_index = max_index.max(index);

            let path = entry.path();
            let metadata = fs::metadata(&path)?;
            if metadata.len() < SEGMENT_HEADER_LEN as u64 {
                continue;
            }

            let mut file = File::open(&path)?;
            let mut header_bytes = [0u8; SEGMENT_HEADER_LEN];
            match file.read_exact(&mut header_bytes) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => continue,
                Err(err) => return Err(err),
            }

            let Ok(header) = parse_segment_header(&header_bytes) else {
                continue;
            };

            segments.push(SegmentInfo {
                index,
                path,
                len: metadata.len(),
                header,
            });
        }

        segments.sort_by_key(|segment| segment.index);

        Ok(Store {
            dir: dir.to_path_buf(),
            config,
            segments,
            max_index,
            open: None,
            dirty: false,
        })
    }

    /// 本写入器对应的录制目录。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 打开时钳制后的配置。
    pub fn config(&self) -> StoreConfig {
        self.config
    }

    /// 当前没有打开的分段，或打开分段的文件总长（含段头）已达到 `segment_bytes`。
    pub fn needs_segment(&self) -> bool {
        match self.open_info() {
            None => true,
            Some(info) => info.len >= self.config.segment_bytes,
        }
    }

    /// 关闭当前打开的分段（若有），创建下一段并写入段头与一帧 checkpoint。
    ///
    /// 新序号为已见最高序号加一；目录为空时为 1。文件以 `create_new` 打开，unix 上
    /// 模式 `0o600`。checkpoint 的 `at_ms` 为 0。随后按 `total_bytes` 淘汰最旧分段
    /// （打开着的新段不会被删）。
    ///
    /// `state.len() > MAX_PAYLOAD - 4` 时返回 [`io::ErrorKind::InvalidInput`]，不写盘。
    /// 返回值是 checkpoint 帧的位置（`offset == 0`）；[`Self::position`] 则指向该帧之后。
    pub fn begin_segment(
        &mut self,
        now_unix_ms: u64,
        cols: u16,
        rows: u16,
        state: Vec<u8>,
    ) -> io::Result<Position> {
        if state.len() > MAX_PAYLOAD - 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "checkpoint state too large: {} (max {})",
                    state.len(),
                    MAX_PAYLOAD - 4
                ),
            ));
        }

        self.close()?;

        let index = self.max_index + 1;
        let path = self.dir.join(segment_file_name(index));

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        self.max_index = index;

        let header_bytes = write_segment_header(now_unix_ms);
        file.write_all(&header_bytes)?;

        let checkpoint = Frame::Checkpoint { cols, rows, state };
        let mut frame_bytes = Vec::with_capacity(encoded_len(&checkpoint));
        encode_frame(0, &checkpoint, &mut frame_bytes);
        file.write_all(&frame_bytes)?;

        let len = SEGMENT_HEADER_LEN as u64 + frame_bytes.len() as u64;
        let header = parse_segment_header(&header_bytes).map_err(|err| {
            io::Error::other(format!("newly written segment header did not parse: {err}"))
        })?;

        self.segments.push(SegmentInfo {
            index,
            path,
            len,
            header,
        });
        self.open = Some(file);
        self.dirty = true;

        self.evict_while_over_quota()?;

        Ok(Position {
            segment: index,
            offset: 0,
        })
    }

    /// 把一帧追加到当前打开的分段。
    ///
    /// 没有打开分段时返回 kind `Other`、消息 `"no open segment"`。
    /// `now_unix_ms - base_unix_ms` 超过 `u32::MAX` 时返回 `"segment clock overflow"`，不写盘。
    /// 载荷超过 [`MAX_PAYLOAD`] 时返回 [`io::ErrorKind::InvalidInput`]，不写盘。
    ///
    /// `at_ms` 为相对段 `base_unix_ms` 的毫秒（下限钳到 0）。编码后一次 `write_all`。
    /// 返回该帧起始位置（写之前的段内偏移）。允许在段中追加 [`Frame::Checkpoint`]。
    pub fn append(&mut self, now_unix_ms: u64, frame: &Frame) -> io::Result<Position> {
        let (file, info) = match (self.open.as_mut(), self.segments.last_mut()) {
            (Some(file), Some(info)) => (file, info),
            _ => {
                return Err(io::Error::other("no open segment"));
            }
        };

        let delta = now_unix_ms.saturating_sub(info.header.base_unix_ms);
        if delta > u64::from(u32::MAX) {
            return Err(io::Error::other("segment clock overflow"));
        }
        let at_ms = delta as u32;

        let total_encoded = encoded_len(frame);
        let payload_len = total_encoded.saturating_sub(FRAME_HEADER_LEN + FRAME_TRAILER_LEN);
        if payload_len > MAX_PAYLOAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("frame payload too large: {payload_len}"),
            ));
        }

        let offset = info.len.saturating_sub(SEGMENT_HEADER_LEN as u64);
        let position = Position {
            segment: info.index,
            offset,
        };

        let mut bytes = Vec::with_capacity(total_encoded);
        encode_frame(at_ms, frame, &mut bytes);
        file.write_all(&bytes)?;
        info.len += bytes.len() as u64;
        self.dirty = true;
        Ok(position)
    }

    /// 若自上次同步以来有写入，对打开着的文件调用 `sync_data` 并清除 dirty。
    /// 否则为空操作。
    pub fn sync(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(file) = self.open.as_mut() {
            file.sync_data()?;
        }
        self.dirty = false;
        Ok(())
    }

    /// `sync` 之后丢掉打开着的文件。之后 [`Self::position`] 为 `None`。可重复调用。
    pub fn close(&mut self) -> io::Result<()> {
        self.sync()?;
        self.open = None;
        Ok(())
    }

    /// 下一次 [`Self::append`] 将写入的位置；没有打开分段时为 `None`。
    ///
    /// `offset` 从段头之后起算。
    pub fn position(&self) -> Option<Position> {
        let info = self.open_info()?;
        Some(Position {
            segment: info.index,
            offset: info.len.saturating_sub(SEGMENT_HEADER_LEN as u64),
        })
    }

    /// 当前已知分段，按序号升序。打开分段的 `len` 在每次追加后保持最新。
    pub fn segments(&self) -> &[SegmentInfo] {
        &self.segments
    }

    /// 所有分段 `len` 之和（含段头）。
    pub fn total_bytes(&self) -> u64 {
        self.segments.iter().map(|segment| segment.len).sum()
    }

    /// 打开着的分段的 `base_unix_ms`。
    pub fn open_segment_base_ms(&self) -> Option<u64> {
        Some(self.open_info()?.header.base_unix_ms)
    }

    fn open_info(&self) -> Option<&SegmentInfo> {
        self.open.as_ref()?;
        self.segments.last()
    }

    /// 总和超过 `total_bytes` 且至少两段时，删除最旧分段文件并从列表去掉。
    /// 打开着的分段是列表最后一项，因此不会被删。
    fn evict_while_over_quota(&mut self) -> io::Result<()> {
        while self.segments.len() >= 2 && self.total_bytes() > self.config.total_bytes {
            let path = self.segments[0].path.clone();
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
            self.segments.remove(0);
        }
        Ok(())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{FrameIter, parse_segment_header};
    use crate::{Timed, VERSION};

    fn default_open() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path(), StoreConfig::default()).expect("open empty store");
        (dir, store)
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    fn collect_frames(path: &Path) -> (crate::SegmentHeader, Vec<Timed>) {
        let bytes = fs::read(path).expect("read segment");
        let header = parse_segment_header(&bytes).expect("parse header");
        let body = &bytes[SEGMENT_HEADER_LEN..];
        let frames: Vec<Timed> = FrameIter::new(body).collect();
        (header, frames)
    }

    #[test]
    fn open_empty_dir_needs_segment_and_has_no_position() {
        let (_dir, store) = default_open();
        assert!(store.needs_segment());
        assert_eq!(store.position(), None);
        assert!(store.segments().is_empty());
        assert_eq!(store.total_bytes(), 0);
        assert_eq!(store.open_segment_base_ms(), None);
        assert_eq!(
            store.config(),
            StoreConfig {
                segment_bytes: 8 << 20,
                total_bytes: 256 << 20,
            }
        );
    }

    #[test]
    fn open_clamps_config_below_4096() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(
            dir.path(),
            StoreConfig {
                segment_bytes: 1,
                total_bytes: 100,
            },
        )
        .expect("open");
        assert_eq!(store.config().segment_bytes, 4096);
        assert_eq!(store.config().total_bytes, 4096);
    }

    #[test]
    fn begin_segment_writes_header_checkpoint_name_and_modes() {
        let (dir, mut store) = default_open();
        let state = b"screen".to_vec();
        let checkpoint = Frame::Checkpoint {
            cols: 80,
            rows: 24,
            state: state.clone(),
        };

        let begun = store
            .begin_segment(1_700_000_000_000, 80, 24, state)
            .expect("begin_segment");
        assert_eq!(
            begun,
            Position {
                segment: 1,
                offset: 0,
            }
        );
        assert_eq!(
            store.position(),
            Some(Position {
                segment: 1,
                offset: encoded_len(&checkpoint) as u64,
            })
        );
        assert_eq!(store.open_segment_base_ms(), Some(1_700_000_000_000));
        assert!(!store.needs_segment());

        assert_eq!(store.segments().len(), 1);
        let info = &store.segments()[0];
        assert_eq!(info.index, 1);
        assert_eq!(
            info.path.file_name().and_then(|n| n.to_str()),
            Some("00000001.seg")
        );
        assert_eq!(info.header.version, VERSION);
        assert_eq!(info.header.flags, 0);
        assert_eq!(info.header.base_unix_ms, 1_700_000_000_000);
        assert_eq!(
            info.len,
            SEGMENT_HEADER_LEN as u64 + encoded_len(&checkpoint) as u64
        );

        let file_path = dir.path().join("00000001.seg");
        assert!(file_path.is_file());
        assert_eq!(fs::metadata(&file_path).expect("meta").len(), info.len);

        #[cfg(unix)]
        {
            assert_eq!(mode_of(&file_path), 0o600);
            assert_eq!(mode_of(dir.path()), 0o700);
        }
    }

    #[test]
    fn append_kinds_increase_offsets_and_len_matches_file() {
        let (dir, mut store) = default_open();
        let base = 1_000_000u64;
        store
            .begin_segment(base, 80, 24, b"ck".to_vec())
            .expect("begin");
        let after_checkpoint = store.position().expect("open position");

        let p_output = store
            .append(base + 10, &Frame::Output(b"hello".to_vec()))
            .expect("output");
        let p_resize = store
            .append(
                base + 20,
                &Frame::Resize {
                    cols: 100,
                    rows: 30,
                },
            )
            .expect("resize");
        let p_mark = store
            .append(base + 30, &Frame::Mark("{\"event\":\"attach\"}".to_owned()))
            .expect("mark");
        let p_exit = store
            .append(base + 40, &Frame::Exit("{\"code\":0}".to_owned()))
            .expect("exit");

        assert_eq!(p_output, after_checkpoint);
        assert!(p_output.offset < p_resize.offset);
        assert!(p_resize.offset < p_mark.offset);
        assert!(p_mark.offset < p_exit.offset);
        assert_eq!(p_output.segment, 1);
        assert_eq!(p_exit.segment, 1);

        let path = dir.path().join("00000001.seg");
        let file_len = fs::metadata(&path).expect("meta").len();
        assert_eq!(store.segments()[0].len, file_len);
        assert_eq!(store.total_bytes(), file_len);
        assert_eq!(
            store.position(),
            Some(Position {
                segment: 1,
                offset: file_len - SEGMENT_HEADER_LEN as u64,
            })
        );
    }

    #[test]
    fn written_file_decodes_with_header_and_frame_iter() {
        let (dir, mut store) = default_open();
        let base = 1_700_000_000_123u64;
        let checkpoint_state = b"screen-state".to_vec();
        store
            .begin_segment(base, 80, 24, checkpoint_state.clone())
            .expect("begin");
        store
            .append(base + 10, &Frame::Output(b"hello".to_vec()))
            .expect("output");
        store
            .append(
                base + 20,
                &Frame::Resize {
                    cols: 100,
                    rows: 30,
                },
            )
            .expect("resize");
        store
            .append(base + 30, &Frame::Mark("mark".to_owned()))
            .expect("mark");
        store
            .append(base + 40, &Frame::Exit("exit".to_owned()))
            .expect("exit");
        store.close().expect("close");

        let path = dir.path().join("00000001.seg");
        let (header, frames) = collect_frames(&path);
        assert_eq!(header.version, VERSION);
        assert_eq!(header.flags, 0);
        assert_eq!(header.base_unix_ms, base);
        assert_eq!(
            frames,
            vec![
                Timed {
                    at_ms: 0,
                    frame: Frame::Checkpoint {
                        cols: 80,
                        rows: 24,
                        state: checkpoint_state,
                    },
                },
                Timed {
                    at_ms: 10,
                    frame: Frame::Output(b"hello".to_vec()),
                },
                Timed {
                    at_ms: 20,
                    frame: Frame::Resize {
                        cols: 100,
                        rows: 30
                    },
                },
                Timed {
                    at_ms: 30,
                    frame: Frame::Mark("mark".to_owned()),
                },
                Timed {
                    at_ms: 40,
                    frame: Frame::Exit("exit".to_owned()),
                },
            ]
        );
    }

    #[test]
    fn reopen_lists_existing_segment_and_next_begin_is_index_two() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = 42u64;
        {
            let mut store = Store::open(dir.path(), StoreConfig::default()).expect("open");
            store
                .begin_segment(base, 40, 12, b"s".to_vec())
                .expect("begin");
            store
                .append(base + 5, &Frame::Output(b"abc".to_vec()))
                .expect("append");
            store.close().expect("close");
        }

        let mut store = Store::open(dir.path(), StoreConfig::default()).expect("reopen");
        assert!(store.needs_segment());
        assert_eq!(store.position(), None);
        assert_eq!(store.open_segment_base_ms(), None);
        assert_eq!(store.segments().len(), 1);

        let info = &store.segments()[0];
        assert_eq!(info.index, 1);
        assert_eq!(info.header.base_unix_ms, base);
        assert_eq!(info.header.version, VERSION);
        let path = dir.path().join("00000001.seg");
        assert_eq!(info.len, fs::metadata(&path).expect("meta").len());
        assert_eq!(info.path, path);

        let begun = store
            .begin_segment(base + 1000, 40, 12, Vec::new())
            .expect("second begin");
        assert_eq!(
            begun,
            Position {
                segment: 2,
                offset: 0,
            }
        );
        assert_eq!(store.segments().len(), 2);
        assert_eq!(store.segments()[1].index, 2);
        assert!(dir.path().join("00000002.seg").is_file());
    }

    #[test]
    fn append_before_begin_segment_is_no_open_segment() {
        let (_dir, mut store) = default_open();
        let err = store
            .append(0, &Frame::Output(b"x".to_vec()))
            .expect_err("append without begin");
        assert_eq!(err.kind(), io::ErrorKind::Other);
        assert_eq!(err.to_string(), "no open segment");
    }

    #[test]
    fn clock_overflow_does_not_change_file_length() {
        let (dir, mut store) = default_open();
        store
            .begin_segment(1000, 80, 24, Vec::new())
            .expect("begin");
        store
            .append(1000, &Frame::Output(b"keep".to_vec()))
            .expect("append before overflow");
        let len_before = store.segments()[0].len;
        let path = dir.path().join("00000001.seg");
        assert_eq!(fs::metadata(&path).expect("meta").len(), len_before);

        let overflow_now = 1000 + u64::from(u32::MAX) + 1;
        let err = store
            .append(overflow_now, &Frame::Output(b"too-late".to_vec()))
            .expect_err("clock overflow");
        assert_eq!(err.kind(), io::ErrorKind::Other);
        assert_eq!(err.to_string(), "segment clock overflow");
        assert_eq!(store.segments()[0].len, len_before);
        assert_eq!(fs::metadata(&path).expect("meta").len(), len_before);
    }

    #[test]
    fn rotation_evicts_oldest_keeps_newest_and_checkpoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(
            dir.path(),
            StoreConfig {
                segment_bytes: 4096,
                total_bytes: 12288,
            },
        )
        .expect("open");

        let mut created = Vec::new();
        let payload = vec![0u8; 1000];
        for i in 0u32..6 {
            let begun = store
                .begin_segment(1_000_000 + u64::from(i) * 1000, 80, 24, Vec::new())
                .expect("begin");
            created.push(begun.segment);
            while !store.needs_segment() {
                store
                    .append(
                        1_000_000 + u64::from(i) * 1000,
                        &Frame::Output(payload.clone()),
                    )
                    .expect("fill segment");
            }
        }

        assert!(
            store.segments().len() <= 3,
            "expected at most 3 segments, got {}",
            store.segments().len()
        );
        let newest = *created.last().expect("created");
        assert!(
            store
                .segments()
                .iter()
                .any(|segment| segment.index == newest),
            "newest segment {newest} should remain"
        );

        for index in &created {
            let path = dir.path().join(segment_file_name(*index));
            let still_listed = store
                .segments()
                .iter()
                .any(|segment| segment.index == *index);
            if still_listed {
                assert!(path.is_file(), "listed segment {index} missing on disk");
            } else {
                assert!(!path.exists(), "evicted segment {index} still on disk");
            }
        }

        let oldest = &store.segments()[0];
        let (_header, frames) = collect_frames(&oldest.path);
        assert!(
            matches!(
                frames.first().map(|t| &t.frame),
                Some(Frame::Checkpoint { .. })
            ),
            "oldest surviving segment must start with a Checkpoint"
        );
    }

    #[test]
    fn close_is_idempotent_and_drop_leaves_file_fully_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path;
        {
            let mut store = Store::open(dir.path(), StoreConfig::default()).expect("open");
            store
                .begin_segment(9, 8, 8, b"drop".to_vec())
                .expect("begin");
            store
                .append(19, &Frame::Output(b"payload".to_vec()))
                .expect("append");
            store.close().expect("close once");
            store.close().expect("close twice");
            assert_eq!(store.position(), None);
            assert!(store.needs_segment());
            path = store.dir().join("00000001.seg");
        }

        let (header, frames) = collect_frames(&path);
        assert_eq!(header.base_unix_ms, 9);
        assert_eq!(frames.len(), 2);
        assert!(matches!(frames[0].frame, Frame::Checkpoint { .. }));
        assert_eq!(frames[0].at_ms, 0);
        assert_eq!(frames[1].at_ms, 10);
        assert_eq!(frames[1].frame, Frame::Output(b"payload".to_vec()));
    }

    #[test]
    fn stray_and_malformed_files_are_ignored_but_name_counts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let notes = dir.path().join("notes.txt");
        let malformed = dir.path().join("00000009.seg");
        fs::write(&notes, b"hello").expect("notes");
        fs::write(&malformed, b"xxx").expect("malformed 3-byte segment");

        let mut store = Store::open(dir.path(), StoreConfig::default()).expect("open");
        assert!(store.segments().is_empty());
        assert!(notes.is_file());
        assert_eq!(fs::read(&malformed).expect("read malformed"), b"xxx");

        let begun = store
            .begin_segment(0, 1, 1, Vec::new())
            .expect("begin after stray");
        assert_eq!(begun.segment, 10);
        assert!(dir.path().join("00000010.seg").is_file());
        assert_eq!(fs::read(&malformed).expect("malformed remains"), b"xxx");
        assert_eq!(fs::read(&notes).expect("notes remain"), b"hello");
        assert!(!store.segments().iter().any(|segment| segment.index == 9));
    }

    #[test]
    fn begin_segment_rejects_oversized_checkpoint_state() {
        let (_dir, mut store) = default_open();
        let too_big = vec![0u8; MAX_PAYLOAD - 3];
        let err = store
            .begin_segment(0, 80, 24, too_big)
            .expect_err("oversized state");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(store.segments().is_empty());
        assert_eq!(store.position(), None);
    }

    #[test]
    fn mid_segment_checkpoint_is_allowed() {
        let (_dir, mut store) = default_open();
        store.begin_segment(0, 80, 24, Vec::new()).expect("begin");
        let pos = store
            .append(
                5,
                &Frame::Checkpoint {
                    cols: 40,
                    rows: 12,
                    state: b"mid".to_vec(),
                },
            )
            .expect("mid checkpoint");
        assert_eq!(pos.segment, 1);
        assert!(pos.offset > 0);
    }
}
