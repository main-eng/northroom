//! Sound packets decoded by AudioToolbox converters, for Matroska and WebM, which AVFoundation
//! can't read. Each stored packet goes in with the codec's configuration (its "magic cookie"),
//! and interleaved 32-bit float comes out at the stream's own rate in the output's channels.
//! Core Audio decodes AAC, Opus, Vorbis, FLAC, MP3, AC-3, E-AC-3, ALAC and PCM this way.
//!
//! Timing: a Matroska block's timestamp is rounded to the file's precision (usually 1 ms), so
//! each run of decoded sound is placed where the previous one ended, and a timestamp is trusted
//! only when it shows the stream jumped. After a seek, the first block's timestamp is snapped
//! back to the codec's grid of packet lengths, so a read after a seek matches a read from the
//! start.

use super::av::{check_status, four_character_code, OSStatus, StreamDescription};
use crate::media::matroska::codec::{audio_configuration, AudioCodec, AudioConfiguration};
use crate::media::matroska::{Demuxer, Packet, TrackKind};
use crate::media::{AudioDecoding, AudioSourceInfo, MediaError, MediaResult};
use std::ffi::c_void;
use std::ptr;

type ConverterRef = *mut c_void;

#[repr(C)]
struct AudioBuffer {
    channels: u32,
    size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    count: u32,
    buffers: [AudioBuffer; 1],
}

#[repr(C)]
struct PacketDescription {
    start: i64,
    variable_frames: u32,
    size: u32,
}

type InputCallback = unsafe extern "C" fn(
    converter: ConverterRef,
    packets: *mut u32,
    data: *mut AudioBufferList,
    descriptions: *mut *mut PacketDescription,
    user: *mut c_void,
) -> OSStatus;

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioConverterNew(
        source: *const StreamDescription,
        destination: *const StreamDescription,
        out: *mut ConverterRef,
    ) -> OSStatus;
    fn AudioConverterDispose(converter: ConverterRef) -> OSStatus;
    fn AudioConverterReset(converter: ConverterRef) -> OSStatus;
    fn AudioConverterSetProperty(
        converter: ConverterRef,
        property: u32,
        size: u32,
        data: *const c_void,
    ) -> OSStatus;
    fn AudioConverterFillComplexBuffer(
        converter: ConverterRef,
        input: InputCallback,
        user: *mut c_void,
        packets: *mut u32,
        output: *mut AudioBufferList,
        descriptions: *mut PacketDescription,
    ) -> OSStatus;
    fn AudioFormatGetProperty(
        property: u32,
        specifier_size: u32,
        specifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> OSStatus;
}

/// The input callback's status once it has handed over its packet. Not an error: it means
/// "nothing more for now", so the converter returns what it has decoded.
const NO_INPUT_NOW: OSStatus = 0x6e6f_6e65; // 'none'
const FLAG_IS_FLOAT: u32 = 1;
const FLAG_IS_BIG_ENDIAN: u32 = 2;
const FLAG_IS_SIGNED_INTEGER: u32 = 4;
const FLAG_IS_PACKED: u32 = 8;
/// Output frames requested per call; a longer packet takes several calls.
const FRAMES_PER_CALL: usize = 16_384;
/// How much earlier than requested a seek starts decoding. A packet that decodes to nothing
/// (Vorbis's first after a reset) leaves the frames laced after it in its block unplaced, so
/// they are dropped; starting this much earlier puts them before the target.
const SEEK_LEAD_NANOSECONDS: i64 = 300_000_000;

/// The packet the converter's input callback hands over next.
struct Input {
    data: Vec<u8>,
    /// Frames of linear PCM in `data`; zero for a compressed packet.
    pcm_frames: u32,
    channels: u32,
    pending: bool,
    /// The stream has ended, so the converter may flush what it holds.
    ended: bool,
    description: PacketDescription,
}

