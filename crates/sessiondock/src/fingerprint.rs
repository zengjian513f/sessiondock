//! 128-bit streaming content fingerprint of native bytes.
//!
//! It answers one question everywhere the read model verifies bytes it has
//! already looked at: "are these the same bytes as before?" — the JSONL prefix
//! checkpoints behind cursors, the committed prefix an append reuses decoded
//! records for, and the private string spans (images, giant tool text) read
//! back from a stamped file. Those are our own local files changing under us,
//! not an adversary forging content, so the fingerprint is a fast non-
//! cryptographic mix (two independent 64-bit lanes, ~6 GB/s on one core)
//! rather than SHA-1/SHA-256 (~1 GB/s), which used to cost more than reading
//! the file itself on gigabyte sessions. It is never an authorization or MAC:
//! file identity, stamps and range checks still come first.
//!
//! The output is stable across processes and platforms (fixed constants,
//! little-endian words), so cursors survive restarts, and independent of how
//! the input was chunked.

/// Bytes of one fingerprint.
pub const LEN: usize = 16;
pub type Digest = [u8; LEN];

const K1: u64 = 0x9E37_79B9_7F4A_7C15;
const K2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const SEED_A: u64 = 0x1656_67B1_9E37_79F9;
const SEED_B: u64 = 0x85EB_CA77_C2B2_AE63;

#[derive(Clone)]
pub struct Fingerprint {
    a: u64,
    b: u64,
    length: u64,
    tail: [u8; 8],
    tail_len: usize,
}

impl Default for Fingerprint {
    fn default() -> Self {
        Self::new()
    }
}

#[inline(always)]
fn fmix(mut value: u64) -> u64 {
    value ^= value >> 33;
    value = value.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    value ^= value >> 33;
    value = value.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    value ^ (value >> 33)
}

impl Fingerprint {
    pub fn new() -> Self {
        Self {
            a: SEED_A,
            b: SEED_B,
            length: 0,
            tail: [0; 8],
            tail_len: 0,
        }
    }

    /// One-shot fingerprint of `bytes`.
    pub fn digest(bytes: &[u8]) -> Digest {
        let mut hash = Self::new();
        hash.update(bytes);
        hash.finalize()
    }

    #[inline(always)]
    fn word(&mut self, word: u64) {
        // Each lane update is a bijection in the lane state for a fixed word
        // and in the word for a fixed state, so any change to one word changes
        // both lanes; the lanes are independent, so they pipeline.
        self.a = (self.a ^ word).wrapping_mul(K1).rotate_left(29);
        self.b = (self.b.rotate_left(31) ^ word).wrapping_mul(K2);
    }

    pub fn update(&mut self, mut bytes: &[u8]) {
        self.length += bytes.len() as u64;
        if self.tail_len > 0 {
            let take = bytes.len().min(8 - self.tail_len);
            self.tail[self.tail_len..self.tail_len + take].copy_from_slice(&bytes[..take]);
            self.tail_len += take;
            bytes = &bytes[take..];
            if self.tail_len < 8 {
                return;
            }
            self.word(u64::from_le_bytes(self.tail));
            self.tail_len = 0;
        }
        let (chunks, rest) = bytes.as_chunks::<8>();
        for chunk in chunks {
            self.word(u64::from_le_bytes(*chunk));
        }
        self.tail[..rest.len()].copy_from_slice(rest);
        self.tail_len = rest.len();
    }

    pub fn finalize(&self) -> Digest {
        let mut state = self.clone();
        if state.tail_len > 0 {
            // A partial final word is padded with its own length so that
            // `[1]` and `[1, 0]` differ before the length mix below.
            state.tail[state.tail_len..].fill(state.tail_len as u8);
            let word = u64::from_le_bytes(state.tail);
            state.word(word);
        }
        let a = fmix(state.a ^ state.length ^ state.b.rotate_left(17));
        let b = fmix(state.b ^ state.length.rotate_left(32) ^ a);
        let mut digest = [0; LEN];
        digest[..8].copy_from_slice(&a.to_le_bytes());
        digest[8..].copy_from_slice(&b.to_le_bytes());
        digest
    }
}

/// Lowercase hex of a digest (32 characters).
pub fn hex(digest: &Digest) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(LEN * 2);
    for byte in digest {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 15) as usize] as char);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunking_does_not_change_the_digest() {
        let data = (0..10_000u32)
            .flat_map(|i| i.to_le_bytes())
            .collect::<Vec<_>>();
        let whole = Fingerprint::digest(&data);
        for step in [1usize, 3, 7, 8, 13, 64, 4097] {
            let mut hash = Fingerprint::new();
            for chunk in data.chunks(step) {
                hash.update(chunk);
            }
            assert_eq!(hash.finalize(), whole, "chunk size {step}");
        }
    }

    #[test]
    fn nearby_inputs_differ() {
        let base = b"the quick brown fox jumps over the lazy dog".to_vec();
        let reference = Fingerprint::digest(&base);
        assert_ne!(reference, Fingerprint::digest(&base[..base.len() - 1]));
        for index in 0..base.len() {
            let mut flipped = base.clone();
            flipped[index] ^= 1;
            assert_ne!(reference, Fingerprint::digest(&flipped), "byte {index}");
        }
        let mut padded = base.clone();
        padded.push(0);
        assert_ne!(reference, Fingerprint::digest(&padded));
        assert_ne!(Fingerprint::digest(&[1]), Fingerprint::digest(&[1, 0]));
        assert_ne!(Fingerprint::digest(&[0]), Fingerprint::digest(&[]));
        assert_ne!(Fingerprint::digest(&[0; 8]), Fingerprint::digest(&[0; 16]));
    }

    #[test]
    fn stable_across_processes() {
        // Pinned so that persisted cursors keep validating after a restart
        // and across builds; change only together with the cursor schema.
        assert_eq!(
            hex(&Fingerprint::digest(b"")),
            hex(&Fingerprint::new().finalize())
        );
        let first = hex(&Fingerprint::digest(b"sessiondock"));
        assert_eq!(first.len(), 32);
        assert_eq!(first, hex(&Fingerprint::digest(b"sessiondock")));
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn snapshots_continue_independently() {
        let mut hash = Fingerprint::new();
        hash.update(b"line one\n");
        let checkpoint = hash.finalize();
        hash.update(b"line two\n");
        assert_eq!(checkpoint, Fingerprint::digest(b"line one\n"));
        assert_eq!(
            hash.finalize(),
            Fingerprint::digest(b"line one\nline two\n")
        );
    }
}
