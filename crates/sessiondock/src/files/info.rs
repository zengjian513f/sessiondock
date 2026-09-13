//! Browser information describes the named leaf like Python's `path_for` /
//! `describe`. Direct references and byte reads retain their usual resolution.
use super::{FileError, FileScope, FileService, boundary, response};
use serde_json::{Value, json};
use std::io::ErrorKind;

impl FileService {
    pub fn browser_info(
        &self,
        scope: &FileScope<'_>,
        reference: &str,
        navigation: Option<&str>,
    ) -> Result<Value, FileError> {
        // Grant validation precedes any navigation, including a missing target.
        self.browser_anchor(scope, reference)?;
        let Some(raw) = navigation.filter(|value| !value.is_empty()) else {
            return self.describe(&self.target(scope, reference, None)?);
        };
        let path = boundary::mutation_path(raw)?;
        let Some(name) = path.file_name() else {
            return self.describe(&self.navigation(raw)?);
        };
        let parent = self.navigation(&boundary::wire_path(path.parent().unwrap())?)?;
        parent.verify_identity()?;
        let directory = parent.directory()?;
        let metadata = directory.symlink_metadata(name).map_err(FileError::io)?;
        if !metadata.is_symlink() {
            return self.describe(&self.navigation(&boundary::wire_path(&path)?)?);
        }
        let stamp = boundary::Stamp::new(&metadata);
        let link = directory.read_link_contents(name).map_err(FileError::io)?;
        let mut info = response::metadata_description(&path, &metadata, "symlink")?;
        info["link_target"] = json!(link.to_str().ok_or_else(|| {
            FileError::new(400, "file_path_encoding", "链接目标无法表示为 UTF-8")
        })?);
        // A dangling link has useful lstat information without a readable
        // target. Existing regular targets still use checked preview handles.
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                let target = self.navigation(&boundary::wire_path(&path)?)?;
                response::describe_preview(&mut info, &target, &path)?;
                target.verify()?;
            }
            Ok(_) => {}
            Err(error)
                if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}
            Err(error) => return Err(FileError::io(error)),
        }
        parent.verify_identity()?;
        if boundary::Stamp::new(&directory.symlink_metadata(name).map_err(FileError::io)?) != stamp
        {
            return Err(FileError::changed());
        }
        Ok(info)
    }
}
