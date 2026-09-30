//! `RunLengthDecode` (PDF 1.7, 7.4.5).

// Direct indexing is used throughout this file: every index is either masked to a
// table width or produced by a loop bounded by the length of the same buffer, so a
// checked access would add noise without adding safety. The surrounding code is
// still panic-free: see docs/PDF-QUIRKS.md for the callers' tolerance rules.
#![allow(clippy::indexing_slicing)]

use crate::Partial;

/// A copy run is 1..=128 bytes; a repeat run is 2..=128 copies of the next byte.
const MAX_RUN: usize = 128;

#[must_use]
pub fn run_length_decode(input: &[u8]) -> Partial {
    let mut out = Vec::with_capacity(input.len() * 2);
    let mut i = 0usize;
    let mut complete = false;
    while i < input.len() {
        let Some(&len) = input.get(i) else { break };
        i += 1;
        match len {
            0..=127 => {
                let n = usize::from(len) + 1;
                let end = i.saturating_add(n);
                let Some(chunk) = input.get(i..end.min(input.len())) else {
                    out.extend_from_slice(input.get(i..).unwrap_or(&[]));
                    break;
                };
                out.extend_from_slice(chunk);
                i = end;
            }
            128 => {
                // EOD
                complete = true;
                break;
            }
            _ => {
                // 129..=255: repeat the next byte (257 - len) times.
                let Some(&b) = input.get(i) else { break };
                i += 1;
                let n = 257 - usize::from(len);
                out.extend(std::iter::repeat_n(b, n));
            }
        }
    }
    let note = if complete {
        None
    } else {
        Some("no EOD marker (128)".into())
    };
    Partial {
        data: out,
        complete,
        note,
    }
}

/// `RunLengthEncode`: used only for round-trip tests and small artifacts.
#[must_use]
pub fn run_length_encode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < input.len() {
        // Find a run of at least 2 identical bytes.
        let mut run = 1usize;
        while i + run < input.len() && run < MAX_RUN && input[i + run] == input[i] {
            run += 1;
        }
        if run >= 2 {
            out.push(u8::try_from(257 - run).unwrap_or(129));
            if let Some(b) = input.get(i) {
                out.push(*b);
            }
            i += run;
        } else {
            // Gather literals until a run of 3 appears or 128 bytes accumulate.
            let start = i;
            while i < input.len() && i - start < MAX_RUN {
                if i + 2 < input.len() && input[i] == input[i + 1] && input[i] == input[i + 2] {
                    break;
                }
                i += 1;
            }
            let n = i - start;
            out.push(u8::try_from(n - 1).unwrap_or(0));
            if let Some(chunk) = input.get(start..i) {
                out.extend_from_slice(chunk);
            }
        }
    }
    out.push(128);
    out
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn literals_and_repeats() {
        // 2 literal bytes, 3 copies of 'A', EOD
        let p = run_length_decode(&[1, b'h', b'i', 254, b'A', 128]);
        assert!(p.complete);
        assert_eq!(p.data, b"hiAAA");
    }

    #[test]
    fn truncated() {
        let p = run_length_decode(&[5, b'a', b'b']);
        assert!(!p.complete);
        assert_eq!(p.data, b"ab");
    }

    #[test]
    fn round_trip() {
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"a".to_vec(),
            b"aaaaaaaaaaaaaaaaaaaa".to_vec(),
            b"abcabcabcabc".to_vec(),
            (0..=255u8).collect(),
        ];
        for c in cases {
            let enc = run_length_encode(&c);
            let dec = run_length_decode(&enc);
            assert!(dec.complete);
            assert_eq!(dec.data, c);
        }
    }
}
