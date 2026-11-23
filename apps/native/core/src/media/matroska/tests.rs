//! Tests the demuxer against files built here element by element: known and unknown sizes,
//! with and without cues and a duration, laced and stripped frames, and block groups.

use super::*;

/// An element with its ID as the spec writes it and the shortest size encoding.
pub(crate) fn element(id: u32, data: &[u8]) -> Vec<u8> {
    let mut bytes: Vec<u8> = id
        .to_be_bytes()
        .into_iter()
        .skip_while(|byte| *byte == 0)
        .collect();
    let length = (1..=8)
        .find(|length| (data.len() as u64) < (1_u64 << (7 * length)) - 1)
        .unwrap_or(8);
    let size = data.len() as u64 | (1 << (7 * length));
    bytes.extend(&size.to_be_bytes()[8 - length..]);
    bytes.extend_from_slice(data);
    bytes
}

/// An element whose size is written as unknown.
fn unsized_element(id: u32, data: &[u8]) -> Vec<u8> {
    let mut bytes: Vec<u8> = id
        .to_be_bytes()
        .into_iter()
        .skip_while(|byte| *byte == 0)
        .collect();
    bytes.push(0xff);
    bytes.extend_from_slice(data);
    bytes
}

pub(crate) fn uint(id: u32, value: u64) -> Vec<u8> {
    let bytes: Vec<u8> = value
        .to_be_bytes()
        .into_iter()
        .skip_while(|byte| *byte == 0)
        .collect();
    element(id, &bytes)
}

/// An unsigned integer always eight bytes long, so a position can be filled in later.
fn wide_uint(id: u32, value: u64) -> Vec<u8> {
    element(id, &value.to_be_bytes())
}

pub(crate) fn header(doc_type: &str) -> Vec<u8> {
    element(EBML, &element(DOC_TYPE, doc_type.as_bytes()))
}

fn simple_block(track: u8, relative: i16, keyframe: bool, frame: &[u8]) -> Vec<u8> {
    let mut data = vec![0x80 | track];
    data.extend(relative.to_be_bytes());
    data.push(if keyframe { KEYFRAME } else { 0 });
    data.extend_from_slice(frame);
    element(SIMPLE_BLOCK, &data)
}

/// Two frames with EBML lacing.
fn laced_block(track: u8, relative: i16, first: &[u8], second: &[u8]) -> Vec<u8> {
    let mut data = vec![0x80 | track];
    data.extend(relative.to_be_bytes());
    data.push(KEYFRAME | 0x06);
    data.push(1);
    data.push(0x80 | first.len() as u8);
    data.extend_from_slice(first);
    data.extend_from_slice(second);
    element(SIMPLE_BLOCK, &data)
}

const FRAMES: u64 = 75;
const FRAME_MS: u64 = 40;
const KEY_EVERY: u64 = 10;
const FRAMES_PER_CLUSTER: u64 = 25;

/// How the test file is written.
#[derive(Clone, Copy)]
struct Layout {
    sizes_known: bool,
    cues: bool,
    duration: bool,
}

/// Three seconds of 25 fps video (a key frame every 10) and a sound track with two 20 ms frames
/// laced per block, in one-second clusters.
fn test_file(layout: Layout) -> Vec<u8> {
    let info = test_info(layout);
    let tracks = test_tracks();
    let clusters: Vec<Vec<u8>> = (0..FRAMES / FRAMES_PER_CLUSTER)
        .map(|cluster| test_cluster(cluster, layout))
        .collect();
    let before = info.len()
        + tracks.len()
        + if layout.cues {
            test_cues(&[0; 3]).len()
        } else {
            0
        };
    let positions: Vec<u64> = clusters
        .iter()
        .scan(before as u64, |at, cluster| {
            let start = *at;
            *at += cluster.len() as u64;
            Some(start)
        })
        .collect();
    let mut segment = [info, tracks].concat();
    if layout.cues {
        segment.extend(test_cues(&positions));
    }
    segment.extend(clusters.concat());
    let mut file = header("webm");
    file.extend(layout.element(SEGMENT, &segment));
    file
}

