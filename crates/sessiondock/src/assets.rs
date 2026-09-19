//! Startup snapshot of regular frontend files. Requests never walk the disk.
//! In-root aliases are resolved at load. HTML pages receive mode, hostname, build and
//! capabilities; stylesheets receive the build for their `?v=` font URLs; other assets
//! use the build ETag. A non-HTML request whose `?v=` equals the build is immutable
//! (one year), so revisits fetch only the page and the API; any other asset request
//! revalidates (`no-cache` + ETag). GET and HEAD only; `files.html` with `open=1`
//! redirects to `file.html`. This is not a dynamic static-file server.
//! The hub binary serves the same snapshot in `Mode::Hub` (`__SESSIONDOCK_MODE__`
//! `hub`, hostname `SessionDock`, storage namespace `sessiondock.hub.<path>.`).
use std::{collections::BTreeMap, fs, io, path::Path};

use axum::{
    body::{Body, Bytes},
    extract::{OriginalUri, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use sha1::{Digest, Sha1};

use crate::state::AppState;

pub struct Assets {
    pub build: String,
    entries: BTreeMap<String, Asset>,
}

/// Which service serves the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Local,
    Hub,
}

/// The hub page brand (the SessionDock hub carries its own name).
pub const HUB_HOSTNAME: &str = "SessionDock";
/// The page's localStorage prefix in hub mode; the served path is appended
/// in the browser (`sessiondock.hub.<location.pathname>.`);
/// `sessiondock.hub.<path>.` distinguishes hubs mounted at different paths.
pub const HUB_STORAGE_NAMESPACE: &str = "sessiondock.hub.";

struct Asset {
    body: Bytes,
    content_type: String,
    html: bool,
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

impl Assets {
    /// Snapshot frontend files and in-root aliases at startup.
    pub fn load(root: &Path, hostname: &str, capabilities: &serde_json::Value) -> io::Result<Self> {
        Self::load_mode(root, hostname, capabilities, Mode::Local)
    }

    /// `load` for either service; in hub mode the hostname is `SessionDock` and
    /// a namespace script appends the page path to the storage prefix.
    pub fn load_mode(
        root: &Path,
        hostname: &str,
        capabilities: &serde_json::Value,
        mode: Mode,
    ) -> io::Result<Self> {
        let root = root.canonicalize()?;
        let mut raw = BTreeMap::<String, Vec<u8>>::new();
        fn collect(
            root: &Path,
            dir: &Path,
            ancestors: &mut Vec<std::path::PathBuf>,
            raw: &mut BTreeMap<String, Vec<u8>>,
        ) -> io::Result<()> {
            let resolved = dir.canonicalize()?;
            if !resolved.starts_with(root) || ancestors.contains(&resolved) {
                return Ok(());
            }
            ancestors.push(resolved);
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                let resolved = match path.canonicalize() {
                    Ok(path) => path,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                if !resolved.starts_with(root) {
                    continue;
                }
                let metadata = fs::metadata(&resolved)?;
                if metadata.is_dir() {
                    collect(root, &path, ancestors, raw)?;
                } else if metadata.is_file() {
                    let data = fs::read(&resolved)?;
                    let name = path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    raw.insert(format!("/{name}"), data);
                }
            }
            ancestors.pop();
            Ok(())
        }
        collect(&root, &root, &mut Vec::new(), &mut raw)?;
        if !raw.contains_key("/index.html") {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "frontend index.html missing",
            ));
        }
        let mut digest = Sha1::new();
        digest.update(b"sessiondock-legacy-v1\0");
        digest.update(capabilities.to_string().as_bytes());
        for (name, data) in &raw {
            digest.update(name.as_bytes());
            digest.update((data.len() as u64).to_be_bytes());
            digest.update(data);
        }
        let build = format!("{:x}", digest.finalize())[..12].to_owned();
        let (mode_name, hostname) = match mode {
            Mode::Local => ("local", hostname),
            Mode::Hub => ("hub", HUB_HOSTNAME),
        };
        let mut injection = format!(
            "<meta name=\"sessiondock-mode\" content=\"{mode_name}\">\n<meta name=\"sessiondock-capabilities\" content=\"{}\">",
            escape_html(&capabilities.to_string())
        );
        if mode == Mode::Hub {
            // Hub storage is keyed on `location.pathname`; the snapshot
            // cannot know the mount path, so the page completes the prefix
            // before the theme script and capabilities.js read it.
            injection.push_str(&format!(
                "\n<script>(()=>{{const m=document.querySelector('meta[name=\"sessiondock-capabilities\"]');try{{const c=JSON.parse(m.content);c.storage_namespace={}+location.pathname+'.';m.content=JSON.stringify(c);}}catch{{}}}})();</script>",
                serde_json::Value::String(HUB_STORAGE_NAMESPACE.to_string())
            ));
        }
        let marker = format!("<meta name=\"sessiondock-mode\" content=\"{mode_name}\">");
        let entries = raw
            .into_iter()
            .map(|(path, mut data)| {
                let html = matches!(
                    path.as_str(),
                    "/index.html" | "/files.html" | "/file.html" | "/records.html"
                );
                if html {
                    data = String::from_utf8_lossy(&data)
                        .replace("__SESSIONDOCK_MODE__", mode_name)
                        .replace("__SESSIONDOCK_HOSTNAME__", &escape_html(hostname))
                        .replace("__SESSIONDOCK_ASSET_VERSION__", &build)
                        .replace(&marker, &injection)
                        .into_bytes();
                } else if path.ends_with(".css") {
                    // typography.css versions its font URLs the same way the pages
                    // version their scripts, so fonts are immutable-cacheable too.
                    data = String::from_utf8_lossy(&data)
                        .replace("__SESSIONDOCK_ASSET_VERSION__", &build)
                        .into_bytes();
                }
                let mut content_type = mime_guess::from_path(&path)
                    .first_or_octet_stream()
                    .to_string();
                if content_type.starts_with("text/") || content_type == "application/javascript" {
                    content_type.push_str("; charset=utf-8");
                }
                (
                    path,
                    Asset {
                        body: data.into(),
                        content_type,
                        html,
                    },
                )
            })
            .collect();
        Ok(Self { build, entries })
    }
}

