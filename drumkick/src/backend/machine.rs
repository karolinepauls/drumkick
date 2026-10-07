use crate::{backend::sound, sequence::BeatNo};
use cpal::{
    BufferSize, CallbackInfo, Device, Error, ErrorKind, FromSample, I24, SampleFormat, SizedSample,
    Stream, StreamConfig, SupportedBufferSize,
    traits::{DeviceTrait, HostTrait},
};
use std::cell::RefCell;
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::Duration;

pub const BUFFER_SIZE: u32 = 2048;

pub const MIN_BPM: f64 = 1.0;
pub const MAX_BPM: f64 = 400.0;

pub fn configure_backend(
    device: Option<&str>,
    buffer_size: u32,
    cmd_receiver: Receiver<Command>,
    resp_sender: SyncSender<Response>,
) -> Result<Stream, Box<dyn std::error::Error>> {
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
    eprintln!("Requested config: {config:?}");

    config.buffer_size = BufferSize::Fixed(
        if let &SupportedBufferSize::Range { min, max } = default_config.buffer_size() {
            buffer_size.clamp(min, max)
        } else {
            buffer_size
        },
    );

    match default_config.sample_format() {
        SampleFormat::I8 => build_output_stream::<i8>(&device, config, cmd_receiver, resp_sender),
        SampleFormat::I16 => build_output_stream::<i16>(&device, config, cmd_receiver, resp_sender),
        SampleFormat::I24 => build_output_stream::<I24>(&device, config, cmd_receiver, resp_sender),
        SampleFormat::I32 => build_output_stream::<i32>(&device, config, cmd_receiver, resp_sender),
        // SampleFormat::I48 => build_output_stream::<I48>(&device, config, cmd_receiver, resp_sender),
        SampleFormat::I64 => build_output_stream::<i64>(&device, config, cmd_receiver, resp_sender),
        SampleFormat::U8 => build_output_stream::<u8>(&device, config, cmd_receiver, resp_sender),
        SampleFormat::U16 => build_output_stream::<u16>(&device, config, cmd_receiver, resp_sender),
        // SampleFormat::U24 => build_output_stream::<U24>(&device, config, cmd_receiver, resp_sender),
        SampleFormat::U32 => build_output_stream::<u32>(&device, config, cmd_receiver, resp_sender),
        // SampleFormat::U48 => build_output_stream::<U48>(&device, config, cmd_receiver, resp_sender),
        SampleFormat::U64 => build_output_stream::<u64>(&device, config, cmd_receiver, resp_sender),
        SampleFormat::F32 => build_output_stream::<f32>(&device, config, cmd_receiver, resp_sender),
        SampleFormat::F64 => build_output_stream::<f64>(&device, config, cmd_receiver, resp_sender),
        sample_format => panic!("Unsupported sample format '{sample_format}'"),
    }
}

