use std::{
    collections::VecDeque,
    io::{self, Cursor},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source, buffer::SamplesBuffer};

use crate::settings::SoundSettings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
enum ClickSound {
    AgoBackspace,
    BefhpTab,
    Ciq,
    SpaceFallback,
    Lsw,
    MxEnter,
    J,
    N,
}

impl ClickSound {
    const ALL: [Self; 8] = [
        Self::AgoBackspace,
        Self::BefhpTab,
        Self::Ciq,
        Self::SpaceFallback,
        Self::Lsw,
        Self::MxEnter,
        Self::J,
        Self::N,
    ];

    fn select_for_key(key: KeyEvent) -> Self {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('w') {
            return Self::AgoBackspace;
        }
        match key.code {
            KeyCode::Backspace => Self::AgoBackspace,
            KeyCode::Tab => Self::BefhpTab,
            KeyCode::Enter => Self::MxEnter,
            KeyCode::Char(character) => match character.to_ascii_lowercase() {
                'a' | 'g' | 'o' | '2' | '@' => Self::AgoBackspace,
                'b' | 'e' | 'f' | 'h' | 'p' | '3' | '6' | '7' | '9' | '#' | '^' | '&' | '(' => {
                    Self::BefhpTab
                }
                'c' | 'i' | 'q' | '4' | '$' => Self::Ciq,
                'l' | 's' | 'w' | '0' | ')' => Self::Lsw,
                'm' | 'x' | '1' | '!' => Self::MxEnter,
                'j' | '8' | '*' => Self::J,
                'n' => Self::N,
                _ => Self::SpaceFallback,
            },
            _ => Self::SpaceFallback,
        }
    }

    fn read_bundled_audio(self) -> &'static [u8] {
        match self {
            Self::AgoBackspace => include_bytes!("../assets/sounds/click-08.wav"),
            Self::BefhpTab => include_bytes!("../assets/sounds/click-01.wav"),
            Self::Ciq => include_bytes!("../assets/sounds/click-09.wav"),
            Self::SpaceFallback => include_bytes!("../assets/sounds/click-06.wav"),
            Self::Lsw => include_bytes!("../assets/sounds/click-05.wav"),
            Self::MxEnter => include_bytes!("../assets/sounds/click-04.wav"),
            Self::J => include_bytes!("../assets/sounds/click-07.wav"),
            Self::N => include_bytes!("../assets/sounds/click-02.wav"),
        }
    }
}
const MAX_QUEUED_CLICKS: usize = 8;
const MAX_OVERLAPPING_CLICKS: usize = 8;
const MAX_CLICK_DELAY: Duration = Duration::from_millis(80);
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(10);
const UNAVAILABLE_NOTICE: &str = "Sound unavailable; typing continues. Change Sound to retry.";

struct AudioControl {
    volume: AtomicU8,
    revision: AtomicU64,
}

struct ClickRequest {
    sound: ClickSound,
    queued_at: Instant,
    revision: u64,
}

impl ClickRequest {
    fn is_current(&self, revision: u64) -> bool {
        self.revision == revision && self.queued_at.elapsed() <= MAX_CLICK_DELAY
    }
}

pub struct KeystrokeAudio {
    clicks: SyncSender<ClickRequest>,
    control: Arc<AudioControl>,
    notices: Receiver<&'static str>,
    settings: SoundSettings,
}

impl KeystrokeAudio {
    pub fn start(settings: SoundSettings) -> io::Result<Self> {
        Self::start_with_output(settings, DeviceOutput::open)
    }

    fn start_with_output<Output, Factory>(
        settings: SoundSettings,
        open_output: Factory,
    ) -> io::Result<Self>
    where
        Output: ClickOutput,
        Factory: FnMut(Arc<AtomicBool>) -> Result<Output> + Send + 'static,
    {
        let (click_sender, click_receiver) = mpsc::sync_channel(MAX_QUEUED_CLICKS);
        let (notice_sender, notice_receiver) = mpsc::sync_channel(1);
        let control = Arc::new(AudioControl {
            volume: AtomicU8::new(settings.calculate_effective_volume()),
            revision: AtomicU64::new(0),
        });
        let worker = AudioWorker {
            clicks: click_receiver,
            control: Arc::clone(&control),
            notices: notice_sender,
            open_output,
        };
        thread::Builder::new()
            .name("chivava-audio".into())
            .spawn(move || worker.run())?;
        Ok(Self {
            clicks: click_sender,
            control,
            notices: notice_receiver,
            settings,
        })
    }