impl Layout {
    /// An element with its size, or of unknown size when the layout omits sizes.
    fn element(self, id: u32, data: &[u8]) -> Vec<u8> {
        if self.sizes_known {
            element(id, data)
        } else {
            unsized_element(id, data)
        }
    }
}

fn test_info(layout: Layout) -> Vec<u8> {
    let mut data = uint(TIMESTAMP_SCALE, 1_000_000);
    if layout.duration {
        data.extend(element(
            DURATION,
            &(FRAMES as f64 * FRAME_MS as f64).to_be_bytes(),
        ));
    }
    element(INFO, &data)
}

fn test_tracks() -> Vec<u8> {
    let mut video = uint(TRACK_NUMBER, 1);
    video.extend(uint(TRACK_TYPE, 1));
    video.extend(element(CODEC_ID, b"V_VP9"));
    video.extend(uint(DEFAULT_DURATION, FRAME_MS * 1_000_000));
    video.extend(element(
        VIDEO,
        &[uint(PIXEL_WIDTH, 64), uint(PIXEL_HEIGHT, 36)].concat(),
    ));
    let mut audio = uint(TRACK_NUMBER, 2);
    audio.extend(uint(TRACK_TYPE, 2));
    audio.extend(element(CODEC_ID, b"A_PCM/INT/LIT"));
    audio.extend(uint(DEFAULT_DURATION, 20_000_000));
    audio.extend(element(
        AUDIO,
        &[
            element(SAMPLING_FREQUENCY, &48_000_f64.to_be_bytes()),
            uint(CHANNELS, 1),
            uint(BIT_DEPTH, 16),
        ]
        .concat(),
    ));
    element(
        TRACKS,
        &[element(TRACK_ENTRY, &video), element(TRACK_ENTRY, &audio)].concat(),
    )
}

/// Cluster `cluster`: a second of video frames, each next to a laced block of sound.
fn test_cluster(cluster: u64, layout: Layout) -> Vec<u8> {
    let mut data = uint(CLUSTER_TIMESTAMP, cluster * FRAMES_PER_CLUSTER * FRAME_MS);
    for index in 0..FRAMES_PER_CLUSTER {
        let frame = cluster * FRAMES_PER_CLUSTER + index;
        let relative = (index * FRAME_MS) as i16;
        data.extend(simple_block(
            1,
            relative,
            frame.is_multiple_of(KEY_EVERY),
            &[frame as u8, 0xee],
        ));
        data.extend(laced_block(
            2,
            relative,
            &[frame as u8, 1],
            &[frame as u8, 2, 2],
        ));
    }
    layout.element(CLUSTER, &data)
}

/// Cues for every key frame, given the clusters' `positions`. Cues precede the clusters, with
/// eight-byte positions so their length is known before the positions are.
fn test_cues(positions: &[u64]) -> Vec<u8> {
    let mut data = Vec::new();
    for frame in (0..FRAMES).step_by(KEY_EVERY as usize) {
        let cluster = positions[(frame / FRAMES_PER_CLUSTER) as usize];
        let place = element(
            CUE_TRACK_POSITIONS,
            &[uint(CUE_TRACK, 1), wide_uint(CUE_CLUSTER_POSITION, cluster)].concat(),
        );
        data.extend(element(
            CUE_POINT,
            &[uint(CUE_TIME, frame * FRAME_MS), place].concat(),
        ));
    }
    element(CUES, &data)
}

fn written(bytes: &[u8]) -> (tempfile::TempDir, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("test.webm");
    std::fs::write(&path, bytes).unwrap();
    let path = path.to_string_lossy().into_owned();
    (directory, path)
}

fn all_packets(demuxer: &mut Demuxer) -> Vec<Packet> {
    std::iter::from_fn(|| demuxer.next_packet().unwrap()).collect()
}