unsafe extern "C" fn provide_input(
    _converter: ConverterRef,
    packets: *mut u32,
    data: *mut AudioBufferList,
    descriptions: *mut *mut PacketDescription,
    user: *mut c_void,
) -> OSStatus {
    // SAFETY: `user` is the converter's `Input`, alive and not otherwise borrowed while the
    // converter runs; Core Audio passes valid pointers for the packet count and buffer list and
    // keeps the data handed over only until it calls again.
    unsafe {
        let input = &mut *user.cast::<Input>();
        if !input.pending {
            *packets = 0;
            return if input.ended { 0 } else { NO_INPUT_NOW };
        }
        input.pending = false;
        let buffer = &mut (*data).buffers[0];
        buffer.channels = input.channels;
        buffer.size = input.data.len() as u32;
        buffer.data = input.data.as_mut_ptr().cast();
        if input.pcm_frames > 0 {
            *packets = input.pcm_frames;
        } else {
            *packets = 1;
            input.description = PacketDescription {
                start: 0,
                variable_frames: 0,
                size: input.data.len() as u32,
            };
            if !descriptions.is_null() {
                *descriptions = &raw mut input.description;
            }
        }
        0
    }
}

/// An `AudioConverter` from a stream's packets to interleaved float.
struct Converter {
    raw: ConverterRef,
    input: Box<Input>,
    channels: usize,
    bytes_per_pcm_frame: u32,
}

impl Drop for Converter {
    fn drop(&mut self) {
        // SAFETY: the converter is live and owned here.
        unsafe { AudioConverterDispose(self.raw) };
    }
}

impl Converter {
    fn new(configuration: &AudioConfiguration, channels: u32) -> MediaResult<Self> {
        let rate = f64::from(configuration.sample_rate);
        let (source, bytes_per_pcm_frame) = match configuration.codec {
            AudioCodec::Pcm {
                bits,
                float,
                big_endian,
            } => {
                let bytes = bits / 8 * configuration.channels;
                let mut flags = FLAG_IS_PACKED;
                if float {
                    flags |= FLAG_IS_FLOAT;
                } else if bits > 8 {
                    flags |= FLAG_IS_SIGNED_INTEGER;
                }
                if big_endian {
                    flags |= FLAG_IS_BIG_ENDIAN;
                }
                let description = StreamDescription {
                    sample_rate: rate,
                    format_id: four_character_code(b"lpcm"),
                    format_flags: flags,
                    bytes_per_packet: bytes,
                    frames_per_packet: 1,
                    bytes_per_frame: bytes,
                    channels_per_frame: configuration.channels,
                    bits_per_channel: bits,
                    reserved: 0,
                };
                (description, bytes)
            }
            codec => (compressed_description(codec, configuration), 0),
        };
        let destination = StreamDescription {
            sample_rate: rate,
            format_id: four_character_code(b"lpcm"),
            format_flags: FLAG_IS_FLOAT | FLAG_IS_PACKED,
            bytes_per_packet: 4 * channels,
            frames_per_packet: 1,
            bytes_per_frame: 4 * channels,
            channels_per_frame: channels,
            bits_per_channel: 32,
            reserved: 0,
        };
        let mut raw = ptr::null_mut();
        // SAFETY: both descriptions are valid; the converter is returned +1 and owned by `Self`.
        check_status(
            unsafe { AudioConverterNew(&source, &destination, &mut raw) },
            &format!("make a {} decoder", configuration.name),
        )?;
        let converter = Self {
            raw,
            input: Box::new(Input {
                data: Vec::new(),
                pcm_frames: 0,
                channels: configuration.channels,
                pending: false,
                ended: false,
                description: PacketDescription {
                    start: 0,
                    variable_frames: 0,
                    size: 0,
                },
            }),
            channels: channels as usize,
            bytes_per_pcm_frame,
        };
        if !configuration.cookie.is_empty() {
            const DECOMPRESSION_MAGIC_COOKIE: u32 = four_character_code(b"dmgc");
            // SAFETY: the converter is live; the cookie is read for its length during the call.
            let status = unsafe {
                AudioConverterSetProperty(
                    converter.raw,
                    DECOMPRESSION_MAGIC_COOKIE,
                    configuration.cookie.len() as u32,
                    configuration.cookie.as_ptr().cast(),
                )
            };
            check_status(
                status,
                &format!("configure the {} decoder", configuration.name),
            )?;
        }
        Ok(converter)
    }

