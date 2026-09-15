//! Serialized message bytes of one projected file, kept next to its events
//! so a hot read splices cached JSON instead of cloning and re-serializing
//! every message (docs/read-model.md "视图字节缓存").
//!
//! The unit is one `Vec<Event>` (a parsed leaf, or the inherited prefix of a
//! view): every non-status message's exact `serde_json::to_vec` output is
//! stored back to back, comma-separated, with per-message offsets, so any
//! contiguous range of non-status positions is one `memcpy`. Status events
//! are kept separately (they never enter a `messages` array but do enter the
//! semantic digest). Events whose projection is not a pure function of the
//! event — typed images (descriptor registration is a per-request side
//! effect, and more than sixteen mint a grant) or text-discovered file
//! references (fresh file tokens) — are flagged `special` and projected per
//! request exactly as before; their message bytes are still cached for the
//! page budget and the digest.
//!
//! Bytes are optional: a transient projection (search) computes the same
//! digest and accounting in one streaming pass and retains nothing.

use serde_json::Value;
use sha1::{Digest, Sha1};

use super::Event;

/// One projected event as the encoder saw it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Entry {
    pub end: u64,
    /// Non-status position (index into the `messages` array of a full read),
    /// or the status slot.
    pub slot: usize,
    pub status: bool,
    /// Typed images or text-discovered references: projected per request.
    pub special: bool,
    /// Text-discovered image references (`media::discover`), for the page budget.
    pub discovered: usize,
    /// `serde_json::to_vec(&event.message).len()`, without any media field.
    pub message_len: usize,
}

#[derive(Debug, Default)]
pub(crate) struct EncodedEvents {
    entries: Vec<Entry>,
    /// Entry index of each non-status position.
    messages: Vec<usize>,
    /// Non-status messages, comma-separated, when retained.
    bytes: Option<Vec<u8>>,
    /// Start of each non-status message in `bytes`, plus a sentinel of
    /// `bytes.len() + 1` so `starts[i + 1] - 1` is always the end of `i`.
    starts: Vec<usize>,
    /// Status messages, when retained (digest input only).
    statuses: Option<Vec<Vec<u8>>>,
    /// Σ `message_len`: the serialized-message accounting of the view LRU.
    total_len: usize,
    /// `projection_digest` of every event (the committed semantic digest).
    digest: String,
    /// Messages whose bytes were copied from the previous parse of the same
    /// file instead of being re-serialized (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    reused: usize,
}

/// Order-sensitive structural equality: two values serialize identically iff
/// this holds (`serde_json::Value::eq` ignores object key order and folds
/// `-0.0`/`0.0`, neither of which is byte-identical output).
pub(crate) fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|((ka, va), (kb, vb))| ka == kb && same_value(va, vb))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| same_value(x, y))
        }
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) if a.is_f64() || b.is_f64() => {
                a.is_f64() == b.is_f64() && x.to_bits() == y.to_bits()
            }
            _ => a == b,
        },
        _ => a == b,
    }
}

#[cfg(test)]
thread_local! {
    /// Non-empty retaining encodings built on this thread (tests: one fill
    /// per file, hot reads none). Builds run on the opening thread, under
    /// the view lock.
    pub(crate) static RETAINED_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The previous encoding of the same file, whose unchanged prefix messages
/// may be copied instead of re-serialized.
pub(crate) struct Previous<'a> {
    pub events: &'a [Event],
    pub encoded: &'a EncodedEvents,
}