const EVERY_LAYOUT: [Layout; 3] = [
    Layout {
        sizes_known: true,
        cues: true,
        duration: true,
    },
    Layout {
        sizes_known: true,
        cues: false,
        duration: true,
    },
    Layout {
        sizes_known: false,
        cues: false,
        duration: false,
    },
];

#[test]
fn a_video_track_reads_in_order_with_its_times_and_key_frames() {
    for layout in EVERY_LAYOUT {
        let (_directory, path) = written(&test_file(layout));
        assert!(is_matroska(&path));
        let mut video = Demuxer::open(&path, TrackKind::Video).unwrap().unwrap();
        assert_eq!(video.track().codec_id, "V_VP9");
        assert_eq!(
            video
                .track()
                .video
                .as_ref()
                .map(|video| (video.width, video.height)),
            Some((64, 36))
        );
        // A live-written file has no duration; it ends where the last frame ends.
        assert_eq!(video.segment().duration_seconds, Some(3.0));
        assert_eq!(video.seeks_by_cues(), layout.cues);
        let packets = all_packets(&mut video);
        assert_eq!(packets.len(), FRAMES as usize);
        for (index, packet) in packets.iter().enumerate() {
            assert_eq!(packet.data, vec![index as u8, 0xee]);
            assert_eq!(packet.timestamp_ns, index as i64 * 40_000_000);
            assert_eq!(packet.keyframe, (index as u64).is_multiple_of(KEY_EVERY));
            assert_eq!(packet.duration_ns, Some(40_000_000));
        }
    }
}

#[test]
fn laced_frames_come_out_one_by_one_timed_from_their_block() {
    let (_directory, path) = written(&test_file(EVERY_LAYOUT[0]));
    let mut audio = Demuxer::open(&path, TrackKind::Audio).unwrap().unwrap();
    let packets = all_packets(&mut audio);
    assert_eq!(packets.len(), 2 * FRAMES as usize);
    assert_eq!(packets[6].data, vec![3, 1]);
    assert_eq!(packets[7].data, vec![3, 2, 2]);
    assert_eq!(
        (packets[6].timestamp_ns, packets[6].lace_index),
        (120_000_000, 0)
    );
    assert_eq!(
        (packets[7].timestamp_ns, packets[7].lace_index),
        (140_000_000, 1)
    );
}

#[test]
fn seeks_land_on_the_same_key_frame_with_or_without_cues() {
    for layout in EVERY_LAYOUT {
        for ignore_cues in [false, true] {
            let (_directory, path) = written(&test_file(layout));
            let mut video = Demuxer::open(&path, TrackKind::Video).unwrap().unwrap();
            if ignore_cues {
                video.ignore_cues();
            }
            let mut first_after = |seconds: f64| {
                video.seek((seconds * 1e9) as i64, true).unwrap();
                video
                    .next_packet()
                    .unwrap()
                    .map(|packet| packet.timestamp_ns as f64 / 1e9)
            };
            // Key frames are every 0.4 s; clusters start every second.
            assert_eq!(first_after(1.5), Some(1.2));
            assert_eq!(first_after(1.2), Some(1.2));
            assert_eq!(first_after(1.199), Some(0.8));
            // The cluster at 1 s starts with a frame that is not a key frame.
            assert_eq!(first_after(1.0), Some(0.8));
            assert_eq!(first_after(0.0), Some(0.0));
            assert_eq!(first_after(-1.0), Some(0.0));
            assert_eq!(first_after(99.0), Some(2.8));
            // Reading goes on from there in order.
            video.seek(2_000_000_000, true).unwrap();
            let rest: Vec<i64> = all_packets(&mut video)
                .iter()
                .map(|packet| packet.timestamp_ns / 40_000_000)
                .collect();
            assert_eq!(rest, (50..75).collect::<Vec<_>>());
        }
    }
}