    /// Decodes one packet; `None` for `data` flushes what the converter still holds.
    fn decode(&mut self, packet: Option<Vec<u8>>) -> MediaResult<Vec<f32>> {
        match packet {
            Some(data) => {
                self.input.pcm_frames = (data.len() as u32)
                    .checked_div(self.bytes_per_pcm_frame)
                    .unwrap_or(0);
                self.input.data = data;
                self.input.pending = true;
            }
            None => self.input.ended = true,
        }
        let mut samples = Vec::new();
        loop {
            let start = samples.len();
            samples.resize(start + FRAMES_PER_CALL * self.channels, 0.0);
            let mut output = AudioBufferList {
                count: 1,
                buffers: [AudioBuffer {
                    channels: self.channels as u32,
                    size: (FRAMES_PER_CALL * self.channels * 4) as u32,
                    data: samples[start..].as_mut_ptr().cast(),
                }],
            };
            let mut frames = FRAMES_PER_CALL as u32;
            // SAFETY: the converter is live; the output buffer holds `frames` frames; the input
            // stays alive and unborrowed for the call.
            let status = unsafe {
                AudioConverterFillComplexBuffer(
                    self.raw,
                    provide_input,
                    (&raw mut *self.input).cast(),
                    &mut frames,
                    &mut output,
                    ptr::null_mut(),
                )
            };
            if status != 0 && status != NO_INPUT_NOW {
                self.input.pending = false;
                return Err(MediaError(format!("decode sound: status {status}")));
            }
            samples.truncate(start + frames as usize * self.channels);
            if (frames as usize) < FRAMES_PER_CALL {
                return Ok(samples);
            }
        }
    }

    /// Resets the converter, for decoding from a new position.
    fn reset(&mut self) {
        self.input.pending = false;
        self.input.ended = false;
        // SAFETY: the converter is live.
        unsafe { AudioConverterReset(self.raw) };
    }
}

/// The description of a compressed stream, as Core Audio reads it from the cookie or as the
/// track states it.
fn compressed_description(
    codec: AudioCodec,
    configuration: &AudioConfiguration,
) -> StreamDescription {
    let format_id = four_character_code(match codec {
        AudioCodec::Aac => b"aac ",
        AudioCodec::Opus => b"opus",
        AudioCodec::Vorbis => b"vorb",
        AudioCodec::Flac => b"flac",
        AudioCodec::Mp3 => b".mp3",
        AudioCodec::Mp2 => b".mp2",
        AudioCodec::Mp1 => b".mp1",
        AudioCodec::Ac3 => b"ac-3",
        AudioCodec::Eac3 => b"ec-3",
        AudioCodec::Alac => b"alac",
        AudioCodec::Pcm { .. } => b"lpcm",
    });
    let stated = StreamDescription {
        sample_rate: f64::from(configuration.sample_rate),
        format_id,
        format_flags: 0,
        bytes_per_packet: 0,
        frames_per_packet: configuration.frames_per_packet.unwrap_or(0),
        bytes_per_frame: 0,
        channels_per_frame: configuration.channels,
        bits_per_channel: 0,
        reserved: 0,
    };
    if configuration.cookie.is_empty() {
        return stated;
    }
    const FORMAT_INFO: u32 = four_character_code(b"fmti");
    let mut described = StreamDescription {
        reserved: 0,
        ..stated
    };
    let mut size = std::mem::size_of::<StreamDescription>() as u32;
    // SAFETY: the cookie is read for its length; the description is written for its size.
    let status = unsafe {
        AudioFormatGetProperty(
            FORMAT_INFO,
            configuration.cookie.len() as u32,
            configuration.cookie.as_ptr().cast(),
            &mut size,
            (&raw mut described).cast(),
        )
    };
    if status != 0
        || described.format_id != format_id
        || described.sample_rate <= 0.0
        || described.channels_per_frame == 0
    {
        return stated;
    }
    described
}

