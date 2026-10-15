// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The control channel between the pool and a worker: length-prefixed, versioned frames over the
//! worker's stdin and stdout pipes. Pixels never travel here (they go through shared memory) and
//! nothing is serialised with a general-purpose format: every message is a fixed sequence of
//! little-endian integers and length-prefixed strings, decoded by hand with every length checked
//! before anything is allocated. The worker's replies are **untrusted**: a compromised worker can
//! send anything, so every reader returns a typed error instead of panicking or over-allocating.
//!
//! ```text
//! frame  = magic "ACWP" | version u8 | kind u8 | reserved u16 (0) | payload length u64 | payload
//! ```
//!
//! | Kind | Direction | Payload |
//! |---|---|---|
//! | `Hello` | worker to pool, once, after the sandbox is applied | protocol, pid, sandbox level text, probes, libheif version, codecs |
//! | `Job` | pool to worker | fixed [`JobHeader`], then the input file bytes |
//! | `Result` | worker to pool | one [`Reply`] |
//! | `Shutdown` | pool to worker | empty |
//! | `Escape` | worker to pool, test builds only | text lines |

use auto_crop_codecs::{CodecError, DecodeLimits, Format, Limit};
use std::io::{self, Read, Write};

pub const MAGIC: [u8; 4] = *b"ACWP";
/// Bumped on any incompatible change; a worker of another version is refused at `Hello`.
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 16;

/// Largest `Hello` and `Escape` payloads.
pub const MAX_HELLO: u64 = 8 * 1024;
/// Largest `Result` payload apart from the ICC profile (which the pool caps separately).
pub const MAX_REPLY_BASE: u64 = 16 * 1024;
/// Longest text field a reader accepts.
pub const MAX_TEXT: usize = 2048;
/// Most notices in one reply.
pub const MAX_NOTICES: usize = 32;
/// Longest notice code.
pub const MAX_NOTICE_LEN: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
    #[error("the peer closed the channel")]
    Closed,
    #[error("bad frame magic")]
    BadMagic,
    #[error("protocol version {0} is not {VERSION}")]
    BadVersion(u8),
    #[error("unknown frame kind {0}")]
    BadKind(u8),
    #[error("frame of {len} bytes exceeds the {max} byte cap for its kind")]
    TooLong { len: u64, max: u64 },
    #[error("malformed message: {0}")]
    Malformed(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Hello = 1,
    Job = 2,
    Result = 3,
    Shutdown = 4,
    Escape = 5,
}

impl Kind {
    fn from_u8(b: u8) -> Option<Kind> {
        Some(match b {
            1 => Kind::Hello,
            2 => Kind::Job,
            3 => Kind::Result,
            4 => Kind::Shutdown,
            5 => Kind::Escape,
            _ => return None,
        })
    }
}

fn header(kind: Kind, len: u64) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..4].copy_from_slice(&MAGIC);
    h[4] = VERSION;
    h[5] = kind as u8;
    h[8..].copy_from_slice(&len.to_le_bytes());
    h
}

/// Writes one frame whose payload is `head` followed by `tail` (so a job's input bytes are not
/// copied into a second buffer).
pub fn write_frame<W: Write>(w: &mut W, kind: Kind, head: &[u8], tail: &[u8]) -> io::Result<()> {
    w.write_all(&header(kind, (head.len() + tail.len()) as u64))?;
    w.write_all(head)?;
    w.write_all(tail)?;
    w.flush()
}

