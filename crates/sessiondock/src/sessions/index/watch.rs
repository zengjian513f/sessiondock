//! Directory-level discovery cache backed by `inotify` on local Linux filesystems.
//!
//! Unchanged refreshes avoid walking thousands of session directories while
//! keeping identical discovery output, symlink/hardlink alias semantics, and
//! mutation visibility latency. Non-Linux targets, remote/FUSE filesystems,
//! watch exhaustion, and queue overflows fall back to ordinary directory walks.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cap_fs_ext::DirExt;
use cap_std::ambient_authority;
use cap_std::fs::Dir;

use super::{Discovered, SessionError, SessionRoots, Stamp, file_metadata, utf8_name};

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::ffi::CString;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    const WATCH_MASK: u32 = libc::IN_CREATE
        | libc::IN_DELETE
        | libc::IN_MODIFY
        | libc::IN_ATTRIB
        | libc::IN_CLOSE_WRITE
        | libc::IN_MOVED_FROM
        | libc::IN_MOVED_TO
        | libc::IN_DELETE_SELF
        | libc::IN_MOVE_SELF
        | libc::IN_ONLYDIR;

    pub(super) fn is_local_watchable_fs(path: &Path) -> bool {
        let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        let mut buf: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(cpath.as_ptr(), &mut buf) } != 0 {
            return false;
        }
        // Local kernel filesystems whose mutations pass synchronously through VFS fsnotify:
        // ext2/ext3/ext4, btrfs, tmpfs, xfs, ramfs, f2fs, overlayfs.
        matches!(
            buf.f_type as u64,
            0xef53
                | 0x9123_683e
                | 0x0102_1994
                | 0x5846_5342
                | 0x8584_58f6
                | 0xf2f5_2010
                | 0x794c_7630
        )
    }

    pub(super) struct Inotify {
        fd: OwnedFd,
        wd_to_path: HashMap<i32, PathBuf>,
        path_to_wd: HashMap<PathBuf, i32>,
    }

    pub(super) enum DrainOutcome {
        Dirty(Vec<PathBuf>),
        Overflow,
    }

    impl Inotify {
        pub(super) fn new() -> Option<Self> {
            let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
            if raw < 0 {
                return None;
            }
            Some(Self {
                fd: unsafe { OwnedFd::from_raw_fd(raw) },
                wd_to_path: HashMap::new(),
                path_to_wd: HashMap::new(),
            })
        }

        pub(super) fn watch(&mut self, path: &Path) -> bool {
            if self.path_to_wd.contains_key(path) {
                return true;
            }
            if !is_local_watchable_fs(path) {
                return false;
            }
            let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else {
                return false;
            };
            let wd =
                unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), cpath.as_ptr(), WATCH_MASK) };
            if wd < 0 {
                return false;
            }
            if let Some(old) = self.wd_to_path.insert(wd, path.to_path_buf())
                && old != path
            {
                self.path_to_wd.remove(&old);
            }
            self.path_to_wd.insert(path.to_path_buf(), wd);
            true
        }

        pub(super) fn unwatch_path(&mut self, path: &Path) {
            if let Some(wd) = self.path_to_wd.remove(path) {
                self.wd_to_path.remove(&wd);
                unsafe {
                    libc::inotify_rm_watch(self.fd.as_raw_fd(), wd);
                }
            }
        }

        pub(super) fn clear_watches(&mut self) {
            for &wd in self.wd_to_path.keys() {
                unsafe {
                    libc::inotify_rm_watch(self.fd.as_raw_fd(), wd);
                }
            }
            self.wd_to_path.clear();
            self.path_to_wd.clear();
        }

        pub(super) fn drain(&mut self) -> DrainOutcome {
            let mut dirty = Vec::new();
            let mut buf = [0u64; 1024]; // 8 KiB aligned buffer
            let byte_buf = unsafe {
                std::slice::from_raw_parts_mut(
                    buf.as_mut_ptr() as *mut u8,
                    std::mem::size_of_val(&buf),
                )
            };
            loop {
                let n = unsafe {
                    libc::read(
                        self.fd.as_raw_fd(),
                        byte_buf.as_mut_ptr() as *mut libc::c_void,
                        byte_buf.len(),
                    )
                };
                if n < 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if n == 0 {
                    break;
                }
                let n = n as usize;
                let mut offset = 0usize;
                while offset + std::mem::size_of::<libc::inotify_event>() <= n {
                    let event =
                        unsafe { &*(byte_buf.as_ptr().add(offset) as *const libc::inotify_event) };
                    let step = std::mem::size_of::<libc::inotify_event>() + event.len as usize;
                    offset += step;
                    if event.mask & libc::IN_Q_OVERFLOW != 0 {
                        return DrainOutcome::Overflow;
                    }
                    let path = if event.mask & libc::IN_IGNORED != 0 {
                        let removed = self.wd_to_path.remove(&event.wd);
                        if let Some(ref p) = removed {
                            self.path_to_wd.remove(p);
                        }
                        removed
                    } else {
                        self.wd_to_path.get(&event.wd).cloned()
                    };
                    if let Some(p) = path {
                        if event.mask
                            & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_IGNORED)
                            != 0
                            && let Some(parent) = p.parent()
                        {
                            dirty.push(parent.to_path_buf());
                        }
                        dirty.push(p);
                    }
                }
            }
            DrainOutcome::Dirty(dirty)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    ProjectRoot(&'static str),
    Project(&'static str),
    ProjectSession(&'static str),
    ProjectSubagents(&'static str),
    DroidRoot,
    DroidProject,
    CodexDir,
    GrokRoot,
    GrokProject,
    GrokSession,
    MirrorRoot(&'static str),
    MirrorProject(&'static str),
    MirrorSession(&'static str),
    KiroAgents,
    KiroAgent,
}

struct DirNode {
    path: PathBuf,
    kind: NodeKind,
    dirty: bool,
    has_dirty_child: bool,
    always_recheck: bool,
    has_recheck_child: bool,
    items: Vec<Discovered>,
    children: BTreeMap<String, DirNode>,
}

impl DirNode {
    fn new(path: PathBuf, kind: NodeKind, force_recheck: bool) -> Self {
        Self {
            path,
            kind,
            dirty: true,
            has_dirty_child: false,
            always_recheck: force_recheck,
            has_recheck_child: force_recheck,
            items: Vec::new(),
            children: BTreeMap::new(),
        }
    }

    fn needs_visit(&self) -> bool {
        self.dirty || self.has_dirty_child || self.always_recheck || self.has_recheck_child
    }

    fn mark_dirty_path(&mut self, target: &Path) -> bool {
        if target == self.path {
            self.dirty = true;
            return true;
        }
        let Ok(rel) = target.strip_prefix(&self.path) else {
            return false;
        };
        let Some(first) = rel.components().next() else {
            self.dirty = true;
            return true;
        };
        let Some(name) = first.as_os_str().to_str() else {
            self.dirty = true;
            return true;
        };
        if let Some(child) = self.children.get_mut(name) {
            if child.mark_dirty_path(target) {
                self.has_dirty_child = true;
                return true;
            }
        } else {
            // Event for a child not yet in tree (or just removed): rescan this parent.
            self.dirty = true;
            return true;
        }
        false
    }

    fn mark_all_dirty(&mut self) {
        self.dirty = true;
        for child in self.children.values_mut() {
            child.mark_all_dirty();
        }
        self.has_dirty_child = !self.children.is_empty();
    }

    fn collect_items(&self, out: &mut Vec<Discovered>) {
        out.extend(self.items.iter().cloned());
        for child in self.children.values() {
            child.collect_items(out);
        }
    }

    #[cfg(target_os = "linux")]
    fn drop_watches(&self, inotify: &mut Option<linux::Inotify>) {
        if let Some(ino) = inotify.as_mut() {
            ino.unwatch_path(&self.path);
        }
        for child in self.children.values() {
            child.drop_watches(inotify);
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn drop_watches(&self, _inotify: &mut Option<()>) {}
}

struct RootState {
    source: &'static str,
    root: PathBuf,
    missing: bool,
    node: Option<DirNode>,
}

pub(super) struct DiscoveryCache {
    #[cfg(target_os = "linux")]
    inotify: Option<linux::Inotify>,
    #[cfg(not(target_os = "linux"))]
    inotify: Option<()>,
    roots: Vec<RootState>,
    initialized: bool,
    published: Arc<[Discovered]>,
    generation: u64,
}

impl Default for DiscoveryCache {
    fn default() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            inotify: linux::Inotify::new(),
            #[cfg(not(target_os = "linux"))]
            inotify: None,
            roots: Vec::new(),
            initialized: false,
            published: Arc::from([]),
            generation: 0,
        }
    }
}

fn has_alias_risk(path: &Path) -> bool {
    let Ok(sym) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if sym.file_type().is_symlink() {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if sym.is_file() && sym.nlink() > 1 {
            return true;
        }
    }
    false
}

/// Lists directory children. Returns `(sorted_ (name, is_symlink) pairs, saw_any_symlink)`.
fn list_subdirs(dir: &Dir, path: &Path, follow_symlinks: bool) -> (Vec<(String, bool)>, bool) {
    let mut names = Vec::new();
    let mut saw_symlink = false;
    let Ok(entries) = dir.entries() else {
        return (names, true);
    };
    for entry in entries {
        let Ok(entry) = entry else {
            saw_symlink = true;
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            saw_symlink = true;
            continue;
        };
        let Some(name) = utf8_name(&entry) else {
            continue;
        };
        if kind.is_dir() {
            names.push((name, false));
        } else if kind.is_symlink() {
            saw_symlink = true;
            if follow_symlinks
                && std::fs::metadata(path.join(&name)).is_ok_and(|meta| meta.is_dir())
            {
                names.push((name, true));
            }
        }
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    (names, saw_symlink)
}

/// Lists regular/symlinked files with their stamps, plus whether any symlink or hardlink (`nlink > 1`) exists.
fn list_files(dir: &Dir, path: &Path) -> (Vec<(String, Stamp)>, bool) {
    let mut found = Vec::new();
    let mut alias_risk = false;
    let Ok(entries) = dir.entries() else {
        return (found, true);
    };
    for entry in entries {
        let Ok(entry) = entry else {
            alias_risk = true;
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            alias_risk = true;
            continue;
        };
        if kind.is_symlink() {
            alias_risk = true;
        } else if !kind.is_file() {
            continue;
        }
        let Some(name) = utf8_name(&entry) else {
            continue;
        };
        let full = path.join(&name);
        let Ok(meta) = file_metadata(&full) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        #[cfg(unix)]
        if !alias_risk {
            use cap_fs_ext::MetadataExt;
            if meta.nlink() > 1 {
                alias_risk = true;
            }
        }
        found.push((name, Stamp::of(&meta)));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    (found, alias_risk)
}

fn root_kind(source: &'static str) -> NodeKind {
    match source {
        "claude" | "codebuddy" => NodeKind::ProjectRoot(source),
        "droid" => NodeKind::DroidRoot,
        "codex" => NodeKind::CodexDir,
        "grok" => NodeKind::GrokRoot,
        "opencode" | "agy" => NodeKind::MirrorRoot(source),
        "kiro" => NodeKind::MirrorRoot("kiro"),
        _ => NodeKind::GrokRoot,
    }
}

#[cfg(target_os = "linux")]
fn ensure_watch(inotify: &mut Option<linux::Inotify>, path: &Path, force_recheck: bool) -> bool {
    if force_recheck {
        return false;
    }
    inotify.as_mut().is_some_and(|ino| ino.watch(path))
}

#[cfg(not(target_os = "linux"))]
fn ensure_watch(_inotify: &mut Option<()>, _path: &Path, _force_recheck: bool) -> bool {
    false
}

impl DiscoveryCache {
    pub(super) fn discover(
        &mut self,
        configured: &SessionRoots,
    ) -> Result<(Arc<[Discovered]>, u64), SessionError> {
        #[cfg(not(target_os = "linux"))]
        {
            let found = super::discover_walk(configured)?;
            let changed = !self.initialized || !same_discovered(&self.published, &found);
            if changed {
                self.published = found.into();
                self.generation = self.generation.wrapping_add(1);
                self.initialized = true;
            }
            return Ok((self.published.clone(), self.generation));
        }

        #[cfg(target_os = "linux")]
        {
            if self.inotify.is_none() {
                let found = super::discover_walk(configured)?;
                let changed = !self.initialized || !same_discovered(&self.published, &found);
                if changed {
                    self.published = found.into();
                    self.generation = self.generation.wrapping_add(1);
                    self.initialized = true;
                }
                return Ok((self.published.clone(), self.generation));
            }

            if !self.initialized {
                self.roots.clear();
                for (source, path) in configured_roots(configured) {
                    if let Some(root) = path {
                        self.roots.push(RootState {
                            source,
                            root: root.clone(),
                            missing: true,
                            node: None,
                        });
                    }
                }
            } else if let Some(ino) = self.inotify.as_mut() {
                match ino.drain() {
                    linux::DrainOutcome::Overflow => {
                        ino.clear_watches();
                        for root in &mut self.roots {
                            if let Some(node) = &mut root.node {
                                node.mark_all_dirty();
                            }
                        }
                    }
                    linux::DrainOutcome::Dirty(paths) => {
                        for path in paths {
                            for root in &mut self.roots {
                                if path == root.root || path.starts_with(&root.root) {
                                    if let Some(node) = &mut root.node {
                                        node.mark_dirty_path(&path);
                                    } else {
                                        root.missing = true;
                                    }
                                } else if root.root.starts_with(&path)
                                    && let Some(node) = &mut root.node
                                {
                                    node.mark_all_dirty();
                                }
                            }
                        }
                    }
                }
            }

            let mut any_work = !self.initialized;
            for root in &mut self.roots {
                if root.missing {
                    if std::fs::symlink_metadata(&root.root).is_ok() {
                        any_work = true;
                    }
                } else if root.node.as_ref().is_none_or(DirNode::needs_visit) {
                    any_work = true;
                }
            }
            if !any_work {
                return Ok((self.published.clone(), self.generation));
            }

            for root in &mut self.roots {
                if !root.missing && root.node.as_ref().is_some_and(|n| !n.needs_visit()) {
                    continue;
                }
                let force_recheck = has_alias_risk(&root.root);
                // Add root watch BEFORE opening/enumerating so concurrent mutations cannot slip between.
                let watched_root = if std::fs::symlink_metadata(&root.root).is_ok() {
                    ensure_watch(&mut self.inotify, &root.root, force_recheck)
                } else {
                    false
                };
                let dir = match Dir::open_ambient_dir(&root.root, ambient_authority()) {
                    Ok(dir) => dir,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        if let Some(old) = root.node.take() {
                            old.drop_watches(&mut self.inotify);
                        }
                        root.missing = true;
                        continue;
                    }
                    Err(_) => {
                        if let Some(node) = &mut root.node {
                            node.dirty = true;
                        }
                        return Err(SessionError::new(
                            503,
                            format!("{} 数据源目录暂时不可枚举", root.source),
                        ));
                    }
                };
                if dir.entries().is_err() {
                    if let Some(node) = &mut root.node {
                        node.dirty = true;
                    }
                    return Err(SessionError::new(
                        503,
                        format!("{} 数据源目录暂时不可枚举", root.source),
                    ));
                }
                root.missing = false;
                let node = root.node.get_or_insert_with(|| {
                    DirNode::new(
                        root.root.clone(),
                        root_kind(root.source),
                        force_recheck || !watched_root,
                    )
                });
                if !watched_root {
                    node.always_recheck = true;
                }
                if node.needs_visit() {
                    refresh_node(
                        node,
                        &root.root,
                        Some(&dir),
                        &mut self.inotify,
                        force_recheck,
                    )?;
                }
            }

            let mut flattened = Vec::with_capacity(self.published.len());
            for root in &self.roots {
                if let Some(node) = &root.node {
                    node.collect_items(&mut flattened);
                }
            }
            if !self.initialized || !same_discovered(&self.published, &flattened) {
                self.published = flattened.into();
                self.generation = self.generation.wrapping_add(1);
            }
            self.initialized = true;
            Ok((self.published.clone(), self.generation))
        }
    }
}

fn configured_roots(roots: &SessionRoots) -> [(&'static str, &Option<PathBuf>); 8] {
    [
        ("claude", &roots.claude),
        ("codex", &roots.codex),
        ("grok", &roots.grok),
        ("opencode", &roots.opencode),
        ("agy", &roots.agy),
        ("kiro", &roots.kiro),
        ("codebuddy", &roots.codebuddy),
        ("droid", &roots.droid),
    ]
}

fn same_discovered(a: &[Discovered], b: &[Discovered]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.source == y.source
                && x.path == y.path
                && x.data == y.data
                && x.summary_path == y.summary_path
                && x.agent_id == y.agent_id
                && x.key() == y.key()
        })
}

#[cfg(target_os = "linux")]
type WatchBackend = linux::Inotify;
#[cfg(not(target_os = "linux"))]
type WatchBackend = ();

fn sync_children(
    node: &mut DirNode,
    desired: Vec<(String, NodeKind, bool)>,
    inotify: &mut Option<WatchBackend>,
) {
    let mut next = BTreeMap::new();
    let mut old = std::mem::take(&mut node.children);
    for (name, kind, child_force_recheck) in desired {
        let entry = match old.remove(&name) {
            Some(mut existing) => {
                if child_force_recheck {
                    existing.always_recheck = true;
                }
                existing
            }
            None => DirNode::new(node.path.join(&name), kind, child_force_recheck),
        };
        next.insert(name, entry);
    }
    for (_, removed) in old {
        removed.drop_watches(inotify);
    }
    node.children = next;
}

fn refresh_children(
    node: &mut DirNode,
    root: &Path,
    parent_dir: Option<&Dir>,
    nofollow_open: bool,
    inotify: &mut Option<WatchBackend>,
    inherited_recheck: bool,
) -> Result<(), SessionError> {
    let mut removed_names = Vec::new();
    for (name, child) in &mut node.children {
        if !child.needs_visit() {
            continue;
        }
        let child_recheck = inherited_recheck || child.always_recheck;
        let opened = if matches!(child.kind, NodeKind::GrokSession) {
            None
        } else if nofollow_open {
            match parent_dir {
                Some(pdir) => pdir.open_dir_nofollow(name).ok(),
                None => Dir::open_ambient_dir(&child.path, ambient_authority())
                    .ok()
                    .filter(|_| {
                        std::fs::symlink_metadata(&child.path).is_ok_and(|m| m.file_type().is_dir())
                    }),
            }
        } else {
            Dir::open_ambient_dir(&child.path, ambient_authority()).ok()
        };
        if !matches!(child.kind, NodeKind::GrokSession) && opened.is_none() {
            // Unreadable or vanished child directory hides only its own files.
            removed_names.push(name.clone());
            continue;
        }
        refresh_node(child, root, opened.as_ref(), inotify, child_recheck)?;
    }
    for name in removed_names {
        if let Some(removed) = node.children.remove(&name) {
            removed.drop_watches(inotify);
        }
    }
    node.has_dirty_child = false;
    node.has_recheck_child = node
        .children
        .values()
        .any(|c| c.always_recheck || c.has_recheck_child);
    Ok(())
}

fn refresh_node(
    node: &mut DirNode,
    root: &Path,
    preopened: Option<&Dir>,
    inotify: &mut Option<WatchBackend>,
    inherited_recheck: bool,
) -> Result<(), SessionError> {
    let need_self = node.dirty || node.always_recheck || inherited_recheck;
    if need_self {
        let watched = ensure_watch(inotify, &node.path, inherited_recheck);
        let mut local_recheck = inherited_recheck || !watched;
        match node.kind {
            NodeKind::ProjectRoot(source) => {
                let dir = preopened.expect("root dir");
                let (subs, sym) = list_subdirs(dir, &node.path, true);
                local_recheck |= sym;
                node.items.clear();
                let desired = subs
                    .into_iter()
                    .map(|(n, is_sym)| (n, NodeKind::Project(source), local_recheck || is_sym))
                    .collect();
                sync_children(node, desired, inotify);
                refresh_children(node, root, Some(dir), false, inotify, inherited_recheck)?;
            }
            NodeKind::Project(source) => {
                let dir = preopened.expect("project dir");
                let (listed, file_alias) = list_files(dir, &node.path);
                let (subs, dir_sym) = list_subdirs(dir, &node.path, true);
                local_recheck |= file_alias || dir_sym;
                node.items.clear();
                for (name, stamp) in listed {
                    if !name.ends_with(".jsonl") || stamp.size == 0 {
                        continue;
                    }
                    let path = node.path.join(&name);
                    node.items.push(Discovered {
                        source,
                        root: root.to_path_buf(),
                        data: path.clone(),
                        path,
                        summary_path: None,
                        stamp: Some(stamp),
                        summary_stamp: None,
                        events_stamp: None,
                        agent_id: None,
                    });
                }
                let desired = subs
                    .into_iter()
                    .map(|(n, is_sym)| {
                        (
                            n,
                            NodeKind::ProjectSession(source),
                            inherited_recheck || is_sym,
                        )
                    })
                    .collect();
                sync_children(node, desired, inotify);
                refresh_children(node, root, Some(dir), false, inotify, inherited_recheck)?;
            }
            NodeKind::ProjectSession(source) => {
                node.items.clear();
                let sub_path = node.path.join("subagents");
                let mut desired = Vec::new();
                if let Ok(sym) = std::fs::symlink_metadata(&sub_path) {
                    let is_sym = sym.file_type().is_symlink();
                    local_recheck |= is_sym;
                    if sym.is_dir()
                        || (is_sym && std::fs::metadata(&sub_path).is_ok_and(|m| m.is_dir()))
                    {
                        desired.push((
                            "subagents".to_owned(),
                            NodeKind::ProjectSubagents(source),
                            inherited_recheck || is_sym,
                        ));
                    }
                }
                sync_children(node, desired, inotify);
                refresh_children(node, root, preopened, false, inotify, inherited_recheck)?;
            }
            NodeKind::ProjectSubagents(source) => {
                let dir = preopened.expect("subagents dir");
                let (listed, file_alias) = list_files(dir, &node.path);
                local_recheck |= file_alias;
                node.items.clear();
                let sidecars: HashMap<&str, Stamp> = listed
                    .iter()
                    .filter(|(name, _)| source == "claude" && name.ends_with(".meta.json"))
                    .map(|(name, stamp)| (name.as_str(), *stamp))
                    .collect();
                for (name, stamp) in &listed {
                    let Some(stem) = name.strip_suffix(".jsonl") else {
                        continue;
                    };
                    let Some(id) = stem.strip_prefix("agent-") else {
                        continue;
                    };
                    if stamp.size == 0 {
                        continue;
                    }
                    let sidecar = format!("{stem}.meta.json");
                    let summary_stamp = sidecars.get(sidecar.as_str()).copied();
                    let path = node.path.join(name);
                    node.items.push(Discovered {
                        source,
                        root: root.to_path_buf(),
                        data: path.clone(),
                        path,
                        summary_path: summary_stamp.map(|_| node.path.join(&sidecar)),
                        stamp: Some(*stamp),
                        summary_stamp,
                        events_stamp: None,
                        agent_id: Some(id.to_owned()),
                    });
                }
                node.children.clear();
                node.has_dirty_child = false;
                node.has_recheck_child = false;
            }
            NodeKind::DroidRoot => {
                let dir = preopened.expect("droid root");
                let (listed, file_alias) = list_files(dir, &node.path);
                let (subs, dir_sym) = list_subdirs(dir, &node.path, true);
                local_recheck |= file_alias || dir_sym;
                node.items.clear();
                for (name, stamp) in listed {
                    if name.ends_with(".jsonl") && stamp.size > 0 {
                        let path = node.path.join(name);
                        node.items.push(Discovered {
                            source: "droid",
                            root: root.to_path_buf(),
                            data: path.clone(),
                            path,
                            summary_path: None,
                            stamp: Some(stamp),
                            summary_stamp: None,
                            events_stamp: None,
                            agent_id: None,
                        });
                    }
                }
                let desired = subs
                    .into_iter()
                    .map(|(n, is_sym)| (n, NodeKind::DroidProject, inherited_recheck || is_sym))
                    .collect();
                sync_children(node, desired, inotify);
                refresh_children(node, root, Some(dir), false, inotify, inherited_recheck)?;
            }
            NodeKind::DroidProject => {
                let dir = preopened.expect("droid project");
                let (listed, file_alias) = list_files(dir, &node.path);
                local_recheck |= file_alias;
                node.items.clear();
                for (name, stamp) in listed {
                    if !name.ends_with(".jsonl") || stamp.size == 0 {
                        continue;
                    }
                    let path = node.path.join(name);
                    node.items.push(Discovered {
                        source: "droid",
                        root: root.to_path_buf(),
                        data: path.clone(),
                        path,
                        summary_path: None,
                        stamp: Some(stamp),
                        summary_stamp: None,
                        events_stamp: None,
                        agent_id: None,
                    });
                }
                node.children.clear();
                node.has_dirty_child = false;
                node.has_recheck_child = false;
            }
            NodeKind::CodexDir => {
                let dir = preopened.expect("codex dir");
                let (listed, file_alias) = list_files(dir, &node.path);
                let (subs, _) = list_subdirs(dir, &node.path, false);
                local_recheck |= file_alias;
                node.items.clear();
                for (name, stamp) in listed {
                    if !name.ends_with(".jsonl") || stamp.size == 0 {
                        continue;
                    }
                    let path = node.path.join(name);
                    node.items.push(Discovered {
                        source: "codex",
                        root: root.to_path_buf(),
                        data: path.clone(),
                        path,
                        summary_path: None,
                        stamp: Some(stamp),
                        summary_stamp: None,
                        events_stamp: None,
                        agent_id: None,
                    });
                }
                let desired = subs
                    .into_iter()
                    .map(|(n, _)| (n, NodeKind::CodexDir, inherited_recheck))
                    .collect();
                sync_children(node, desired, inotify);
                refresh_children(node, root, Some(dir), true, inotify, inherited_recheck)?;
            }
            NodeKind::GrokRoot => {
                let dir = preopened.expect("grok root");
                let (subs, _) = list_subdirs(dir, &node.path, false);
                node.items.clear();
                let desired = subs
                    .into_iter()
                    .map(|(n, _)| (n, NodeKind::GrokProject, inherited_recheck))
                    .collect();
                sync_children(node, desired, inotify);
                refresh_children(node, root, Some(dir), true, inotify, inherited_recheck)?;
            }
            NodeKind::GrokProject => {
                let dir = preopened.expect("grok project");
                let (subs, _) = list_subdirs(dir, &node.path, false);
                node.items.clear();
                let desired = subs
                    .into_iter()
                    .map(|(n, _)| (n, NodeKind::GrokSession, inherited_recheck))
                    .collect();
                sync_children(node, desired, inotify);
                refresh_children(node, root, Some(dir), true, inotify, inherited_recheck)?;
            }
            NodeKind::GrokSession => {
                node.items.clear();
                if !std::fs::symlink_metadata(&node.path).is_ok_and(|m| m.file_type().is_dir()) {
                    node.always_recheck = true;
                    node.dirty = false;
                    return Ok(());
                }
                let summary_file = node.path.join("summary.json");
                let chat_file = node.path.join("chat_history.jsonl");
                let events_file = node.path.join("events.jsonl");
                local_recheck |= has_alias_risk(&summary_file)
                    || has_alias_risk(&chat_file)
                    || has_alias_risk(&events_file);
                if let Ok(summary) = file_metadata(&summary_file)
                    && summary.is_file()
                {
                    let chat = file_metadata(&chat_file)
                        .ok()
                        .filter(|meta| meta.is_file())
                        .map(|meta| Stamp::of(&meta));
                    let events_stamp = file_metadata(&events_file)
                        .ok()
                        .filter(|meta| meta.is_file())
                        .map(|meta| Stamp::of(&meta));
                    node.items.push(Discovered {
                        source: "grok",
                        root: root.to_path_buf(),
                        data: chat_file,
                        summary_path: Some(summary_file),
                        path: node.path.clone(),
                        stamp: chat,
                        summary_stamp: Some(Stamp::of(&summary)),
                        events_stamp,
                        agent_id: None,
                    });
                }
                node.children.clear();
                node.has_dirty_child = false;
                node.has_recheck_child = false;
            }
            NodeKind::MirrorRoot(source) => {
                let dir = preopened.expect("mirror root");
                let (subs, _) = list_subdirs(dir, &node.path, false);
                node.items.clear();
                let desired = subs
                    .into_iter()
                    .map(|(n, _)| (n, NodeKind::MirrorProject(source), inherited_recheck))
                    .collect();
                sync_children(node, desired, inotify);
                refresh_children(node, root, Some(dir), true, inotify, inherited_recheck)?;
            }
            NodeKind::MirrorProject(source) => {
                let dir = preopened.expect("mirror project");
                let (subs, sym) = list_subdirs(dir, &node.path, true);
                local_recheck |= sym;
                node.items.clear();
                let desired = subs
                    .into_iter()
                    .map(|(n, is_sym)| {
                        (
                            n,
                            NodeKind::MirrorSession(source),
                            inherited_recheck || is_sym,
                        )
                    })
                    .collect();
                sync_children(node, desired, inotify);
                refresh_children(node, root, Some(dir), false, inotify, inherited_recheck)?;
            }
            NodeKind::MirrorSession(source) => {
                node.items.clear();
                let summary_file = node.path.join(super::super::opencode::SUMMARY_FILE);
                let data_file = node.path.join(super::super::opencode::MESSAGES_FILE);
                local_recheck |= has_alias_risk(&summary_file) || has_alias_risk(&data_file);
                if let Ok(summary) = file_metadata(&summary_file)
                    && summary.is_file()
                {
                    let stamp = file_metadata(&data_file)
                        .ok()
                        .filter(|meta| meta.is_file())
                        .map(|meta| Stamp::of(&meta));
                    node.items.push(Discovered {
                        source,
                        root: root.to_path_buf(),
                        summary_path: Some(summary_file),
                        data: data_file,
                        path: node.path.clone(),
                        stamp,
                        summary_stamp: Some(Stamp::of(&summary)),
                        events_stamp: None,
                        agent_id: None,
                    });
                    if source == "kiro" {
                        let agents_path = node.path.join("agents");
                        let mut desired = Vec::new();
                        if let Ok(sym) = std::fs::symlink_metadata(&agents_path) {
                            let is_sym = sym.file_type().is_symlink();
                            local_recheck |= is_sym;
                            if sym.is_dir()
                                || (is_sym
                                    && std::fs::metadata(&agents_path).is_ok_and(|m| m.is_dir()))
                            {
                                desired.push((
                                    "agents".to_owned(),
                                    NodeKind::KiroAgents,
                                    inherited_recheck || is_sym,
                                ));
                            }
                        }
                        sync_children(node, desired, inotify);
                        refresh_children(node, root, preopened, false, inotify, inherited_recheck)?;
                    } else {
                        node.children.clear();
                        node.has_dirty_child = false;
                        node.has_recheck_child = false;
                    }
                } else {
                    for (_, removed) in std::mem::take(&mut node.children) {
                        removed.drop_watches(inotify);
                    }
                    node.has_dirty_child = false;
                    node.has_recheck_child = false;
                }
            }
            NodeKind::KiroAgents => {
                let dir = preopened.expect("kiro agents dir");
                let (subs, sym) = list_subdirs(dir, &node.path, true);
                local_recheck |= sym;
                node.items.clear();
                let desired = subs
                    .into_iter()
                    .map(|(n, is_sym)| (n, NodeKind::KiroAgent, inherited_recheck || is_sym))
                    .collect();
                sync_children(node, desired, inotify);
                refresh_children(node, root, Some(dir), false, inotify, inherited_recheck)?;
            }
            NodeKind::KiroAgent => {
                node.items.clear();
                let summary_file = node.path.join(super::super::opencode::SUMMARY_FILE);
                let data_file = node.path.join(super::super::opencode::MESSAGES_FILE);
                local_recheck |= has_alias_risk(&summary_file) || has_alias_risk(&data_file);
                if let Ok(summary) = file_metadata(&summary_file)
                    && summary.is_file()
                {
                    let stamp = file_metadata(&data_file)
                        .ok()
                        .filter(|meta| meta.is_file())
                        .map(|meta| Stamp::of(&meta));
                    node.items.push(Discovered {
                        source: "kiro",
                        root: root.to_path_buf(),
                        summary_path: Some(summary_file),
                        data: data_file,
                        path: node.path.clone(),
                        stamp,
                        summary_stamp: Some(Stamp::of(&summary)),
                        events_stamp: None,
                        agent_id: None,
                    });
                }
                node.children.clear();
                node.has_dirty_child = false;
                node.has_recheck_child = false;
            }
        }
        node.always_recheck = local_recheck;
        node.dirty = false;
    } else {
        let nofollow = matches!(
            node.kind,
            NodeKind::CodexDir
                | NodeKind::GrokRoot
                | NodeKind::GrokProject
                | NodeKind::MirrorRoot(_)
        );
        refresh_children(node, root, preopened, nofollow, inotify, inherited_recheck)?;
    }
    Ok(())
}
