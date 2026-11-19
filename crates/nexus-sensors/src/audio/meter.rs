//! The microphone's level: the loudest sample of every 64 ms, in dBFS,
//! with -60 dB and below as 0 and full scale as 100.

use std::io::{self, Read};

/// Samples per second the meter records; one channel, 16 bit.
pub const METER_RATE: u32 = 16_000;
/// Samples per reading: 64 ms.
pub const METER_WINDOW: usize = 1_024;
/// The quietest level the meter shows above zero.
const FLOOR_DB: f32 = -60.0;

/// Loudest sample of a window, 0..=1 of full scale.
pub fn peak_of(samples: &[i16]) -> f32 {
    let loudest = samples
        .iter()
        .map(|sample| sample.unsigned_abs())
        .max()
        .unwrap_or(0);
    (f32::from(loudest) / 32_768.0).min(1.0)
}

/// Maps a peak (0..=1) to the meter's 0..=100: -60..0 dBFS linearly.
pub fn level_from_peak(peak: f32) -> u8 {
    if peak <= 0.0 {
        return 0;
    }
    let db = 20.0 * peak.log10();
    let level = (db - FLOOR_DB) / -FLOOR_DB * 100.0;
    level.round().clamp(0.0, 100.0) as u8
}

/// Reads raw 16-bit mono samples and calls `level` once per window until
/// the input ends.
pub fn read_levels(mut reader: impl Read, mut level: impl FnMut(u8)) -> io::Result<()> {
    let mut bytes = vec![0u8; METER_WINDOW * 2];
    loop {
        if let Err(error) = reader.read_exact(&mut bytes) {
            return match error.kind() {
                io::ErrorKind::UnexpectedEof => Ok(()),
                _ => Err(error),
            };
        }
        let samples: Vec<i16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair))
            .collect();
        level(level_from_peak(peak_of(&samples)));
    }
}
