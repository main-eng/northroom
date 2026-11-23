//! A Matroska and WebM demuxer (RFC 9559) for platforms whose frameworks don't read them. It
//! reads the segment info, tracks and codec settings, cues, and one track's frames in file
//! order with timestamps, unlaced. The platform decodes the frames (`apple` hands them to
//! VideoToolbox and AudioToolbox). Seeks use the cues when present and cluster timestamps
//! otherwise; live-written files (no sizes, cues or duration) work too.

mod block;
pub(crate) mod codec;
mod ebml;

use crate::media::{MediaError, MediaResult};
use block::{block_header, frames, KEYFRAME};
use ebml::{children, float, signed, text, unsigned, Header, Source};
use std::collections::VecDeque;

const EBML: u32 = 0x1a45_dfa3;
const DOC_TYPE: u32 = 0x4282;
const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114d_9b74;
const SEEK: u32 = 0x4dbb;
const SEEK_ID: u32 = 0x53ab;
const SEEK_POSITION: u32 = 0x53ac;
const INFO: u32 = 0x1549_a966;
const TIMESTAMP_SCALE: u32 = 0x2a_d7b1;
const DURATION: u32 = 0x4489;
const TRACKS: u32 = 0x1654_ae6b;
const TRACK_ENTRY: u32 = 0xae;
const TRACK_NUMBER: u32 = 0xd7;
const TRACK_TYPE: u32 = 0x83;
const FLAG_ENABLED: u32 = 0xb9;
const DEFAULT_DURATION: u32 = 0x23_e383;
const CODEC_ID: u32 = 0x86;
const CODEC_PRIVATE: u32 = 0x63a2;
const CODEC_DELAY: u32 = 0x56aa;
const SEEK_PRE_ROLL: u32 = 0x56bb;
const VIDEO: u32 = 0xe0;
const PIXEL_WIDTH: u32 = 0xb0;
const PIXEL_HEIGHT: u32 = 0xba;
const COLOUR: u32 = 0x55b0;
const MATRIX_COEFFICIENTS: u32 = 0x55b1;
const BITS_PER_CHANNEL: u32 = 0x55b2;
const CHROMA_SITING_HORIZONTAL: u32 = 0x55b7;
const CHROMA_SITING_VERTICAL: u32 = 0x55b8;
const RANGE: u32 = 0x55b9;
const TRANSFER_CHARACTERISTICS: u32 = 0x55ba;
const PRIMARIES: u32 = 0x55bb;
const AUDIO: u32 = 0xe1;
const SAMPLING_FREQUENCY: u32 = 0xb5;
const CHANNELS: u32 = 0x9f;
const BIT_DEPTH: u32 = 0x6264;
const CONTENT_ENCODINGS: u32 = 0x6d80;
const CONTENT_ENCODING: u32 = 0x6240;
const CONTENT_ENCODING_SCOPE: u32 = 0x5032;
const CONTENT_ENCODING_TYPE: u32 = 0x5033;
const CONTENT_COMPRESSION: u32 = 0x5034;
const CONTENT_COMPRESSION_ALGORITHM: u32 = 0x4254;
const CONTENT_COMPRESSION_SETTINGS: u32 = 0x4255;
const CLUSTER: u32 = 0x1f43_b675;
const CLUSTER_TIMESTAMP: u32 = 0xe7;
const SIMPLE_BLOCK: u32 = 0xa3;
const BLOCK_GROUP: u32 = 0xa0;
const BLOCK: u32 = 0xa1;
const BLOCK_DURATION: u32 = 0x9b;
const REFERENCE_BLOCK: u32 = 0xfb;
const DISCARD_PADDING: u32 = 0x75a2;
const CUES: u32 = 0x1c53_bb6b;
const CUE_POINT: u32 = 0xbb;
const CUE_TIME: u32 = 0xb3;
const CUE_TRACK_POSITIONS: u32 = 0xb7;
const CUE_TRACK: u32 = 0xf7;
const CUE_CLUSTER_POSITION: u32 = 0xf1;

/// Elements a cluster may hold besides blocks; anything else ends a cluster of unknown size.
const OTHER_CLUSTER_CHILDREN: [u32; 6] = [
    0xa7,   // Position
    0xab,   // PrevSize
    0xec,   // Void
    0xbf,   // CRC-32
    0x5854, // SilentTracks
    0xaf,   // EncryptedBlock
];

/// `TimestampScale`'s default: timestamps in milliseconds.
const DEFAULT_TIMESTAMP_SCALE: u64 = 1_000_000;
/// `ContentCompAlgo` for header stripping: bytes every frame starts with, omitted from the file.
const HEADER_STRIPPING: u64 = 3;
const NANOSECONDS_PER_SECOND: f64 = 1e9;

