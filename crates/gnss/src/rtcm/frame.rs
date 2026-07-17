//! RTCM 3.x stream deframing.
//!
//! Wire format: 0xD3, 6 reserved zero bits + 10-bit payload length, payload,
//! 24-bit CRC-24Q over everything before it.
//!
//! Resilience contract: on a CRC failure the scanner advances exactly ONE
//! byte and rescans. Skipping a whole frame on a failed check would let a
//! corrupted length byte swallow the next valid frame and desync the stream
//! indefinitely; single-byte advance guarantees resync at the next intact
//! frame. Every consumed byte is accounted for exactly once: either inside
//! an emitted frame or in `garbage_bytes` (the one-byte advance after a CRC
//! failure counts as garbage; `crc_failures` counts events, not bytes).

use super::crc24q::crc24q;

/// One deframing outcome. `Frame` payloads borrow the deframer's buffer and
/// are only valid during the callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameEvent<'a> {
    /// A CRC-clean frame. `msg_type` is the first 12 bits of the payload
    /// (0 if the payload is shorter than 2 bytes, which real messages never
    /// are).
    Frame { msg_type: u16, payload: &'a [u8] },
    /// A candidate frame failed its CRC; one byte was consumed.
    CrcError,
    /// This many non-frame bytes were discarded (counted in `garbage_bytes`).
    Garbage(usize),
}

/// Incremental deframer. Feed arbitrary chunks; frames are emitted as soon
/// as they complete. Held state is bounded by the maximum frame size (1029
/// bytes) plus the current input chunk: consumed prefixes are drained.
#[derive(Debug, Default)]
pub struct Deframer {
    /// Frames rejected by CRC since construction.
    pub crc_failures: u64,
    /// Bytes discarded as garbage since construction.
    pub garbage_bytes: u64,
    buf: Vec<u8>,
}