#[test]
fn a_sound_track_seeks_to_the_frame_at_or_before_the_time() {
    for layout in EVERY_LAYOUT {
        let (_directory, path) = written(&test_file(layout));
        let mut audio = Demuxer::open(&path, TrackKind::Audio).unwrap().unwrap();
        // The video's cues are not the sound's: its seeks go through the clusters.
        assert!(!audio.seeks_by_cues());
        audio.seek(1_010_000_000, false).unwrap();
        let packet = audio.next_packet().unwrap().unwrap();
        assert_eq!((packet.timestamp_ns, packet.lace_index), (1_000_000_000, 0));
        audio.seek(1_030_000_000, false).unwrap();
        assert_eq!(
            audio.next_packet().unwrap().unwrap().timestamp_ns,
            1_000_000_000
        );
    }
}

#[test]
fn block_groups_say_key_frames_by_their_references_and_carry_padding() {
    let tracks = {
        let mut audio = uint(TRACK_NUMBER, 1);
        audio.extend(uint(TRACK_TYPE, 2));
        audio.extend(element(CODEC_ID, b"A_OPUS"));
        // Header stripping: every frame starts with 0xfc, which the file leaves out.
        let compression = element(
            CONTENT_COMPRESSION,
            &[
                uint(CONTENT_COMPRESSION_ALGORITHM, 3),
                element(CONTENT_COMPRESSION_SETTINGS, &[0xfc]),
            ]
            .concat(),
        );
        audio.extend(element(
            CONTENT_ENCODINGS,
            &element(CONTENT_ENCODING, &compression),
        ));
        element(TRACKS, &element(TRACK_ENTRY, &audio))
    };
    let group = |relative: i16, reference: bool, padding: i64| {
        let mut block = vec![0x81];
        block.extend(relative.to_be_bytes());
        block.extend([0, 7]);
        let mut data = element(BLOCK, &block);
        data.extend(uint(BLOCK_DURATION, 20));
        if reference {
            data.extend(element(REFERENCE_BLOCK, &[0xec]));
        }
        if padding != 0 {
            data.extend(element(DISCARD_PADDING, &padding.to_be_bytes()));
        }
        element(BLOCK_GROUP, &data)
    };
    let cluster = element(
        CLUSTER,
        &[
            uint(CLUSTER_TIMESTAMP, 0),
            group(0, false, 0),
            group(20, true, 13_000_000),
        ]
        .concat(),
    );
    let mut file = header("matroska");
    file.extend(element(
        SEGMENT,
        &[
            element(INFO, &uint(TIMESTAMP_SCALE, 1_000_000)),
            tracks,
            cluster,
        ]
        .concat(),
    ));
    let (_directory, path) = written(&file);
    let mut audio = Demuxer::open(&path, TrackKind::Audio).unwrap().unwrap();
    assert_eq!(audio.segment().duration_seconds, Some(0.04));
    assert!(Demuxer::open(&path, TrackKind::Video).unwrap().is_none());
    let packets = all_packets(&mut audio);
    assert_eq!(
        packets
            .iter()
            .map(|packet| packet.data.clone())
            .collect::<Vec<_>>(),
        vec![vec![0xfc, 7], vec![0xfc, 7]]
    );
    assert_eq!(
        packets
            .iter()
            .map(|packet| packet.keyframe)
            .collect::<Vec<_>>(),
        vec![true, false]
    );
    assert_eq!(packets[1].duration_ns, Some(20_000_000));
    assert_eq!(packets[1].discard_padding_ns, 13_000_000);
}

#[test]
fn files_that_are_not_matroska_are_refused() {
    let (_directory, path) = written(b"\x00\x00\x00\x18ftypmp42");
    assert!(!is_matroska(&path));
    let mut other = element(EBML, &element(DOC_TYPE, b"other"));
    other.extend(element(SEGMENT, &[]));
    let (_directory, path) = written(&other);
    assert!(Demuxer::open(&path, TrackKind::Video).is_err());
    // Compressed frames cannot be read.
    let compression = element(CONTENT_COMPRESSION, &uint(CONTENT_COMPRESSION_ALGORITHM, 0));
    let entry = [
        uint(TRACK_NUMBER, 1),
        uint(TRACK_TYPE, 1),
        element(CONTENT_ENCODINGS, &element(CONTENT_ENCODING, &compression)),
    ]
    .concat();
    let mut zlib = header("matroska");
    zlib.extend(element(
        SEGMENT,
        &element(TRACKS, &element(TRACK_ENTRY, &entry)),
    ));
    let (_directory, path) = written(&zlib);
    assert!(Demuxer::open(&path, TrackKind::Video).is_err());
}

