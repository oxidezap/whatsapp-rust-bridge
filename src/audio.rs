use js_sys::{ArrayBuffer, Reflect, Uint8Array};
use std::io::Cursor;
use symphonia::core::audio::{Audio, AudioBuffer, GenericAudioBufferRef};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, Track, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Duration, TimeBase, Timestamp};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::JsFuture;
use web_sys::{ReadableStream, ReadableStreamDefaultReader};

// ---------------------------------------------------------------------------
// Host-agnostic core functions (no JS types)
// ---------------------------------------------------------------------------

/// Generate a 64-bucket audio waveform from raw audio bytes.
/// Each bucket is a value 0–100 representing relative amplitude.
pub fn generate_waveform(audio_data: Vec<u8>) -> Result<Vec<u8>, String> {
    if audio_data.is_empty() {
        return Err("Audio buffer is empty".into());
    }

    let DecoderContext {
        mut format,
        mut decoder,
        track_id,
        total_frames,
    } = prepare_decoder(audio_data)?;

    let mut bins = [(0.0f32, 0u32); WAVEFORM_SAMPLES];
    let estimated_samples = total_frames.unwrap_or(2_000_000);
    let estimated_packets = (estimated_samples / 1152).max(64) as usize;
    let target_packets = 512;
    let packet_skip = (estimated_packets / target_packets).max(1);

    let mut packet_counter = 0usize;
    let mut total_samples_processed = 0u64;

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(Error::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => {
                return Err(format!("Audio decode error: {e}"));
            }
        };

        if packet.track_id != track_id {
            continue;
        }

        packet_counter += 1;
        if packet_skip > 1 && !packet_counter.is_multiple_of(packet_skip) {
            continue;
        }

        let packet_start = u64::try_from(packet.pts.get()).unwrap_or(0);

        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                accumulate_to_bins(
                    &audio_buf,
                    &mut bins,
                    packet_start,
                    estimated_samples,
                    &mut total_samples_processed,
                );
            }
            Err(Error::IoError(ref e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::NotFound
                ) =>
            {
                break;
            }
            Err(Error::DecodeError(_)) => {
                continue;
            }
            Err(Error::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => {
                return Err(format!("Failed to decode audio frame: {e}"));
            }
        }
    }

    if total_samples_processed == 0 {
        return Err("No audio samples decoded".into());
    }

    Ok(finalize_waveform(&bins))
}

/// Compute audio duration in seconds from raw audio bytes.
pub fn compute_duration(audio_data: Vec<u8>) -> Result<f64, String> {
    if audio_data.is_empty() {
        return Err("Audio buffer is empty".into());
    }

    let mss = MediaSourceStream::new(Box::new(Cursor::new(audio_data)), Default::default());

    let mut format = symphonia::default::get_probe()
        .probe(
            &Hint::new(),
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| format!("Failed to probe audio format: {e}"))?;

    let track = format
        .default_track(TrackType::Audio)
        .cloned()
        .ok_or_else(|| "No supported audio track found".to_string())?;

    if let Some(duration) = duration_from_track_metadata(&track) {
        return Ok(duration);
    }

    let track_id = track.id;
    let mut stats = DurationAccumulator::default();

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(Error::ResetRequired) => continue,
            Err(e) => {
                return Err(format!("Audio decode error: {e}"));
            }
        };

        if packet.track_id != track_id {
            continue;
        }

        stats.update(packet.pts, packet.dur);
    }

    let ticks = stats
        .elapsed_ticks()
        .ok_or_else(|| "No audio samples decoded".to_string())?;

    convert_ticks_to_seconds(
        ticks,
        track.time_base,
        track
            .codec_params
            .as_ref()
            .and_then(|params| params.audio())
            .and_then(|params| params.sample_rate),
    )
    .ok_or_else(|| "Missing timing information for audio track".to_string())
}

/// WhatsApp uses 64 buckets for visual waveforms.
const WAVEFORM_SAMPLES: usize = 64;

// ---------------------------------------------------------------------------
// WASM wrappers (thin delegation to core functions)
// ---------------------------------------------------------------------------

#[wasm_bindgen(js_name = generateAudioWaveform)]
pub fn generate_audio_waveform(audio_data: Vec<u8>) -> Result<Uint8Array, JsError> {
    let waveform = generate_waveform(audio_data).map_err(|e| JsError::new(&e))?;
    Ok(Uint8Array::from(waveform.as_slice()))
}

