//! The microphone, the speaker and the camera, each on a thread of its own (their handles are not
//! safe to move between threads on every platform, so they are created where they live).
//!
//! A device that is missing or refuses to open is not an error for the call: the person can
//! still listen or watch, and the reason is reported once through `report`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};

use super::codec::{Capture, Mixer, FRAME, VIDEO_FPS};

/// Which device a [`Problem`] is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Microphone,
    Speaker,
    Camera,
}

/// A device that is missing or would not open. `detail` is the system's reason, when there is
/// one; without it the device was not found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub device: Device,
    pub detail: Option<String>,
}

impl Problem {
    fn missing(device: Device) -> Problem {
        Problem {
            device,
            detail: None,
        }
    }

    fn because(device: Device, why: impl std::fmt::Display) -> Problem {
        Problem {
            device,
            detail: Some(why.to_string()),
        }
    }
}

/// Where a device thread says what went wrong. Called once per device.
pub type Report = Arc<dyn Fn(Problem) + Send + Sync>;

/// A running device thread. Dropping it stops the thread and waits for it.
pub struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn spawn(run: impl FnOnce(&AtomicBool) + Send + 'static) -> Worker {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("agora-call".into())
            .spawn(move || run(&flag))
            .ok();
        Worker { stop, thread }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn idle(stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Capture the microphone and hand every 20 ms frame to `frame` while `live` is true.
pub fn microphone(
    live: Arc<AtomicBool>,
    report: Report,
    frame: impl Fn(&[i16]) + Send + 'static,
) -> Worker {
    Worker::spawn(move |stop| {
        let stream = match open_microphone(live, frame) {
            Ok(stream) => stream,
            Err(why) => {
                report(why);
                idle(stop);
                return;
            }
        };
        idle(stop);
        drop(stream);
    })
}

fn open_microphone(
    live: Arc<AtomicBool>,
    frame: impl Fn(&[i16]) + Send + 'static,
) -> Result<Stream, Problem> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or(Problem::missing(Device::Microphone))?;
    let config = device
        .default_input_config()
        .map_err(|e| Problem::because(Device::Microphone, e))?;
    let (rate, channels) = (config.sample_rate().0, config.channels());
    let capture = Mutex::new(Capture::new(rate, channels));
    let handle = move |samples: &[f32]| {
        if !live.load(Ordering::Relaxed) {
            return;
        }
        let mut capture = capture.lock().unwrap_or_else(PoisonError::into_inner);
        capture.push(samples, |pcm| {
            debug_assert_eq!(pcm.len(), FRAME);
            frame(pcm);
        });
    };
    let on_error = |e| tracing::warn!("microphone: {e}");
    let stream = match config.sample_format() {
        SampleFormat::F32 => device.build_input_stream(
            &config.config(),
            move |data: &[f32], _| handle(data),
            on_error,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            &config.config(),
            move |data: &[i16], _| {
                let floats: Vec<f32> = data.iter().map(|s| f32::from(*s) / 32768.0).collect();
                handle(&floats);
            },
            on_error,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            &config.config(),
            move |data: &[u16], _| {
                let floats: Vec<f32> = data
                    .iter()
                    .map(|s| (f32::from(*s) - 32768.0) / 32768.0)
                    .collect();
                handle(&floats);
            },
            on_error,
            None,
        ),
        other => return Err(Problem::because(Device::Microphone, format!("{other:?}"))),
    }
    .map_err(|e| Problem::because(Device::Microphone, e))?;
    stream
        .play()
        .map_err(|e| Problem::because(Device::Microphone, e))?;
    Ok(stream)
}

/// Play what `mixer` holds on the default speaker.
pub fn speaker(mixer: Arc<Mutex<Mixer>>, report: Report) -> Worker {
    Worker::spawn(move |stop| {
        let stream = match open_speaker(mixer) {
            Ok(stream) => stream,
            Err(why) => {
                report(why);
                idle(stop);
                return;
            }
        };
        idle(stop);
        drop(stream);
    })
}

fn open_speaker(mixer: Arc<Mutex<Mixer>>) -> Result<Stream, Problem> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or(Problem::missing(Device::Speaker))?;
    let config = device
        .default_output_config()
        .map_err(|e| Problem::because(Device::Speaker, e))?;
    let (rate, channels) = (config.sample_rate().0, usize::from(config.channels()));
    let on_error = |e| tracing::warn!("speaker: {e}");
    let pull = move |out: &mut [f32]| {
        mixer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pull(out, channels, rate);
    };
    let stream = match config.sample_format() {
        SampleFormat::F32 => device.build_output_stream(
            &config.config(),
            move |out: &mut [f32], _| pull(out),
            on_error,
            None,
        ),
        SampleFormat::I16 => device.build_output_stream(
            &config.config(),
            move |out: &mut [i16], _| {
                let mut floats = vec![0.0f32; out.len()];
                pull(&mut floats);
                for (o, f) in out.iter_mut().zip(floats) {
                    *o = (f * 32767.0) as i16;
                }
            },
            on_error,
            None,
        ),
        SampleFormat::U16 => device.build_output_stream(
            &config.config(),
            move |out: &mut [u16], _| {
                let mut floats = vec![0.0f32; out.len()];
                pull(&mut floats);
                for (o, f) in out.iter_mut().zip(floats) {
                    *o = (f * 32767.0 + 32768.0) as u16;
                }
            },
            on_error,
            None,
        ),
        other => return Err(Problem::because(Device::Speaker, format!("{other:?}"))),
    }
    .map_err(|e| Problem::because(Device::Speaker, e))?;
    stream
        .play()
        .map_err(|e| Problem::because(Device::Speaker, e))?;
    Ok(stream)
}

/// One camera picture, RGB.
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Capture the default camera at [`VIDEO_FPS`] and hand each picture to `shot`.
pub fn camera(report: Report, shot: impl Fn(Shot) + Send + 'static) -> Worker {
    Worker::spawn(move |stop| {
        if let Err(why) = run_camera(stop, shot) {
            report(why);
            idle(stop);
        }
    })
}

fn run_camera(stop: &AtomicBool, shot: impl Fn(Shot)) -> Result<(), Problem> {
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{
        CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
    };
    use nokhwa::Camera;

    let wanted = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(
        CameraFormat::new(Resolution::new(640, 480), FrameFormat::MJPEG, 15),
    ));
    let mut camera = Camera::new(CameraIndex::Index(0), wanted)
        .map_err(|e| Problem::because(Device::Camera, e))?;
    camera
        .open_stream()
        .map_err(|e| Problem::because(Device::Camera, e))?;
    let every = Duration::from_millis(1000 / VIDEO_FPS);
    let mut next = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let buffer = camera
            .frame()
            .map_err(|e| Problem::because(Device::Camera, e))?;
        if Instant::now() < next {
            continue;
        }
        next = Instant::now() + every;
        let image = buffer
            .decode_image::<RgbFormat>()
            .map_err(|e| Problem::because(Device::Camera, e))?;
        shot(Shot {
            width: image.width(),
            height: image.height(),
            rgb: image.into_raw(),
        });
    }
    let _ = camera.stop_stream();
    Ok(())
}
