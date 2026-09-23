/// Basic samples hardcoded in the binary.
use std::io::Cursor;

macro_rules! include_sample {
    ($name:expr) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/samples/", $name))
    };
}

const EMBEDDED_SAMPLES: &[(&str, &[u8])] = &[
    ("CLICK", include_sample!("click_strong.mp3")),
    ("click", include_sample!("click_weak.mp3")),
    ("DRIP", include_sample!("drip_strong.mp3")),
    ("drip", include_sample!("drip_weak.mp3")),
];

pub const EMBEDDED_SAMPLE_NAMES: &[&str] = &{
    let mut embedded_sample_names = [""; EMBEDDED_SAMPLES.len()];
    let mut i = 0;
    while i < EMBEDDED_SAMPLES.len() {
        embedded_sample_names[i] = EMBEDDED_SAMPLES[i].0;
        i += 1;
    }
    embedded_sample_names
};

pub fn get_embedded_sample_names() -> Vec<&'static str> {
    EMBEDDED_SAMPLES
        .iter()
        .map(|(name, _)| name)
        .cloned()
        .collect()
}

pub fn get_embedded_sample(name: &str) -> Option<Cursor<&'static [u8]>> {
    for (sample_name, bytes) in EMBEDDED_SAMPLES {
        if *sample_name == name {
            return Some(Cursor::new(bytes));
        }
    }
    None
}
