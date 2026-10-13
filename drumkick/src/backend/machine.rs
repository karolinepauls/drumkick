use crate::{backend::sound, sequence::BeatNo};
use cpal::{
    BufferSize, CallbackInfo, Device, Error, ErrorKind, FromSample, I24, SampleFormat, SizedSample,
    Stream, StreamConfig, SupportedBufferSize,
    traits::{DeviceTrait, HostTrait},
};
use indexmap::IndexMap;
use std::cell::RefCell;
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::mem;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::time::Duration;
use triple_buffer;

pub const BUFFER_SIZE: u32 = 2048;

pub const MIN_BPM: f64 = 1.0;
pub const MAX_BPM: f64 = 400.0;

pub fn configure_backend(
    device: Option<&str>,
    buffer_size: u32,
) -> Result<(Device, StreamConfig, SampleFormat), Box<dyn std::error::Error>> {
    let host = cpal::default_host();

    let device = if let Some(device) = device {
        let id = &device.parse().expect("failed to parse device id");
        host.device_by_id(id)
    } else {
        host.default_output_device()
    }
    .expect("failed to find output device");
    eprintln!("Output device: {}", device.id()?);

    let default_config = device.default_output_config().unwrap();

    let mut config: StreamConfig = default_config.into();

    config.buffer_size = BufferSize::Fixed(
        if let &SupportedBufferSize::Range { min, max } = default_config.buffer_size() {
            buffer_size.clamp(min, max)
        } else {
            buffer_size
        },
    );

    eprintln!("Established config: {config:?}");
    Ok((device, config, default_config.sample_format()))
}

pub fn build_output_stream(
    device: &Device,
    sample_format: SampleFormat,
    config: StreamConfig,
    machine: Machine,
) -> Result<Stream, Box<dyn std::error::Error>> {
    match sample_format {
        SampleFormat::I8 => build_specific_output_stream::<i8>(device, config, machine),
        SampleFormat::I16 => build_specific_output_stream::<i16>(device, config, machine),
        SampleFormat::I24 => build_specific_output_stream::<I24>(device, config, machine),
        SampleFormat::I32 => build_specific_output_stream::<i32>(device, config, machine),
        // SampleFormat::I48 => build_specific_output_stream::<I48>(&device, config, machine),
        SampleFormat::I64 => build_specific_output_stream::<i64>(device, config, machine),
        SampleFormat::U8 => build_specific_output_stream::<u8>(device, config, machine),
        SampleFormat::U16 => build_specific_output_stream::<u16>(device, config, machine),
        // SampleFormat::U24 => build_specific_output_stream::<U24>(&device, config, machine),
        SampleFormat::U32 => build_specific_output_stream::<u32>(device, config, machine),
        // SampleFormat::U48 => build_specific_output_stream::<U48>(&device, config, machine),
        SampleFormat::U64 => build_specific_output_stream::<u64>(device, config, machine),
        SampleFormat::F32 => build_specific_output_stream::<f32>(device, config, machine),
        SampleFormat::F64 => build_specific_output_stream::<f64>(device, config, machine),
        sample_format => panic!("Unsupported sample format '{sample_format}'"),
    }
}

fn build_specific_output_stream<T>(
    device: &Device,
    config: StreamConfig,
    mut machine: Machine,
) -> Result<Stream, Box<dyn std::error::Error>>
where
    T: SizedSample + FromSample<f32> + std::ops::Add<T, Output = T> + std::fmt::Debug,
{
    let err_fn = |err: Error| match err.kind() {
        ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied => {
            eprintln!("{err}")
        }
        _ => eprintln!("Stream error: {err}"),
    };

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], info: &CallbackInfo| {
            machine.produce_output(data, info);
        },
        err_fn,
        None,
    )?;

    Ok(stream)
}

#[derive(Debug)]
struct PlayingSound {
    start: Duration,
    stop: Duration,
    // Need RefCell for borrow_mut since BinaryHeap doesn't have iter_mut.
    sound: RefCell<sound::LoadedSourceResampler>,
}

impl PlayingSound {
    fn new(start: Duration, source: &Arc<sound::LoadedSource>, output_sample_rate: u32) -> Self {
        Self {
            start,
            stop: start + Duration::from_secs_f64(source.duration()),
            sound: RefCell::new(sound::LoadedSourceResampler::new(
                source.clone(),
                output_sample_rate,
            )),
        }
    }
}

/// semantically incorrect impl, forced by BinaryHeap
impl PartialEq for PlayingSound {
    fn eq(&self, other: &Self) -> bool {
        self.stop.eq(&other.stop)
    }
}
impl Eq for PlayingSound {}