/// How long a sound plays: the segment's length minus the codec delay, which its timestamps
/// include and playback skips.
pub(super) fn played_length(segment_seconds: Option<f64>, codec_delay_ns: u64) -> f64 {
    (segment_seconds.unwrap_or(0.0) - codec_delay_ns as f64 / 1e9).max(0.0)
}

/// A Matroska audio track decoded by Core Audio.
pub(super) struct MatroskaAudio {
    demuxer: Demuxer,
    converter: Converter,
    configuration: AudioConfiguration,
    source: AudioSourceInfo,
    channels: usize,
    /// How far a block's timestamp may be from the sample it starts on, in samples.
    tolerance: i64,
    /// Where the stream's first block starts; the packet-length grid counts from here.
    anchor: i64,
    /// Where the next decoded samples go, once a block has placed them.
    position: Option<i64>,
    /// The timestamp of the stream's first block.
    first_timestamp_ns: i64,
    /// A packet decoded to nothing since the last block was placed, so the rest of its lace
    /// has no known position until the next block's timestamp.
    unplaced: bool,
    /// Samples before this one are dropped (before a seek's target, or within the codec delay).
    first_wanted: i64,
    finished: bool,
}

// SAFETY: the converter is used only through `&mut self`, from one thread at a time.
unsafe impl Send for MatroskaAudio {}

impl MatroskaAudio {
    /// The block's first sample, from its timestamp.
    fn sample_of(&self, timestamp_ns: i64) -> i64 {
        (timestamp_ns as f64 * f64::from(self.configuration.sample_rate) / 1e9).round() as i64
            - self.configuration.delay as i64
    }

    /// A block's first sample snapped back to the codec's grid, when the grid is coarser than
    /// the timestamps' rounding.
    fn on_grid(&self, sample: i64) -> i64 {
        match self.configuration.grid.map(i64::from) {
            Some(grid) if grid > 2 * self.tolerance => {
                self.anchor + ((sample - self.anchor) as f64 / grid as f64).round() as i64 * grid
            }
            _ => sample,
        }
    }

    /// A block's first packet places what follows at its timestamp, unless the samples so far
    /// already reach within the timestamps' rounding of it.
    fn place_block(&mut self, timestamp_ns: i64) {
        self.unplaced = false;
        let stated = self.on_grid(self.sample_of(timestamp_ns));
        if self
            .position
            .is_none_or(|position| (stated - position).abs() > self.tolerance)
        {
            self.position = Some(stated);
        }
    }

    /// Decodes one packet into its samples, minus the padding its block discards at the end,
    /// and the sample they start on; `None` if nothing it gives has a position.
    fn decode_packet(&mut self, packet: Packet) -> MediaResult<Option<(Vec<f32>, i64)>> {
        let first_of_stream =
            packet.lace_index == 0 && packet.timestamp_ns == self.first_timestamp_ns;
        if packet.lace_index == 0 {
            self.place_block(packet.timestamp_ns);
        }
        let mut samples = self.converter.decode(Some(packet.data))?;
        if samples.is_empty() {
            // The stream's first empty packet uses up the codec delay (Vorbis as FFmpeg writes
            // it); any later one leaves its lace unplaced.
            if first_of_stream {
                self.position = self
                    .position
                    .map(|position| position + self.configuration.delay as i64);
            } else {
                self.unplaced = true;
            }
            return Ok(None);
        }
        if self.unplaced {
            return Ok(None);
        }
        let padding = (packet.discard_padding_ns.max(0) as f64
            * f64::from(self.configuration.sample_rate)
            / 1e9)
            .round() as usize;
        let frames = samples.len() / self.channels;
        samples.truncate((frames - padding.min(frames)) * self.channels);
        Ok(Some((samples, self.position.unwrap_or(self.anchor))))
    }

