//! Incremental RFC 9112 chunked transfer decoder.
//!
//! Fed arbitrary byte splits: partial size lines and split CRLFs are carried
//! in explicit state, so any packetization of the same stream decodes to the
//! same payload. This matters because the session guarantees split-invariant
//! behavior to its caller.

/// Hard bound on one size/trailer line. A compliant caster never gets close
/// (a 64-bit hex size is 16 chars); the cap keeps a hostile or broken peer
/// from growing the line accumulator without bound.
const MAX_LINE: usize = 4096;

#[derive(Debug, Default)]
pub(crate) struct Decoder {
    state: State,
    /// Partial size/trailer line carried across feeds.
    line: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    SizeLine,
    Data {
        left: u64,
    },
    CrlfAfterData {
        cr_seen: bool,
    },
    Trailers,
    Done,
}

impl Decoder {
    /// The zero-size chunk and its trailers have been consumed: the body is
    /// complete and no further payload will ever be produced.
    pub(crate) fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// Decode `input`, appending payload bytes to `out`. Err carries a
    /// human-readable detail on malformed framing; the decoder is then dead
    /// and must not be fed again. Bytes arriving after the terminal chunk are
    /// ignored (liberal: some casters keep pushing junk after the final CRLF,
    /// and the session closes at Done anyway).
    pub(crate) fn feed(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<(), String> {
        let mut i = 0;
        while i < input.len() {
            match self.state {
                State::SizeLine => match self.take_line(input, &mut i)? {
                    None => return Ok(()),
                    Some(line) => {
                        let size = parse_size_line(&line)?;
                        self.state = if size == 0 {
                            State::Trailers
                        } else {
                            State::Data { left: size }
                        };
                    }
                },
                State::Data { left } => {
                    let take = left.min((input.len() - i) as u64) as usize;
                    out.extend_from_slice(&input[i..i + take]);
                    i += take;
                    let left = left - take as u64;
                    self.state = if left == 0 {
                        State::CrlfAfterData { cr_seen: false }
                    } else {
                        State::Data { left }
                    };
                }
                State::CrlfAfterData { cr_seen } => {
                    let b = input[i];
                    i += 1;
                    match (cr_seen, b) {
                        (false, b'\r') => self.state = State::CrlfAfterData { cr_seen: true },
                        // Bare LF tolerated, like everywhere else in this crate.
                        (_, b'\n') => self.state = State::SizeLine,
                        _ => {
                            return Err(format!(
                                "expected CRLF after chunk data, got byte 0x{b:02X}"
                            ));
                        }
                    }
                }
                State::Trailers => match self.take_line(input, &mut i)? {
                    None => return Ok(()),
                    Some(line) => {
                        let blank = match line.split_last() {
                            Some((b'\r', rest)) => rest.is_empty(),
                            None => true,
                            Some(_) => false,
                        };
                        // Non-blank trailer fields are ignored: nothing in
                        // NTRIP is carried there.
                        if blank {
                            self.state = State::Done;
                        }
                    }
                },
                State::Done => return Ok(()),
            }
        }
        Ok(())
    }

    /// Accumulate bytes up to and excluding LF into `self.line`; returns the
    /// completed line (taken out) or None when more input is needed.
    fn take_line(&mut self, input: &[u8], i: &mut usize) -> Result<Option<Vec<u8>>, String> {
        match input[*i..].iter().position(|&b| b == b'\n') {
            None => {
                self.line.extend_from_slice(&input[*i..]);
                *i = input.len();
                if self.line.len() > MAX_LINE {
                    return Err("chunk framing line exceeds 4096 bytes".to_string());
                }
                Ok(None)
            }
            Some(rel) => {
                self.line.extend_from_slice(&input[*i..*i + rel]);
                *i += rel + 1;
                if self.line.len() > MAX_LINE {
                    return Err("chunk framing line exceeds 4096 bytes".to_string());
                }
                Ok(Some(std::mem::take(&mut self.line)))
            }
        }
    }
}

fn parse_size_line(line: &[u8]) -> Result<u64, String> {
    let mut l = line;
    if l.last() == Some(&b'\r') {
        l = &l[..l.len() - 1];
    }
    // Chunk extensions (";name=value") are legal and meaningless to us.
    if let Some(p) = l.iter().position(|&b| b == b';') {
        l = &l[..p];
    }
    let hex = l.trim_ascii();
    if hex.is_empty() {
        return Err("empty chunk size line".to_string());
    }
    let mut size: u64 = 0;
    for &b in hex {
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return Err(format!("invalid chunk size character 0x{b:02X}")),
        };
        size = size
            .checked_mul(16)
            .and_then(|s| s.checked_add(u64::from(digit)))
            .ok_or_else(|| "chunk size overflows u64".to_string())?;
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::Decoder;

    const RFC_EXAMPLE: &[u8] = b"4\r\nWiki\r\n7\r\npedia i\r\nB\r\nn \r\nchunks.\r\n0\r\n\r\n";
    const RFC_DECODED: &[u8] = b"Wikipedia in \r\nchunks.";

    #[test]
    fn rfc_example_whole_buffer() {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        d.feed(RFC_EXAMPLE, &mut out).unwrap();
        assert_eq!(out, RFC_DECODED);
        assert!(d.is_done());
    }

    #[test]
    fn rfc_example_byte_at_a_time() {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        for &b in RFC_EXAMPLE {
            d.feed(&[b], &mut out).unwrap();
        }
        assert_eq!(out, RFC_DECODED);
        assert!(d.is_done());
    }

    #[test]
    fn size_line_split_across_feeds() {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        d.feed(b"1", &mut out).unwrap();
        d.feed(b"0\r", &mut out).unwrap();
        d.feed(b"\n", &mut out).unwrap();
        d.feed(b"0123456789ABCDEF\r\n0\r\n\r\n", &mut out).unwrap();
        assert_eq!(out, b"0123456789ABCDEF");
        assert!(d.is_done());
    }

    #[test]
    fn hex_case_and_extensions() {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        d.feed(
            b"A;name=value\r\nabcdefghij\r\nb\r\nABCDEFGHIJK\r\n0\r\n\r\n",
            &mut out,
        )
        .unwrap();
        assert_eq!(out, b"abcdefghijABCDEFGHIJK");
        assert!(d.is_done());
    }

    #[test]
    fn trailers_are_consumed() {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        d.feed(
            b"3\r\nxyz\r\n0\r\nX-Checksum: abc\r\nX-Other: 1\r\n\r\n",
            &mut out,
        )
        .unwrap();
        assert_eq!(out, b"xyz");
        assert!(d.is_done());
    }

    #[test]
    fn bare_lf_tolerated() {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        d.feed(b"3\nabc\n0\n\n", &mut out).unwrap();
        assert_eq!(out, b"abc");
        assert!(d.is_done());
    }

    #[test]
    fn bytes_after_done_are_ignored() {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        d.feed(b"1\r\nZ\r\n0\r\n\r\njunk after end", &mut out)
            .unwrap();
        assert_eq!(out, b"Z");
        assert!(d.is_done());
        d.feed(b"more junk", &mut out).unwrap();
        assert_eq!(out, b"Z");
    }

    #[test]
    fn malformed_size_character() {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        let err = d.feed(b"xyz\r\n", &mut out).unwrap_err();
        assert!(
            err.contains("invalid chunk size"),
            "unexpected detail: {err}"
        );
    }

    #[test]
    fn missing_crlf_after_data() {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        let err = d.feed(b"3\r\nabcXX", &mut out).unwrap_err();
        assert!(err.contains("expected CRLF"), "unexpected detail: {err}");
        assert_eq!(out, b"abc");
    }

    #[test]
    fn size_overflow_rejected() {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        let err = d.feed(b"FFFFFFFFFFFFFFFFF\r\n", &mut out).unwrap_err();
        assert!(err.contains("overflow"), "unexpected detail: {err}");
    }

    #[test]
    fn empty_size_line_rejected() {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        assert!(d.feed(b"\r\n", &mut out).is_err());
    }
}
