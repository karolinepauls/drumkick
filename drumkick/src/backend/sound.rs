use cpal::{FromSample, SizedSample};
use std::default::Default;
use std::fmt;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
pub use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceOrigin {
    File(PathBuf),
    Builtin(String),
}

pub struct LoadedSource {
    pub(crate) samples: Vec<f32>,
    pub channels: usize,
    pub sample_rate: u32,
    pub origin: Option<SourceOrigin>,
}
impl LoadedSource {
    pub fn duration(&self) -> f64 {
        self.samples.len() as f64 / self.sample_rate as f64
    }

    pub fn with_origin(self, origin: Option<SourceOrigin>) -> Self {
        Self { origin, ..self }
    }
}

impl fmt::Debug for LoadedSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadedSource")
            .field("samples", &&self.samples[0..16])
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .finish()
    }
}

pub struct StreamingSource<'a> {
    pub sample_rate_hint: Option<u32>,
    pub channels_hint: Option<usize>,
    pub num_frames_hint: Option<u64>,
    pub track_id: u32,
    pub origin: Option<SourceOrigin>,
    format: Box<dyn FormatReader + 'a>,
    decoder: Box<dyn AudioDecoder>,
}

impl<'a> fmt::Debug for StreamingSource<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamingSource")
            .field("sample_rate_hint", &self.sample_rate_hint)
            .field("channels_hint", &self.channels_hint)
            .field("num_frames_hint", &self.num_frames_hint)
            .field("track_id", &self.track_id)
            .finish_non_exhaustive()
    }
}

impl<'a> StreamingSource<'a> {
    pub fn from_path<P: AsRef<std::path::Path>>(path: P) -> Result<Self, SymphoniaError> {
        let path = path.as_ref();
        let file = std::fs::File::open(path).map_err(SymphoniaError::IoError)?;

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            hint.with_extension(ext);
        }

        Self::from_file(file).map(|s| s.with_origin(Some(SourceOrigin::File(path.into()))))
    }

    pub fn from_file(file: std::fs::File) -> Result<Self, SymphoniaError> {
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        Self::from_media_source(stream)
    }

    pub fn from_media_source(stream: MediaSourceStream<'a>) -> Result<Self, SymphoniaError> {
        Self::from_media_source_with_hint(stream, &Hint::default())
    }

    pub fn from_media_source_with_hint(
        stream: MediaSourceStream<'a>,
        hint: &Hint,
    ) -> Result<Self, SymphoniaError> {
        let format = symphonia::default::get_probe().probe(
            hint,
            stream,
            Default::default(),
            Default::default(),
        )?;

        let track = format
            .default_track(TrackType::Audio)
            .ok_or(SymphoniaError::Unsupported("no audio track"))?;

        let track_id = track.id;

        let audio_codec_params = track
            .codec_params
            .as_ref()
            .ok_or(SymphoniaError::Unsupported("codec parameters determined"))?
            .audio()
            .ok_or(SymphoniaError::Unsupported("audio codec present"))?;

        let decoder = symphonia::default::get_codecs().make_audio_decoder(
            audio_codec_params,
            &AudioDecoderOptions::default(), // the default is gapless
        )?;

        let channels = audio_codec_params.channels.as_ref().map(|x| x.count());
        Ok(Self {
            sample_rate_hint: audio_codec_params.sample_rate,
            channels_hint: channels,
            num_frames_hint: track.num_frames,
            format,
            decoder,
            track_id,
            origin: None,
        })
    }

    pub fn with_origin(self, origin: Option<SourceOrigin>) -> Self {
        Self { origin, ..self }
    }

    /// Read the entire track into memory, consuming the source
    pub fn into_loaded(mut self) -> Result<LoadedSource, SymphoniaError> {
        self.load_without_rewinding()
            .map(|l| l.with_origin(self.origin))
    }

    fn load_without_rewinding(&mut self) -> Result<LoadedSource, SymphoniaError> {
        let mut sample_rate = self.sample_rate_hint.unwrap_or(44_000);
        let mut channels = self.channels_hint.unwrap_or(2);
        // assuming one second of data if num_frames_hint is None
        let size_hint = channels * self.num_frames_hint.unwrap_or(sample_rate as u64) as usize;

        let mut samples = Vec::with_capacity(size_hint);

        loop {
            match self.decode_chunk()? {
                Some(audio_buf) => {
                    let spec = audio_buf.spec();
                    sample_rate = spec.rate();
                    channels = spec.channels().count();

                    let len = samples.len();
                    samples.resize(len + audio_buf.samples_interleaved(), 0.0);
                    audio_buf.copy_to_slice_interleaved(&mut samples[len..]);
                }
                None => {
                    return Ok(LoadedSource {
                        samples,
                        sample_rate,
                        channels,
                        origin: None,
                    });
                }
            }
        }
    }

    fn decode_chunk(&mut self) -> Result<Option<GenericAudioBufferRef<'_>>, SymphoniaError> {
        let packet = loop {
            let p = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    return Ok(None); // end of the stream.
                }
                Err(SymphoniaError::ResetRequired) => {
                    // "The track list has been changed" - basically won't happen.
                    return Ok(None);
                }
                Err(err) => {
                    panic!("{}", err);
                }
            };

            // If the packet does not belong to the selected track, skip over it.
            if p.track_id == self.track_id {
                break p;
            }
        };

        // Decode the packet into audio samples.
        self.decoder.decode(&packet).map(Some)
    }
}

