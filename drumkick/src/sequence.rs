use crate::{backend::sound, beat::BeatNo};
use std::sync::Arc;

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