/// Whether the file at `path` is EBML (Matroska or WebM), from its first four bytes.
pub(crate) fn is_matroska(path: &str) -> bool {
    use std::io::Read;
    let mut magic = [0_u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && u32::from_be_bytes(magic) == EBML
}

/// Whether the Matroska file at `path` has an enabled video track (the one `Demuxer::open`
/// would pick), from its headers alone. `None` if it isn't Matroska or its headers can't be
/// read.
pub(crate) fn holds_video(path: &str) -> Option<bool> {
    if !is_matroska(path) {
        return None;
    }
    let mut source = Source::open(path).ok()?;
    let segment = Segment::read(&mut source).ok()?;
    Some(
        segment
            .tracks
            .iter()
            .any(|track| track.kind == Some(TrackKind::Video) && track.enabled),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackKind {
    Video,
    Audio,
}

/// `Colour`: ISO/IEC 23091-4 code points, each `None` if the file omits it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Colour {
    pub(crate) matrix: Option<u64>,
    pub(crate) bits_per_channel: Option<u64>,
    pub(crate) range: Option<u64>,
    pub(crate) transfer: Option<u64>,
    pub(crate) primaries: Option<u64>,
    pub(crate) chroma_siting_horizontal: Option<u64>,
    pub(crate) chroma_siting_vertical: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct VideoSettings {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) colour: Colour,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AudioSettings {
    pub(crate) sample_rate: f64,
    pub(crate) channels: u32,
    pub(crate) bit_depth: Option<u32>,
}

/// One `TrackEntry`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Track {
    pub(crate) number: u64,
    pub(crate) kind: Option<TrackKind>,
    pub(crate) enabled: bool,
    pub(crate) codec_id: String,
    pub(crate) codec_private: Vec<u8>,
    pub(crate) default_duration_ns: Option<u64>,
    /// Samples the decoder outputs before the first played one, in nanoseconds.
    pub(crate) codec_delay_ns: u64,
    pub(crate) seek_pre_roll_ns: u64,
    pub(crate) video: Option<VideoSettings>,
    pub(crate) audio: Option<AudioSettings>,
    /// Bytes the file omits from the start of every frame (header stripping).
    pub(crate) stripped_header: Vec<u8>,
}

/// A cue point: a track's frame at `time` (in timestamp units) in the cluster at offset
/// `cluster` from the segment's data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CuePoint {
    time: u64,
    track: u64,
    cluster: u64,
}

/// A file's headers: info, tracks and cues.
#[derive(Clone, Debug)]
pub(crate) struct Segment {
    /// Nanoseconds per timestamp unit.
    pub(crate) timestamp_scale: u64,
    pub(crate) duration_seconds: Option<f64>,
    pub(crate) tracks: Vec<Track>,
    cues: Vec<CuePoint>,
    /// Where the segment's data starts; positions within it count from here.
    data_start: u64,
    end: Option<u64>,
    first_cluster: Option<u64>,
}

/// One frame of a track as stored.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Packet {
    pub(crate) data: Vec<u8>,
    /// Presentation time in nanoseconds: the block's timestamp, plus the track's default
    /// duration per frame for later frames in a lace.
    pub(crate) timestamp_ns: i64,
    pub(crate) duration_ns: Option<i64>,
    pub(crate) keyframe: bool,
    /// Index within the block's lace; frame 0 carries the block's timestamp.
    pub(crate) lace_index: usize,
    /// Nanoseconds of decoded sound at its end that aren't played.
    pub(crate) discard_padding_ns: i64,
}

/// A block of the track and its position, enough to continue reading from it.
#[derive(Clone, Copy, Debug)]
struct BlockPlace {
    element: u64,
    cluster: ClusterPlace,
    timestamp_ns: i64,
    keyframe: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClusterPlace {
    /// The cluster element's first byte.
    start: u64,
    timestamp: u64,
    end: Option<u64>,
}

/// The read position: between clusters, or inside one.
#[derive(Clone, Copy, Debug)]
enum Cursor {
    TopLevel(u64),
    InCluster {
        cluster: ClusterPlace,
        position: u64,
    },
    Finished,
}

/// One track of a Matroska file, read frame by frame from a given point.
pub(crate) struct Demuxer {
    source: Source,
    segment: Segment,
    track: Track,
    cursor: Cursor,
    /// Frames of a laced block not yet returned.
    pending: VecDeque<Packet>,
    /// Every cluster's position, scanned once when a seek or the duration needs it.
    clusters: Option<Vec<ClusterPlace>>,
    /// Ignore the cues and seek through clusters, as for a file without cues.
    ignore_cues: bool,
}

/// Which seek-head targets have been read.
#[derive(Default)]
struct HeadersRead {
    info: bool,
    tracks: bool,
    cues: bool,
}

impl HeadersRead {
    /// Whether the seek-head entry for element `id` points to a header not yet read.
    fn wants(&self, id: u32) -> bool {
        match id {
            INFO => !self.info,
            TRACKS => !self.tracks,
            CUES => !self.cues,
            _ => false,
        }
    }
}

impl Segment {
    /// Reads the headers of the file behind `source`.
    fn read(source: &mut Source) -> MediaResult<Self> {
        source.seek(0)?;
        let after_header = read_ebml_header(source)?;
        source.seek(after_header)?;
        let segment = find_segment(source)?;
        let mut parsed = Self {
            timestamp_scale: DEFAULT_TIMESTAMP_SCALE,
            duration_seconds: None,
            tracks: Vec::new(),
            cues: Vec::new(),
            data_start: segment.data_start,
            end: segment.end().map(|end| end.min(source.length())),
            first_cluster: None,
        };
        let mut read = HeadersRead::default();
        let mut sought = parsed.read_leading_headers(source, &mut read)?;
        sought.retain(|(id, _)| read.wants(*id));
        parsed.read_sought_headers(source, &sought, &mut read)?;
        parsed.cues.sort_by_key(|cue| cue.time);
        Ok(parsed)
    }

