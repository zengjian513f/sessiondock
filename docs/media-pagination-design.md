# Media continuation and history pagination

History pages group events for transport, while media continuation exposes all
typed images belonging to a selected message. The first display slice is shown
with the message and subsequent slices use opaque cursors tied to the same view
snapshot.

Pagination targets control response grouping; they do not reject a valid
record, session, giant first event or message with many images. A cursor carries
view identity, agent, event key and offset so it cannot be replayed against a
different snapshot. File changes invalidate the view and its outstanding
cursors.

Media descriptors remain lazy. Embedded and file-backed sources are registered
without decoding all images into the history response, and each GET enforces
Python's 32 MiB decoded-image limit. Cache eviction may discard old materialized
blobs or descriptors, but cache pressure does not impose an input-count or
batch-byte admission rule. Concurrent GETs may all complete; borrowed blobs and
temporary retention overruns do not produce a busy response.

Nested stringified tool outputs are decoded recursively across every candidate
and source range. Every provider-recognized image is eligible for continuation;
display grouping does not truncate the underlying event media.