/// semantically incorrect impl, forced by BinaryHeap
impl PartialOrd for PlayingSound {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
/// also semantically incorrect impl, forced by BinaryHeap
impl Ord for PlayingSound {
    fn cmp(&self, other: &Self) -> Ordering {
        Reverse(self.stop).cmp(&Reverse(other.stop))
    }
}

#[derive(Debug, Clone)]
pub struct SequenceItem {
    pub beat_no: BeatNo,
    pub(crate) source: Arc<sound::LoadedSource>,
}
impl SequenceItem {
    pub fn new(beat_no: BeatNo, source: Arc<sound::LoadedSource>) -> SequenceItem {
        SequenceItem { beat_no, source }
    }
}

#[derive(Debug, Clone)]
pub struct Sequence {
    pub items: Vec<SequenceItem>,
}
impl Sequence {
    pub fn new(items: impl IntoIterator<Item = SequenceItem>) -> Self {
        Self {
            items: items.into_iter().collect(),
        }
    }

    pub fn beat_count(&self) -> u32 {
        self.items
            .last()
            .map(|item| u32::from(item.beat_no.beat()))
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone)]
pub struct PlayingSequence {
    position: f64,
    last_beat: BeatNo,
    started: bool,
    items: Vec<SequenceItem>,
}

impl PlayingSequence {
    pub fn new(sequence: Sequence) -> Self {
        Self {
            position: 0.0,
            last_beat: BeatNo::ONE,
            started: false,
            items: sequence.items,
        }
    }

    fn wraparound_position(&self) -> f64 {
        self.items
            .last()
            .map(|i| i.beat_no.as_position().floor())
            .unwrap_or(0.0)
            + 1.0
    }

    fn get_with_wraparound(&self, index: usize) -> (&SequenceItem, bool) {
        let index = index % self.items.len();

        let this_item = &self.items[index];
        let is_new_measure = self.started && index == 0;

        (this_item, is_new_measure)
    }

    fn mark_started(&mut self) {
        self.started = true;
    }
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

trait SendResp {
    fn send_response(&self, level: ResponseLevel, message: impl Into<String>);
    fn resp_debug(&self, message: impl Into<String>);
}

// TODO: not sync, dropping
impl SendResp for SyncSender<Response> {
    fn send_response(&self, level: ResponseLevel, message: impl Into<String>) {
        if let Err(err) = self.send(Response {
            level,
            message: message.into(),
        }) {
            eprintln!("Error sending response: {err}");
        }
    }

    fn resp_debug(&self, message: impl Into<String>) {
        self.send_response(ResponseLevel::Debug, message);
    }
}

pub type PlayingSequenceMap = IndexMap<u64, PlayingSequence>;
pub type PlaybackProgress = IndexMap<u64, BeatNo>;

/// Playback state, including sequence progress and applied params.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaybackState {
    pub progress: PlaybackProgress,
    pub bpm: f64,
    pub playing: bool,
}

pub type PlaybackStatePublisher = triple_buffer::Input<PlaybackState>;
pub type PlaybackStateReceiver = triple_buffer::Output<PlaybackState>;

/// State the machine is requested to achieve.
#[derive(Debug)]
pub struct TargetState {
    /// PlaybackStatePublisher is a part of the target state since it needs to be preallocated to
    /// enough capacity to accommodate all sequences without reallocating in the audio thread.
    /// It cannot be cloned.
    pub playback_publisher: PlaybackStatePublisher,
    /// All cloneable data (incl. sequences) lives here. It's expected that code controlling the
    /// machine will keep a source-of-truth copy for itself while pushing cloned TargetStateParams
    /// to the machine.
    pub params: TargetStateParams,
}
#[derive(Debug, Clone)]
pub struct TargetStateParams {
    pub bpm: f64,
    pub sequences: PlayingSequenceMap,
    pub playing: bool,
}

pub type TargetStatePublisher = triple_buffer::Input<Option<TargetState>>;
pub type TargetStateReceiver = triple_buffer::Output<Option<TargetState>>;

impl PlaybackState {
    pub fn with_capacity(sequence_cap: usize) -> Self {
        Self {
            progress: IndexMap::with_capacity(sequence_cap),
            bpm: 60.0,
            playing: false,
        }
    }
}

#[derive(Debug)]
pub struct Machine {
    state: TargetState,
    target_state_receiver: TargetStateReceiver,

    config: StreamConfig,
    // TODO: use a non-blocking, bounded channel, possibly one that drops oldest messages on
    // overlow; in a way that doesn't require allocations when pushing messages.
    resp_sender: SyncSender<Response>,
    time: Duration,
    active_sounds: BinaryHeap<PlayingSound>,
}

impl Machine {
    const PLAYING_CAP: usize = 512;

