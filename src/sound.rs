//! Drop's own notification sounds, made from a few sine waves so nothing but
//! this code ships with the program. They wait ready on an audio device kept
//! open from the start, as opening one for each sound is what makes it late.

use std::cell::RefCell;
use std::f32::consts::TAU;

use windows::Win32::Media::Audio::{
    waveOutOpen, waveOutPrepareHeader, waveOutReset, waveOutWrite, CALLBACK_NULL, HWAVEOUT,
    WAVEFORMATEX, WAVEHDR, WAVE_FORMAT_PCM, WAVE_MAPPER,
};

const SAMPLE_RATE: u32 = 22_050;
/// Seconds a tone takes to rise, which keeps it from clicking.
const ATTACK: f32 = 0.003;
/// Seconds of quiet fade at the end of a sound.
const RELEASE: f32 = 0.03;

/// One tone: when it starts and how long it lasts in seconds, the pitch it
/// glides between in hertz, its loudness 0..1, and how fast it dies away.
struct Tone {
    start: f32,
    length: f32,
    pitch: (f32, f32),
    gain: f32,
    decay: f32,
}

/// A message arrives: two rising drops.
const APP_TONES: [Tone; 2] = [
    Tone {
        start: 0.0,
        length: 0.16,
        pitch: (700.0, 1500.0),
        gain: 0.55,
        decay: 26.0,
    },
    Tone {
        start: 0.11,
        length: 0.2,
        pitch: (1000.0, 2000.0),
        gain: 0.3,
        decay: 22.0,
    },
];

/// Drop itself has news: two soft bell notes going up.
const NOTICE_TONES: [Tone; 2] = [
    Tone {
        start: 0.0,
        length: 0.3,
        pitch: (659.0, 659.0),
        gain: 0.4,
        decay: 10.0,
    },
    Tone {
        start: 0.1,
        length: 0.4,
        pitch: (988.0, 988.0),
        gain: 0.4,
        decay: 9.0,
    },
];

/// The open device and the sounds prepared on it.
struct Player {
    device: HWAVEOUT,
    app: *mut WAVEHDR,
    notice: *mut WAVEHDR,
}

thread_local! {
    static PLAYER: RefCell<Option<Player>> = const { RefCell::new(None) };
}

/// Opens the device and readies the sounds. Call once at start; a failure
/// here is tried again by the next sound.
pub fn prepare() {
    PLAYER.with(|player| {
        let mut player = player.borrow_mut();
        if player.is_none() {
            *player = unsafe { open() };
        }
    });
}

unsafe fn open() -> Option<Player> {
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 1,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * 2,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let mut device = HWAVEOUT::default();
    let failed = waveOutOpen(
        Some(&mut device),
        WAVE_MAPPER,
        &format,
        None,
        None,
        CALLBACK_NULL,
    );
    if failed != 0 {
        return None;
    }
    Some(Player {
        device,
        app: prepared(device, &APP_TONES)?,
        notice: prepared(device, &NOTICE_TONES)?,
    })
}

/// The tones ready to play on `device`, kept for as long as Drop runs.
unsafe fn prepared(device: HWAVEOUT, tones: &[Tone]) -> Option<*mut WAVEHDR> {
    let body: &'static mut [u8] = bytes(tones).leak();
    let header = Box::leak(Box::new(WAVEHDR {
        lpData: windows::core::PSTR(body.as_mut_ptr()),
        dwBufferLength: body.len() as u32,
        ..Default::default()
    }));
    let size = size_of::<WAVEHDR>() as u32;
    (waveOutPrepareHeader(device, header, size) == 0).then_some(header as *mut WAVEHDR)
}

/// Plays the sound for a notification from another app.
pub fn play_app() {
    play(|player| player.app);
}

/// Plays the sound for Drop's own notices.
pub fn play_notice() {
    play(|player| player.notice);
}

/// Cuts off whatever still sounds, then starts the chosen sound.
fn play(choose: impl Fn(&Player) -> *mut WAVEHDR) {
    prepare();
    PLAYER.with(|player| {
        if let Some(player) = player.borrow().as_ref() {
            unsafe {
                let _ = waveOutReset(player.device);
                let size = size_of::<WAVEHDR>() as u32;
                let _ = waveOutWrite(player.device, choose(player), size);
            }
        }
    });
}

/// The tones mixed, as 16 bit samples.
fn samples(tones: &[Tone]) -> Vec<i16> {
    let seconds = tones
        .iter()
        .map(|tone| tone.start + tone.length)
        .fold(0.0, f32::max);
    let count = (seconds * SAMPLE_RATE as f32) as usize;
    let mut mix = vec![0.0f32; count];
    for tone in tones {
        let first = (tone.start * SAMPLE_RATE as f32) as usize;
        let mut phase = 0.0f32;
        for (index, sample) in mix[first..].iter_mut().enumerate() {
            let t = index as f32 / SAMPLE_RATE as f32;
            if t >= tone.length {
                break;
            }
            let glide = tone.pitch.0 + (tone.pitch.1 - tone.pitch.0) * (t / tone.length);
            phase += TAU * glide / SAMPLE_RATE as f32;
            let rise = (t / ATTACK).min(1.0);
            let fall = ((tone.length - t) / RELEASE).min(1.0);
            *sample += phase.sin() * tone.gain * (-tone.decay * t).exp() * rise * fall;
        }
    }
    mix.iter()
        .map(|sample| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)
        .collect()
}

/// The tones as raw little-endian samples, as the device takes them.
fn bytes(tones: &[Tone]) -> Vec<u8> {
    samples(tones)
        .into_iter()
        .flat_map(i16::to_le_bytes)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_are_short_and_small() {
        for tones in [&APP_TONES, &NOTICE_TONES] {
            let body = bytes(tones);
            assert!(body.len() < 24_000, "{}", body.len());
            assert_eq!(body.len() % 2, 0);
        }
    }

    #[test]
    fn sounds_stay_in_range_and_end_in_silence() {
        for tones in [&APP_TONES, &NOTICE_TONES] {
            let samples = samples(tones);
            let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!((3_000..i16::MAX as u16).contains(&peak), "{peak}");
            assert!(samples[samples.len() - 1].unsigned_abs() < 300);
            assert!(samples[0].unsigned_abs() < 300);
        }
    }

    #[test]
    fn the_two_sounds_differ() {
        assert_ne!(samples(&APP_TONES), samples(&NOTICE_TONES));
    }
}