pub async fn serve(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
) -> Response {
    serve_asset(&state.assets, &uri, &method, &headers)
}

/// One snapshot entry as a response; the hub's dispatcher calls this for
/// every GET outside `/api/`.
pub fn serve_asset(
    assets: &Assets,
    uri: &axum::http::Uri,
    method: &Method,
    headers: &HeaderMap,
) -> Response {
    if *method != Method::GET && *method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = if uri.path() == "/" {
        "/index.html"
    } else {
        uri.path()
    };
    if path == "/files.html"
        && uri
            .query()
            .unwrap_or("")
            .split('&')
            .any(|part| part == "open=1")
    {
        return (
            StatusCode::SEE_OTHER,
            [(
                header::LOCATION,
                format!("file.html?{}", uri.query().unwrap_or("")),
            )],
        )
            .into_response();
    }
    let Some(asset) = assets.entries.get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let etag = format!("\"{}\"", assets.build);
    // Pages embed `?v=<build>` in every asset URL; a matching request can be
    // cached forever because a new build changes the URL. Stale or missing
    // versions keep revalidating.
    let versioned = !asset.html
        && uri
            .query()
            .unwrap_or("")
            .split('&')
            .any(|part| part.strip_prefix("v=") == Some(assets.build.as_str()));
    // A compressing reverse proxy (nginx gzip) downgrades the ETag to a weak
    // `W/"build"`, and that is what the browser sends back; the snapshot is
    // byte-identical per build, so weak and strong validators both match.
    let cached = !asset.html
        && headers
            .get(header::IF_NONE_MATCH)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|value| {
                value.split(',').any(|candidate| {
                    let candidate = candidate.trim();
                    candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
                })
            });
    let mut response = if cached {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut response = Body::from(if *method == Method::HEAD {
            Bytes::new()
        } else {
            asset.body.clone()
        })
        .into_response();
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, asset.body.len().into());
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, asset.content_type.parse().unwrap());
        response
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        if asset.html {
            "no-store"
        } else if versioned {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        }
        .parse()
        .unwrap(),
    );
    response
        .headers_mut()
        .insert(header::ETAG, etag.parse().unwrap());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_large_files_and_hidden_assets() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("index.html"), "<html></html>").unwrap();
        fs::write(root.path().join("large.bin"), vec![7; 9 * 1024 * 1024]).unwrap();
        fs::write(root.path().join(".asset"), b"hidden").unwrap();
        let assets = Assets::load(root.path(), "test", &serde_json::json!({})).unwrap();
        assert!(assets.entries.contains_key("/large.bin"));
        assert!(assets.entries.contains_key("/.asset"));
    }

    #[test]
    fn versioned_assets_are_immutable_and_stylesheets_get_the_build() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("index.html"), "<html></html>").unwrap();
        fs::write(root.path().join("app.js"), "1").unwrap();
        fs::write(
            root.path().join("typography.css"),
            "url(\"fonts/a.woff2?v=__SESSIONDOCK_ASSET_VERSION__\")",
        )
        .unwrap();
        let assets = Assets::load(root.path(), "test", &serde_json::json!({})).unwrap();
        let cache = |target: &str| {
            let uri: axum::http::Uri = target.parse().unwrap();
            let response = serve_asset(&assets, &uri, &Method::GET, &HeaderMap::new());
            let cache = response.headers().get(header::CACHE_CONTROL).unwrap();
            (response.status(), cache.to_str().unwrap().to_owned())
        };
        let immutable = "public, max-age=31536000, immutable".to_owned();
        let build = assets.build.clone();
        assert_eq!(
            cache(&format!("/app.js?v={build}")),
            (StatusCode::OK, immutable.clone())
        );
        assert_eq!(
            cache(&format!("/typography.css?x=1&v={build}")),
            (StatusCode::OK, immutable)
        );
        assert_eq!(cache("/app.js"), (StatusCode::OK, "no-cache".to_owned()));
        assert_eq!(
            cache("/app.js?v=stale"),
            (StatusCode::OK, "no-cache".to_owned())
        );
        assert_eq!(
            cache(&format!("/?v={build}")),
            (StatusCode::OK, "no-store".to_owned())
        );
        assert_eq!(
            cache(&format!("/index.html?v={build}")),
            (StatusCode::OK, "no-store".to_owned())
        );
        let css = assets.entries.get("/typography.css").unwrap();
        assert_eq!(
            css.body.as_ref(),
            format!("url(\"fonts/a.woff2?v={build}\")").as_bytes()
        );
        let uri: axum::http::Uri = format!("/app.js?v={build}").parse().unwrap();
        let revalidate = |value: String| {
            let mut headers = HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, value.parse().unwrap());
            serve_asset(&assets, &uri, &Method::GET, &headers).status()
        };
        assert_eq!(revalidate(format!("\"{build}\"")), StatusCode::NOT_MODIFIED);
        assert_eq!(
            revalidate(format!("W/\"{build}\"")),
            StatusCode::NOT_MODIFIED
        );
        assert_eq!(
            revalidate(format!("\"old\", W/\"{build}\"")),
            StatusCode::NOT_MODIFIED
        );
        assert_eq!(revalidate("\"old\"".to_owned()), StatusCode::OK);
        assert_eq!(revalidate(build.clone()), StatusCode::OK);
    }

    #[cfg(unix)]
    #[test]
    fn snapshots_in_root_aliases_without_following_escape_or_cycles() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(root.path().join("index.html"), "<html></html>").unwrap();
        fs::create_dir(root.path().join("dir")).unwrap();
        fs::write(root.path().join("dir/file.js"), "test").unwrap();
        symlink("dir", root.path().join("alias")).unwrap();
        symlink("..", root.path().join("dir/cycle")).unwrap();
        fs::write(outside.path().join("secret"), "outside").unwrap();
        symlink(outside.path().join("secret"), root.path().join("escape")).unwrap();
        let assets = Assets::load(root.path(), "test", &serde_json::json!({})).unwrap();
        assert!(assets.entries.contains_key("/dir/file.js"));
        assert!(assets.entries.contains_key("/alias/file.js"));
        assert!(!assets.entries.contains_key("/escape"));
    }
}
