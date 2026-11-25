//! What goes into a call's frames: 16 kHz mono voice squeezed to one byte a sample (G.711
//! µ-law, 128 kbit/s) and camera frames as small JPEGs. Plain functions, no devices, so they
//! are tested here and the device code stays thin.

use std::collections::{HashMap, VecDeque};

use agora_core::UserId;

/// Samples per second on the wire.
pub const RATE: u32 = 16_000;
/// Samples in one audio frame: 20 ms.
pub const FRAME: usize = 320;
/// Widest camera frame sent, in pixels.
pub const VIDEO_WIDTH: u32 = 320;
/// Camera frames per second.
pub const VIDEO_FPS: u64 = 10;
/// JPEG quality of camera frames.
const JPEG_QUALITY: u8 = 55;
/// The most pixels a received frame may have, so a hostile peer cannot make this device
/// allocate without bound.
const MAX_PIXELS: u64 = 1920 * 1080;

const BIAS: i32 = 0x84;
const CLIP: i32 = 32_635;

pub fn mulaw_encode(sample: i16) -> u8 {
    let mut s = i32::from(sample);
    let sign = if s < 0 {
        s = -s;
        0x80
    } else {
        0
    };
    s = s.min(CLIP) + BIAS;
    let mut exponent = 7;
    let mut mask = 0x4000;
    while exponent > 0 && s & mask == 0 {
        exponent -= 1;
        mask >>= 1;
    }
    let mantissa = (s >> (exponent + 3)) & 0x0f;
    !(sign | (exponent << 4) | mantissa) as u8
}

pub fn mulaw_decode(byte: u8) -> i16 {
    let b = i32::from(!byte);
    let exponent = (b >> 4) & 0x07;
    let mantissa = b & 0x0f;
    let magnitude = (((mantissa << 3) + BIAS) << exponent) - BIAS;
    let value = if b & 0x80 != 0 { -magnitude } else { magnitude };
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

pub fn encode_audio(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().copied().map(mulaw_encode).collect()
}

pub fn decode_audio(bytes: &[u8]) -> Vec<f32> {
    bytes
        .iter()
        .map(|b| f32::from(mulaw_decode(*b)) / 32768.0)
        .collect()
}

/// Turns what a microphone delivers (any rate, any channel count) into 16 kHz mono frames of
/// [`FRAME`] samples, by averaging channels and interpolating linearly.
pub struct Capture {
    channels: usize,
    step: f64,
    pos: f64,
    last: f32,
    pending: Vec<i16>,
}

impl Capture {
    pub fn new(rate: u32, channels: u16) -> Capture {
        Capture {
            channels: usize::from(channels.max(1)),
            step: f64::from(rate.max(1)) / f64::from(RATE),
            pos: 0.0,
            last: 0.0,
            pending: Vec::with_capacity(FRAME * 2),
        }
    }

    /// Feed interleaved samples; `frame` gets every full frame that results.
    pub fn push(&mut self, interleaved: &[f32], mut frame: impl FnMut(&[i16])) {
        for chunk in interleaved.chunks(self.channels) {
            let mono = chunk.iter().sum::<f32>() / chunk.len() as f32;
            // `pos` is the position of the next output sample relative to the previous input
            // sample, in input samples; output samples falling before this input are emitted.
            while self.pos < 1.0 {
                let t = self.pos as f32;
                let value = self.last + (mono - self.last) * t;
                self.pending.push((value.clamp(-1.0, 1.0) * 32767.0) as i16);
                self.pos += self.step;
                if self.pending.len() == FRAME {
                    frame(&self.pending);
                    self.pending.clear();
                }
            }
            self.pos -= 1.0;
            self.last = mono;
        }
    }
}

/// Where a speaker's samples wait to be played.
struct Voice {
    queue: VecDeque<f32>,
    /// Position between `queue[0]` and `queue[1]`, 0 to 1.
    pos: f64,
    playing: bool,
}

/// Mixes everyone else's voices for the speaker, absorbing network jitter: a voice starts after
/// [`Mixer::START`] samples are buffered, stops when it runs dry, and never holds more than
/// [`Mixer::MAX`] (older audio is dropped, so a stall does not turn into delay).
#[derive(Default)]
pub struct Mixer {
    voices: HashMap<UserId, Voice>,
}

impl Mixer {
    /// 60 ms.
    pub const START: usize = 960;
    /// 400 ms.
    pub const MAX: usize = 6400;

    pub fn push(&mut self, from: UserId, samples: &[f32]) {
        let voice = self.voices.entry(from).or_insert_with(|| {
            tracing::info!("call: sound is arriving from {from}");
            Voice {
                queue: VecDeque::new(),
                pos: 0.0,
                playing: false,
            }
        });
        voice.queue.extend(samples.iter().copied());
        while voice.queue.len() > Mixer::MAX {
            voice.queue.pop_front();
        }
        if !voice.playing && voice.queue.len() >= Mixer::START {
            voice.playing = true;
        }
    }

    /// Forget someone who left the call.
    pub fn remove(&mut self, who: UserId) {
        self.voices.remove(&who);
    }

    /// Fill `out` (interleaved, `channels` per frame) at `rate` samples per second.
    pub fn pull(&mut self, out: &mut [f32], channels: usize, rate: u32) {
        out.fill(0.0);
        let channels = channels.max(1);
        let step = f64::from(RATE) / f64::from(rate.max(1));
        for voice in self.voices.values_mut() {
            if !voice.playing {
                continue;
            }
            for frame in out.chunks_mut(channels) {
                let (Some(a), Some(b)) =
                    (voice.queue.front().copied(), voice.queue.get(1).copied())
                else {
                    voice.playing = false;
                    voice.pos = 0.0;
                    break;
                };
                let sample = a + (b - a) * voice.pos as f32;
                for slot in frame.iter_mut() {
                    *slot += sample;
                }
                voice.pos += step;
                while voice.pos >= 1.0 {
                    voice.pos -= 1.0;
                    voice.queue.pop_front();
                }
            }
        }
        for sample in out.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }
    }
}

