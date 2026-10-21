use cpal::{Stream, traits::StreamTrait};
use indexmap::IndexMap;
use std::cell::RefCell;
use std::fmt;
use std::sync::mpsc::{Receiver, TryIter, sync_channel};

use crate::backend::machine::{BUFFER_SIZE, MAX_BPM, MIN_BPM, PlaybackProgress, Response};
use crate::sequence::Sequence;
use std::sync::Arc;
use std::time::Duration;

use crate::backend::{machine, sound};
use crate::measure_grammar::BeatSpec;

pub struct DrumkickOpts {
    pub buffer_size: u32,
    pub sound_capacity: usize,
    pub message_buffer_size: usize,
}

impl Default for DrumkickOpts {
    fn default() -> Self {
        DrumkickOpts {
            buffer_size: BUFFER_SIZE,
            sound_capacity: 512,
            message_buffer_size: 128,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InitialState {
    pub bpm: f64,
    pub samples: IndexMap<String, Arc<sound::LoadedSource>>,
    pub measure_beats: Vec<BeatSpec>,
    pub sequence: Sequence,
}

/// The metronome/drum machine handle, controlling the real machine living in the audio thread
pub struct MachineHandle {
    resp_receiver: Receiver<Response>,
    target_state_params: machine::TargetStateParams,
    target_state_publisher: machine::TargetStatePublisher,
    // RefCell to allow pulling most recent state from non-mut methods.
    playback_state_receiver: RefCell<machine::PlaybackStateReceiver>,
    last_playback_state: RefCell<machine::PlaybackState>,
    stream: Stream,
}

impl MachineHandle {
    pub fn new(
        opts: DrumkickOpts,
        track: InitialState,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (resp_sender, resp_receiver) = sync_channel(opts.message_buffer_size);
        let (device, stream_config, sample_format) =
            machine::configure_backend(None, opts.buffer_size)?;

        let initial_sequences =
            IndexMap::from([(0u64, machine::MachineSequence::new(track.sequence.clone()))]);

        let (playback_state_publisher, playback_state_receiver) =
            Self::build_playback_state_buffer(initial_sequences.len());

        let target_state_params = machine::TargetStateParams {
            bpm: track.bpm,
            sequences: initial_sequences,
            playing: false,
        };
        let initial_state = machine::TargetState {
            playback_publisher: playback_state_publisher,
            params: target_state_params.clone(),
        };
        let (target_state_publisher, target_state_receiver) =
            triple_buffer::TripleBuffer::default().split();

        let machine = machine::Machine::new(
            initial_state,
            target_state_receiver,
            stream_config,
            resp_sender,
        );
        let stream = machine::build_output_stream(&device, sample_format, stream_config, machine)?;

        Ok(MachineHandle {
            stream,
            target_state_params,
            target_state_publisher,
            playback_state_receiver: RefCell::new(playback_state_receiver),
            last_playback_state: RefCell::new(machine::PlaybackState::default()),
            resp_receiver,
        })
    }

    pub fn playing(&self) -> bool {
        self.target_state_params.playing
    }

    pub fn bpm(&self) -> f64 {
        self.target_state_params.bpm
    }

    fn build_playback_state_buffer(
        sequences_cap: usize,
    ) -> (
        machine::PlaybackStatePublisher,
        machine::PlaybackStateReceiver,
    ) {
        triple_buffer::triple_buffer(&machine::PlaybackState::with_capacity(sequences_cap))
    }

    fn publish_target_state(&mut self) {
        let (playback_state_publisher, playback_state_receiver) =
            Self::build_playback_state_buffer(self.target_state_params.sequences.len());

        self.playback_state_receiver
            .replace(playback_state_receiver);

        self.target_state_publisher
            .write(Some(machine::TargetState {
                playback_publisher: playback_state_publisher,
                params: self.target_state_params.clone(),
            }));
    }

    pub fn set_sequences<'a>(&mut self, sequences: impl Iterator<Item = (u64, &'a Sequence)>) {
        let playing_sequences: machine::PlayingSequenceMap = sequences
            .map(|(k, seq)| (k, machine::MachineSequence::new(seq.clone())))
            .collect();

        self.target_state_params.sequences = playing_sequences;
        self.publish_target_state()
    }

    pub fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.target_state_params.playing = false;
        self.publish_target_state();
        self.stream.stop(Some(Duration::from_millis(50)))?;
        Ok(())
    }

    pub fn toggle_playing(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.target_state_params.playing {
            self.pause();
        } else {
            self.play()?;
        }
        Ok(())
    }

    pub fn play(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.target_state_params.playing = true;
        self.stream.start()?;
        self.publish_target_state();
        Ok(())
    }

    pub fn pause(&mut self) {
        self.target_state_params.playing = false;
        self.publish_target_state();
    }

    pub fn set_bpm(&mut self, bpm: f64) {
        let bpm = bpm.clamp(MIN_BPM, MAX_BPM);
        self.target_state_params.bpm = bpm;
        self.publish_target_state();
    }

    pub fn iter_resposes(&mut self) -> TryIter<'_, Response> {
        self.resp_receiver.try_iter()
    }

    pub fn playback_state(&self) -> machine::PlaybackState {
        self.playback_state_receiver.borrow_mut().read().clone()
    }

    pub fn playback_progress(&self) -> PlaybackProgress {
        self.playback_state().progress
    }

    // Check if playback state value changed from the time `playback_state_updated` was last called.
    pub fn playback_state_updated(&self) -> bool {
        let this_playback_state = self.playback_state();

        let updated = {
            let last_playback_state = self.last_playback_state.borrow();
            *last_playback_state != this_playback_state
        };

        self.last_playback_state
            .replace(this_playback_state.clone());

        updated
    }
}

impl fmt::Debug for MachineHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MachineHandle")
            .field("target_state_params", &self.target_state_params)
            .field("playback_state", &self.playback_state())
            .finish_non_exhaustive()
    }
}