/// Accumulate audio samples into waveform bins
#[inline]
fn accumulate_to_bins(
    buffer: &GenericAudioBufferRef<'_>,
    bins: &mut [(f32, u32); WAVEFORM_SAMPLES],
    packet_start: u64,
    estimated_total: u64,
    total_processed: &mut u64,
) {
    // Optimized path for S16 (most common for MP3)
    if let GenericAudioBufferRef::S16(buf) = buffer {
        accumulate_s16(buf, bins, packet_start, estimated_total, total_processed);
        return;
    }

    // Generic path using Symphonia's format-converting planar copy.
    let frames = buffer.frames();
    let channel_count = buffer.num_planes();

    if frames == 0 || channel_count == 0 {
        return;
    }

    let mut samples = Vec::<Vec<f32>>::with_capacity(channel_count);
    buffer.copy_to_vecs_planar(&mut samples);

    let inv_channels = 1.0f32 / channel_count as f32;
    let bin_count = WAVEFORM_SAMPLES as u64;
    let est_max = estimated_total.max(1);

    for (i, _) in samples[0].iter().enumerate() {
        // Average all channels
        let sample: f32 = (0..channel_count).map(|c| samples[c][i]).sum::<f32>() * inv_channels;

        let bin_idx = ((packet_start + i as u64) * bin_count / est_max) as usize;
        let bin_idx = bin_idx.min(WAVEFORM_SAMPLES - 1);
        bins[bin_idx].0 += sample.abs();
        bins[bin_idx].1 += 1;
    }
    *total_processed += frames as u64;
}

/// Optimized path for S16 (most common for MP3)
#[inline]
fn accumulate_s16(
    buffer: &AudioBuffer<i16>,
    bins: &mut [(f32, u32); WAVEFORM_SAMPLES],
    packet_start: u64,
    estimated_total: u64,
    total_processed: &mut u64,
) {
    let channel_count = buffer.num_planes();
    let frames = buffer.frames();
    if frames == 0 || channel_count == 0 {
        return;
    }

    const SCALE: f32 = 1.0 / 32768.0;
    let bin_count = WAVEFORM_SAMPLES as u64;
    let est_max = estimated_total.max(1);

    // Helper to update bin at frame index
    let mut update_bin = |i: usize, sample: f32| {
        let bin_idx = ((packet_start + i as u64) * bin_count / est_max) as usize;
        let bin_idx = bin_idx.min(WAVEFORM_SAMPLES - 1);
        bins[bin_idx].0 += sample;
        bins[bin_idx].1 += 1;
    };

    match channel_count {
        1 => {
            let channel = buffer.plane(0).expect("audio plane must exist");
            for (i, &sample) in channel.iter().enumerate().take(frames) {
                update_bin(i, (sample as f32 * SCALE).abs());
            }
        }
        2 => {
            let (chan0, chan1) = buffer
                .plane_pair(0, 1)
                .expect("stereo audio planes must exist");
            for i in 0..frames {
                let sample = ((chan0[i] as f32 + chan1[i] as f32) * 0.5 * SCALE).abs();
                update_bin(i, sample);
            }
        }
        _ => {
            let inv_channels = 1.0 / channel_count as f32;
            for i in 0..frames {
                let sum: i32 = (0..channel_count)
                    .map(|c| buffer.plane(c).expect("audio plane must exist")[i] as i32)
                    .sum();
                let sample = (sum as f32 * inv_channels * SCALE).abs();
                update_bin(i, sample);
            }
        }
    }

    *total_processed += frames as u64;
}

/// Convert accumulated bins to final waveform (0-100 range)
#[inline]
fn finalize_waveform(bins: &[(f32, u32); WAVEFORM_SAMPLES]) -> Vec<u8> {
    // Calculate averages and find max in single pass
    let averages: Vec<f32> = bins
        .iter()
        .map(
            |(sum, count)| {
                if *count > 0 { sum / *count as f32 } else { 0.0 }
            },
        )
        .collect();

    let max_avg = averages.iter().copied().fold(0.0f32, f32::max);

    if max_avg == 0.0 {
        return vec![0; WAVEFORM_SAMPLES];
    }

    // Normalize to 0-100 range
    let scale = 100.0 / max_avg;
    averages
        .iter()
        .map(|avg| (avg * scale).min(100.0) as u8)
        .collect()
}

#[wasm_bindgen(js_name = getAudioDuration, skip_typescript)]
pub async fn get_audio_duration(input: JsValue) -> Result<f64, JsError> {
    let audio_bytes = normalize_audio_input(input).await?;
    compute_duration(audio_bytes).map_err(|e| JsError::new(&e))
}