    pub fn configure(&mut self, settings: SoundSettings) {
        if self.settings != settings {
            self.control
                .volume
                .store(settings.calculate_effective_volume(), Ordering::Release);
            self.control.revision.fetch_add(1, Ordering::AcqRel);
            self.settings = settings;
        }
    }

    pub fn play_click(&self, key: KeyEvent) {
        if self.settings.calculate_effective_volume() > 0 {
            let _ = self.clicks.try_send(ClickRequest {
                sound: ClickSound::select_for_key(key),
                queued_at: Instant::now(),
                revision: self.control.revision.load(Ordering::Acquire),
            });
        }
    }

    pub fn take_notice(&self) -> Option<&'static str> {
        self.notices.try_recv().ok()
    }
}

trait ClickOutput {
    fn set_volume(&mut self, volume: u8);
    fn play_click(&mut self, sample: SamplesBuffer);
}

struct DeviceOutput {
    stream: OutputStream,
    voices: VecDeque<Sink>,
    volume: f32,
}

impl DeviceOutput {
    fn open(is_failed: Arc<AtomicBool>) -> Result<Self> {
        let mut stream = OutputStreamBuilder::from_default_device()?
            .with_error_callback(move |_| is_failed.store(true, Ordering::Release))
            .with_buffer_size(rodio::cpal::BufferSize::Fixed(256))
            .open_stream_or_fallback()?;
        stream.log_on_drop(false);
        Ok(Self {
            stream,
            voices: VecDeque::new(),
            volume: 0.0,
        })
    }
}

impl ClickOutput for DeviceOutput {
    fn set_volume(&mut self, volume: u8) {
        self.volume = f32::from(volume.min(100)) / 100.0;
        self.voices.retain(|voice| !voice.empty());
        for voice in &self.voices {
            voice.set_volume(self.volume);
        }
    }

    fn play_click(&mut self, sample: SamplesBuffer) {
        if self.voices.len() >= MAX_OVERLAPPING_CLICKS
            && let Some(voice) = self.voices.pop_front()
        {
            voice.stop();
        }
        let voice = Sink::connect_new(self.stream.mixer());
        voice.set_volume(self.volume);
        voice.append(sample);
        self.voices.push_back(voice);
    }
}

struct AudioWorker<Factory> {
    clicks: Receiver<ClickRequest>,
    control: Arc<AudioControl>,
    notices: SyncSender<&'static str>,
    open_output: Factory,
}

