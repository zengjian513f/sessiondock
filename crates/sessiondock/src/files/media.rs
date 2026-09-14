//! One trusted selected-view reference index shared by an entire media window.
use super::{CheckedImage, FileError, FileScope, FileService, normalize_media_ref, references};
use serde_json::Value;
use std::sync::Arc;

/// Media indexes cached per immutable view revision: a history request with
/// images and every file-backed media GET reauthorize against the same view,
/// so the regex scan of the whole branch runs once per revision, not per
/// request. Bounded by entries and by retained references.
pub(super) const MEDIA_INDEX_CACHE_ENTRIES: usize = 8;
pub(super) const MEDIA_INDEX_CACHE_REFS: usize = 1_000_000;

pub(crate) struct ScopedFiles<'a> {
    scope: FileScope<'a>,
    index: Arc<references::ReferenceIndex>,
}

impl FileService {
    #[cfg(test)]
    pub(crate) fn scoped_media<'a>(
        &'a self,
        scope: &FileScope<'a>,
        native_refs: &[&str],
    ) -> Result<ScopedFiles<'a>, FileError> {
        self.scoped_media_iter(
            scope.uid,
            scope.agent,
            scope.cwd,
            scope.messages,
            native_refs,
            None,
        )
    }

    /// `messages` and `native_refs` must come from the same complete, selected
    /// SessionStore branch/agent. Request parameters never establish references.
    /// `revision` is the immutable identity of that view (uid, agent, cursor,
    /// file stamp); when given, the reference index is reused for the same
    /// revision instead of rescanning the branch.
    pub(crate) fn scoped_media_iter<'a, 'm>(
        &'a self,
        uid: &'a str,
        agent: Option<&'a str>,
        cwd: &'a str,
        messages: impl IntoIterator<Item = &'m Value>,
        native_refs: &[&str],
        revision: Option<&str>,
    ) -> Result<ScopedFiles<'a>, FileError> {
        let scope = FileScope {
            uid,
            agent,
            cwd,
            messages: &[],
        };
        references::validate_scope(&scope)?;
        let index = match revision.and_then(|revision| self.cached_media_index(revision)) {
            Some(index) => index,
            None => {
                let index = Arc::new(references::ReferenceIndex::media(messages, native_refs)?);
                if let Some(revision) = revision {
                    self.retain_media_index(revision, index.clone());
                }
                index
            }
        };
        Ok(ScopedFiles { scope, index })
    }
    fn cached_media_index(&self, revision: &str) -> Option<Arc<references::ReferenceIndex>> {
        let mut cache = self.media_indexes.lock().ok()?;
        let position = cache.iter().position(|(key, _)| key == revision)?;
        let entry = cache.remove(position);
        let index = entry.1.clone();
        cache.push(entry);
        Some(index)
    }
    fn retain_media_index(&self, revision: &str, index: Arc<references::ReferenceIndex>) {
        let Ok(mut cache) = self.media_indexes.lock() else {
            return;
        };
        cache.retain(|(key, _)| key != revision);
        cache.push((revision.to_owned(), index));
        let retained = |cache: &Vec<(String, Arc<references::ReferenceIndex>)>| {
            cache
                .iter()
                .map(|(_, index)| index.refs.len())
                .sum::<usize>()
        };
        while cache.len() > MEDIA_INDEX_CACHE_ENTRIES
            || (cache.len() > 1 && retained(&cache) > MEDIA_INDEX_CACHE_REFS)
        {
            cache.remove(0);
        }
    }
}

impl ScopedFiles<'_> {
    pub(crate) fn identity(&self) -> (&str, Option<&str>) {
        (self.scope.uid, self.scope.agent)
    }
    /// Media paths resolve directly against cwd.
    pub(crate) fn image(&self, reference: &str) -> Result<CheckedImage, FileError> {
        let reference = normalize_media_ref(reference)?;
        // Missing references should not probe any paths in the branch.
        if !self.index.refs.contains(&reference) {
            return Err(FileError::new(
                404,
                "file_not_referenced",
                "该路径未出现在所选会话分支中",
            ));
        }
        let path = super::boundary::expand_user(&reference);
        let path = if path.is_absolute() {
            path
        } else {
            let cwd = std::path::Path::new(self.scope.cwd);
            if !cwd.is_absolute() {
                return Err(FileError::new(
                    404,
                    "file_not_found",
                    "会话没有绝对工作目录",
                ));
            }
            cwd.join(path)
        };
        let path = path.canonicalize().map_err(FileError::io)?;
        let target = super::boundary::open_target(super::boundary::volume_root(&path)?, path)?;
        CheckedImage::new(target)
    }
}

#[cfg(test)]
mod tests;