/// Reads one frame. `max_len` gives the payload cap for each kind; nothing is allocated before the
/// length has been checked against it.
pub fn read_frame<R: Read>(
    r: &mut R,
    max_len: impl Fn(Kind) -> u64,
) -> Result<(Kind, Vec<u8>), ProtoError> {
    let mut h = [0u8; HEADER_LEN];
    match r.read_exact(&mut h) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(ProtoError::Closed),
        Err(e) => return Err(ProtoError::Io(e)),
    }
    if h[..4] != MAGIC {
        return Err(ProtoError::BadMagic);
    }
    if h[4] != VERSION {
        return Err(ProtoError::BadVersion(h[4]));
    }
    let kind = Kind::from_u8(h[5]).ok_or(ProtoError::BadKind(h[5]))?;
    let len = u64::from_le_bytes(h[8..16].try_into().expect("8 bytes"));
    let max = max_len(kind);
    if len > max {
        return Err(ProtoError::TooLong { len, max });
    }
    let len = usize::try_from(len).map_err(|_| ProtoError::TooLong { len, max })?;
    let mut buf = Vec::new();
    buf.try_reserve_exact(len)
        .map_err(|_| ProtoError::TooLong {
            len: len as u64,
            max,
        })?;
    // Fill in bounded steps: a lying length followed by a short stream must not need the whole
    // buffer to be zeroed first, and a closed pipe ends the loop.
    let mut remaining = len;
    let mut chunk = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let n = remaining.min(chunk.len());
        match r.read_exact(&mut chunk[..n]) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(ProtoError::Closed),
            Err(e) => return Err(ProtoError::Io(e)),
        }
        buf.extend_from_slice(&chunk[..n]);
        remaining -= n;
    }
    Ok((kind, buf))
}

// -------------------------------------------------------------------------------------------
// field codecs

#[derive(Default)]
pub(crate) struct Enc(pub Vec<u8>);

impl Enc {
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    /// A string, cut at `max` bytes on a character boundary, with a u16 length.
    pub fn text(&mut self, s: &str, max: usize) -> &mut Self {
        let mut end = s.len().min(max);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.u16(end as u16);
        self.0.extend_from_slice(&s.as_bytes()[..end]);
        self
    }
}

pub(crate) struct Dec<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Dec<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtoError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.b.len())
            .ok_or(ProtoError::Malformed("message is shorter than its fields"))?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8, ProtoError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, ProtoError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("2")))
    }
    pub fn u32(&mut self) -> Result<u32, ProtoError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4")))
    }
    pub fn u64(&mut self) -> Result<u64, ProtoError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8")))
    }
    /// A length-prefixed string: at most [`MAX_TEXT`] bytes, lossily decoded, control characters
    /// replaced (the text ends up in logs and messages).
    pub fn text(&mut self) -> Result<String, ProtoError> {
        let n = usize::from(self.u16()?);
        if n > MAX_TEXT {
            return Err(ProtoError::Malformed("text field too long"));
        }
        let raw = self.take(n)?;
        Ok(String::from_utf8_lossy(raw)
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect())
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let r = &self.b[self.pos..];
        self.pos = self.b.len();
        r
    }
}

// -------------------------------------------------------------------------------------------
// Hello

/// What a worker says about itself once it is confined. Everything in it is a claim of the
/// worker; the pool uses it for reporting and policy, never as proof of safety by itself (the
/// confinement is real because the escape tests show it, not because the text says so).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub pid: u32,
    /// The level the worker applied, for example `rlimit+landlock=FullyEnforced+seccomp`.
    pub level: String,
    /// What the worker itself observed after applying it (blocked file open, socket, spawn).
    pub probes: String,
    /// The libheif the worker loaded, empty when the worker has no HEIF support.
    pub libheif: String,
    pub hevc: bool,
    pub av1: bool,
}

impl Hello {
    pub fn encode(&self) -> Vec<u8> {
        let mut e = Enc::default();
        e.u32(self.pid)
            .text(&self.level, 512)
            .text(&self.probes, 512)
            .text(&self.libheif, 64)
            .u8(u8::from(self.hevc))
            .u8(u8::from(self.av1));
        e.0
    }

    pub fn decode(b: &[u8]) -> Result<Hello, ProtoError> {
        let mut d = Dec::new(b);
        Ok(Hello {
            pid: d.u32()?,
            level: d.text()?,
            probes: d.text()?,
            libheif: d.text()?,
            hevc: d.u8()? != 0,
            av1: d.u8()? != 0,
        })
    }
}

// -------------------------------------------------------------------------------------------
// Job

pub const FLAG_APPLY_ORIENTATION: u32 = 1;
pub const FLAG_FLATTEN_ALPHA: u32 = 2;

/// The fixed part of a `Job` payload; the input bytes follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobHeader {
    pub job_id: u64,
    pub flags: u32,
    pub limits: DecodeLimits,
    /// Bytes of shared memory the worker may write the pixels into (at least `w * h * 3`).
    pub shm_len: u64,
    /// Windows: the value of the shared-memory handle duplicated into the worker. 0 elsewhere.
    pub shm_handle: u64,
}

