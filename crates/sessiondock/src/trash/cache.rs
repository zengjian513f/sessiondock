//! Read-through manifest cache. Directory enumeration and stat keep external
//! changes visible without reopening unchanged manifests or immutable rollouts.
use super::*;

#[derive(Debug, PartialEq, Eq)]
struct Version {
    stamp: manifest::Stamp,
    #[cfg(unix)]
    changed: (i64, i64, u32),
}
impl Version {
    fn read(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        Some(Self {
            stamp: manifest::Stamp::from_metadata(&metadata),
            #[cfg(unix)]
            changed: {
                use std::os::unix::fs::MetadataExt;
                (metadata.ctime(), metadata.ctime_nsec(), metadata.mode())
            },
        })
    }
}

#[derive(Default)]
pub(super) struct Manifests {
    entries: BTreeMap<
        String,
        (
            Option<Version>,
            Option<Version>,
            Result<Manifest, TrashError>,
        ),
    >,
}
impl Manifests {
    pub fn read(&mut self, id: &str, path: &Path) -> Result<Manifest, TrashError> {
        let directory = Version::read(path);
        let manifest = Version::read(&Manifest::path(path));
        if let Some((prior_dir, prior_file, value)) = self.entries.get(id)
            && directory.is_some()
            && manifest.is_some()
            && *prior_dir == directory
            && *prior_file == manifest
        {
            return value.clone();
        }
        // Keep the pre-read versions: a concurrent replacement is detected on
        // the next scan, never labelled with the version of unread bytes.
        let value = Manifest::read(path);
        self.entries
            .insert(id.to_owned(), (directory, manifest, value.clone()));
        value
    }
    pub fn retain(&mut self, present: &BTreeSet<String>) {
        self.entries.retain(|id, _| present.contains(id));
    }
}
