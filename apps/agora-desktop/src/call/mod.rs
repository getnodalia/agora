//! Calls: talking and seeing each other in a channel.
//!
//! The server only says who is in a call and passes frames along; everything about the media is
//! here. Each device seals its microphone and camera with the conversation's call key
//! (`agora_e2e::call`), so the server relays what it cannot read, and opens what the others
//! send. [`codec`] turns audio and pictures into bytes, [`devices`] runs the hardware, and
//! [`Call`] ties them to one channel for as long as this device is in its call.

pub mod codec;
pub mod devices;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use agora_client::MediaSender;
use agora_core::{ChannelId, UserId};
use agora_e2e::call::{CallKey, OpenError, Opener, Sealer, Stream};
use agora_protocol::media::{self, Down, Kind, Up};

use codec::{Mixer, Picture};
use devices::{Problem, Report, Worker};

/// What happened to a frame that arrived.
#[derive(Debug, PartialEq, Eq)]
pub enum Arrived {
    /// Played or shown, or ignored because it was not for this call.
    Handled,
    /// Sealed under an epoch this device has no key for: fetch the current key.
    NeedKey,
}

/// The latest picture of each person, waiting for the window thread to turn it into a texture.
#[derive(Default)]
struct Pictures {
    next: u64,
    latest: HashMap<UserId, (u64, Picture)>,
}

/// This device's part in a channel's call.
pub struct Call {
    pub channel: ChannelId,
    me: UserId,
    sealer: Arc<Mutex<Sealer>>,
    opener: Opener,
    mixer: Arc<Mutex<Mixer>>,
    pictures: Arc<Mutex<Pictures>>,
    /// The microphone is on (not muted).
    pub mic_on: Arc<AtomicBool>,
    pub camera_on: bool,
    media: MediaSender,
    report: Report,
    problems: Arc<Mutex<Vec<Problem>>>,
    _microphone: Worker,
    _speaker: Worker,
    camera: Option<Worker>,
    /// The textures drawn for each person, with the picture number they show.
    pub textures: HashMap<UserId, (u64, egui::TextureHandle)>,
}

impl Call {
    /// Start sending and playing. Missing devices are reported through [`Call::take_problems`].
    pub fn start(channel: ChannelId, me: UserId, key: CallKey, media: MediaSender) -> Call {
        let sealer = Arc::new(Mutex::new(Sealer::new(channel, me, key.clone())));
        let mut opener = Opener::new(channel);
        opener.add_key(key);
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        let problems = Arc::new(Mutex::new(Vec::<Problem>::new()));
        let report: Report = {
            let problems = problems.clone();
            Arc::new(move |why| {
                problems
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(why);
            })
        };
        let mic_on = Arc::new(AtomicBool::new(true));
        let microphone = {
            let sealer = sealer.clone();
            let media = media.clone();
            devices::microphone(mic_on.clone(), report.clone(), move |pcm| {
                send(
                    &sealer,
                    &media,
                    channel,
                    Kind::Audio,
                    &codec::encode_audio(pcm),
                );
            })
        };
        let speaker = devices::speaker(mixer.clone(), report.clone());
        Call {
            channel,
            me,
            sealer,
            opener,
            mixer,
            pictures: Arc::new(Mutex::new(Pictures::default())),
            mic_on,
            camera_on: false,
            media,
            report,
            problems,
            _microphone: microphone,
            _speaker: speaker,
            camera: None,
            textures: HashMap::new(),
        }
    }

    pub fn muted(&self) -> bool {
        !self.mic_on.load(Ordering::Relaxed)
    }

    pub fn set_muted(&self, muted: bool) {
        self.mic_on.store(!muted, Ordering::Relaxed);
    }