/// A picture, as RGBA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Shrink an RGB image to at most `max_width` wide (nearest neighbour; it only has to look
/// right at call-tile size) and encode it as a JPEG. `None` when the image is empty or the
/// encoder fails.
pub fn encode_video(rgb: &[u8], width: u32, height: u32, max_width: u32) -> Option<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgb.len() < w.checked_mul(h)?.checked_mul(3)? {
        return None;
    }
    let out_w = width.min(max_width).max(1);
    let out_h = (u64::from(height) * u64::from(out_w) / u64::from(width)).max(1) as u32;
    let mut small = Vec::with_capacity(out_w as usize * out_h as usize * 3);
    for y in 0..out_h as usize {
        let sy = y * h / out_h as usize;
        for x in 0..out_w as usize {
            let sx = x * w / out_w as usize;
            let at = (sy * w + sx) * 3;
            small.extend_from_slice(rgb.get(at..at + 3)?);
        }
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode(&small, out_w, out_h, image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(out)
}

/// Decode a received JPEG, refusing anything larger than [`MAX_PIXELS`].
pub fn decode_video(bytes: &[u8]) -> Option<Picture> {
    let mut decoder =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_PIXELS * 4 * 2);
    decoder.limits(limits);
    let image = decoder.decode().ok()?;
    let (w, h) = (u64::from(image.width()), u64::from(image.height()));
    if w * h > MAX_PIXELS {
        return None;
    }
    Some(Picture {
        width: image.width() as usize,
        height: image.height() as usize,
        rgba: image.to_rgba8().into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulaw_keeps_speech_close_and_silence_silent() {
        for s in [-32000i16, -1000, -100, -1, 0, 1, 100, 1000, 32000] {
            let back = mulaw_decode(mulaw_encode(s));
            let error = (i32::from(back) - i32::from(s)).abs();
            assert!(
                error <= 1 + i32::from(s).abs() / 16,
                "{s} came back as {back}"
            );
        }
        assert_eq!(mulaw_decode(mulaw_encode(0)), 0);
    }

    #[test]
    fn a_microphone_becomes_whole_frames_at_the_wire_rate() {
        // One second of a 48 kHz stereo tone is 50 frames of 320 samples at 16 kHz.
        let mut capture = Capture::new(48_000, 2);
        let tone: Vec<f32> = (0..48_000)
            .flat_map(|i| {
                let v = (i as f32 * 0.05).sin() * 0.5;
                [v, v]
            })
            .collect();
        let mut frames = 0;
        let mut peak = 0i16;
        capture.push(&tone, |f| {
            assert_eq!(f.len(), FRAME);
            frames += 1;
            peak = peak.max(f.iter().copied().max().unwrap());
        });
        assert!((49..=50).contains(&frames), "{frames} frames");
        assert!(peak > 10_000);
        // Odd chunk sizes lose nothing.
        let mut capture = Capture::new(44_100, 1);
        let mut samples = 0;
        for chunk in vec![0.1f32; 44_100].chunks(441 + 7) {
            capture.push(chunk, |f| samples += f.len());
        }
        assert!((15_000..=16_000).contains(&samples), "{samples} samples");
    }

    #[test]
    fn the_mixer_waits_for_a_cushion_mixes_voices_and_drops_a_stall() {
        let mut mixer = Mixer::default();
        let (ana, luis) = (UserId(1), UserId(2));
        mixer.push(ana, &vec![0.25; 500]);
        let mut out = [1.0f32; 64];
        mixer.pull(&mut out, 1, RATE);
        assert!(
            out.iter().all(|s| *s == 0.0),
            "plays before the cushion is there"
        );

        mixer.push(ana, &vec![0.25; 600]);
        mixer.push(luis, &vec![0.5; 1000]);
        let mut out = [0.0f32; 64];
        mixer.pull(&mut out, 1, RATE);
        assert!(out.iter().all(|s| (*s - 0.75).abs() < 1e-3));
        // Stereo output at twice the rate repeats each sample on both channels.
        let mut out = [0.0f32; 64];
        mixer.pull(&mut out, 2, 32_000);
        assert!(out.chunks(2).all(|f| f[0] == f[1]));

        // A burst far beyond the limit keeps only the newest audio.
        mixer.push(ana, &vec![0.1; Mixer::MAX * 3]);
        assert!(mixer.voices[&ana].queue.len() <= Mixer::MAX);
        mixer.remove(ana);
        assert!(!mixer.voices.contains_key(&ana));
    }

    #[test]
    fn video_frames_round_trip_and_hostile_ones_are_refused() {
        let (w, h) = (640u32, 480u32);
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgb.extend_from_slice(&[(x / 3) as u8, (y / 2) as u8, 128]);
            }
        }
        let jpeg = encode_video(&rgb, w, h, VIDEO_WIDTH).unwrap();
        assert!(
            jpeg.len() < agora_protocol::media::MAX_PAYLOAD - 100,
            "{} bytes",
            jpeg.len()
        );
        let picture = decode_video(&jpeg).unwrap();
        assert_eq!((picture.width, picture.height), (320, 240));
        assert_eq!(picture.rgba.len(), 320 * 240 * 4);

        assert!(encode_video(&[], 0, 0, 320).is_none());
        assert!(encode_video(&rgb[..100], w, h, 320).is_none());
        assert!(decode_video(b"not a jpeg").is_none());
        assert!(decode_video(&[]).is_none());
    }
}