    /// Reads the headers before the first cluster, noting where it starts, and returns the
    /// seek-head entries that point elsewhere (read next).
    fn read_leading_headers(
        &mut self,
        source: &mut Source,
        read: &mut HeadersRead,
    ) -> MediaResult<Vec<(u32, u64)>> {
        let mut sought = Vec::new();
        let mut position = self.data_start;
        while self.end.is_none_or(|end| position < end) {
            source.seek(position)?;
            let Some(element) = source.header()? else {
                break;
            };
            match element.id {
                CLUSTER => {
                    self.first_cluster = Some(position);
                    break;
                }
                SEEK_HEAD => sought.extend(seek_entries(&source.data(&element)?)?),
                INFO | TRACKS | CUES => {
                    self.keep_header(element.id, &source.data(&element)?, read)?
                }
                _ => {}
            }
            position = element.end().ok_or_else(|| {
                MediaError(format!(
                    "element {:#x} of unknown size in the segment",
                    element.id
                ))
            })?;
        }
        Ok(sought)
    }

    /// Reads the headers the seek head points to, where present.
    fn read_sought_headers(
        &mut self,
        source: &mut Source,
        sought: &[(u32, u64)],
        read: &mut HeadersRead,
    ) -> MediaResult<()> {
        for &(id, offset) in sought {
            let place = self.data_start + offset;
            if place >= source.length() {
                continue;
            }
            source.seek(place)?;
            let Some(element) = source.header()?.filter(|element| element.id == id) else {
                continue;
            };
            self.keep_header(id, &source.data(&element)?, read)?;
        }
        Ok(())
    }