#[derive(Debug)]
pub struct LoadedSourceResampler {
    source: Arc<LoadedSource>,
    source_pos: usize,
    source_fractional_pos: f64,
    target_sample_rate: u32,
}
impl LoadedSourceResampler {
    pub fn new(source: Arc<LoadedSource>, target_sample_rate: u32) -> Self {
        Self {
            source,
            source_pos: 0,
            source_fractional_pos: 0.0,
            target_sample_rate,
        }
    }

    /// Write one resampled sample to each of the channels in `dst_frame`.
    ///
    /// `dst_frame` is assumed to be all zeros. The resulting interpolated value will be ADDED into
    /// it rather than simply assigned.
    ///
    /// `dst_frame` should generally be large enough to fit all source channels. If not, then
    /// destination channel number will wrap around and begin writing to the 0th channel again,
    /// which is good for mixing down to mono and nothing else.
    ///
    /// If the source has less channels than the destination, similar logic takes place.
    pub fn write_frame<T>(&mut self, dst_frame: &mut [T])
    where
        T: SizedSample + FromSample<f32> + std::ops::Add<T, Output = T>,
    {
        for ch_num in 0..self.source.channels {
            let mut channel_samples = self
                .source
                .samples
                .iter()
                .skip(self.source_pos * self.source.channels)
                .copied()
                .skip(ch_num)
                .step_by(self.source.channels);

            let interpolated = interpolate_frame(
                channel_samples.next().unwrap_or(0.0),
                channel_samples.next().unwrap_or(0.0),
                channel_samples.next().unwrap_or(0.0),
                channel_samples.next().unwrap_or(0.0),
                self.source_fractional_pos as f32,
            );

            let index = ch_num % dst_frame.len();
            let val = dst_frame[index] + T::from_sample(interpolated);
            dst_frame[index] = val;
        }

        // If the destination has more channels than the source, fill those channels with filled
        // source channels.
        for ch_num in self.source.channels..dst_frame.len() {
            dst_frame[ch_num] = dst_frame[ch_num % self.source.channels]
        }

        self.source_fractional_pos +=
            self.source.sample_rate as f64 / self.target_sample_rate as f64;

        self.source_pos += self.source_fractional_pos.floor() as usize;
        self.source_fractional_pos = self.source_fractional_pos.fract();
    }
}

// Stolen from https://github.com/tesselode/kira, licenced under MIT
// Algorithm: https://yehar.com/blog/wp-content/uploads/2009/08/deip.pdf
fn interpolate_frame(previous: f32, current: f32, next_1: f32, next_2: f32, fraction: f32) -> f32 {
    let c0 = current;
    let c1 = (next_1 - previous) * 0.5;
    let c2 = previous - current * 2.5 + next_1 * 2.0 - next_2 * 0.5;
    let c3 = (next_2 - previous) * 0.5 + (current - next_1) * 1.5;
    ((c3 * fraction + c2) * fraction + c1) * fraction + c0
}

pub fn load_from_path(
    path: impl AsRef<std::path::Path>,
) -> Result<Arc<LoadedSource>, SymphoniaError> {
    StreamingSource::from_path(path)?
        .into_loaded()
        .map(Arc::new)
}

pub fn load_from_cursor(
    cursor: Cursor<&'static [u8]>,
    origin: &str,
) -> Result<Arc<LoadedSource>, SymphoniaError> {
    let media_source = MediaSourceStream::new(Box::new(cursor), Default::default());
    StreamingSource::from_media_source(media_source)?
        .with_origin(Some(SourceOrigin::Builtin(origin.to_string())))
        .into_loaded()
        .map(Arc::new)
}