    /// Turn the camera on or off. Our own picture is shown from what the camera captures, so
    /// the person sees what the others see.
    pub fn set_camera(&mut self, on: bool) {
        self.camera_on = on;
        if !on {
            self.camera = None;
            self.pictures
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .latest
                .remove(&self.me);
            self.textures.remove(&self.me);
            return;
        }
        if self.camera.is_some() {
            return;
        }
        let (sealer, media, pictures, me, channel) = (
            self.sealer.clone(),
            self.media.clone(),
            self.pictures.clone(),
            self.me,
            self.channel,
        );
        self.camera = Some(devices::camera(self.report.clone(), move |shot| {
            let Some(jpeg) =
                codec::encode_video(&shot.rgb, shot.width, shot.height, codec::VIDEO_WIDTH)
            else {
                return;
            };
            if jpeg.len() > media::MAX_PAYLOAD - 64 {
                return;
            }
            // Show what is being sent.
            if let Some(picture) = codec::decode_video(&jpeg) {
                store(&pictures, me, picture);
            }
            send(&sealer, &media, channel, Kind::Video, &jpeg);
        }));
    }

    /// Use the key of a newer epoch for sending and for opening.
    pub fn add_key(&mut self, key: CallKey) {
        self.opener.add_key(key.clone());
        self.sealer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .rekey(key);
    }

    /// Someone left the call: stop playing and showing them.
    pub fn forget(&mut self, who: UserId) {
        self.mixer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(who);
        self.pictures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .latest
            .remove(&who);
        self.textures.remove(&who);
    }

    /// A binary frame from the server.
    pub fn arrived(&mut self, bytes: &[u8]) -> Arrived {
        let Some(down) = Down::parse(bytes) else {
            return Arrived::Handled;
        };
        if down.channel != self.channel || down.sender == self.me {
            return Arrived::Handled;
        }
        let stream = match down.kind {
            Kind::Audio => Stream::Audio,
            Kind::Video => Stream::Video,
        };
        let media = match self.opener.open(down.sender, stream, down.payload) {
            Ok(media) => media,
            Err(OpenError::UnknownEpoch) => return Arrived::NeedKey,
            // Forged, replayed or malformed: dropped like any lost frame.
            Err(_) => return Arrived::Handled,
        };
        match down.kind {
            Kind::Audio => self
                .mixer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(down.sender, &codec::decode_audio(&media)),
            Kind::Video => {
                if let Some(picture) = codec::decode_video(&media) {
                    store(&self.pictures, down.sender, picture);
                }
            }
        }
        Arrived::Handled
    }

    /// Update textures from the newest pictures. Returns the ones that changed.
    pub fn refresh_textures(&mut self, ctx: &egui::Context) {
        let pictures = self.pictures.lock().unwrap_or_else(PoisonError::into_inner);
        for (who, (number, picture)) in &pictures.latest {
            if self.textures.get(who).is_some_and(|(n, _)| n == number) {
                continue;
            }
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [picture.width, picture.height],
                &picture.rgba,
            );
            match self.textures.get_mut(who) {
                Some((n, texture)) => {
                    texture.set(image, egui::TextureOptions::LINEAR);
                    *n = *number;
                }
                None => {
                    let texture = ctx.load_texture(
                        format!("call-{who}"),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.textures.insert(*who, (*number, texture));
                }
            }
        }
        self.textures
            .retain(|who, _| pictures.latest.contains_key(who));
    }

    /// What the devices reported since the last call, for the person to read.
    pub fn take_problems(&self) -> Vec<Problem> {
        std::mem::take(&mut *self.problems.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

fn store(pictures: &Mutex<Pictures>, who: UserId, picture: Picture) {
    let mut pictures = pictures.lock().unwrap_or_else(PoisonError::into_inner);
    pictures.next += 1;
    let number = pictures.next;
    pictures.latest.insert(who, (number, picture));
}

fn send(sealer: &Mutex<Sealer>, media: &MediaSender, channel: ChannelId, kind: Kind, plain: &[u8]) {
    let stream = match kind {
        Kind::Audio => Stream::Audio,
        Kind::Video => Stream::Video,
    };
    let sealed = sealer
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .seal(stream, plain);
    let Some(sealed) = sealed else {
        return;
    };
    if sealed.len() > media::MAX_PAYLOAD {
        return;
    }
    media.send(
        Up {
            kind,
            channel,
            payload: &sealed,
        }
        .encode(),
    );
}