pub const JOB_HEADER_LEN: usize = 8 + 4 + 8 * 6 + 4 + 4 + 8 + 8 + 8;

impl JobHeader {
    pub fn encode(&self) -> Vec<u8> {
        let l = &self.limits;
        let mut e = Enc::default();
        e.u64(self.job_id)
            .u32(self.flags)
            .u64(l.max_pixels)
            .u64(l.max_file_bytes)
            .u64(l.max_file_bytes_streamed)
            .u64(l.max_metadata_bytes)
            .u64(l.max_est_bytes)
            .u64(l.max_decode_ms.unwrap_or(0))
            .u32(l.max_scans)
            .u32(l.max_frames)
            .u64(self.shm_len)
            .u64(self.shm_handle)
            .u64(0);
        debug_assert_eq!(e.0.len(), JOB_HEADER_LEN);
        e.0
    }

    /// Splits a `Job` payload into its header and the input bytes.
    pub fn decode(payload: &[u8]) -> Result<(JobHeader, &[u8]), ProtoError> {
        let mut d = Dec::new(payload);
        let job_id = d.u64()?;
        let flags = d.u32()?;
        let max_pixels = d.u64()?;
        let max_file_bytes = d.u64()?;
        let max_file_bytes_streamed = d.u64()?;
        let max_metadata_bytes = d.u64()?;
        let max_est_bytes = d.u64()?;
        let ms = d.u64()?;
        let max_scans = d.u32()?;
        let max_frames = d.u32()?;
        let shm_len = d.u64()?;
        let shm_handle = d.u64()?;
        let _reserved = d.u64()?;
        let limits = DecodeLimits {
            max_pixels,
            max_file_bytes,
            max_file_bytes_streamed,
            max_metadata_bytes,
            max_scans,
            max_frames,
            max_est_bytes,
            max_decode_ms: (ms != 0).then_some(ms),
        };
        Ok((
            JobHeader {
                job_id,
                flags,
                limits,
                shm_len,
                shm_handle,
            },
            d.rest(),
        ))
    }
}

// -------------------------------------------------------------------------------------------
// Result

/// A successful decode as the worker reports it (the pixels are in shared memory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkReply {
    pub job_id: u64,
    pub width: u32,
    pub height: u32,
    pub exif_orientation: u8,
    pub source_bit_depth: u8,
    pub frames: u32,
    pub notices: Vec<String>,
    pub icc: Option<Vec<u8>>,
}

/// A typed decode failure as the worker reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrReply {
    pub job_id: u64,
    pub code: u8,
    pub a: u64,
    pub b: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Ok(OkReply),
    Err(ErrReply),
}

const ERR_UNSUPPORTED: u8 = 1;
const ERR_NOT_DECODABLE: u8 = 2;
const ERR_HEVC_MISSING: u8 = 3;
const ERR_UNSUPPORTED_FEATURE: u8 = 4;
const ERR_CORRUPT: u8 = 5;
const ERR_TOO_LARGE: u8 = 6;
const ERR_LIMIT: u8 = 7;
const ERR_ENCODE: u8 = 8;
const ERR_PANIC: u8 = 9;
const ERR_TIMEOUT: u8 = 10;
const ERR_MEMORY: u8 = 11;
const ERR_CRASHED: u8 = 12;

fn limit_code(l: Limit) -> u64 {
    match l {
        Limit::Pixels => 1,
        Limit::FileBytes => 2,
        Limit::EstBytes => 3,
        Limit::MetadataBytes => 4,
        Limit::Scans => 5,
        Limit::Frames => 6,
    }
}

fn limit_from(c: u64) -> Limit {
    match c {
        1 => Limit::Pixels,
        2 => Limit::FileBytes,
        4 => Limit::MetadataBytes,
        5 => Limit::Scans,
        6 => Limit::Frames,
        _ => Limit::EstBytes,
    }
}

const FORMATS: [Format; 9] = [
    Format::Jpeg,
    Format::Png,
    Format::Tiff,
    Format::Webp,
    Format::Heic,
    Format::Avif,
    Format::Gif,
    Format::Bmp,
    Format::Jxl,
];

