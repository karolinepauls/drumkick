use drumkick::{MAX_BPM, MIN_BPM};

pub fn parse_bpm(text: &str) -> Result<f64, String> {
    text.parse::<f64>()
        .map(|bpm| bpm.clamp(MIN_BPM, MAX_BPM))
        .map_err(|_| {
            format!(
                "BPM must be a decimal number (e.g. \"60\" or \"115.5\") - got {:?}",
                text
            )
        })
}