impl<Output: ClickOutput, Factory: FnMut(Arc<AtomicBool>) -> Result<Output>> AudioWorker<Factory> {
    fn run(mut self) {
        let Ok(samples) = load_click_samples() else {
            let _ = self
                .notices
                .try_send("Bundled sound unavailable; typing continues.");
            return;
        };
        let mut output: Option<Output> = None;
        let mut previous_revision = 0;
        let mut is_unavailable = false;
        let is_device_failed = Arc::new(AtomicBool::new(false));
        let mut pending_click: Option<ClickRequest> = None;
        loop {
            let revision = self.control.revision.load(Ordering::Acquire);
            let volume = self.control.volume.load(Ordering::Acquire);
            if previous_revision != revision {
                is_unavailable = false;
                previous_revision = revision;
            }
            if is_device_failed.swap(false, Ordering::AcqRel) {
                output = None;
                is_unavailable = true;
                let _ = self.notices.try_send(UNAVAILABLE_NOTICE);
            }
            if volume == 0 {
                output = None;
            } else if output.is_none() && !is_unavailable {
                match (self.open_output)(Arc::clone(&is_device_failed)) {
                    Ok(device) => output = Some(device),
                    Err(_) => {
                        is_unavailable = true;
                        let _ = self.notices.try_send(UNAVAILABLE_NOTICE);
                    }
                }
            }
            if let Some(device) = &mut output {
                device.set_volume(volume);
                if let Some(click) = pending_click
                    .take()
                    .filter(|click| click.is_current(revision))
                {
                    device.play_click(samples[click.sound as usize].clone());
                }
            }
            pending_click = match self.clicks.recv_timeout(CONTROL_POLL_INTERVAL) {
                Ok(click) => Some(click),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
        }
    }
}

fn load_click_samples() -> Result<Vec<SamplesBuffer>> {
    ClickSound::ALL
        .iter()
        .map(|sound| {
            let decoder = Decoder::try_from(Cursor::new(sound.read_bundled_audio()))?;
            Ok(SamplesBuffer::new(
                decoder.channels(),
                decoder.sample_rate(),
                decoder.collect::<Vec<_>>(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
        sync::Mutex,
    };

    fn fingerprint_click(sample: SamplesBuffer) -> String {
        let mut hash = DefaultHasher::new();
        for value in sample {
            value.to_bits().hash(&mut hash);
        }
        format!("click {}", hash.finish())
    }

    #[test]
    #[ignore = "requires an audio output device; plays three quiet clicks"]
    fn plays_overlapping_clicks_through_the_default_device() {
        let is_failed = Arc::new(AtomicBool::new(false));
        let mut output = DeviceOutput::open(Arc::clone(&is_failed)).unwrap();
        output.set_volume(25);
        for sample in load_click_samples().unwrap().into_iter().take(3) {
            output.play_click(sample);
            thread::sleep(Duration::from_millis(30));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while output.voices.iter().any(|voice| !voice.empty()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(output.voices.iter().all(Sink::empty));
        assert!(!is_failed.load(Ordering::Acquire));
    }

    #[test]
    fn key_groups_are_stable_case_insensitive_and_cover_every_sound() {
        for (index, sound) in ClickSound::ALL.into_iter().enumerate() {
            assert_eq!(sound as usize, index);
        }
        for (characters, expected) in [
            ("ago2@", ClickSound::AgoBackspace),
            ("befhp3679#^&(", ClickSound::BefhpTab),
            ("ciq4$", ClickSound::Ciq),
            ("dkrtuvyz5% .,:;[]{}+-_=/ ?é界", ClickSound::SpaceFallback),
            ("lsw0)", ClickSound::Lsw),
            ("mx1!", ClickSound::MxEnter),
            ("j8*", ClickSound::J),
            ("n", ClickSound::N),
        ] {
            for character in characters.chars() {
                for (character, modifiers) in [
                    (character, KeyModifiers::NONE),
                    (character.to_ascii_uppercase(), KeyModifiers::SHIFT),
                ] {
                    assert_eq!(
                        ClickSound::select_for_key(KeyEvent::new(
                            KeyCode::Char(character),
                            modifiers
                        )),
                        expected,
                        "{character}"
                    );
                }
            }
        }
        for (key, expected) in [
            (
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                ClickSound::AgoBackspace,
            ),
            (
                KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
                ClickSound::AgoBackspace,
            ),
            (
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                ClickSound::MxEnter,
            ),
            (
                KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                ClickSound::BefhpTab,
            ),
        ] {
            assert_eq!(ClickSound::select_for_key(key), expected);
        }
    }

    #[test]
    fn repeated_words_keep_their_sound_sequence_after_unrelated_input() {
        let select_word = |word: &str| {
            word.chars()
                .map(|character| {
                    ClickSound::select_for_key(KeyEvent::new(
                        KeyCode::Char(character),
                        KeyModifiers::NONE,
                    ))
                })
                .collect::<Vec<_>>()
        };
        let first = select_word("hello world");
        let _ = select_word("other keys 1234");
        assert_eq!(first, select_word("hello world"));
        assert_eq!(first, select_word("HELLO WORLD"));
    }

    #[test]
    fn decodes_distinct_clicks_with_normalized_peaks_and_silent_boundaries() {
        let samples = load_click_samples().unwrap();
        assert_eq!(samples.len(), ClickSound::ALL.len());
        let mut fingerprints = std::collections::HashSet::new();
        for sample in samples {
            assert_eq!(sample.channels(), 1);
            assert_eq!(sample.sample_rate(), 44100);
            assert!(
                (Duration::from_millis(180)..Duration::from_millis(280))
                    .contains(&sample.total_duration().unwrap())
            );
            let values: Vec<_> = sample.collect();
            assert_eq!(values.first(), Some(&0.0));
            assert_eq!(values.last(), Some(&0.0));
            let peak = values
                .iter()
                .map(|value| value.abs())
                .fold(0.0_f32, f32::max);
            assert!((0.449..0.451).contains(&peak), "Unnormalized peak: {peak}");
            assert!(
                fingerprints.insert(
                    values
                        .iter()
                        .map(|value| value.to_bits())
                        .collect::<Vec<_>>()
                )
            );
        }
    }

    #[test]
    fn queued_clicks_expire_and_configuration_changes_invalidate_them() {
        let stale = ClickRequest {
            sound: ClickSound::J,
            queued_at: Instant::now() - Duration::from_secs(1),
            revision: 0,
        };
        assert!(!stale.is_current(0));
        let fresh = ClickRequest {
            sound: ClickSound::N,
            queued_at: Instant::now(),
            revision: 1,
        };
        assert!(fresh.is_current(1));
        assert!(!fresh.is_current(2));
    }

    #[test]
    fn enqueue_never_waits_for_playback_and_muting_does_not_queue() {
        let (clicks, receiver) = mpsc::sync_channel(MAX_QUEUED_CLICKS);
        let (_, notices) = mpsc::sync_channel(1);
        let mut audio = KeystrokeAudio {
            clicks,
            control: Arc::new(AudioControl {
                volume: AtomicU8::new(50),
                revision: AtomicU64::new(0),
            }),
            notices,
            settings: SoundSettings::create_initial(),
        };
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        for _ in 0..1000 {
            audio.play_click(key);
        }
        let queued: Vec<_> = receiver.try_iter().collect();
        assert_eq!(queued.len(), MAX_QUEUED_CLICKS);
        assert!(
            queued
                .iter()
                .all(|click| click.sound == ClickSound::AgoBackspace)
        );
        audio.configure(SoundSettings {
            is_enabled: false,
            volume: 50,
        });
        audio.play_click(key);
        assert!(receiver.try_recv().is_err());
        audio.configure(SoundSettings {
            is_enabled: true,
            volume: 0,
        });
        audio.play_click(key);
        assert!(receiver.try_recv().is_err());
    }

    struct FakeOutput {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl ClickOutput for FakeOutput {
        fn set_volume(&mut self, volume: u8) {
            self.events.lock().unwrap().push(format!("volume {volume}"));
        }
        fn play_click(&mut self, sample: SamplesBuffer) {
            self.events.lock().unwrap().push(fingerprint_click(sample));
        }
    }

    #[test]
    fn startup_opens_output_in_parallel_without_waiting_for_device_or_keystrokes() {
        let (opening, started) = mpsc::channel();
        let (release, initialization) = mpsc::channel();
        let audio = KeystrokeAudio::start_with_output(
            SoundSettings {
                is_enabled: true,
                volume: 25,
            },
            move |_| {
                opening.send(thread::current().id())?;
                initialization.recv_timeout(Duration::from_secs(2))?;
                Ok(FakeOutput {
                    events: Arc::new(Mutex::new(vec![])),
                })
            },
        )
        .unwrap();
        assert_ne!(
            started.recv_timeout(Duration::from_secs(2)).unwrap(),
            thread::current().id()
        );
        audio.play_click(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        release.send(()).unwrap();
        drop(audio);
        assert_eq!(
            started.recv_timeout(Duration::from_secs(2)),
            Err(RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn enabling_muted_audio_initializes_output_without_a_keystroke() {
        for initial_settings in [
            SoundSettings {
                is_enabled: false,
                volume: 25,
            },
            SoundSettings {
                is_enabled: true,
                volume: 0,
            },
        ] {
            let (opening, started) = mpsc::channel();
            let mut audio = KeystrokeAudio::start_with_output(initial_settings, move |_| {
                opening.send(())?;
                Ok(FakeOutput {
                    events: Arc::new(Mutex::new(vec![])),
                })
            })
            .unwrap();
            assert_eq!(
                started.recv_timeout(Duration::from_millis(50)),
                Err(RecvTimeoutError::Timeout)
            );
            audio.configure(SoundSettings {
                is_enabled: true,
                volume: 25,
            });
            started.recv_timeout(Duration::from_secs(2)).unwrap();
            drop(audio);
            assert_eq!(
                started.recv_timeout(Duration::from_secs(2)),
                Err(RecvTimeoutError::Disconnected)
            );
        }
    }

    #[test]
    fn worker_preserves_sound_identity_and_discards_stale_requests() {
        let events = Arc::new(Mutex::new(vec![]));
        let recorded = Arc::clone(&events);
        let (sender, clicks) = mpsc::sync_channel(MAX_QUEUED_CLICKS);
        let (notices, _) = mpsc::sync_channel(1);
        let requested = [
            ClickSound::AgoBackspace,
            ClickSound::Lsw,
            ClickSound::AgoBackspace,
        ];
        for discarded in [
            ClickRequest {
                sound: ClickSound::N,
                queued_at: Instant::now() - Duration::from_secs(1),
                revision: 0,
            },
            ClickRequest {
                sound: ClickSound::J,
                queued_at: Instant::now(),
                revision: 1,
            },
        ] {
            sender.send(discarded).unwrap();
        }
        for sound in requested {
            sender
                .send(ClickRequest {
                    sound,
                    queued_at: Instant::now(),
                    revision: 0,
                })
                .unwrap();
        }
        drop(sender);
        AudioWorker {
            clicks,
            control: Arc::new(AudioControl {
                volume: AtomicU8::new(25),
                revision: AtomicU64::new(0),
            }),
            notices,
            open_output: move |_| {
                Ok(FakeOutput {
                    events: Arc::clone(&recorded),
                })
            },
        }
        .run();
        let events = events.lock().unwrap();
        let clicks: Vec<_> = events
            .iter()
            .filter(|event| event.starts_with("click"))
            .collect();
        let samples = load_click_samples().unwrap();
        let expected = requested.map(|sound| fingerprint_click(samples[sound as usize].clone()));
        assert_eq!(clicks, expected.iter().collect::<Vec<_>>());
        assert_ne!(clicks[0], clicks[1]);
        assert_eq!(clicks[0], clicks[2]);
        assert!(events.iter().any(|event| event == "volume 25"));
    }

    #[test]
    fn device_failure_is_reported_once_without_stopping_input() {
        let (sender, clicks) = mpsc::sync_channel(MAX_QUEUED_CLICKS);
        let (notices, errors) = mpsc::sync_channel(1);
        for _ in 0..3 {
            sender
                .send(ClickRequest {
                    sound: ClickSound::SpaceFallback,
                    queued_at: Instant::now(),
                    revision: 0,
                })
                .unwrap();
        }
        drop(sender);
        let mut attempts = 0;
        AudioWorker {
            clicks,
            control: Arc::new(AudioControl {
                volume: AtomicU8::new(50),
                revision: AtomicU64::new(0),
            }),
            notices,
            open_output: |_| -> Result<FakeOutput> {
                attempts += 1;
                anyhow::bail!("no audio device")
            },
        }
        .run();
        assert_eq!(attempts, 1);
        assert_eq!(
            errors.try_iter().collect::<Vec<_>>(),
            vec![UNAVAILABLE_NOTICE]
        );
    }

    #[test]
    fn worker_retries_failed_output_after_settings_change() {
        let events = Arc::new(Mutex::new(vec![]));
        let recorded = Arc::clone(&events);
        let control = Arc::new(AudioControl {
            volume: AtomicU8::new(50),
            revision: AtomicU64::new(0),
        });
        let worker_control = Arc::clone(&control);
        let (sender, clicks) = mpsc::sync_channel(MAX_QUEUED_CLICKS);
        let (notices, errors) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let mut attempts = 0;
            AudioWorker {
                clicks,
                control: worker_control,
                notices,
                open_output: |_| {
                    attempts += 1;
                    if attempts == 1 {
                        anyhow::bail!("device temporarily unavailable");
                    }
                    Ok(FakeOutput {
                        events: Arc::clone(&recorded),
                    })
                },
            }
            .run();
            attempts
        });
        assert_eq!(
            errors.recv_timeout(Duration::from_secs(2)).unwrap(),
            UNAVAILABLE_NOTICE
        );
        control.volume.store(25, Ordering::Release);
        control.revision.store(1, Ordering::Release);
        sender
            .send(ClickRequest {
                sound: ClickSound::N,
                queued_at: Instant::now(),
                revision: 1,
            })
            .unwrap();
        drop(sender);
        assert_eq!(worker.join().unwrap(), 2);
        let events = events.lock().unwrap();
        assert!(events.iter().any(|event| event == "volume 25"));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.starts_with("click"))
                .count(),
            1
        );
    }

    #[test]
    fn disabled_audio_never_opens_a_device() {
        let (sender, clicks) = mpsc::sync_channel(MAX_QUEUED_CLICKS);
        let (notices, _) = mpsc::sync_channel(1);
        drop(sender);
        AudioWorker {
            clicks,
            control: Arc::new(AudioControl {
                volume: AtomicU8::new(0),
                revision: AtomicU64::new(0),
            }),
            notices,
            open_output: |_| -> Result<FakeOutput> { panic!("muted audio opened a device") },
        }
        .run();
    }
}