impl EncodedEvents {
    /// Encode `events` (retaining the bytes when `retain`). With `previous`,
    /// a message whose event is structurally identical (same `end`, same
    /// message tree, no typed media) to the one at the same index of the
    /// previous parse copies its cached bytes: an append re-serializes only
    /// the new tail and any earlier message the projection amended.
    pub(crate) fn build<'a>(
        events: impl IntoIterator<Item = &'a Event>,
        retain: bool,
        previous: Option<Previous<'_>>,
    ) -> Result<Self, serde_json::Error> {
        let mut entries = Vec::new();
        let mut messages = Vec::new();
        let mut bytes = Vec::new();
        let mut starts = Vec::new();
        let mut statuses = Vec::new();
        let mut scratch = Vec::new();
        let mut total_len = 0usize;
        let mut digest = Sha1::new();
        let mut reused = 0usize;
        let previous = previous.filter(|previous| previous.encoded.bytes.is_some());
        for (index, event) in events.into_iter().enumerate() {
            let status = event.message["role"] == "status";
            let reusable = previous.as_ref().and_then(|previous| {
                let old = previous.events.get(index)?;
                let entry = previous.encoded.entries.get(index)?;
                (!status
                    && !entry.status
                    && !entry.special
                    && event.media.is_empty()
                    && old.end == event.end
                    && same_value(&old.message, &event.message))
                .then_some(())?;
                Some((previous.encoded.message(entry.slot)?, *entry))
            });
            let (encoded, entry) = match reusable {
                Some((cached, entry)) => {
                    reused += 1;
                    (cached, entry)
                }
                None => {
                    scratch.clear();
                    serde_json::to_writer(&mut scratch, &event.message)?;
                    let discovered = if status {
                        0
                    } else {
                        crate::media::discover(&event.message).len()
                    };
                    (
                        scratch.as_slice(),
                        Entry {
                            end: event.end,
                            slot: 0,
                            status,
                            special: !event.media.is_empty() || discovered > 0,
                            discovered,
                            message_len: scratch.len(),
                        },
                    )
                }
            };
            let mut entry = entry;
            digest.update(event.end.to_le_bytes());
            digest.update((encoded.len() as u64).to_le_bytes());
            digest.update(encoded);
            for image in &event.media {
                digest.update(image.semantic_key().as_bytes());
            }
            total_len = total_len.saturating_add(encoded.len());
            if status {
                entry.slot = statuses.len();
                if retain {
                    statuses.push(encoded.to_vec());
                } else {
                    statuses.push(Vec::new());
                }
            } else {
                entry.slot = messages.len();
                messages.push(index);
                if retain {
                    if !starts.is_empty() {
                        bytes.push(b',');
                    }
                    starts.push(bytes.len());
                    bytes.extend_from_slice(encoded);
                }
            }
            entries.push(entry);
        }
        if retain {
            starts.push(bytes.len() + 1);
            #[cfg(test)]
            if !entries.is_empty() {
                RETAINED_BUILDS.with(|builds| builds.set(builds.get() + 1));
            }
        }
        Ok(Self {
            entries,
            messages,
            bytes: retain.then_some(bytes),
            starts,
            statuses: retain.then_some(statuses),
            total_len,
            digest: format!("{:x}", digest.finalize()),
            reused,
        })
    }

    /// The committed semantic digest (`projection_digest(events, committed)`).
    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }

    /// Σ serialized message bytes (the view LRU's accounting unit).
    pub(crate) fn total_len(&self) -> usize {
        self.total_len
    }

    /// Bytes physically retained by this encoding.
    #[cfg(test)]
    pub(crate) fn retained(&self) -> usize {
        self.bytes.as_ref().map_or(0, Vec::len)
            + self
                .statuses
                .as_ref()
                .map_or(0, |statuses| statuses.iter().map(Vec::len).sum())
    }

    /// Messages copied from the previous parse instead of re-serialized.
    #[cfg(test)]
    pub(crate) fn reused(&self) -> usize {
        self.reused
    }

    /// Non-status messages.
    pub(crate) fn message_count(&self) -> usize {
        self.messages.len()
    }

    #[cfg(test)]
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry of non-status position `position`.
    pub(crate) fn message_entry(&self, position: usize) -> Option<&Entry> {
        self.entries.get(*self.messages.get(position)?)
    }

    /// The index in the encoded event list of non-status position `position`.
    pub(crate) fn event_index(&self, position: usize) -> Option<usize> {
        self.messages.get(position).copied()
    }

    /// The exact serialized message at non-status position `position`
    /// (`None` when bytes are not retained).
    pub(crate) fn message(&self, position: usize) -> Option<&[u8]> {
        let bytes = self.bytes.as_ref()?;
        let start = *self.starts.get(position)?;
        let end = self.starts[position + 1] - 1;
        Some(&bytes[start..end])
    }

    /// The comma-joined messages of non-status positions `range`
    /// (`None` when bytes are not retained or the range is empty).
    pub(crate) fn messages(&self, range: std::ops::Range<usize>) -> Option<&[u8]> {
        if range.is_empty() || range.end > self.messages.len() {
            return None;
        }
        let bytes = self.bytes.as_ref()?;
        Some(&bytes[self.starts[range.start]..self.starts[range.end] - 1])
    }

    /// `projection_digest(events, end)` from the cached bytes (`None` when
    /// bytes are not retained; the caller re-serializes then).
    pub(crate) fn digest_upto(&self, events: &[Event], end: u64) -> Option<String> {
        let statuses = self.statuses.as_ref()?;
        self.bytes.as_ref()?;
        let mut digest = Sha1::new();
        for (entry, event) in self.entries.iter().zip(events) {
            if entry.end > end {
                continue;
            }
            digest.update(entry.end.to_le_bytes());
            digest.update((entry.message_len as u64).to_le_bytes());
            if entry.status {
                digest.update(&statuses[entry.slot]);
            } else {
                digest.update(self.message(entry.slot)?);
            }
            for image in &event.media {
                digest.update(image.semantic_key().as_bytes());
            }
        }
        Some(format!("{:x}", digest.finalize()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(end: u64, message: Value) -> Event {
        Event {
            end,
            message,
            media: Vec::new(),
        }
    }

    #[test]
    fn bytes_are_exact_serializations_and_ranges_are_contiguous() {
        let events = vec![
            event(1, json!({"role":"user","text":"a","counted":true})),
            event(
                2,
                json!({"role":"status","text":"working","state":"working"}),
            ),
            event(
                3,
                json!({"role":"assistant","text":"b\n\"c\"","counted":false}),
            ),
            event(4, json!({"role":"assistant","text":"![x](/tmp/x.png)"})),
        ];
        let encoded = EncodedEvents::build(&events, true, None).unwrap();
        assert_eq!(encoded.message_count(), 3);
        assert_eq!(encoded.entries().len(), 4);
        for (position, index) in [(0usize, 0usize), (1, 2), (2, 3)] {
            assert_eq!(
                encoded.message(position).unwrap(),
                serde_json::to_vec(&events[index].message).unwrap()
            );
        }
        let joined = encoded.messages(0..3).unwrap();
        let expected = [0usize, 2, 3]
            .iter()
            .map(|index| serde_json::to_string(&events[*index].message).unwrap())
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(joined, expected.as_bytes());
        assert_eq!(encoded.messages(1..2).unwrap(), encoded.message(1).unwrap());
        assert!(encoded.messages(0..0).is_none());
        assert!(encoded.messages(2..4).is_none());
        let entries = encoded.entries();
        assert!(entries[1].status && !entries[0].status);
        assert!(entries[3].special && entries[3].discovered == 1);
        assert!(!entries[2].special);
        assert_eq!(
            encoded.total_len(),
            events
                .iter()
                .map(|event| serde_json::to_vec(&event.message).unwrap().len())
                .sum::<usize>()
        );
        assert_eq!(
            encoded.digest(),
            super::super::projection_digest(&events, 4)
        );
        assert_eq!(
            encoded.digest_upto(&events, 2).unwrap(),
            super::super::projection_digest(&events, 2)
        );
        let transient = EncodedEvents::build(&events, false, None).unwrap();
        assert_eq!(transient.digest(), encoded.digest());
        assert_eq!(transient.total_len(), encoded.total_len());
        assert_eq!(transient.retained(), 0);
        assert!(transient.message(0).is_none() && transient.digest_upto(&events, 2).is_none());
    }

    #[test]
    fn append_reuses_unchanged_prefix_bytes_and_re_encodes_amended_messages() {
        let old = vec![
            event(1, json!({"role":"user","text":"a"})),
            event(2, json!({"role":"assistant","text":"b","aborted":false})),
            event(3, json!({"role":"status","text":"idle"})),
        ];
        let before = EncodedEvents::build(&old, true, None).unwrap();
        let new = vec![
            event(1, json!({"role":"user","text":"a"})),
            event(2, json!({"role":"assistant","text":"b","aborted":true})),
            event(3, json!({"role":"status","text":"idle"})),
            event(4, json!({"role":"user","text":"c"})),
        ];
        let extended = EncodedEvents::build(
            &new,
            true,
            Some(Previous {
                events: &old,
                encoded: &before,
            }),
        )
        .unwrap();
        let cold = EncodedEvents::build(&new, true, None).unwrap();
        assert_eq!(extended.reused(), 1);
        assert_eq!(extended.bytes, cold.bytes);
        assert_eq!(extended.starts, cold.starts);
        assert_eq!(extended.digest(), cold.digest());
        assert_eq!(extended.total_len(), cold.total_len());
        // A shorter (truncated) file reuses nothing beyond its own length.
        let short = EncodedEvents::build(
            &new[..1],
            true,
            Some(Previous {
                events: &new,
                encoded: &cold,
            }),
        )
        .unwrap();
        assert_eq!(short.reused(), 1);
        assert_eq!(short.message_count(), 1);
    }

    #[test]
    fn same_value_is_key_order_and_float_sign_sensitive() {
        assert!(same_value(
            &json!({"a":1,"b":[2,3]}),
            &json!({"a":1,"b":[2,3]})
        ));
        assert!(!same_value(&json!({"a":1,"b":2}), &json!({"b":2,"a":1})));
        assert!(!same_value(&json!(-0.0), &json!(0.0)));
        assert!(!same_value(&json!(1), &json!(1.0)));
        assert!(same_value(&json!(1.5), &json!(1.5)));
        assert!(!same_value(&json!(null), &json!(false)));
    }
}