fn build_output_stream<T>(
    device: &Device,
    config: StreamConfig,
    cmd_receiver: Receiver<Command>,
    resp_sender: SyncSender<Response>,
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

    let mut machine = Machine::new(config, cmd_receiver, resp_sender);
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
    beat_no: BeatNo,
    source: Arc<sound::LoadedSource>,
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

#[derive(Debug)]
struct PlayingSequence {
    position: f64,
    started: bool,
    items: Vec<SequenceItem>,
}

impl PlayingSequence {
    fn new(sequence: Sequence) -> Self {
        Self {
            position: 0.0,
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

#[derive(Debug)]
pub enum Command {
    Play,
    Pause,
    Stop,
    SetBPM(f64),
    SetSequence(u64, Option<Sequence>),
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
    fn resp_user(&self, message: impl Into<String>);
}

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

    fn resp_user(&self, message: impl Into<String>) {
        self.send_response(ResponseLevel::User, message);
    }
}

#[derive(Debug)]
pub struct Machine {
    config: StreamConfig,
    cmd_receiver: Receiver<Command>,
    resp_sender: SyncSender<Response>,
    bpm: f64,
    time: Duration,
    sequences: HashMap<u64, PlayingSequence>,
    active_sounds: BinaryHeap<PlayingSound>,
    paused: bool,
}

impl Machine {
    const SEQ_CAP: usize = 128;
    const PLAYING_CAP: usize = 512;

    pub fn new(
        config: StreamConfig,
        cmd_receiver: Receiver<Command>,
        resp_sender: SyncSender<Response>,
    ) -> Self {
        Machine {
            config,
            cmd_receiver,
            resp_sender,
            bpm: 60.0,
            time: Duration::ZERO,
            sequences: HashMap::with_capacity(Self::SEQ_CAP),
            active_sounds: BinaryHeap::with_capacity(Self::PLAYING_CAP),
            paused: true,
        }
    }

    fn beats_per_second(&self) -> f64 {
        self.bpm / 60.0
    }

    pub fn produce_output<T>(&mut self, data: &mut [T], info: &CallbackInfo)
    where
        T: SizedSample + FromSample<f32> + std::ops::Add<T, Output = T> + std::fmt::Debug,
    {
        if info.xrun() {
            self.resp_sender
                .resp_debug(format!("output underrun at {:?}", self.time));
        }
        self.handle_commands();

        let beats_per_second = self.beats_per_second();

        let buffer_duration = Duration::from_nanos(
            data.len() as u64 / u64::from(self.config.channels) * 1_000_000_000
                / u64::from(self.config.sample_rate),
        );

        let buffer_start_time = self.time;
        let buffer_end_time = self.time + buffer_duration;

        let mut last_beat: Option<BeatNo> = None;
        if !self.paused {
            for sequence in self.sequences.values_mut() {
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

                    if Some(item.beat_no) != last_beat {
                        self.resp_sender
                            .resp_user(format!("Playing beat {}", item.beat_no));
                    }

                    self.active_sounds.push(PlayingSound::new(
                        item_start,
                        &item.source,
                        self.config.sample_rate,
                    ));

                    last_beat = Some(item.beat_no);
                    item_index += 1;
                    sequence.mark_started();
                }

                sequence.position = buffer_end_position;
                while sequence.position > sequence.wraparound_position() {
                    sequence.position -= sequence.wraparound_position();
                }
            }
        }

        // Produce samples for playing sounds
        for playing in self.active_sounds.iter() {
            let mut sound = playing.sound.borrow_mut();

            let delay = playing
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
        while let Some(playing) = self.active_sounds.peek() {
            if playing.stop < buffer_end_time {
                self.active_sounds.pop();
            } else {
                break; // Reached playing sounds.
            }
        }

        self.time = buffer_end_time;
    }

    fn set_sequence(&mut self, seq_id: u64, sequence: Option<Sequence>) {
        match sequence {
            Some(seq) => {
                let replaced = self
                    .sequences
                    .insert(seq_id, PlayingSequence::new(seq))
                    .is_some();
                self.resp_sender.resp_user(if replaced {
                    "sequence replaced"
                } else {
                    "sequence inserted"
                });
            }
            None => {
                let deleted = self.sequences.remove(&seq_id).is_some();
                if deleted {
                    self.resp_sender.resp_user("sequence deleted")
                } else {
                    // Here goes the idea not to allocate in the audio thread.
                    self.resp_sender.resp_debug(format!(
                        "Sequence {seq_id} not deleted (did not exist). Avaliable sequences: {:?}",
                        self.sequences.keys()
                    ))
                }
            }
        }
    }

    fn handle_commands(&mut self) {
        while let Ok(command) = self.cmd_receiver.try_recv() {
            self.handle_command(command)
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::SetBPM(bpm) => {
                let bpm = bpm.clamp(MIN_BPM, MAX_BPM);
                self.bpm = bpm;

                self.resp_sender.resp_user(format!("BPM set to {bpm}"));
            }
            Command::SetSequence(seq_id, seq) => {
                self.set_sequence(seq_id, seq);
            }
            Command::Play => {
                self.paused = false;
            }
            Command::Pause | Command::Stop => {
                self.paused = true;
            }
        }
    }
}
