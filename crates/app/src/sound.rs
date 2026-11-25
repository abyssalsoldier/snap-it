//! A short, synthesised "shutter" click so the user always knows a capture happened.
//! Generated in memory: no bundled audio files.

use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

fn wav() -> &'static [u8] {
    static W: OnceLock<Vec<u8>> = OnceLock::new();
    W.get_or_init(|| {
        let rate = 22_050u32;
        let n = rate as usize * 90 / 1000;
        let mut seed = 0x1234_5678u32;
        let samples: Vec<i16> = (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
                // Two quick decaying clicks, like a shutter opening and closing.
                let env = (-t * 90.0).exp() + 0.6 * (-(t - 0.045).max(0.0) * 120.0).exp() * (t >= 0.045) as u8 as f32;
                let tone = (t * 2.0 * std::f32::consts::PI * 1800.0).sin() * 0.3;
                ((noise * 0.7 + tone) * env * 0.35 * i16::MAX as f32) as i16
            })
            .collect();
        let data_len = (samples.len() * 2) as u32;
        let mut w = Vec::with_capacity(44 + data_len as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&1u16.to_le_bytes()); // mono
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 2).to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    })
}

pub fn shutter() {
    unsafe {
        let _ = PlaySoundW(PCWSTR(wav().as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wav_header_is_consistent() {
        let w = super::wav();
        assert_eq!(&w[..4], b"RIFF");
        let riff = u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff + 8, w.len());
    }
}