    pub fn new(
        state: TargetState,
        target_state_receiver: TargetStateReceiver,
        config: StreamConfig,
        resp_sender: SyncSender<Response>,
    ) -> Self {
        Machine {
            state,
            target_state_receiver,
            config,
            resp_sender,
            time: Duration::ZERO,
            active_sounds: BinaryHeap::with_capacity(Self::PLAYING_CAP),
        }
    }

    fn beats_per_second(&self) -> f64 {
        self.state.params.bpm / 60.0
    }

    pub fn produce_output<T>(&mut self, data: &mut [T], info: &CallbackInfo)
    where
        T: SizedSample + FromSample<f32> + std::ops::Add<T, Output = T> + std::fmt::Debug,
    {
        if info.xrun() {
            self.resp_sender
                .resp_debug(format!("output underrun at {:?}", self.time));
        }
        self.update_state();

        let beats_per_second = self.beats_per_second();

        let buffer_duration = Duration::from_nanos(
            data.len() as u64 / u64::from(self.config.channels) * 1_000_000_000
                / u64::from(self.config.sample_rate),
        );

        let buffer_start_time = self.time;
        let buffer_end_time = self.time + buffer_duration;

        // Begin writing monitor state (sent on drop).
        let mut monitor_buffer = self.state.playback_publisher.input_buffer_publisher();
        monitor_buffer.bpm = self.state.params.bpm;
        monitor_buffer.playing = self.state.params.playing;

        if self.state.params.playing {
            for (seq_id, sequence) in self.state.params.sequences.iter_mut() {
                let buffer_start_position = sequence.position;
                let buffer_end_position =
                    buffer_start_position + buffer_duration.as_secs_f64() * beats_per_second;

                // Bisect to find the first element to be scheduled.
                let mut item_index = sequence
                    .items
                    .partition_point(|item| item.beat_no.as_position() < buffer_start_position);

                let mut wraparounds: f64 = 0.0;
                loop {
                    let (item, wrapped_around) = sequence.get_with_wraparound(item_index);
                    wraparounds += f64::from(wrapped_around);

                    let item_position =
                        item.beat_no.as_position() + sequence.wraparound_position() * wraparounds;

                    if item_position > buffer_end_position {
                        break;
                    }

                    let item_position_in_buffer = (item_position - buffer_start_position)
                        / (buffer_end_position - buffer_start_position);
                    let item_start =
                        buffer_start_time + buffer_duration.mul_f64(item_position_in_buffer);

                    self.active_sounds.push(PlayingSound::new(
                        item_start,
                        &item.source,
                        self.config.sample_rate,
                    ));

                    item_index += 1;
                    sequence.last_beat = item.beat_no;
                    sequence.mark_started();
                }

                sequence.position = buffer_end_position;
                while sequence.position > sequence.wraparound_position() {
                    sequence.position -= sequence.wraparound_position();
                }
                monitor_buffer.progress.insert(*seq_id, sequence.last_beat);
            }
        }

        // Produce samples for playing sounds
        for playing_sound in self.active_sounds.iter() {
            let mut sound = playing_sound.sound.borrow_mut();

            let delay = playing_sound
                .start
                .saturating_sub(buffer_start_time)
                .as_secs_f64();
            let first_frame_index = (delay * self.config.sample_rate as f64) as usize;

            let frames = data
                .chunks_mut(self.config.channels.into())
                .skip(first_frame_index);

            for frame in frames {
                sound.write_frame(frame);
            }
        }

        // Remove finished sounds
        while let Some(playing_sound) = self.active_sounds.peek() {
            if playing_sound.stop < buffer_end_time {
                self.active_sounds.pop();
            } else {
                break; // Reached playing sounds.
            }
        }

        self.time = buffer_end_time;
    }

    fn update_state(&mut self) {
        self.target_state_receiver.update();
        // mem::take will leave None in the buffer, meaning that output_buffer_mut() will remain
        // None until the next update.
        // .
        if let Some(mut new_state) = mem::take(self.target_state_receiver.output_buffer_mut()) {
            new_state.params.bpm = new_state.params.bpm.clamp(MIN_BPM, MAX_BPM);

            // Update new state with progress from current sequences
            for (id, new_seq) in new_state.params.sequences.iter_mut() {
                if let Some(seq) = self.state.params.sequences.get(id) {
                    new_seq.position = seq.position;
                    new_seq.started = seq.started;
                }
            }

            self.state = new_state;
            self.resp_sender.resp_debug(format!(
                "State updated {} {}",
                self.state.params.bpm, self.state.params.playing
            ));
        }
    }
}
