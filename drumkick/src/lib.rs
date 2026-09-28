use kira::backend::cpal::CpalBackendSettings;
use kira::backend::cpal::cpal::{
    BufferSize, default_host,
    traits::{DeviceTrait, HostTrait},
};
use kira::{
    AudioManager, AudioManagerSettings, DefaultBackend, Tween,
    clock::{ClockHandle, ClockSpeed, ClockTime},
    sound::static_sound::{StaticSoundData, StaticSoundSettings},
    track::MainTrackBuilder,
};
pub use pest::set_error_detail as pest_set_error_detail;
use std::fmt;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::{thread, time};

pub mod embedded_samples;
pub mod measure_grammar;
pub mod samples;

pub const BUFFER_SIZE: u32 = 2048;
pub const PRECISION: usize = 32;
pub const MIN_BPM: f64 = 1.0;
pub const MAX_BPM: f64 = 400.0;
const MIN_MEASURES: u64 = 2;

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
pub enum Command {
    Play,
    Pause,
    Stop,
    SetBPM(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResponseLevel {
    Debug,
    User,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub level: ResponseLevel,
    pub message: String,
}

#[derive(Debug)]
pub struct Beat {
    pub samples: Vec<StaticSoundData>,
    pub time: f64,
}

#[derive(Debug)]
pub struct Track {
    pub bpm: f64,
    pub measure_beats: Vec<Beat>,
}

impl Track {
    pub fn beats_in_measure(&self) -> u64 {
        self.measure_beats[self.measure_beats.len() - 1]
            .time
            .floor() as u64
    }
}

/// The metronome/drum machine
pub struct DrumkickBackend {
    cmd_receiver: Receiver<Command>,
    resp_sender: SyncSender<Response>,
    manager: AudioManager,
    clock: ClockHandle,
    track: Track,
}

impl DrumkickBackend {
    pub fn new(
        cmd_receiver: Receiver<Command>,
        resp_sender: SyncSender<Response>,
        opts: DrumkickOpts,
        track: Track,
    ) -> Result<Self, Box<dyn std::error::Error>> {
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
        let clock = manager.add_clock(ClockSpeed::TicksPerMinute(track.bpm))?;

        Ok(DrumkickBackend {
            cmd_receiver,
            resp_sender,
            manager,
            clock,
            track,
        })
    }

    // Consume self and start playback in a different thread, while receiving control messages.
    //
    // Initially playback will be paused.
    //
    // This is an awkward loop forced by Kira's limitations (no callbacks when a sound is played
    // to let the app schedule new sounds). Future versions will likely use CPAL, maybe gutting
    // Kira for its resampling code.
    pub fn run(mut self) {
        let mut next_beat_at = self.clock.time();
        self.resp_user("Metronome started");
        let mut clock_running = false;

        loop {
            let measures_ahead = safe_measures_ahead(self.track.bpm);

            next_beat_at = schedule_beats(
                &mut self.manager,
                next_beat_at,
                &self.track.measure_beats,
                self.track.beats_in_measure(),
                measures_ahead,
            )
            .unwrap();
            self.resp_debug(format!("Scheduled {measures_ahead} measures"));

            // Sleep until the next/last measure is half-finished, basing on clock.time() instead of
            // system time. The clock actually gets updated every time the audio thread gets a callback
            // (probably every 2048 audio wave samples), which makes it not very precise but the
            // important feature is that the clock won't desync with audio device time.
            loop {
                let single_beat_duration = beat_duration(self.track.bpm);

                let clock_time = self.clock.time();
                let sleep_target = next_beat_at - self.track.beats_in_measure() / 2;

                #[rustfmt::skip]
                    let sleep_time_in_beats = (
                        (sleep_target.ticks as f64 - clock_time.ticks as f64) +
                        (sleep_target.fraction - clock_time.fraction)
                    ).max(0.0);

                let wait_duration = single_beat_duration * sleep_time_in_beats;

                self.resp_debug(format!("receiving for {wait_duration}"));
                let command = self
                    .cmd_receiver
                    .recv_timeout(time::Duration::from_secs_f64(wait_duration))
                    .ok();
                String::new().to_string();
                match command {
                    None => {
                        if clock_running {
                            break;
                        } else {
                            continue;
                        }
                    }
                    Some(Command::Play) => {
                        self.resp_user("Playing");
                        clock_running = true;
                        self.clock.start();
                    }
                    Some(Command::Pause) => {
                        self.resp_user("Paused");
                        clock_running = false;
                        self.clock.pause();
                    }
                    Some(Command::Stop) => {
                        self.resp_user("Stopped");
                        self.clock.stop();
                        return;
                    }
                    Some(Command::SetBPM(bpm)) => {
                        let bpm = bpm.clamp(MIN_BPM, MAX_BPM);

                        self.resp_user(format!("BPM set to {bpm}"));
                        self.track.bpm = bpm;
                        self.clock.set_speed(
                            ClockSpeed::TicksPerMinute(self.track.bpm),
                            Tween {
                                duration: time::Duration::from_millis(50),
                                ..Default::default()
                            },
                        );
                    }
                }
            }
        }
    }

    fn send_response(&mut self, level: ResponseLevel, message: impl Into<String>) {
        if let Err(err) = self.resp_sender.send(Response {
            level,
            message: message.into(),
        }) {
            eprintln!("Error sending response: {err}");
        }
    }

    fn resp_debug(&mut self, message: impl Into<String>) {
        self.send_response(ResponseLevel::Debug, message);
    }
    fn resp_user(&mut self, message: impl Into<String>) {
        self.send_response(ResponseLevel::User, message);
    }
}

impl fmt::Debug for DrumkickBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DrumkickBackend").finish_non_exhaustive()
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

pub fn start_drumkick(
    opts: DrumkickOpts,
    track: Track,
) -> Result<(SyncSender<Command>, Receiver<Response>), Box<dyn std::error::Error>> {
    let (cmd_sender, cmd_receiver) = sync_channel(opts.message_buffer_size);
    let (resp_sender, resp_receiver) = sync_channel(opts.message_buffer_size);

    let backend = DrumkickBackend::new(cmd_receiver, resp_sender, opts, track)?;

    thread::spawn(move || backend.run());

    Ok((cmd_sender, resp_receiver))
}