/// A second of sound in a WebM, after a video track that is enabled if `video` is true.
fn sound_file(video: Option<bool>) -> Vec<u8> {
    let mut entries = Vec::new();
    if let Some(enabled) = video {
        let mut track = [
            uint(TRACK_NUMBER, 1),
            uint(TRACK_TYPE, 1),
            element(CODEC_ID, b"V_VP9"),
        ]
        .concat();
        track.extend(uint(FLAG_ENABLED, u64::from(enabled)));
        track.extend(element(
            VIDEO,
            &[uint(PIXEL_WIDTH, 64), uint(PIXEL_HEIGHT, 36)].concat(),
        ));
        entries.extend(element(TRACK_ENTRY, &track));
    }
    let mut sound = [
        uint(TRACK_NUMBER, 2),
        uint(TRACK_TYPE, 2),
        element(CODEC_ID, b"A_PCM/INT/LIT"),
    ]
    .concat();
    sound.extend(element(
        AUDIO,
        &[
            element(SAMPLING_FREQUENCY, &48_000_f64.to_be_bytes()),
            uint(CHANNELS, 1),
            uint(BIT_DEPTH, 16),
        ]
        .concat(),
    ));
    entries.extend(element(TRACK_ENTRY, &sound));
    let info = element(
        INFO,
        &[
            uint(TIMESTAMP_SCALE, 1_000_000),
            element(DURATION, &1000_f64.to_be_bytes()),
        ]
        .concat(),
    );
    let cluster = element(
        CLUSTER,
        &[
            uint(CLUSTER_TIMESTAMP, 0),
            simple_block(2, 0, true, &[0; 96]),
        ]
        .concat(),
    );
    let mut file = header("webm");
    file.extend(element(
        SEGMENT,
        &[info, element(TRACKS, &entries), cluster].concat(),
    ));
    file
}

/// A `.webm` or `.mkv` whose headers name no video track the reader would open is sound, so a
/// voice note in a WebM is listed, shown and played as sound.
#[test]
fn matroska_with_no_video_track_is_sound() {
    use crate::workspace::{media_of_file, read_file, FileRead, Media};
    let directory = tempfile::tempdir().unwrap();
    let write = |name: &str, bytes: &[u8]| {
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };
    let film = test_file(EVERY_LAYOUT[0]);
    let sound = sound_file(None);
    let disabled = sound_file(Some(false));
    for (name, bytes, video, media) in [
        ("film.webm", &film, Some(true), Media::Video),
        ("film.MKV", &film, Some(true), Media::Video),
        ("voice.webm", &sound, Some(false), Media::Audio),
        ("voice.mkv", &sound, Some(false), Media::Audio),
        ("voice.mka", &sound, Some(false), Media::Audio),
        ("muted-picture.webm", &disabled, Some(false), Media::Audio),
        (
            "enabled-picture.webm",
            &sound_file(Some(true)),
            Some(true),
            Media::Video,
        ),
        // Unreadable headers leave the file as its extension says.
        (
            "broken.webm",
            &b"not EBML at all".to_vec(),
            None,
            Media::Video,
        ),
        (
            "broken.mka",
            &b"not EBML at all".to_vec(),
            None,
            Media::Audio,
        ),
    ] {
        let path = write(name, bytes);
        assert_eq!(holds_video(&path.to_string_lossy()), video, "{name}");
        assert_eq!(media_of_file(&path), Some(media), "{name}");
        assert_eq!(read_file(&path), FileRead::Media(media), "{name}");
    }
    // Cached per version of the file: rewritten with a picture, it is a film again.
    let path = write("voice.webm", &film);
    assert_eq!(media_of_file(&path), Some(Media::Video));
    assert_eq!(
        media_of_file(&directory.path().join("gone.webm")),
        Some(Media::Video),
        "by its name"
    );
    // Every other kind goes by extension alone; no file is read.
    assert_eq!(
        media_of_file(&write("clip.mp4", &sound)),
        Some(Media::Video)
    );
}

