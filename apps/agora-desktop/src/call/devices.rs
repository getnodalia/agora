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

/// The picture size and rate asked of the camera: small, because it is shrunk before it is sent.
const CAMERA_PIXELS: u64 = 640 * 480;
const CAMERA_FPS: u32 = 15;
/// How long to wait for the person to answer the system's camera question.
const CAMERA_PERMISSION_WAIT: Duration = Duration::from_secs(60);

/// How a camera's native picture format ranks when two would do: the cheapest to decode first.
fn format_rank(format: nokhwa::utils::FrameFormat) -> Option<u8> {
    use nokhwa::utils::FrameFormat;
    match format {
        FrameFormat::MJPEG => Some(0),
        FrameFormat::YUYV => Some(1),
        FrameFormat::NV12 => Some(2),
        FrameFormat::RAWRGB => Some(3),
        FrameFormat::RAWBGR => Some(4),
        _ => None,
    }
}

/// The modes a camera lists, best first: the size nearest 640x480, then the rate nearest 15, then
/// the format that decodes cheapest. Cameras differ in what they list (the Mac's offer no MJPEG,
/// and not every size at every rate), and a listed mode is not always one the camera accepts: on
/// macOS a rate is listed for the low end of a range but only the top of the range can be set,
/// so the caller tries these in order until one is taken.
fn camera_candidates(formats: &[nokhwa::utils::CameraFormat]) -> Vec<nokhwa::utils::CameraFormat> {
    let mut ranked: Vec<_> = formats
        .iter()
        .filter_map(|mode| {
            let rank = format_rank(mode.format())?;
            let size = u64::from(mode.width()) * u64::from(mode.height());
            if size == 0 {
                return None;
            }
            let rate = mode.frame_rate();
            Some((
                (
                    size.abs_diff(CAMERA_PIXELS),
                    rate.abs_diff(CAMERA_FPS),
                    rank,
                ),
                *mode,
            ))
        })
        .collect();
    ranked.sort_by_key(|(key, _)| *key);
    ranked.into_iter().map(|(_, mode)| mode).collect()
}

/// Apply the first of these modes the camera accepts and say which it was. `None` when none was
/// taken: the camera then keeps the mode it already has, which is still a picture worth showing.
fn first_accepted<T, E>(
    candidates: &[nokhwa::utils::CameraFormat],
    mut apply: impl FnMut(nokhwa::utils::CameraFormat) -> Result<T, E>,
) -> Option<nokhwa::utils::CameraFormat> {
    candidates.iter().copied().find(|mode| apply(*mode).is_ok())
}

/// Open the first camera. On a Mac the system lists a camera's rate range by both ends, but only
/// the top of a range can be set, so the mode `RequestedFormatType::None` picks (the first one
/// listed, at the low end) is refused: that was the "640x480@15FPS YUYV Not Found/Rejected/
/// Unsupported" which kept the camera from ever starting. The requests here name the highest
/// rate and then the highest size, which are always the top of a range, and `None` comes last
/// for the cameras on which the others find nothing.
fn open_camera() -> Result<nokhwa::Camera, String> {
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};

    first_opened(
        &[
            RequestedFormatType::AbsoluteHighestFrameRate,
            RequestedFormatType::AbsoluteHighestResolution,
            RequestedFormatType::None,
        ],
        |request| {
            nokhwa::Camera::new(
                CameraIndex::Index(0),
                RequestedFormat::new::<RgbFormat>(*request),
            )
        },
    )
}

/// Open with each request in turn until one is taken. When none is, every reason is reported:
/// "the camera would not open" is of no use to whoever has to fix it.
fn first_opened<R: std::fmt::Debug, T, E: std::fmt::Display>(
    requests: &[R],
    mut open: impl FnMut(&R) -> Result<T, E>,
) -> Result<T, String> {
    let mut reasons = Vec::new();
    for request in requests {
        match open(request) {
            Ok(opened) => return Ok(opened),
            Err(why) => {
                tracing::warn!(?request, %why, "camera did not open");
                reasons.push(format!("{request:?}: {why}"));
            }
        }
    }
    Err(if reasons.is_empty() {
        "no way of opening the camera".to_string()
    } else {
        reasons.join("; ")
    })
}

