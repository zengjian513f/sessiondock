// Grid wire helpers: newline-delimited JSON, resize encoding, grapheme splits.
// Pure: no DOM. Complete lines are decoded after splitting on byte 0x0A so a
// UTF-8 character split across frames stays in the byte tail, not a string.

const SEGMENT_LIMIT = 256;
const segmentCache = new Map();
const graphemeSegmenter = typeof Intl === 'object' && Intl && typeof Intl.Segmenter === 'function'
  ? new Intl.Segmenter(undefined, {granularity: 'grapheme'})
  : null;

function toBytes(bytes) {
  if (bytes instanceof Uint8Array) return bytes;
  if (bytes instanceof ArrayBuffer) return new Uint8Array(bytes);
  if (ArrayBuffer.isView(bytes)) {
    return new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }
  return new Uint8Array(0);
}

export class LineDecoder {
  constructor() {
    this.dropped = 0;
    this._tail = new Uint8Array(0);
    this._decoder = new TextDecoder('utf-8');
  }

  push(bytes) {
    const chunk = toBytes(bytes);
    const merged = new Uint8Array(this._tail.length + chunk.length);
    merged.set(this._tail, 0);
    merged.set(chunk, this._tail.length);
    const messages = [];
    let offset = 0;
    for (let i = 0; i < merged.length; i++) {
      if (merged[i] !== 0x0A) continue;
      const line = merged.subarray(offset, i);
      offset = i + 1;
      // Complete line: 0x0A is ASCII, so the slice is a whole UTF-8 sequence.
      const text = this._decoder.decode(line, {stream: true});
      try {
        messages.push(JSON.parse(text));
      } catch {
        this.dropped++;
      }
    }
    this._tail = offset >= merged.length ? new Uint8Array(0) : merged.slice(offset);
    return messages;
  }
}

export function encodeResize(cols, rows) {
  return JSON.stringify({t: 'resize', cols, rows});
}

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
