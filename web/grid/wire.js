// Cached grapheme segmentation for terminal cell materialization.

const SEGMENT_LIMIT = 256;
const segmentCache = new Map();
const graphemeSegmenter = typeof Intl === 'object' && Intl && typeof Intl.Segmenter === 'function'
  ? new Intl.Segmenter(undefined, {granularity: 'grapheme'})
  : null;

export function segmentText(text) {
  const key = text == null ? '' : String(text);
  const cached = segmentCache.get(key);
  if (cached) {
    segmentCache.delete(key);
    segmentCache.set(key, cached);
    return cached;
  }
  let segments;
  if (graphemeSegmenter) {
    segments = [];
    for (const part of graphemeSegmenter.segment(key)) segments.push(part.segment);
  } else {
    segments = Array.from(key);
  }
  if (segmentCache.size >= SEGMENT_LIMIT) {
    segmentCache.delete(segmentCache.keys().next().value);
  }
  segmentCache.set(key, segments);
  return segments;
}