/// Ask the system for permission to use the camera and wait for the answer (macOS asks; the
/// others always allow). A refusal stays refused until the person changes it in the settings.
fn camera_permission(stop: &AtomicBool) -> Result<(), Problem> {
    nokhwa::nokhwa_initialize(|_| {});
    let until = Instant::now() + CAMERA_PERMISSION_WAIT;
    while !nokhwa::nokhwa_check() {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if Instant::now() >= until {
            return Err(Problem::because(
                Device::Camera,
                "Ágora has no permission to use the camera (System Settings, Privacy & Security, Camera)",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn run_camera(stop: &AtomicBool, shot: impl Fn(Shot)) -> Result<(), Problem> {
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{RequestedFormat, RequestedFormatType};

    camera_permission(stop)?;
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut camera = open_camera().map_err(|why| Problem::because(Device::Camera, why))?;
    let modes = camera
        .compatible_camera_formats()
        .map_err(|e| Problem::because(Device::Camera, e))?;
    // The camera now streams in the mode it opened with. Move to a smaller one when it accepts
    // one: the picture is shrunk before it is sent, so a smaller one is cheaper all round.
    let candidates = camera_candidates(&modes);
    match first_accepted(&candidates, |mode| {
        camera.set_camera_requset(RequestedFormat::new::<RgbFormat>(
            RequestedFormatType::Exact(mode),
        ))
    }) {
        Some(mode) => tracing::info!(%mode, "camera mode chosen"),
        None => tracing::info!(
            listed = modes.len(),
            "camera keeps the mode it opened with ({})",
            camera.camera_format()
        ),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{
        CameraFormat, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
    };

    fn mode(width: u32, height: u32, format: FrameFormat, fps: u32) -> CameraFormat {
        CameraFormat::new(Resolution::new(width, height), format, fps)
    }

    /// What a Mac's built-in camera lists: no MJPEG, and no 640x480.
    fn mac_modes() -> Vec<CameraFormat> {
        vec![
            mode(1920, 1080, FrameFormat::NV12, 30),
            mode(1280, 720, FrameFormat::NV12, 30),
            mode(1280, 720, FrameFormat::YUYV, 30),
            mode(640, 360, FrameFormat::NV12, 30),
        ]
    }

    #[test]
    fn a_mac_camera_without_mjpeg_or_640x480_still_gets_a_mode() {
        let mac = mac_modes();
        // The request that failed on macOS: one exact format, at a size the camera lacks.
        let old = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(mode(
            640,
            480,
            FrameFormat::MJPEG,
            15,
        )));
        assert_eq!(old.fulfill(&mac), None);

        let picked = camera_candidates(&mac)[0];
        assert_eq!(picked.resolution(), Resolution::new(640, 360));
    }

    #[test]
    fn the_size_nearest_640x480_wins_over_a_bigger_picture() {
        let modes = vec![
            mode(1920, 1080, FrameFormat::MJPEG, 30),
            mode(640, 480, FrameFormat::MJPEG, 30),
            mode(320, 240, FrameFormat::MJPEG, 30),
        ];
        let picked = camera_candidates(&modes)[0];
        assert_eq!(picked.resolution(), Resolution::new(640, 480));
    }

    #[test]
    fn at_one_size_the_rate_nearest_15_wins_then_the_cheaper_format() {
        let modes = vec![
            mode(640, 480, FrameFormat::NV12, 30),
            mode(640, 480, FrameFormat::NV12, 15),
            mode(640, 480, FrameFormat::YUYV, 15),
            mode(640, 480, FrameFormat::MJPEG, 15),
        ];
        let picked = camera_candidates(&modes)[0];
        assert_eq!(picked.frame_rate(), 15);
        assert_eq!(picked.format(), FrameFormat::MJPEG);
    }

    #[test]
    fn modes_that_cannot_be_decoded_or_are_empty_are_never_picked() {
        let modes = vec![
            mode(640, 480, FrameFormat::GRAY, 15),
            mode(0, 0, FrameFormat::MJPEG, 15),
            mode(0, 480, FrameFormat::YUYV, 15),
        ];
        assert!(camera_candidates(&modes).is_empty());
        assert!(camera_candidates(&[]).is_empty());
    }

    /// What Daniel's Mac did: it lists 640x480 at 15 (the low end of a 15-30 range), and only the
    /// top of a range can be set, so that mode was refused and the camera never started.
    #[test]
    fn a_listed_mode_the_camera_refuses_is_skipped_for_the_next_one() {
        let modes = vec![
            mode(640, 480, FrameFormat::YUYV, 15),
            mode(640, 480, FrameFormat::YUYV, 30),
            mode(1280, 720, FrameFormat::NV12, 30),
        ];
        let candidates = camera_candidates(&modes);
        assert_eq!(candidates[0], mode(640, 480, FrameFormat::YUYV, 15));
        let mut tried = Vec::new();
        let taken = first_accepted(&candidates, |m| {
            tried.push(m);
            if m.frame_rate() == 30 {
                Ok(())
            } else {
                Err("Not Found/Rejected/Unsupported")
            }
        });
        assert_eq!(taken, Some(mode(640, 480, FrameFormat::YUYV, 30)));
        assert_eq!(tried.len(), 2);
    }

    #[test]
    fn a_camera_that_refuses_everything_is_left_in_its_own_mode() {
        let candidates = camera_candidates(&mac_modes());
        let taken = first_accepted(&candidates, |_| Err::<(), _>("refused"));
        assert_eq!(taken, None);
    }

    #[test]
    fn opening_falls_through_to_the_next_request_and_says_why_when_all_fail() {
        let mut asked = Vec::new();
        let opened = first_opened(&["highest rate", "highest size", "any"], |r| {
            asked.push(*r);
            if *r == "any" {
                Ok(7)
            } else {
                Err("Not Found/Rejected/Unsupported")
            }
        });
        assert_eq!(opened, Ok(7));
        assert_eq!(asked, ["highest rate", "highest size", "any"]);

        let failed = first_opened(&["a", "b"], |_| Err::<(), _>("refused")).unwrap_err();
        assert_eq!(failed, "\"a\": refused; \"b\": refused");
        assert!(first_opened::<&str, (), &str>(&[], |_| Ok(())).is_err());
    }

    /// The Mac lists the ends of a range, and opens only at the top, so the first request tried
    /// must land on a rate that is the top of some range, never the low end `None` picks.
    #[test]
    fn the_first_request_lands_on_the_top_of_a_range_not_its_low_end() {
        let listed = vec![
            mode(640, 480, FrameFormat::YUYV, 15),
            mode(640, 480, FrameFormat::YUYV, 30),
            mode(1280, 720, FrameFormat::YUYV, 15),
            mode(1280, 720, FrameFormat::YUYV, 30),
        ];
        let none = RequestedFormat::new::<RgbFormat>(RequestedFormatType::None);
        assert_eq!(none.fulfill(&listed).map(|m| m.frame_rate()), Some(15));
        let highest =
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
        assert_eq!(
            highest.fulfill(&listed),
            Some(mode(1280, 720, FrameFormat::YUYV, 30))
        );
    }
}
