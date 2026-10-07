//! An authenticated file browser keeps its directory grant after the original
//! directory is renamed or removed. Each API
//! request still resolves the selected native session before consulting it.
use super::{FileError, FileScope, FileService, ResolvedTarget};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

const FILE: &str = "file-browser-grants.json";
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    uid: String,
    agent: Option<String>,
    reference: String,
}
#[derive(Default)]
pub(super) struct Grants {
    directory: Option<Arc<Dir>>,
    entries: Mutex<BTreeMap<Key, PathBuf>>,
}
impl Grants {
    fn open(directory: Option<&Path>) -> Result<Self, FileError> {
        let Some(directory) = directory else {
            return Ok(Self::default());
        };
        let directory =
            Arc::new(Dir::open_ambient_dir(directory, ambient_authority()).map_err(FileError::io)?);
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let entries = match directory.open_with(FILE, &options) {
            Ok(mut file) => {
                let mut raw = Vec::new();
                file.read_to_end(&mut raw).map_err(FileError::io)?;
                // An unreadable legacy grant cannot confer authority: re-grant
                // it through a current selected directory reference instead.
                serde_json::from_slice::<Vec<(Key, PathBuf)>>(&raw)
                    .unwrap_or_default()
                    .into_iter()
                    .collect()
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(FileError::io(error)),
        };
        Ok(Self {
            directory: Some(directory),
            entries: Mutex::new(entries),
        })
    }
    fn key(scope: &FileScope<'_>, reference: &str) -> Key {
        Key {
            uid: scope.uid.into(),
            agent: scope.agent.map(str::to_owned),
            reference: reference.into(),
        }
    }
    fn lookup(&self, key: &Key) -> Option<PathBuf> {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .cloned()
    }
    fn insert(&self, key: Key, path: PathBuf) -> Result<PathBuf, FileError> {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = entries.get(&key) {
            return Ok(existing.clone());
        }
        let mut next = entries.clone();
        next.insert(key, path.clone());
        if let Some(directory) = &self.directory {
            let data = serde_json::to_vec(&next.iter().collect::<Vec<_>>())
                .map_err(|_| FileError::new(503, "file_grant_store", "无法保存目录浏览授权"))?;
            let mut random = [0u8; 16];
            getrandom::fill(&mut random)
                .map_err(|_| FileError::new(503, "file_grant_store", "无法创建目录授权暂存文件"))?;
            let temporary = format!(
                ".file-grants-{}.tmp",
                random
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            let result = (|| {
                let mut options = OpenOptions::new();
                options
                    .write(true)
                    .create_new(true)
                    .follow(FollowSymlinks::No);
                #[cfg(unix)]
                cap_std::fs::OpenOptionsExt::mode(&mut options, 0o600);
                let mut file = directory.open_with(&temporary, &options)?;
                file.write_all(&data)?;
                file.sync_all()?;
                drop(file);
                directory.rename(&temporary, directory, FILE)
            })();
            if let Err(error) = result {
                let _ = directory.remove_file(&temporary);
                return Err(FileError::io(error));
            }
        }
        *entries = next;
        Ok(path)
    }
}
impl FileService {
    pub fn with_grants(mut self, state: Option<&Path>) -> Result<Self, FileError> {
        self.grants = Grants::open(state)?;
        Ok(self)
    }
    fn directory_grant(
        &self,
        scope: &FileScope<'_>,
        reference: &str,
    ) -> Result<PathBuf, FileError> {
        super::references::validate_scope(scope)?;
        if reference.is_empty()
            || reference.chars().count() > super::MAX_PATH_BYTES
            || reference.contains('\0')
        {
            return Err(FileError::new(400, "file_path_invalid", "无效的目录引用"));
        }
        let key = Grants::key(scope, reference);
        if let Some(path) = self.grants.lookup(&key) {
            return Ok(path);
        }
        let target = self.target(scope, reference, None)?;
        if target.kind() != "directory" {
            return Err(FileError::new(
                400,
                "file_directory_anchor_required",
                "文件浏览入口必须是会话提及的目录",
            ));
        }
        target.verify()?;
        self.grants.insert(key, target.path().to_path_buf())
    }
    /// A writer's request paths are independent of the initial directory after
    /// the grant. Keep a checked volume-root handle even if that directory no
    /// longer exists; writer-side OS/private-data guards apply to each target.
    pub fn browser_anchor(
        &self,
        scope: &FileScope<'_>,
        reference: &str,
    ) -> Result<ResolvedTarget, FileError> {
        let granted = self.directory_grant(scope, reference)?;
        let mut root = PathBuf::new();
        for component in granted.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => root.push(component.as_os_str()),
                _ => break,
            }
        }
        self.navigation(
            root.to_str()
                .ok_or_else(|| FileError::new(400, "file_path_encoding", "目录路径不是 UTF-8"))?,
        )
    }
}