/// The wire form of a codec error (what the worker sends).
pub fn err_to_wire(job_id: u64, e: &CodecError) -> ErrReply {
    let (code, a, b, text) = match e {
        CodecError::Unsupported => (ERR_UNSUPPORTED, 0, 0, String::new()),
        CodecError::NotDecodable(f) => (ERR_NOT_DECODABLE, 0, 0, f.extension().to_owned()),
        CodecError::HevcDecoderMissing => (ERR_HEVC_MISSING, 0, 0, String::new()),
        CodecError::UnsupportedFeature(m) => (ERR_UNSUPPORTED_FEATURE, 0, 0, m.clone()),
        CodecError::Corrupt(m) => (ERR_CORRUPT, 0, 0, m.clone()),
        CodecError::TooLarge(px) => (ERR_TOO_LARGE, *px, 0, String::new()),
        CodecError::LimitExceeded { limit, actual, cap } => {
            (ERR_LIMIT, *actual, *cap, limit_code(*limit).to_string())
        }
        CodecError::Encode(m) => (ERR_ENCODE, 0, 0, m.clone()),
        CodecError::InternalPanic(m) => (ERR_PANIC, 0, 0, m.clone()),
        CodecError::DecodeTimeout => (ERR_TIMEOUT, 0, 0, String::new()),
        CodecError::MemoryLimit => (ERR_MEMORY, 0, 0, String::new()),
        CodecError::WorkerCrashed(m) => (ERR_CRASHED, 0, 0, m.clone()),
        CodecError::Cancelled => (ERR_CRASHED, 0, 0, "cancelled".to_owned()),
    };
    ErrReply {
        job_id,
        code,
        a,
        b,
        text,
    }
}

/// The codec error an [`ErrReply`] stands for. Unknown codes become `Corrupt` (the worker is
/// untrusted: a lie can fail one item, nothing more).
pub fn err_from_wire(r: &ErrReply) -> CodecError {
    match r.code {
        ERR_UNSUPPORTED => CodecError::Unsupported,
        ERR_NOT_DECODABLE => FORMATS
            .iter()
            .find(|f| f.extension() == r.text)
            .map_or(CodecError::Unsupported, |f| CodecError::NotDecodable(*f)),
        ERR_HEVC_MISSING => CodecError::HevcDecoderMissing,
        ERR_UNSUPPORTED_FEATURE => CodecError::UnsupportedFeature(r.text.clone()),
        ERR_CORRUPT => CodecError::Corrupt(r.text.clone()),
        ERR_TOO_LARGE => CodecError::TooLarge(r.a),
        ERR_LIMIT => CodecError::LimitExceeded {
            limit: limit_from(r.text.parse().unwrap_or(3)),
            actual: r.a,
            cap: r.b,
        },
        ERR_ENCODE => CodecError::Encode(r.text.clone()),
        ERR_PANIC => CodecError::InternalPanic(r.text.clone()),
        ERR_TIMEOUT => CodecError::DecodeTimeout,
        ERR_MEMORY => CodecError::MemoryLimit,
        ERR_CRASHED => CodecError::WorkerCrashed(r.text.clone()),
        _ => CodecError::Corrupt("the decoder worker sent an unknown error".into()),
    }
}