/// How a copy laces its frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lacing {
    Xiph,
    Ebml,
    Fixed,
}

/// A copy of a file's sound track with frames laced `per_block` to a block (FFmpeg doesn't
/// lace), in clusters of about a second: the same frames, stored differently.
pub(crate) fn laced_copy(source: &str, out: &str, lacing: Lacing, per_block: usize) {
    let mut demuxer = Demuxer::open(source, TrackKind::Audio).unwrap().unwrap();
    let scale = demuxer.segment().timestamp_scale;
    let duration = demuxer.segment().duration_seconds.unwrap_or(0.0);
    let info = element(
        INFO,
        &[
            uint(TIMESTAMP_SCALE, scale),
            element(DURATION, &(duration * 1e9 / scale as f64).to_be_bytes()),
        ]
        .concat(),
    );
    let tracks = element(
        TRACKS,
        &element(TRACK_ENTRY, &sound_track_entry(demuxer.track())),
    );
    let blocks = group_into_blocks(all_packets(&mut demuxer), lacing, per_block);
    let units = |ns: i64| ns.div_euclid(scale as i64);
    let cluster_length = 1_000_000_000 / scale as i64;
    let mut clusters = Vec::new();
    let mut cluster: Option<(i64, Vec<u8>)> = None;
    for block in blocks {
        let time = units(block[0].timestamp_ns);
        if cluster
            .as_ref()
            .is_none_or(|(start, _)| time - start >= cluster_length)
        {
            clusters.extend(cluster.take().map(cluster_element));
            cluster = Some((time.max(0), Vec::new()));
        }
        let (start, data) = cluster.as_mut().unwrap();
        let relative = i16::try_from(time - *start).unwrap();
        data.extend(block_element(&block, lacing, relative));
    }
    clusters.extend(cluster.map(cluster_element));
    let mut file = header("matroska");
    file.extend(element(
        SEGMENT,
        &[info, tracks, clusters.concat()].concat(),
    ));
    std::fs::write(out, file).unwrap();
}

/// The track entry of a copied sound track: codec, delays and audio settings as read.
fn sound_track_entry(track: &Track) -> Vec<u8> {
    let audio = track.audio.as_ref().unwrap();
    let mut entry = [
        uint(TRACK_NUMBER, 1),
        uint(TRACK_TYPE, 2),
        element(CODEC_ID, track.codec_id.as_bytes()),
    ]
    .concat();
    if !track.codec_private.is_empty() {
        entry.extend(element(CODEC_PRIVATE, &track.codec_private));
    }
    if track.codec_delay_ns > 0 {
        entry.extend(uint(CODEC_DELAY, track.codec_delay_ns));
    }
    if track.seek_pre_roll_ns > 0 {
        entry.extend(uint(SEEK_PRE_ROLL, track.seek_pre_roll_ns));
    }
    let mut settings = [
        element(SAMPLING_FREQUENCY, &audio.sample_rate.to_be_bytes()),
        uint(CHANNELS, u64::from(audio.channels)),
    ]
    .concat();
    if let Some(bits) = audio.bit_depth {
        settings.extend(uint(BIT_DEPTH, u64::from(bits)));
    }
    entry.extend(element(AUDIO, &settings));
    entry
}