    /// Stores the info, tracks or cues, and marks that header as read.
    fn keep_header(&mut self, id: u32, data: &[u8], read: &mut HeadersRead) -> MediaResult<()> {
        match id {
            INFO => {
                self.read_info(data)?;
                read.info = true;
            }
            TRACKS => {
                self.tracks = tracks(data)?;
                read.tracks = true;
            }
            CUES => {
                self.cues = cue_points(data)?;
                read.cues = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn read_info(&mut self, data: &[u8]) -> MediaResult<()> {
        let mut duration = None;
        for (id, data) in children(data)? {
            match id {
                TIMESTAMP_SCALE => self.timestamp_scale = unsigned(data)?.max(1),
                DURATION => duration = Some(float(data)?),
                _ => {}
            }
        }
        self.duration_seconds = duration
            .filter(|units| units.is_finite() && *units > 0.0)
            .map(|units| units * self.timestamp_scale as f64 / NANOSECONDS_PER_SECOND);
        Ok(())
    }

    fn nanoseconds(&self, units: i64) -> i64 {
        units.saturating_mul(self.timestamp_scale as i64)
    }
}

/// Reads the EBML header at the source's position, which must declare Matroska or WebM, and
/// returns where the next element starts.
fn read_ebml_header(source: &mut Source) -> MediaResult<u64> {
    let header = source
        .header()?
        .filter(|header| header.id == EBML)
        .ok_or_else(|| MediaError("not an EBML file".into()))?;
    let doc_type = children(&source.data(&header)?)?
        .into_iter()
        .find(|(id, _)| *id == DOC_TYPE)
        .map_or_else(|| "matroska".to_owned(), |(_, data)| text(data));
    if doc_type != "matroska" && doc_type != "webm" {
        return Err(MediaError(format!(
            "an EBML file of type {doc_type}, not Matroska"
        )));
    }
    Ok(header.end().unwrap_or(header.data_start))
}

/// The segment header: the first segment element from the source's position.
fn find_segment(source: &mut Source) -> MediaResult<Header> {
    loop {
        let header = source
            .header()?
            .ok_or_else(|| MediaError("the file has no segment".into()))?;
        if header.id == SEGMENT {
            return Ok(header);
        }
        source.seek(
            header.end().ok_or_else(|| {
                MediaError("an element of unknown size before the segment".into())
            })?,
        )?;
    }
}

fn seek_entries(data: &[u8]) -> MediaResult<Vec<(u32, u64)>> {
    let mut entries = Vec::new();
    for (id, data) in children(data)? {
        if id != SEEK {
            continue;
        }
        let (mut target, mut position) = (None, None);
        for (id, data) in children(data)? {
            match id {
                SEEK_ID => target = Some(unsigned(data)? as u32),
                SEEK_POSITION => position = Some(unsigned(data)?),
                _ => {}
            }
        }
        if let (Some(target), Some(position)) = (target, position) {
            entries.push((target, position));
        }
    }
    Ok(entries)
}

fn tracks(data: &[u8]) -> MediaResult<Vec<Track>> {
    children(data)?
        .into_iter()
        .filter(|(id, _)| *id == TRACK_ENTRY)
        .map(|(_, data)| track(data))
        .collect()
}

fn track(data: &[u8]) -> MediaResult<Track> {
    let mut track = Track {
        number: 0,
        kind: None,
        enabled: true,
        codec_id: String::new(),
        codec_private: Vec::new(),
        default_duration_ns: None,
        codec_delay_ns: 0,
        seek_pre_roll_ns: 0,
        video: None,
        audio: None,
        stripped_header: Vec::new(),
    };
    for (id, data) in children(data)? {
        match id {
            TRACK_NUMBER => track.number = unsigned(data)?,
            TRACK_TYPE => {
                track.kind = match unsigned(data)? {
                    1 => Some(TrackKind::Video),
                    2 => Some(TrackKind::Audio),
                    _ => None,
                }
            }
            FLAG_ENABLED => track.enabled = unsigned(data)? != 0,
            DEFAULT_DURATION => {
                track.default_duration_ns = Some(unsigned(data)?).filter(|duration| *duration > 0)
            }
            CODEC_ID => track.codec_id = text(data),
            CODEC_PRIVATE => track.codec_private = data.to_vec(),
            CODEC_DELAY => track.codec_delay_ns = unsigned(data)?,
            SEEK_PRE_ROLL => track.seek_pre_roll_ns = unsigned(data)?,
            VIDEO => track.video = Some(video_settings(data)?),
            AUDIO => track.audio = Some(audio_settings(data)?),
            CONTENT_ENCODINGS => track.stripped_header = stripped_header(data)?,
            _ => {}
        }
    }
    Ok(track)
}

fn video_settings(data: &[u8]) -> MediaResult<VideoSettings> {
    let mut video = VideoSettings::default();
    for (id, data) in children(data)? {
        match id {
            PIXEL_WIDTH => video.width = unsigned(data)? as u32,
            PIXEL_HEIGHT => video.height = unsigned(data)? as u32,
            COLOUR => {
                for (id, data) in children(data)? {
                    let value = Some(unsigned(data)?);
                    let colour = &mut video.colour;
                    match id {
                        MATRIX_COEFFICIENTS => colour.matrix = value,
                        BITS_PER_CHANNEL => colour.bits_per_channel = value,
                        RANGE => colour.range = value,
                        TRANSFER_CHARACTERISTICS => colour.transfer = value,
                        PRIMARIES => colour.primaries = value,
                        CHROMA_SITING_HORIZONTAL => colour.chroma_siting_horizontal = value,
                        CHROMA_SITING_VERTICAL => colour.chroma_siting_vertical = value,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(video)
}

fn audio_settings(data: &[u8]) -> MediaResult<AudioSettings> {
    let mut audio = AudioSettings {
        sample_rate: 8000.0,
        channels: 1,
        bit_depth: None,
    };
    for (id, data) in children(data)? {
        match id {
            SAMPLING_FREQUENCY => audio.sample_rate = float(data)?,
            CHANNELS => audio.channels = unsigned(data)? as u32,
            BIT_DEPTH => audio.bit_depth = Some(unsigned(data)? as u32),
            _ => {}
        }
    }
    Ok(audio)
}

/// The bytes header stripping removed from every frame. Errors for an encoding that can't be
/// undone here (compression other than stripping, or encryption).
fn stripped_header(data: &[u8]) -> MediaResult<Vec<u8>> {
    let mut stripped = Vec::new();
    for (id, data) in children(data)? {
        if id == CONTENT_ENCODING {
            stripped.extend(encoding_stripped_bytes(data)?);
        }
    }
    Ok(stripped)
}

/// The bytes one `ContentEncoding` strips from the track's frames (none when its scope isn't
/// the frames). Errors for encryption or compression other than stripping.
fn encoding_stripped_bytes(data: &[u8]) -> MediaResult<Vec<u8>> {
    let (mut scope, mut kind, mut compression) = (1, 0, None);
    for (id, data) in children(data)? {
        match id {
            CONTENT_ENCODING_SCOPE => scope = unsigned(data)?,
            CONTENT_ENCODING_TYPE => kind = unsigned(data)?,
            CONTENT_COMPRESSION => compression = Some(data),
            _ => {}
        }
    }
    if kind != 0 {
        return Err(MediaError("the track is encrypted".into()));
    }
    let (algorithm, settings) = compression_settings(compression.unwrap_or_default())?;
    if algorithm != HEADER_STRIPPING {
        return Err(MediaError(format!(
            "the track's frames are compressed (algorithm {algorithm})"
        )));
    }
    Ok(if scope & 1 != 0 { settings } else { Vec::new() })
}

/// A `ContentCompression`'s algorithm and settings.
fn compression_settings(data: &[u8]) -> MediaResult<(u64, Vec<u8>)> {
    // Zlib is the default algorithm.
    let (mut algorithm, mut settings) = (0, Vec::new());
    for (id, data) in children(data)? {
        match id {
            CONTENT_COMPRESSION_ALGORITHM => algorithm = unsigned(data)?,
            CONTENT_COMPRESSION_SETTINGS => settings = data.to_vec(),
            _ => {}
        }
    }
    Ok((algorithm, settings))
}

fn cue_points(data: &[u8]) -> MediaResult<Vec<CuePoint>> {
    let mut points = Vec::new();
    for (id, data) in children(data)? {
        if id == CUE_POINT {
            points.extend(cue_point(data)?);
        }
    }
    Ok(points)
}

/// One `CuePoint`'s positions, one per track position it lists; none without a time.
fn cue_point(data: &[u8]) -> MediaResult<Vec<CuePoint>> {
    let mut time = None;
    let mut positions = Vec::new();
    for (id, data) in children(data)? {
        match id {
            CUE_TIME => time = Some(unsigned(data)?),
            CUE_TRACK_POSITIONS => positions.extend(cue_track_position(data)?),
            _ => {}
        }
    }
    let Some(time) = time else {
        return Ok(Vec::new());
    };
    Ok(positions
        .into_iter()
        .map(|(track, cluster)| CuePoint {
            time,
            track,
            cluster,
        })
        .collect())
}

/// A `CueTrackPositions`' track and cluster position, if it has both.
fn cue_track_position(data: &[u8]) -> MediaResult<Option<(u64, u64)>> {
    let (mut track, mut cluster) = (None, None);
    for (id, data) in children(data)? {
        match id {
            CUE_TRACK => track = Some(unsigned(data)?),
            CUE_CLUSTER_POSITION => cluster = Some(unsigned(data)?),
            _ => {}
        }
    }
    Ok(track.zip(cluster))
}

impl Demuxer {
    /// The file's first enabled track of `kind`, read from the start; `None` if there is none.
    pub(crate) fn open(path: &str, kind: TrackKind) -> MediaResult<Option<Self>> {
        let mut source = Source::open(path)?;
        let mut segment = Segment::read(&mut source)?;
        let Some(track) = segment
            .tracks
            .iter()
            .find(|track| track.kind == Some(kind) && track.enabled)
            .cloned()
        else {
            return Ok(None);
        };
        let first_cluster = segment.first_cluster;
        segment.cues.retain(|cue| cue.track == track.number);
        let mut demuxer = Self {
            source,
            segment,
            track,
            cursor: first_cluster.map_or(Cursor::Finished, Cursor::TopLevel),
            pending: VecDeque::new(),
            clusters: None,
            ignore_cues: false,
        };
        if demuxer.segment.duration_seconds.is_none() {
            demuxer.segment.duration_seconds = demuxer.measure_duration()?;
        }
        Ok(Some(demuxer))
    }

    pub(crate) fn segment(&self) -> &Segment {
        &self.segment
    }

    pub(crate) fn track(&self) -> &Track {
        &self.track
    }

    /// Whether seeks use the cues: the file has cues for the track and they're enabled.
    #[cfg(test)]
    pub(crate) fn seeks_by_cues(&self) -> bool {
        !self.ignore_cues && !self.segment.cues.is_empty()
    }

    /// Seeks through clusters even when the file has cues (to cross-check the two).
    #[cfg(test)]
    pub(crate) fn ignore_cues(&mut self) {
        self.ignore_cues = true;
    }

    /// The track's next frame in file order, or `None` at the end.
    pub(crate) fn next_packet(&mut self) -> MediaResult<Option<Packet>> {
        loop {
            if let Some(packet) = self.pending.pop_front() {
                return Ok(Some(packet));
            }
            let Some(block) = self.next_block(BlockRead::Frames)? else {
                return Ok(None);
            };
            self.pending.extend(block.packets);
        }
    }

    /// Seeks to the track's last frame at or before `target_ns` (the last key frame if
    /// `key_only`), or to the first such frame if none is that early.
    pub(crate) fn seek(&mut self, target_ns: i64, key_only: bool) -> MediaResult<()> {
        self.pending.clear();
        let wanted = |place: &BlockPlace| place.keyframe || !key_only;
        let Some(mut start) = self.seek_start(target_ns)? else {
            self.cursor = Cursor::Finished;
            return Ok(());
        };
        loop {
            self.cursor = Cursor::TopLevel(start);
            let (found, first) = self.last_block_at_or_before(target_ns, wanted)?;
            // Nothing that early from here: try the previous cluster, or the stream's first frame.
            let earlier = if found.is_none() {
                self.cluster_before(start)?
            } else {
                None
            };
            let Some(cluster) = earlier else {
                self.cursor = found
                    .or(first)
                    .map_or(Cursor::Finished, |place| Cursor::InCluster {
                        cluster: place.cluster,
                        position: place.element,
                    });
                return Ok(());
            };
            start = cluster;
        }
    }

    /// The cluster a seek to `target_ns` starts from: the last cue at or before it, else the
    /// last cluster starting at or before it (the first if none does). `None` if the file has
    /// no clusters.
    fn seek_start(&mut self, target_ns: i64) -> MediaResult<Option<u64>> {
        let cue = (!self.ignore_cues)
            .then(|| {
                self.segment
                    .cues
                    .iter()
                    .rev()
                    .find(|cue| self.segment.nanoseconds(cue.time as i64) <= target_ns)
            })
            .flatten()
            .map(|cue| self.segment.data_start + cue.cluster);
        if let Some(cluster) = cue {
            return Ok(Some(cluster));
        }
        let scale = self.segment.timestamp_scale as i64;
        let clusters = self.clusters()?;
        let index = clusters
            .iter()
            .rposition(|cluster| (cluster.timestamp as i64).saturating_mul(scale) <= target_ns)
            .unwrap_or(0);
        Ok(clusters.get(index).map(|cluster| cluster.start))
    }

    /// Reads on from the cursor: the last `wanted` block at or before `target_ns` and the first
    /// `wanted` block read, each `None` if there is none.
    fn last_block_at_or_before(
        &mut self,
        target_ns: i64,
        wanted: impl Fn(&BlockPlace) -> bool,
    ) -> MediaResult<(Option<BlockPlace>, Option<BlockPlace>)> {
        let (mut found, mut first) = (None, None);
        while let Some(place) = self.next_block(BlockRead::Place)?.map(|block| block.place) {
            if !wanted(&place) {
                continue;
            }
            first.get_or_insert(place);
            if place.timestamp_ns > target_ns {
                break;
            }
            found = Some(place);
        }
        Ok((found, first))
    }

    /// The start of the last cluster before the one starting at `start`.
    fn cluster_before(&mut self, start: u64) -> MediaResult<Option<u64>> {
        let clusters = self.clusters()?;
        Ok(clusters
            .iter()
            .rposition(|cluster| cluster.start < start)
            .map(|index| clusters[index].start))
    }

    /// Every cluster's position, scanned from the first.
    fn clusters(&mut self) -> MediaResult<&[ClusterPlace]> {
        if self.clusters.is_none() {
            let mut clusters = Vec::new();
            let mut position = self.segment.first_cluster;
            while let Some(start) = position {
                if self.segment.end.is_some_and(|end| start >= end) {
                    break;
                }
                self.source.seek(start)?;
                let Some(element) = self.source.header()? else {
                    break;
                };
                if element.id != CLUSTER {
                    position = element.end();
                    continue;
                }
                let (timestamp, end) = self.cluster_extent(&element)?;
                clusters.push(ClusterPlace {
                    start,
                    timestamp,
                    end: element.end(),
                });
                position = end;
            }
            self.clusters = Some(clusters);
        }
        Ok(self.clusters.as_deref().unwrap_or_default())
    }

    /// A cluster's timestamp and where the element after it starts (found by reading through
    /// a cluster of unknown size).
    fn cluster_extent(&mut self, cluster: &Header) -> MediaResult<(u64, Option<u64>)> {
        let mut position = cluster.data_start;
        let mut timestamp = None;
        while cluster.end().is_none_or(|end| position < end) {
            self.source.seek(position)?;
            let Some(child) = self.source.header()? else {
                return Ok((timestamp.unwrap_or(0), None));
            };
            if cluster.size.is_none() && !is_cluster_child(child.id) {
                return Ok((timestamp.unwrap_or(0), Some(position)));
            }
            if child.id == CLUSTER_TIMESTAMP {
                timestamp = Some(unsigned(&self.source.data(&child)?)?);
                if cluster.size.is_some() {
                    break;
                }
            }
            position = child
                .end()
                .ok_or_else(|| MediaError("an element of unknown size inside a cluster".into()))?;
        }
        Ok((timestamp.unwrap_or(0), cluster.end()))
    }

    /// The file's duration when its info omits it: where the last frame of any track ends,
    /// read from the last clusters.
    fn measure_duration(&mut self) -> MediaResult<Option<f64>> {
        let clusters = self.clusters()?.to_vec();
        let saved = self.cursor;
        let durations: Vec<(u64, Option<u64>)> = self
            .segment
            .tracks
            .iter()
            .map(|track| (track.number, track.default_duration_ns))
            .collect();
        let mut end_ns: Option<i64> = None;
        for cluster in clusters.iter().rev().take(3) {
            end_ns = end_ns.max(self.frames_end_in(cluster, &durations)?);
        }
        self.cursor = saved;
        Ok(end_ns
            .filter(|end| *end > 0)
            .map(|end| end as f64 / NANOSECONDS_PER_SECOND))
    }

    /// Where the last frame of any track in `cluster` ends, in nanoseconds; `durations` holds
    /// each track's number and default frame duration.
    fn frames_end_in(
        &mut self,
        cluster: &ClusterPlace,
        durations: &[(u64, Option<u64>)],
    ) -> MediaResult<Option<i64>> {
        self.source.seek(cluster.start)?;
        let Some(element) = self.source.header()? else {
            return Ok(None);
        };
        let mut end_ns = None;
        let mut position = element.data_start;
        while cluster.end.is_none_or(|end| position < end) {
            self.source.seek(position)?;
            let Some(child) = self.source.header()? else {
                break;
            };
            if element.size.is_none() && !is_cluster_child(child.id) {
                break;
            }
            let Some(next) = child.end() else { break };
            if matches!(child.id, SIMPLE_BLOCK | BLOCK_GROUP) {
                let data = self.source.data(&child)?;
                end_ns = end_ns.max(self.block_end_ns(child.id, data, cluster, durations)?);
            }
            position = next;
        }
        Ok(end_ns)
    }

    /// Where a `SimpleBlock`'s or `BlockGroup`'s frames end, in nanoseconds: its stated
    /// duration, else the track's default per laced frame. `None` if its header can't be read.
    fn block_end_ns(
        &self,
        id: u32,
        data: Vec<u8>,
        cluster: &ClusterPlace,
        durations: &[(u64, Option<u64>)],
    ) -> MediaResult<Option<i64>> {
        let (block, duration) = if id == SIMPLE_BLOCK {
            (data, None)
        } else {
            let (block, units) = block_group_parts(&data)?;
            (
                block,
                units.map(|units| self.segment.nanoseconds(units as i64)),
            )
        };
        let Ok(header) = block_header(&block) else {
            return Ok(None);
        };
        let start = self
            .segment
            .nanoseconds(cluster.timestamp as i64 + i64::from(header.relative_timestamp));
        let lace = frames(&block, &header).map_or(1, |frames| frames.len()) as i64;
        let default = durations
            .iter()
            .find(|(number, _)| *number == header.track)
            .and_then(|(_, duration)| *duration);
        let length = duration
            .or(default.map(|each| each as i64 * lace))
            .unwrap_or(0);
        Ok(Some(start + length))
    }

    /// The track's next block: its position, plus its frames when `read` asks for them.
    fn next_block(&mut self, read: BlockRead) -> MediaResult<Option<ReadBlock>> {
        loop {
            match self.cursor {
                Cursor::Finished => return Ok(None),
                Cursor::TopLevel(position) => self.cursor = self.enter_top_level(position)?,
                Cursor::InCluster { cluster, position } => {
                    if let Some(block) = self.step_in_cluster(cluster, position, read)? {
                        return Ok(Some(block));
                    }
                }
            }
        }
    }

    /// Where reading goes from the top-level element at `position`: into it if it's a cluster,
    /// past it otherwise.
    fn enter_top_level(&mut self, position: u64) -> MediaResult<Cursor> {
        if self.segment.end.is_some_and(|end| position >= end) {
            return Ok(Cursor::Finished);
        }
        self.source.seek(position)?;
        let Some(element) = self.source.header()? else {
            return Ok(Cursor::Finished);
        };
        if element.id != CLUSTER {
            // A top-level element of unknown size other than a cluster can't be skipped.
            return Ok(element.end().map_or(Cursor::Finished, Cursor::TopLevel));
        }
        Ok(Cursor::InCluster {
            cluster: ClusterPlace {
                start: position,
                timestamp: 0,
                end: element.end(),
            },
            position: element.data_start,
        })
    }

    /// Reads the element at `position` in `cluster` and moves the cursor past it; returns the
    /// track's block if it is one.
    fn step_in_cluster(
        &mut self,
        mut cluster: ClusterPlace,
        position: u64,
        read: BlockRead,
    ) -> MediaResult<Option<ReadBlock>> {
        if cluster.end.is_some_and(|end| position >= end) {
            self.cursor = Cursor::TopLevel(position);
            return Ok(None);
        }
        self.source.seek(position)?;
        let Some(child) = self.source.header()? else {
            self.cursor = Cursor::Finished;
            return Ok(None);
        };
        if cluster.end.is_none() && !is_cluster_child(child.id) {
            self.cursor = Cursor::TopLevel(position);
            return Ok(None);
        }
        let next = child
            .end()
            .ok_or_else(|| MediaError("an element of unknown size inside a cluster".into()))?;
        let block = match child.id {
            CLUSTER_TIMESTAMP => {
                cluster.timestamp = unsigned(&self.source.data(&child)?)?;
                None
            }
            SIMPLE_BLOCK => self.read_simple_block(&child, cluster, read)?,
            BLOCK_GROUP => self.read_block_group(&child, cluster, read)?,
            _ => None,
        };
        self.cursor = Cursor::InCluster {
            cluster,
            position: next,
        };
        Ok(block.map(|mut block| {
            block.place.element = position;
            block
        }))
    }

    /// A `SimpleBlock` of the track, or `None` if it belongs to another track. Reads only its
    /// head when `read` asks for its position alone.
    fn read_simple_block(
        &mut self,
        element: &Header,
        cluster: ClusterPlace,
        read: BlockRead,
    ) -> MediaResult<Option<ReadBlock>> {
        let size = element.size.unwrap_or(0) as usize;
        let head = self.source.read(size.min(12))?;
        let header = block_header(&head)?;
        if header.track != self.track.number {
            return Ok(None);
        }
        let keyframe = header.flags & KEYFRAME != 0;
        let data = match read {
            BlockRead::Frames => self.source.data(element)?,
            BlockRead::Place => head,
        };
        let stored = StoredBlock {
            keyframe,
            duration_ns: None,
            discard_padding_ns: 0,
        };
        Ok(Some(self.block(&data, header, cluster, stored, read)?))
    }

    /// A `BlockGroup`'s block for the track, or `None` if it belongs to another track or has
    /// none. It is a key frame unless it references another.
    fn read_block_group(
        &mut self,
        element: &Header,
        cluster: ClusterPlace,
        read: BlockRead,
    ) -> MediaResult<Option<ReadBlock>> {
        let parts = self.source.data(element)?;
        let mut block = None;
        let (mut duration, mut referenced, mut discard) = (None, false, 0);
        for (id, data) in children(&parts)? {
            match id {
                BLOCK => block = Some(data),
                BLOCK_DURATION => duration = Some(unsigned(data)? as i64),
                REFERENCE_BLOCK => referenced = true,
                DISCARD_PADDING => discard = signed(data)?,
                _ => {}
            }
        }
        let Some(data) = block else { return Ok(None) };
        let header = block_header(data)?;
        if header.track != self.track.number {
            return Ok(None);
        }
        let stored = StoredBlock {
            keyframe: !referenced,
            duration_ns: duration.map(|units| self.segment.nanoseconds(units)),
            discard_padding_ns: discard,
        };
        Ok(Some(self.block(data, header, cluster, stored, read)?))
    }

    fn block(
        &self,
        data: &[u8],
        header: block::BlockHeader,
        cluster: ClusterPlace,
        stored: StoredBlock,
        read: BlockRead,
    ) -> MediaResult<ReadBlock> {
        let StoredBlock {
            keyframe,
            duration_ns,
            discard_padding_ns,
        } = stored;
        let timestamp_ns = self
            .segment
            .nanoseconds(cluster.timestamp as i64 + i64::from(header.relative_timestamp));
        let place = BlockPlace {
            element: 0,
            cluster,
            timestamp_ns,
            keyframe,
        };
        if read == BlockRead::Place {
            return Ok(ReadBlock {
                place,
                packets: Vec::new(),
            });
        }
        let ranges = frames(data, &header)?;
        let count = ranges.len();
        let each = self
            .track
            .default_duration_ns
            .map(|duration| duration as i64);
        let packets = ranges
            .into_iter()
            .enumerate()
            .map(|(index, range)| {
                let mut frame = Vec::with_capacity(self.track.stripped_header.len() + range.len());
                frame.extend_from_slice(&self.track.stripped_header);
                frame.extend_from_slice(&data[range]);
                let last = index + 1 == count;
                Packet {
                    data: frame,
                    timestamp_ns: timestamp_ns + each.unwrap_or(0) * index as i64,
                    duration_ns: if count == 1 {
                        duration_ns.or(each)
                    } else {
                        each
                    },
                    keyframe,
                    lace_index: index,
                    discard_padding_ns: if last { discard_padding_ns } else { 0 },
                }
            })
            .collect();
        Ok(ReadBlock { place, packets })
    }
}

struct ReadBlock {
    place: BlockPlace,
    packets: Vec<Packet>,
}

/// How much of a block to read: its position only (to seek), or its frames too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockRead {
    Place,
    Frames,
}

/// What a block says about its frames besides its header: whether they're a key frame, their
/// duration if stated, and the unplayed sound at the end of its last frame.
#[derive(Clone, Copy, Debug)]
struct StoredBlock {
    keyframe: bool,
    duration_ns: Option<i64>,
    discard_padding_ns: i64,
}

/// A `BlockGroup`'s block and its duration in timestamp units, if stated.
fn block_group_parts(data: &[u8]) -> MediaResult<(Vec<u8>, Option<u64>)> {
    let parts = children(data)?;
    let block = parts
        .iter()
        .find(|(id, _)| *id == BLOCK)
        .map(|(_, data)| data.to_vec())
        .unwrap_or_default();
    let duration = parts
        .iter()
        .find(|(id, _)| *id == BLOCK_DURATION)
        .map(|(_, data)| unsigned(data))
        .transpose()?;
    Ok((block, duration))
}

fn is_cluster_child(id: u32) -> bool {
    id == CLUSTER_TIMESTAMP
        || id == SIMPLE_BLOCK
        || id == BLOCK_GROUP
        || OTHER_CLUSTER_CHILDREN.contains(&id)
}

#[cfg(test)]
pub(crate) mod tests;
