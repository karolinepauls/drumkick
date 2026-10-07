use cpal::{Stream, traits::StreamTrait};
pub use pest::set_error_detail as pest_set_error_detail;
use std::fmt;
use std::sync::mpsc::{Receiver, SendError, SyncSender, TryIter, sync_channel};

use std::time::Duration;
mod backend;
pub mod embedded_samples;
pub mod measure_grammar;
pub mod samples;
pub mod sequence;
pub use backend::machine::{
    BUFFER_SIZE, Command, MAX_BPM, MIN_BPM, Response, ResponseLevel, Sequence,
};
use backend::{machine, sound};

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

#[derive(Debug)]
pub struct Beat {
    pub samples: Vec<sound::LoadedSource>,
    pub time: f64,
}

#[derive(Debug, Clone)]
pub struct Track {
    pub bpm: f64,
    pub sequence: Sequence,
}

/// The metronome/drum machine handle, controlling the real machine living in the audio thread
pub struct MachineHandle {
    resp_receiver: Receiver<Response>,
    cmd_sender: SyncSender<Command>,
    stream: Stream,
    playing: bool,
    track: Track,
}

impl MachineHandle {
    pub fn new(opts: DrumkickOpts, track: Track) -> Result<Self, Box<dyn std::error::Error>> {
        let (cmd_sender, cmd_receiver) = sync_channel(opts.message_buffer_size);
        let (resp_sender, resp_receiver) = sync_channel(opts.message_buffer_size);

        let stream = machine::configure_backend(None, opts.buffer_size, cmd_receiver, resp_sender)?;
        let mut backend = MachineHandle {
            stream,
            playing: false,
            track: track.clone(),
            resp_receiver,
            cmd_sender,
        };
        backend.set_bpm(track.bpm)?;
        backend.set_sequence(0, Some(track.sequence))?;

        Ok(backend)
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    pub fn bpm(&self) -> f64 {
        self.track.bpm
    }

    pub fn set_sequence(
        &mut self,
        seq_id: u64,
        seq: Option<Sequence>,
    ) -> Result<(), SendError<Command>> {
        self.cmd_sender.send(Command::SetSequence(seq_id, seq))
    }

    pub fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.playing = false;
        self.cmd_sender.send(Command::Pause)?;
        self.stream.stop(Some(Duration::from_millis(50)))?;
        Ok(())
    }

    pub fn toggle_playing(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.playing {
            self.pause()?;
        } else {
            self.play()?;
        }
        Ok(())
    }

    pub fn play(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.playing = true;
        self.stream.start()?;
        self.cmd_sender.send(Command::Play)?;
        Ok(())
    }

    pub fn pause(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.playing = false;
        self.cmd_sender.send(Command::Pause)?;
        Ok(())
    }

    pub fn set_bpm(&mut self, bpm: f64) -> Result<(), SendError<Command>> {
        let bpm = bpm.clamp(MIN_BPM, MAX_BPM);
        self.track.bpm = bpm;
        self.cmd_sender.send(Command::SetBPM(bpm))
    }

    pub fn iter_resposes(&mut self) -> TryIter<'_, Response> {
        self.resp_receiver.try_iter()
    }
}

impl fmt::Debug for MachineHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DrumkickBackend").finish_non_exhaustive()
    }
}
