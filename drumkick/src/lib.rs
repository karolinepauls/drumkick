use kira::backend::cpal::cpal::{
    default_host,
    traits::{DeviceTrait, HostTrait},
    BufferSize,
};
use kira::backend::cpal::CpalBackendSettings;
use kira::{
    clock::{ClockHandle, ClockSpeed, ClockTime},
    sound::static_sound::{StaticSoundData, StaticSoundSettings},
    track::MainTrackBuilder,
    AudioManager, AudioManagerSettings, DefaultBackend, Tween,
};
pub use pest::set_error_detail as pest_set_error_detail;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::{thread, time};

pub mod embedded_samples;
pub mod measure_grammar;
pub mod samples;

const BUFFER_SIZE: u32 = 2048;
const PRECISION: usize = 32;
const MIN_MEASURES: u64 = 4;

pub struct DrumkickOpts {
    pub buffer_size: u32,
    pub precision: usize,
    pub sound_capacity: usize,
    pub message_buffer_size: usize,
}

impl Default for DrumkickOpts {
    fn default() -> Self {
        DrumkickOpts {
            buffer_size: BUFFER_SIZE,
            precision: PRECISION,
            sound_capacity: 512,
            message_buffer_size: 128,
        }
    }
}

#[derive(Debug)]
pub enum Message {
    Play,
    Pause,
    Stop,
    SetBPM(f64),
}

#[derive(Debug)]
pub struct Beat {
    pub samples: Vec<StaticSoundData>,
    pub time: f64,
}

#[derive(Debug)]
pub struct Track {
    pub beats_per_minute: f64,
    pub beats_in_measure: u64,
    pub measure_beats: Vec<Beat>,
}

/// The metronome/drum machine
pub struct DrumkickBackend {
    receiver: Receiver<Message>,
    manager: AudioManager,
    clock: ClockHandle,
    track: Track,
}

impl DrumkickBackend {
    pub fn new(
        opts: DrumkickOpts,
        track: Track,
    ) -> Result<(SyncSender<Message>, Self), Box<dyn std::error::Error>> {
        let host = default_host();
        let device = host.default_output_device().unwrap();
        let mut cpal_config = device.default_output_config()?.config();
        cpal_config.buffer_size = BufferSize::Fixed(opts.buffer_size);

        let cpal_settings = CpalBackendSettings {
            device: Some(device),
            config: Some(cpal_config),
        };
        let settings = AudioManagerSettings {
            backend_settings: cpal_settings,
            internal_buffer_size: opts.precision,
            main_track_builder: MainTrackBuilder::default().sound_capacity(opts.sound_capacity),
            ..AudioManagerSettings::default()
        };
        let mut manager = AudioManager::<DefaultBackend>::new(settings)?;
        let clock = manager.add_clock(ClockSpeed::TicksPerMinute(track.beats_per_minute))?;

        let (sender, receiver) = sync_channel(opts.message_buffer_size);

        Ok((
            sender,
            DrumkickBackend {
                receiver,
                manager,
                clock,
                track,
            },
        ))
    }

    // Consume self and start playback in a different thread, while receiving control messages.
    pub fn play(mut self) -> Result<(), Box<dyn std::error::Error>> {
        thread::spawn(move || {
            let mut next_beat_at = self.clock.time();
            eprintln!("Metronome started");
            let mut clock_started = false;

            loop {
                let measures_ahead = safe_measures_ahead(self.track.beats_per_minute);

                next_beat_at = schedule_beats(
                    &mut self.manager,
                    next_beat_at,
                    &self.track.measure_beats,
                    self.track.beats_in_measure,
                    measures_ahead,
                )
                .unwrap();
                eprintln!("Scheduled {measures_ahead} measures");

                if !clock_started {
                    self.clock.start();
                    clock_started = true;
                }

                // Sleep until the next/last measure is half-finished, basing on clock.time() instead of
                // system time. The clock actually gets updated every time the audio thread gets a callback
                // (probably every 2048 audio wave samples), which makes it not very precise but the
                // important feature is that the clock won't desync with audio device time.
                loop {
                    let single_beat_duration = beat_duration(self.track.beats_per_minute);

                    let clock_time = self.clock.time();
                    let sleep_target = next_beat_at - self.track.beats_in_measure / 2;
                    let sleep_time_in_beats = ((sleep_target.ticks as f64
                        - clock_time.ticks as f64)
                        + (sleep_target.fraction - clock_time.fraction))
                        .max(0.0);

                    let wait_duration = single_beat_duration * sleep_time_in_beats;

                    eprintln!("receiving for {}", wait_duration);
                    let message = self
                        .receiver
                        .recv_timeout(time::Duration::from_secs_f64(wait_duration))
                        .ok();
                    match message {
                        None => {
                            break;
                        }
                        Some(Message::Play) => {
                            eprintln!("START");
                            self.clock.start();
                        }
                        Some(Message::Pause) => {
                            eprintln!("PAUSE");
                            self.clock.pause();
                        }
                        Some(Message::Stop) => {
                            eprintln!("STOP");
                            self.clock.stop();
                            return;
                        }
                        Some(Message::SetBPM(bpm)) => {
                            eprintln!("BPM: {bpm}");
                            self.track.beats_per_minute = bpm;
                            self.clock.set_speed(
                                ClockSpeed::TicksPerMinute(self.track.beats_per_minute),
                                Tween {
                                    duration: time::Duration::from_millis(50),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                }
            }
        });
        Ok(())
    }
}

fn beat_duration(bpm: f64) -> f64 {
    assert!(bpm > 0.0, "BPM must be greater than 0, is {}", bpm);
    let beats_per_second = bpm / 60.0;
    1.0 / beats_per_second
}

/// It should be enough to ensure there is 1 measure scheduled ahead of the current measure (or
/// 2 if we consider that checking/scheduling may overlap with the current measure finishing).
/// However, considering OS scheduling problems, let's make sure that "measures ahead" grow in
/// proportion to the BPM.
fn safe_measures_ahead(bpm: f64) -> u64 {
    let measures_ahead_float = (bpm / 60.0 * 2.0).ceil();
    assert!(
        measures_ahead_float.is_finite(),
        "Expecting measures ahead to be finite but it's {}",
        measures_ahead_float
    );
    (measures_ahead_float as u64).max(MIN_MEASURES)
}

fn schedule_beats(
    manager: &mut AudioManager,
    start_at: ClockTime,
    beats: &[Beat],
    beats_in_measure: u64,
    measures: u64,
) -> Result<ClockTime, Box<dyn std::error::Error>> {
    for measure_no in 0..measures {
        let measure_time = start_at + beats_in_measure * measure_no;

        for beat in beats.iter() {
            let beat_time = measure_time + beat.time;

            for sample in &beat.samples {
                // We could break out of this loop once the manager runs out of available sounds
                // and return appropriate next_beat_at.
                manager.play(
                    sample
                        .clone()
                        .with_settings(StaticSoundSettings::new().start_time(beat_time)),
                )?;
            }
        }
    }

    let next_beat_at = start_at + beats_in_measure * measures;
    Ok(next_beat_at)
}