impl Reply {
    pub fn job_id(&self) -> u64 {
        match self {
            Reply::Ok(o) => o.job_id,
            Reply::Err(e) => e.job_id,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut e = Enc::default();
        match self {
            Reply::Ok(o) => {
                e.u64(o.job_id)
                    .u8(0)
                    .u32(o.width)
                    .u32(o.height)
                    .u8(o.exif_orientation)
                    .u8(o.source_bit_depth)
                    .u32(o.frames);
                let notices = &o.notices[..o.notices.len().min(MAX_NOTICES)];
                e.u8(notices.len() as u8);
                for n in notices {
                    e.text(n, MAX_NOTICE_LEN);
                }
                match &o.icc {
                    Some(icc) => {
                        e.u8(1).u32(icc.len() as u32);
                        e.0.extend_from_slice(icc);
                    }
                    None => {
                        e.u8(0);
                    }
                }
            }
            Reply::Err(r) => {
                e.u64(r.job_id)
                    .u8(1)
                    .u8(r.code)
                    .u64(r.a)
                    .u64(r.b)
                    .text(&r.text, 512);
            }
        }
        e.0
    }

    /// Decodes a reply; `max_icc` is the profile cap of the job it answers.
    pub fn decode(b: &[u8], max_icc: u64) -> Result<Reply, ProtoError> {
        let mut d = Dec::new(b);
        let job_id = d.u64()?;
        match d.u8()? {
            0 => {
                let width = d.u32()?;
                let height = d.u32()?;
                let exif_orientation = d.u8()?;
                let source_bit_depth = d.u8()?;
                let frames = d.u32()?;
                let n = usize::from(d.u8()?);
                if n > MAX_NOTICES {
                    return Err(ProtoError::Malformed("too many notices"));
                }
                let mut notices = Vec::with_capacity(n);
                for _ in 0..n {
                    let t = d.text()?;
                    if t.len() > MAX_NOTICE_LEN {
                        return Err(ProtoError::Malformed("notice too long"));
                    }
                    notices.push(t);
                }
                let icc = match d.u8()? {
                    0 => None,
                    1 => {
                        let len = u64::from(d.u32()?);
                        if len > max_icc {
                            return Err(ProtoError::Malformed("profile longer than its cap"));
                        }
                        let rest = d.rest();
                        if rest.len() as u64 != len {
                            return Err(ProtoError::Malformed("profile length mismatch"));
                        }
                        Some(rest.to_vec())
                    }
                    _ => return Err(ProtoError::Malformed("bad profile flag")),
                };
                Ok(Reply::Ok(OkReply {
                    job_id,
                    width,
                    height,
                    exif_orientation,
                    source_bit_depth,
                    frames,
                    notices,
                    icc,
                }))
            }
            1 => Ok(Reply::Err(ErrReply {
                job_id,
                code: d.u8()?,
                a: d.u64()?,
                b: d.u64()?,
                text: d.text()?,
            })),
            _ => Err(ProtoError::Malformed("bad reply status")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(k: Kind) -> u64 {
        match k {
            Kind::Job => 1 << 20,
            _ => 1 << 16,
        }
    }

    #[test]
    fn a_frame_round_trips_and_input_is_not_copied_into_the_head() {
        let mut buf = Vec::new();
        write_frame(&mut buf, Kind::Job, b"head", b"tail").unwrap();
        let (k, p) = read_frame(&mut buf.as_slice(), caps).unwrap();
        assert_eq!(k, Kind::Job);
        assert_eq!(p, b"headtail");
    }

    #[test]
    fn hostile_frames_are_refused_before_anything_is_allocated() {
        // wrong magic
        let mut f = header(Kind::Hello, 0).to_vec();
        f[0] = b'X';
        assert!(matches!(
            read_frame(&mut f.as_slice(), caps),
            Err(ProtoError::BadMagic)
        ));
        // other version
        let mut f = header(Kind::Hello, 0).to_vec();
        f[4] = 9;
        assert!(matches!(
            read_frame(&mut f.as_slice(), caps),
            Err(ProtoError::BadVersion(9))
        ));
        // unknown kind
        let mut f = header(Kind::Hello, 0).to_vec();
        f[5] = 77;
        assert!(matches!(
            read_frame(&mut f.as_slice(), caps),
            Err(ProtoError::BadKind(77))
        ));
        // a length of 2^63 for a result: refused by the cap, not by an allocation failure
        let f = header(Kind::Result, 1 << 63).to_vec();
        assert!(matches!(
            read_frame(&mut f.as_slice(), caps),
            Err(ProtoError::TooLong { .. })
        ));
        // a length within the cap but a stream that ends early
        let mut f = header(Kind::Result, 1000).to_vec();
        f.extend_from_slice(&[1, 2, 3]);
        assert!(matches!(
            read_frame(&mut f.as_slice(), caps),
            Err(ProtoError::Closed)
        ));
        // an empty stream
        assert!(matches!(
            read_frame(&mut [].as_slice(), caps),
            Err(ProtoError::Closed)
        ));
    }

    #[test]
    fn hello_round_trips() {
        let h = Hello {
            pid: 42,
            level: "rlimit+landlock=FullyEnforced+seccomp".into(),
            probes: "file=blocked net=blocked spawn=blocked".into(),
            libheif: "1.23.5".into(),
            hevc: true,
            av1: false,
        };
        assert_eq!(Hello::decode(&h.encode()).unwrap(), h);
        assert!(Hello::decode(&h.encode()[..5]).is_err());
    }

    #[test]
    fn a_job_header_round_trips_with_the_input_after_it() {
        let limits = DecodeLimits {
            max_decode_ms: Some(1234),
            ..DecodeLimits::default().with_max_pixels(5_000_000)
        };
        let jh = JobHeader {
            job_id: 7,
            flags: FLAG_FLATTEN_ALPHA,
            limits,
            shm_len: 99,
            shm_handle: 0xABC,
        };
        let mut payload = jh.encode();
        assert_eq!(payload.len(), JOB_HEADER_LEN);
        payload.extend_from_slice(b"input bytes");
        let (back, input) = JobHeader::decode(&payload).unwrap();
        assert_eq!(back, jh);
        assert_eq!(input, b"input bytes");
        assert!(JobHeader::decode(&payload[..JOB_HEADER_LEN - 1]).is_err());
    }

    #[test]
    fn replies_round_trip_and_the_profile_is_capped() {
        let ok = Reply::Ok(OkReply {
            job_id: 3,
            width: 640,
            height: 480,
            exif_orientation: 6,
            source_bit_depth: 10,
            frames: 2,
            notices: vec!["depth.reduced_to_8".into(), "heic.multi_image".into()],
            icc: Some(vec![9; 100]),
        });
        assert_eq!(Reply::decode(&ok.encode(), 1000).unwrap(), ok);
        // the same bytes with a smaller profile cap
        assert!(Reply::decode(&ok.encode(), 99).is_err());
        let plain = Reply::Ok(OkReply {
            icc: None,
            notices: vec![],
            ..match ok.clone() {
                Reply::Ok(o) => o,
                Reply::Err(_) => unreachable!(),
            }
        });
        assert_eq!(Reply::decode(&plain.encode(), 0).unwrap(), plain);
        // truncations never panic
        let bytes = ok.encode();
        for cut in 0..bytes.len() {
            let _ = Reply::decode(&bytes[..cut], 1000);
        }
    }

    #[test]
    fn every_codec_error_survives_the_wire() {
        let errs = [
            CodecError::Unsupported,
            CodecError::NotDecodable(Format::Heic),
            CodecError::HevcDecoderMissing,
            CodecError::UnsupportedFeature("x".into()),
            CodecError::Corrupt("c".into()),
            CodecError::TooLarge(123),
            CodecError::LimitExceeded {
                limit: Limit::Frames,
                actual: 5,
                cap: 3,
            },
            CodecError::Encode("e".into()),
            CodecError::InternalPanic("p".into()),
            CodecError::DecodeTimeout,
            CodecError::MemoryLimit,
            CodecError::WorkerCrashed("w".into()),
        ];
        for e in errs {
            let w = Reply::Err(err_to_wire(5, &e));
            let Reply::Err(back) = Reply::decode(&w.encode(), 0).unwrap() else {
                panic!()
            };
            assert_eq!(err_from_wire(&back), e);
        }
    }

    #[test]
    fn text_fields_are_cut_cleaned_and_bounded() {
        let long = "é".repeat(5000);
        let r = Reply::Err(ErrReply {
            job_id: 1,
            code: ERR_CORRUPT,
            a: 0,
            b: 0,
            text: format!("{long}\n\u{1b}[31m"),
        });
        let Reply::Err(back) = Reply::decode(&r.encode(), 0).unwrap() else {
            panic!()
        };
        assert!(back.text.len() <= 512);
        assert!(!back.text.contains('\n'));
        // a text length above the reader's cap is malformed
        let mut e = Enc::default();
        e.u64(1).u8(1).u8(ERR_CORRUPT).u64(0).u64(0).u16(60_000);
        e.0.extend(std::iter::repeat_n(b'a', 60_000));
        assert!(Reply::decode(&e.0, 0).is_err());
    }
}
