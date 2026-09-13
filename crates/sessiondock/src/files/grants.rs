//! An authenticated file browser keeps its directory grant after the original
//! directory is renamed or removed, as Python's file manager does. Each API
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
    pub fn browser_target(
        &self,
        scope: &FileScope<'_>,
        reference: &str,
        navigation: Option<&str>,
    ) -> Result<ResolvedTarget, FileError> {
        self.directory_grant(scope, reference)?;
        match navigation.filter(|value| !value.is_empty()) {
            Some(path) => self.navigation(path),
            // The grant survives a renamed/deleted entry, but it is not a
            // cached resolution of that reference. Python resolves a fresh
            // click against the current selected session cwd and filesystem.
            None => self.target(scope, reference, None),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn grant_survives_removed_anchor_and_restart_but_is_scoped_to_session_and_agent() {
        let temp = tempfile::tempdir().unwrap();
        let anchor = temp.path().join("mentioned");
        let other = temp.path().join("other");
        let state = temp.path().join("state");
        for p in [&anchor, &other, &state] {
            std::fs::create_dir(p).unwrap();
        }
        let reference = anchor.to_str().unwrap();
        let messages = vec![json!({"role":"assistant","text":reference})];
        let scope = FileScope {
            uid: "claude:grant-test",
            agent: None,
            cwd: reference,
            messages: &messages,
        };
        let files = FileService::open(vec![anchor.clone()])
            .unwrap()
            .with_grants(Some(&state))
            .unwrap();
        files.browser_target(&scope, reference, None).unwrap();
        std::fs::remove_dir(&anchor).unwrap();
        drop(files);
        let files = FileService::open(vec![other.clone()])
            .unwrap()
            .with_grants(Some(&state))
            .unwrap();
        let now = FileScope {
            messages: &[],
            ..scope
        };
        assert!(
            files
                .browser_target(&now, reference, other.to_str())
                .is_ok()
        );
        assert!(files.browser_anchor(&now, reference).is_ok());
        assert!(
            files
                .browser_target(
                    &FileScope {
                        uid: "claude:another",
                        ..now
                    },
                    reference,
                    other.to_str()
                )
                .is_err()
        );
        assert!(
            files
                .browser_target(
                    &FileScope {
                        agent: Some("another-agent"),
                        ..now
                    },
                    reference,
                    other.to_str()
                )
                .is_err()
        );
        assert!(
            files
                .browser_target(&now, "/unmentioned-directory", other.to_str())
                .is_err()
        );
    }

    #[test]
    fn second_grant_replaces_store_and_both_scoped_navigations_survive_restart() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let navigation = temp.path().join("navigation");
        let state = temp.path().join("state");
        for path in [&first, &second, &navigation, &state] {
            std::fs::create_dir(path).unwrap();
        }
        let first_ref = first.to_str().unwrap();
        let second_ref = second.to_str().unwrap();
        let first_messages = vec![json!({"role":"assistant","text":first_ref})];
        let second_messages = vec![json!({"role":"assistant","text":second_ref})];
        let first_scope = FileScope {
            uid: "claude:first-grant",
            agent: None,
            cwd: first_ref,
            messages: &first_messages,
        };
        let second_scope = FileScope {
            uid: "codex:second-grant",
            agent: Some("worker"),
            cwd: second_ref,
            messages: &second_messages,
        };
        let files = FileService::open(vec![temp.path().to_owned()])
            .unwrap()
            .with_grants(Some(&state))
            .unwrap();
        files.browser_target(&first_scope, first_ref, None).unwrap();
        let original = std::fs::read(state.join(FILE)).unwrap();
        assert_eq!(
            serde_json::from_slice::<Vec<(Key, PathBuf)>>(&original)
                .unwrap()
                .len(),
            1
        );
        // The second insert must atomically replace the existing grant file.
        files
            .browser_target(&second_scope, second_ref, None)
            .unwrap();
        let updated = std::fs::read(state.join(FILE)).unwrap();
        assert_ne!(updated, original);
        assert_eq!(
            serde_json::from_slice::<Vec<(Key, PathBuf)>>(&updated)
                .unwrap()
                .len(),
            2
        );
        drop(files);
        std::fs::remove_dir(&first).unwrap();
        std::fs::remove_dir(&second).unwrap();
        let files = FileService::open(vec![navigation.clone()])
            .unwrap()
            .with_grants(Some(&state))
            .unwrap();
        let first_now = FileScope {
            messages: &[],
            ..first_scope
        };
        let second_now = FileScope {
            messages: &[],
            ..second_scope
        };
        for (scope, reference) in [(&first_now, first_ref), (&second_now, second_ref)] {
            assert!(
                files
                    .browser_target(scope, reference, navigation.to_str())
                    .is_ok()
            );
            assert!(files.browser_anchor(scope, reference).is_ok());
        }
        assert!(
            files
                .browser_target(&second_now, first_ref, navigation.to_str())
                .is_err()
        );
        assert!(
            files
                .browser_target(&first_now, second_ref, navigation.to_str())
                .is_err()
        );
        assert!(
            files
                .browser_target(
                    &FileScope {
                        agent: None,
                        ..second_now
                    },
                    second_ref,
                    navigation.to_str()
                )
                .is_err()
        );
        assert_eq!(std::fs::read(state.join(FILE)).unwrap(), updated);
    }
}