/// Blocks: runs of frames, a run ending early at a frame with end padding (which gets its own
/// block group) or when a fixed lace's frame size changes.
fn group_into_blocks(packets: Vec<Packet>, lacing: Lacing, per_block: usize) -> Vec<Vec<Packet>> {
    let mut blocks: Vec<Vec<Packet>> = Vec::new();
    for packet in packets {
        let start_new = blocks
            .last()
            .is_none_or(|block| block_ends_before(block, &packet, lacing, per_block));
        if start_new {
            blocks.push(Vec::new());
        }
        blocks.last_mut().unwrap().push(packet);
    }
    blocks
}

/// Whether `next` starts a new block rather than joining `block`.
fn block_ends_before(block: &[Packet], next: &Packet, lacing: Lacing, per_block: usize) -> bool {
    let full = block.len() >= per_block;
    let padded = block
        .last()
        .is_some_and(|last| last.discard_padding_ns != 0)
        || next.discard_padding_ns != 0;
    let size_changes = lacing == Lacing::Fixed && block[0].data.len() != next.data.len();
    full || padded || size_changes
}

/// A cluster element starting at `start` (in timestamp units) holding `data`.
fn cluster_element((start, data): (i64, Vec<u8>)) -> Vec<u8> {
    element(
        CLUSTER,
        &[uint(CLUSTER_TIMESTAMP, start.max(0) as u64), data].concat(),
    )
}

/// One block of frames on track 1, laced if it holds more than one: a simple block, or a block
/// group carrying the last frame's discard padding.
fn block_element(block: &[Packet], lacing: Lacing, relative: i16) -> Vec<u8> {
    let mut bytes = vec![0x81];
    bytes.extend(relative.to_be_bytes());
    let laced = block.len() > 1;
    let flag = match (laced, lacing) {
        (false, _) => 0,
        (true, Lacing::Xiph) => 0x02,
        (true, Lacing::Fixed) => 0x04,
        (true, Lacing::Ebml) => 0x06,
    };
    let padding = block.last().map_or(0, |last| last.discard_padding_ns);
    bytes.push(flag | if padding == 0 { KEYFRAME } else { 0 });
    if laced {
        bytes.push((block.len() - 1) as u8);
        let sizes: Vec<usize> = block[..block.len() - 1]
            .iter()
            .map(|packet| packet.data.len())
            .collect();
        write_lace_sizes(&mut bytes, &sizes, lacing);
    }
    for packet in block {
        bytes.extend(&packet.data);
    }
    if padding == 0 {
        return element(SIMPLE_BLOCK, &bytes);
    }
    element(
        BLOCK_GROUP,
        &[
            element(BLOCK, &bytes),
            element(DISCARD_PADDING, &padding.to_be_bytes()),
        ]
        .concat(),
    )
}

/// Writes the sizes of every frame but the last, as `lacing` stores them.
fn write_lace_sizes(bytes: &mut Vec<u8>, sizes: &[usize], lacing: Lacing) {
    match lacing {
        Lacing::Xiph => {
            for size in sizes {
                bytes.extend(std::iter::repeat_n(255, size / 255));
                bytes.push((size % 255) as u8);
            }
        }
        Lacing::Ebml => {
            let first = sizes[0] as u64;
            let length = (1..=8)
                .find(|length| first < (1_u64 << (7 * length)) - 1)
                .unwrap();
            bytes.extend(&(first | (1 << (7 * length))).to_be_bytes()[8 - length..]);
            for pair in sizes.windows(2) {
                write_ebml_lace_difference(bytes, pair[1] as i64 - pair[0] as i64);
            }
        }
        Lacing::Fixed => {}
    }
}

/// Writes one EBML lace size difference as a signed variable-length integer.
fn write_ebml_lace_difference(bytes: &mut Vec<u8>, difference: i64) {
    let length = (1..=8)
        .find(|length| difference.abs() < (1_i64 << (7 * length - 1)) - 1)
        .unwrap();
    let value = (difference + (1_i64 << (7 * length - 1)) - 1) as u64 | (1 << (7 * length));
    bytes.extend(&value.to_be_bytes()[8 - length..]);
}