    /// Advances the position past `samples`, which start on sample `start`, and returns those
    /// from the first wanted one on; `None` if none are wanted.
    fn hand_out_wanted(&mut self, mut samples: Vec<f32>, start: i64) -> Option<(i64, Vec<f32>)> {
        let frames = (samples.len() / self.channels) as i64;
        self.position = Some(start + frames);
        let skipped = (self.first_wanted.max(0) - start).clamp(0, frames);
        samples.drain(..skipped as usize * self.channels);
        (!samples.is_empty()).then_some((start + skipped, samples))
    }
}

impl AudioDecoding for MatroskaAudio {
    fn open(path: &str, channels: u32) -> MediaResult<Option<Self>> {
        let Some(mut demuxer) = Demuxer::open(path, TrackKind::Audio)? else {
            return Ok(None);
        };
        let configuration = audio_configuration(demuxer.track())?;
        let converter = Converter::new(&configuration, channels)?;
        let source = AudioSourceInfo {
            codec_name: configuration.name.clone(),
            sample_rate: configuration.sample_rate,
            channels: configuration.channels,
            duration_seconds: played_length(
                demuxer.segment().duration_seconds,
                demuxer.track().codec_delay_ns,
            ),
        };
        let scale = demuxer.segment().timestamp_scale as f64;
        let tolerance =
            (scale * f64::from(configuration.sample_rate) / 1e9 / 2.0).ceil() as i64 + 1;
        let first = demuxer.next_packet()?;
        let first_timestamp_ns = first.as_ref().map_or(0, |packet| packet.timestamp_ns);
        // PCM blocks all hold the same number of frames except the last; that is its grid.
        let mut configuration = configuration;
        if let (AudioCodec::Pcm { bits, .. }, None, Some(first)) =
            (configuration.codec, configuration.grid, &first)
        {
            let frames = first.data.len() as u32 / (bits / 8 * configuration.channels).max(1);
            configuration.grid = (frames > 0).then_some(frames);
        }
        demuxer.seek(i64::MIN, false)?;
        let mut audio = Self {
            demuxer,
            converter,
            configuration,
            source,
            channels: channels as usize,
            tolerance,
            anchor: 0,
            position: None,
            first_timestamp_ns,
            unplaced: false,
            first_wanted: 0,
            finished: false,
        };
        audio.anchor = audio.sample_of(first_timestamp_ns);
        Ok(Some(audio))
    }

    fn source(&self) -> &AudioSourceInfo {
        &self.source
    }

    fn read(&mut self) -> MediaResult<Option<(i64, Vec<f32>)>> {
        while !self.finished {
            let decoded = match self.demuxer.next_packet()? {
                Some(packet) => self.decode_packet(packet)?,
                None => {
                    self.finished = true;
                    Some((
                        self.converter.decode(None)?,
                        self.position.unwrap_or(self.anchor),
                    ))
                }
            };
            let Some((samples, start)) = decoded else {
                continue;
            };
            if let Some(wanted) = self.hand_out_wanted(samples, start) {
                return Ok(Some(wanted));
            }
        }
        Ok(None)
    }

    fn seek(&mut self, frame: u64) -> MediaResult<()> {
        let delay = self.configuration.delay;
        let target_ns = ((frame + delay) as f64 * 1e9 / f64::from(self.configuration.sample_rate))
            .floor() as i64;
        // A timestamp may be up to its precision later than its block's first sample.
        let scale = self.demuxer.segment().timestamp_scale as i64;
        self.demuxer
            .seek(target_ns - scale - SEEK_LEAD_NANOSECONDS, false)?;
        self.converter.reset();
        self.position = None;
        self.unplaced = false;
        self.first_wanted = frame as i64;
        self.finished = false;
        Ok(())
    }
}
