//! Read-only, single-directory path completion in a conversation's cwd.
use serde::Serialize;
use std::{io, path::Path};

#[derive(Serialize)]
pub struct Candidate {
    path: String,
    directory: bool,
}

pub fn complete(cwd: &Path, text: &str) -> io::Result<Vec<Candidate>> {
    let Some(expanded) = crate::lifecycle::model::expand_user(Path::new(text)) else {
        return Ok(Vec::new());
    };
    if text == "~" {
        return Ok(if expanded.is_dir() {
            vec![Candidate {
                path: "~/".into(),
                directory: true,
            }]
        } else {
            Vec::new()
        });
    }
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        if !cwd.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "会话工作目录不可用",
            ));
        }
        cwd.join(expanded)
    };
    let separator = |ch| ch == '/' || (cfg!(windows) && ch == '\\');
    let split = text.rfind(separator).map_or(0, |i| i + 1);
    let (display_parent, prefix) = text.split_at(split);
    let parent = if text.is_empty() || text.ends_with(separator) {
        absolute.as_path()
    } else {
        absolute.parent().unwrap_or(cwd)
    };
    let slash = if cfg!(windows) && text.contains('\\') {
        "\\"
    } else {
        "/"
    };
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(prefix) || (name.starts_with('.') && !prefix.starts_with('.')) {
            continue;
        }
        // Follow directory symlinks, retaining the user's spelling.
        let directory = entry.path().is_dir();
        rows.push(Candidate {
            path: format!(
                "{display_parent}{name}{}",
                if directory { slash } else { "" }
            ),
            directory,
        });
    }
    rows.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(rows)
}