impl Deframer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume `bytes`, invoking `on_event` for every frame, CRC failure,
    /// and garbage run found. Incomplete trailing data is held for the next
    /// call.
    pub fn feed(&mut self, bytes: &[u8], on_event: &mut dyn FnMut(FrameEvent)) {
        self.buf.extend_from_slice(bytes);
        let mut pos = 0usize; // start of unconsumed data
        let mut run = 0usize; // garbage bytes pending in [pos - run, pos)
        while let Some(&b) = self.buf.get(pos) {
            if b != 0xD3 {
                run += 1;
                pos += 1;
                continue;
            }
            let Some(&after) = self.buf.get(pos + 1) else {
                break; // lone trailing 0xD3: could be a sync, wait for more
            };
            if after & 0xFC != 0 {
                // False sync: only the 0xD3 is condemned; the next byte may
                // itself start a real frame.
                run += 1;
                pos += 1;
                continue;
            }
            // Candidate frame. Garbage before it is now definite - flush so
            // the prefix can be drained even if the frame stays incomplete.
            if run > 0 {
                self.garbage_bytes += run as u64;
                on_event(FrameEvent::Garbage(run));
                run = 0;
            }
            if pos + 3 > self.buf.len() {
                break; // length byte not yet received
            }
            let len = usize::from(self.buf[pos + 1] & 0x03) << 8 | usize::from(self.buf[pos + 2]);
            let total = 3 + len + 3;
            if pos + total > self.buf.len() {
                break; // frame incomplete, wait for more input
            }
            let body = &self.buf[pos..pos + 3 + len];
            let stated = u32::from(self.buf[pos + 3 + len]) << 16
                | u32::from(self.buf[pos + 4 + len]) << 8
                | u32::from(self.buf[pos + 5 + len]);
            if crc24q(body) == stated {
                let payload = &self.buf[pos + 3..pos + 3 + len];
                let msg_type = if len >= 2 {
                    u16::from(payload[0]) << 4 | u16::from(payload[1] >> 4)
                } else {
                    0
                };
                on_event(FrameEvent::Frame { msg_type, payload });
                pos += total;
            } else {
                self.crc_failures += 1;
                on_event(FrameEvent::CrcError);
                // Advance exactly one byte; the rejected sync byte joins the
                // garbage tally so the byte accounting stays complete.
                run += 1;
                pos += 1;
            }
        }
        if run > 0 {
            self.garbage_bytes += run as u64;
            on_event(FrameEvent::Garbage(run));
        }
        // Drain the consumed prefix so memory stays bounded on any input.
        self.buf.drain(..pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical 1005 example frame from the RTCM 3 documentation.
    const RTCM_1005: [u8; 25] = [
        0xD3, 0x00, 0x13, 0x3E, 0xD7, 0xD3, 0x02, 0x02, 0x98, 0x0E, 0xDE, 0xEF, 0x34, 0xB4, 0xBD,
        0x62, 0xAC, 0x09, 0x41, 0x98, 0x6F, 0x33, 0x36, 0x0B, 0x98,
    ];

    /// Owned mirror of FrameEvent for recording across borrows.
    #[derive(Debug, PartialEq, Eq)]
    enum Ev {
        Frame(u16, Vec<u8>),
        Crc,
        Garbage(usize),
    }

    fn feed(d: &mut Deframer, bytes: &[u8]) -> Vec<Ev> {
        let mut evs = Vec::new();
        d.feed(bytes, &mut |e| {
            evs.push(match e {
                FrameEvent::Frame { msg_type, payload } => Ev::Frame(msg_type, payload.to_vec()),
                FrameEvent::CrcError => Ev::Crc,
                FrameEvent::Garbage(n) => Ev::Garbage(n),
            })
        });
        evs
    }

    /// Wrap a payload in a valid frame (header + CRC).
    fn build(payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 1024);
        let mut v = vec![0xD3, (payload.len() >> 8) as u8, payload.len() as u8];
        v.extend_from_slice(payload);
        let c = crc24q(&v);
        v.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8]);
        v
    }

    #[test]
    fn single_frame_single_feed() {
        let mut d = Deframer::new();
        let evs = feed(&mut d, &RTCM_1005);
        assert_eq!(evs, vec![Ev::Frame(1005, RTCM_1005[3..22].to_vec())]);
        assert_eq!((d.crc_failures, d.garbage_bytes), (0, 0));
    }

    #[test]
    fn frame_delivered_byte_at_a_time() {
        let mut d = Deframer::new();
        let mut all = Vec::new();
        for &b in &RTCM_1005 {
            all.extend(feed(&mut d, &[b]));
        }
        assert_eq!(all, vec![Ev::Frame(1005, RTCM_1005[3..22].to_vec())]);
        assert_eq!((d.crc_failures, d.garbage_bytes), (0, 0));
    }

    #[test]
    fn back_to_back_frames_in_one_feed() {
        // Second frame embeds message number 1077 in its first 12 bits.
        let p2 = [0x43, 0x50, 0xAA, 0xBB, 0xCC];
        let mut stream = RTCM_1005.to_vec();
        stream.extend(build(&p2));
        let mut d = Deframer::new();
        let evs = feed(&mut d, &stream);
        assert_eq!(
            evs,
            vec![
                Ev::Frame(1005, RTCM_1005[3..22].to_vec()),
                Ev::Frame(1077, p2.to_vec()),
            ]
        );
        assert_eq!((d.crc_failures, d.garbage_bytes), (0, 0));
    }

    #[test]
    fn garbage_before_frame_is_counted_and_flushed() {
        let mut stream = b"ICY 200 OK\r\n".to_vec();
        stream.extend(RTCM_1005);
        let mut d = Deframer::new();
        let evs = feed(&mut d, &stream);
        assert_eq!(
            evs,
            vec![Ev::Garbage(12), Ev::Frame(1005, RTCM_1005[3..22].to_vec())]
        );
        assert_eq!(d.garbage_bytes, 12);
    }

    #[test]
    fn garbage_accumulates_across_feeds() {
        let mut d = Deframer::new();
        assert_eq!(feed(&mut d, b"junk!"), vec![Ev::Garbage(5)]);
        assert_eq!(feed(&mut d, b"more"), vec![Ev::Garbage(4)]);
        assert_eq!(d.garbage_bytes, 9);
        // A frame still gets through afterwards.
        let evs = feed(&mut d, &RTCM_1005);
        assert_eq!(evs, vec![Ev::Frame(1005, RTCM_1005[3..22].to_vec())]);
    }

    #[test]
    fn corrupted_payload_byte_then_clean_resync() {
        // Synthetic first frame chosen so no byte but the sync is 0xD3:
        // after the CRC failure the whole frame must degrade to garbage in
        // one clean run, then the following frame decodes.
        let mut corrupt = build(&[0x3E, 0xD0, 0x11, 0x22, 0x33, 0x44]);
        assert!(
            corrupt[1..].iter().all(|&b| b != 0xD3),
            "test frame must not contain an interior 0xD3"
        );
        corrupt[5] ^= 0xFF; // flip a payload byte (0x11 -> 0xEE) -> CRC fails
        let total = corrupt.len(); // 12
        let mut stream = corrupt;
        stream.extend(RTCM_1005);
        let mut d = Deframer::new();
        let evs = feed(&mut d, &stream);
        assert_eq!(
            evs,
            vec![
                Ev::Crc,
                Ev::Garbage(total),
                Ev::Frame(1005, RTCM_1005[3..22].to_vec())
            ]
        );
        assert_eq!((d.crc_failures, d.garbage_bytes), (1, total as u64));
    }

    #[test]
    fn corrupted_length_byte_never_desyncs() {
        // Corrupt the length from 19 to 16: CRC is computed over the wrong
        // span and fails. The payload's own 0xD3 0x02 then looks like a
        // candidate with a huge length (0x202), which resolves as a second
        // CRC failure once enough bytes exist; the clean frame must still
        // come through.
        let mut corrupt = RTCM_1005.to_vec();
        corrupt[2] = 16;
        let mut stream = corrupt;
        stream.extend(RTCM_1005);
        stream.extend(std::iter::repeat_n(0u8, 600)); // let the false candidate complete
        let mut d = Deframer::new();
        let evs = feed(&mut d, &stream);
        let frames: Vec<_> = evs.iter().filter(|e| matches!(e, Ev::Frame(..))).collect();
        assert_eq!(frames, vec![&Ev::Frame(1005, RTCM_1005[3..22].to_vec())]);
        assert_eq!(d.crc_failures, 2);
    }

    #[test]
    fn false_sync_inside_payload_is_not_split() {
        // Payload deliberately contains 0xD3 0x00, a plausible inner sync.
        let p = [0x3E, 0xD0, 0xD3, 0x00, 0x13, 0x55, 0x66];
        let mut d = Deframer::new();
        let evs = feed(&mut d, &build(&p));
        assert_eq!(evs, vec![Ev::Frame(1005, p.to_vec())]);
        assert_eq!((d.crc_failures, d.garbage_bytes), (0, 0));
    }

    #[test]
    fn false_sync_in_garbage_with_short_tail() {
        // 0xD3 followed by a byte with high bits set is not a sync; both
        // bytes are garbage, one scan step at a time.
        let mut d = Deframer::new();
        let evs = feed(&mut d, &[0xD3, 0xFF, 0xD3, 0x7F, 0x00]);
        assert_eq!(evs, vec![Ev::Garbage(5)]);
        assert_eq!(d.garbage_bytes, 5);
    }

    #[test]
    fn trailing_d3_is_held_not_discarded() {
        let mut d = Deframer::new();
        assert_eq!(feed(&mut d, &[0x00, 0xD3]), vec![Ev::Garbage(1)]);
        assert_eq!(d.garbage_bytes, 1);
        // Rest of a valid frame arrives: held byte must still head it.
        let evs = feed(&mut d, &RTCM_1005[1..]);
        assert_eq!(evs, vec![Ev::Frame(1005, RTCM_1005[3..22].to_vec())]);
    }

    #[test]
    fn split_across_feeds_at_awkward_boundaries() {
        for cut in [1, 2, 3, 4, 22, 24] {
            let mut d = Deframer::new();
            let mut evs = feed(&mut d, &RTCM_1005[..cut]);
            evs.extend(feed(&mut d, &RTCM_1005[cut..]));
            assert_eq!(
                evs,
                vec![Ev::Frame(1005, RTCM_1005[3..22].to_vec())],
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn zero_length_payload_frame() {
        // Degenerate but CRC-valid; msg_type reads as 0 by definition.
        let f = build(&[]);
        let mut d = Deframer::new();
        assert_eq!(feed(&mut d, &f), vec![Ev::Frame(0, Vec::new())]);
    }

    #[test]
    fn buffer_is_drained_after_consumption() {
        let mut d = Deframer::new();
        for _ in 0..1_000 {
            feed(&mut d, &RTCM_1005);
            feed(&mut d, b"noise");
        }
        // Everything was consumed each round; nothing accumulates.
        assert!(d.buf.is_empty(), "buf holds {} bytes", d.buf.len());
        assert_eq!(d.garbage_bytes, 5_000);
    }
}