#[wasm_bindgen(typescript_custom_section)]
const TS_AUDIO_DURATION: &str = r#"
export type AudioDurationInput =
    | Uint8Array
    | ArrayBuffer
    | ReadableStream<Uint8Array | ArrayBuffer | ArrayBufferView>;

export function getAudioDuration(input: AudioDurationInput): Promise<number>;
"#;

fn duration_from_track_metadata(track: &Track) -> Option<f64> {
    if let (Some(duration), Some(time_base)) = (track.duration, track.time_base) {
        return convert_ticks_to_seconds(duration.get(), Some(time_base), None);
    }

    let frames = track.num_frames?;
    let sample_rate = track.codec_params.as_ref()?.audio()?.sample_rate;

    convert_ticks_to_seconds(frames, track.time_base, sample_rate)
}

#[inline]
fn convert_ticks_to_seconds(
    ticks: u64,
    time_base: Option<TimeBase>,
    sample_rate: Option<u32>,
) -> Option<f64> {
    if ticks == 0 {
        return None;
    }

    if let Some(tb) = time_base {
        return Some(ticks as f64 * f64::from(tb.numer.get()) / f64::from(tb.denom.get()));
    }

    sample_rate.map(|rate| ticks as f64 / rate as f64)
}

#[derive(Default)]
struct DurationAccumulator {
    first_ts: Option<i64>,
    max_end_ts: i128,
}

impl DurationAccumulator {
    #[inline]
    fn update(&mut self, ts: Timestamp, dur: Duration) {
        let ts = ts.get();
        if self.first_ts.is_none() {
            self.first_ts = Some(ts);
        }

        let end = i128::from(ts) + i128::from(dur.get());
        if end > self.max_end_ts {
            self.max_end_ts = end;
        }
    }

    #[inline]
    fn elapsed_ticks(&self) -> Option<u64> {
        let start = self.first_ts?;
        u64::try_from(self.max_end_ts - i128::from(start)).ok()
    }
}

async fn normalize_audio_input(input: JsValue) -> Result<Vec<u8>, JsError> {
    if input.is_instance_of::<Uint8Array>() || input.is_instance_of::<ArrayBuffer>() {
        return Ok(copy_uint8_array(&Uint8Array::new(&input)));
    }

    if input.is_instance_of::<ReadableStream>() {
        return read_stream(input.unchecked_ref::<ReadableStream>()).await;
    }

    Err(JsError::new(
        "Unsupported input type. Expected Uint8Array, ArrayBuffer, or ReadableStream",
    ))
}

#[inline]
fn copy_uint8_array(array: &Uint8Array) -> Vec<u8> {
    array.to_vec()
}

async fn read_stream(stream: &ReadableStream) -> Result<Vec<u8>, JsError> {
    let reader = stream.get_reader();
    let reader = reader.unchecked_into::<ReadableStreamDefaultReader>();
    read_from_reader(reader).await
}

fn js_val_err(value: JsValue) -> JsError {
    let message = value
        .as_string()
        .unwrap_or_else(|| format!("JavaScript error: {value:?}"));
    JsError::new(&message)
}

async fn read_from_reader(reader: ReadableStreamDefaultReader) -> Result<Vec<u8>, JsError> {
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);

    loop {
        let result = JsFuture::from(reader.read()).await.map_err(js_val_err)?;

        let done = Reflect::get(&result, &JsValue::from_str("done"))
            .map_err(js_val_err)?
            .as_bool()
            .unwrap_or(false);
        if done {
            break;
        }

        let value = Reflect::get(&result, &JsValue::from_str("value")).map_err(js_val_err)?;
        if !value.is_undefined() && !value.is_null() {
            buf.extend_from_slice(&Uint8Array::new(&value).to_vec());
        }
    }

    reader.release_lock();
    Ok(buf)
}

struct DecoderContext {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    total_frames: Option<u64>,
}

fn prepare_decoder(audio_data: Vec<u8>) -> Result<DecoderContext, String> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(audio_data)), Default::default());

    let format = symphonia::default::get_probe()
        .probe(
            &Hint::new(),
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| format!("Failed to probe audio format: {e}"))?;

    let track = format
        .default_track(TrackType::Audio)
        .cloned()
        .ok_or_else(|| "No supported audio track found".to_string())?;

    let track_id = track.id;
    let total_frames = track.num_frames;
    let audio_params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or_else(|| "Audio track has no codec parameters".to_string())?;

    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(audio_params, &AudioDecoderOptions::default())
        .map_err(|e| format!("Failed to create decoder: {e}"))?;

    Ok(DecoderContext {
        format,
        decoder,
        track_id,
        total_frames,
    })
}
