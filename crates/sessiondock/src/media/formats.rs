//! Image decoding belongs to the client. SessionDock does not add container,
//! dimension, pixel, frame-count or codec-profile admission rules beyond
//! Python's decoded 32 MiB item limit.

use super::MediaError;

pub(super) fn inspect(_mime: &str, _bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    Err(MediaError::Unsupported)
}
