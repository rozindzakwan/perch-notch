//! Dictation pipeline tests (feature `speech`).
#![cfg(feature = "speech")]

use perch_app::speech::{pick_model, to_16k_mono, BASE, SMALL, TINY};

#[test]
fn model_choice_follows_the_machine() {
    assert_eq!(pick_model(2.0, 2), TINY);
    assert_eq!(pick_model(8.0, 4), BASE);
    assert_eq!(pick_model(32.0, 16), SMALL);
    assert_eq!(
        pick_model(32.0, 2),
        TINY,
        "few cores stay on the fast model"
    );
}

#[test]
fn resampling_to_16k_mono() {
    // 48 kHz stereo, 0.1 s: left = 1.0, right = 0.0 -> mono 0.5.
    let stereo: Vec<f32> = (0..4800).flat_map(|_| [1.0f32, 0.0]).collect();
    let out = to_16k_mono(&stereo, 48_000, 2);
    assert_eq!(out.len(), 1600);
    assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-6));
    assert_eq!(
        to_16k_mono(&[0.25; 160], 16_000, 1).len(),
        160,
        "16 kHz mono passes through"
    );
    assert!(to_16k_mono(&[], 44_100, 2).is_empty());
}

/// Downloads the tiny model (77 MB, checksum-verified) and transcribes speech
/// synthesised by `espeak-ng`. Opt-in: needs network once and espeak-ng.
#[test]
#[ignore = "downloads the speech model; needs espeak-ng"]
fn transcribes_synthesised_speech_end_to_end() {
    let dir = std::env::var_os("PERCH_TEST_MODEL_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("perch-models"));
    let model = dir.join(TINY.file);
    if !model.exists() {
        perch_app::speech::download(&TINY, &model, &|m| eprintln!("{m}"))
            .unwrap_or_else(|e| panic!("download: {e}"));
    }
    assert_eq!(
        perch_app::speech::sha256_file(&model).unwrap_or_default(),
        TINY.sha256,
        "the cached model matches the pinned checksum"
    );
    let wav = std::env::temp_dir().join("perch-speech-test.wav");
    let status = std::process::Command::new("espeak-ng")
        .args(["-v", "en-us", "-s", "140", "-w"])
        .arg(&wav)
        .arg("The quick brown fox jumps over the lazy dog.")
        .status()
        .unwrap_or_else(|e| panic!("espeak-ng: {e}"));
    assert!(status.success());
    let reader = hound::WavReader::open(&wav).unwrap_or_else(|e| panic!("{e}"));
    let spec = reader.spec();
    let samples: Vec<f32> = reader
        .into_samples::<i16>()
        .map(|s| f32::from(s.unwrap_or(0)) / 32768.0)
        .collect();
    let audio = to_16k_mono(&samples, spec.sample_rate, spec.channels);
    let text = perch_app::speech::transcribe_path(&model, TINY.file, &audio)
        .unwrap_or_else(|e| panic!("{e}"))
        .to_lowercase();
    eprintln!("transcribed: {text}");
    // The tiny model mishears espeak's robotic voice on some words (e.g.
    // "roundfax" for "brown fox"), so require clear recognition of at least
    // two of the key words rather than a perfect transcript.
    let hits = ["quick", "fox", "lazy", "dog"]
        .iter()
        .filter(|w| text.contains(*w))
        .count();
    assert!(hits >= 2, "only {hits} key words recognised in {text:?}");
}
