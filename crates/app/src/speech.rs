//! Offline dictation (feature `speech`): push-to-talk or press-to-toggle,
//! transcribed locally with whisper.cpp (`whisper-rs`), then pasted.
//!
//! - The model is chosen from RAM and CPU cores, downloaded once from a pinned
//!   whisper.cpp revision on Hugging Face, and checked against its SHA-256 and
//!   size before use. That download is the only network access; audio never
//!   leaves the machine.
//! - The microphone is opened only while recording.
//! - Transcription runs on a worker thread; the result arrives through the
//!   event loop proxy and is pasted via the clipboard plus a synthetic Ctrl+V.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use winit::event_loop::EventLoopProxy;

use crate::model::{Model, Now};
use crate::shell::{SpeechMsg, UserEvent};

/// Pinned whisper.cpp model repository revision.
pub const MODEL_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

/// A downloadable model with its expected size and SHA-256.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    /// File name in the repository.
    pub file: &'static str,
    /// Expected size in bytes.
    pub size: u64,
    /// Expected SHA-256 (hex).
    pub sha256: &'static str,
}

/// Multilingual models (language is auto-detected).
pub const TINY: ModelSpec = ModelSpec {
    file: "ggml-tiny.bin",
    size: 77_691_713,
    sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
};
/// Base model.
pub const BASE: ModelSpec = ModelSpec {
    file: "ggml-base.bin",
    size: 147_951_465,
    sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
};
/// Small model.
pub const SMALL: ModelSpec = ModelSpec {
    file: "ggml-small.bin",
    size: 487_601_967,
    sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
};

/// Pick the largest model this machine transcribes quickly enough. Pure.
pub fn pick_model(ram_gb: f64, cores: usize) -> ModelSpec {
    if ram_gb >= 16.0 && cores >= 8 {
        SMALL
    } else if ram_gb >= 4.0 && cores >= 4 {
        BASE
    } else {
        TINY
    }
}

/// Mix to mono and resample to 16 kHz (linear interpolation). Pure.
pub fn to_16k_mono(samples: &[f32], rate: u32, channels: u16) -> Vec<f32> {
    let ch = usize::from(channels.max(1));
    let mono: Vec<f32> = samples
        .chunks(ch)
        .map(|f| f.iter().sum::<f32>() / f.len() as f32)
        .collect();
    if rate == 16_000 || mono.is_empty() {
        return mono;
    }
    let ratio = f64::from(rate) / 16_000.0;
    let n = (mono.len() as f64 / ratio).floor() as usize;
    (0..n)
        .map(|i| {
            let pos = i as f64 * ratio;
            let j = pos.floor() as usize;
            let frac = (pos - j as f64) as f32;
            let a = mono.get(j).copied().unwrap_or(0.0);
            let b = mono.get(j + 1).copied().unwrap_or(a);
            a + (b - a) * frac
        })
        .collect()
}

/// Lower-case hex SHA-256 of a file.
pub fn sha256_file(path: &std::path::Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn model_dir() -> Option<PathBuf> {
    crate::persist::cache_dir().map(|d| d.join("models"))
}

fn model_path(spec: &ModelSpec) -> Option<PathBuf> {
    model_dir().map(|d| d.join(spec.file))
}

fn chosen() -> ModelSpec {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    pick_model(crate::sys::total_ram_gb(), cores)
}

fn send(proxy: &EventLoopProxy<UserEvent>, msg: SpeechMsg) {
    let _ = proxy.send_event(UserEvent::Speech(msg));
}

/// Download a model from the pinned revision and verify its size and SHA-256
/// (blocking; run on a worker thread). Nothing is kept if the check fails.
pub fn download(
    spec: &ModelSpec,
    dest: &std::path::Path,
    progress: &dyn Fn(String),
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/{MODEL_REVISION}/{}",
        spec.file
    );
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = dest.with_extension("part");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .user_agent(concat!(
            "Perch/",
            env!("CARGO_PKG_VERSION"),
            " (desktop notch app)"
        ))
        .build();
    let resp = agent
        .get(&url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut done: u64 = 0;
    let mut last_report = Instant::now();
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if last_report.elapsed() > Duration::from_secs(2) {
            last_report = Instant::now();
            progress(format!(
                "Downloading speech model {}%",
                done * 100 / spec.size.max(1)
            ));
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if done != spec.size || hex != spec.sha256 {
        let _ = std::fs::remove_file(&tmp);
        return Err("the downloaded model failed its checksum; nothing was kept".into());
    }
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

struct Recording {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    channels: u16,
    started: Instant,
    proxy: EventLoopProxy<UserEvent>,
}

thread_local! {
    static REC: RefCell<Option<Recording>> = const { RefCell::new(None) };
}

static CTX: Mutex<Option<(String, whisper_rs::WhisperContext)>> = Mutex::new(None);
static DOWNLOADING: Mutex<bool> = Mutex::new(false);

fn start_recording(proxy: EventLoopProxy<UserEvent>) -> Result<Recording, String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let dev = host.default_input_device().ok_or("no microphone found")?;
    let cfg = dev
        .default_input_config()
        .map_err(|e| format!("microphone: {e}"))?;
    let (rate, channels) = (cfg.sample_rate().0, cfg.channels());
    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let err = |e| log::warn!("microphone stream: {e}");
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::F32 => {
            let s = samples.clone();
            dev.build_input_stream(
                &cfg.into(),
                move |d: &[f32], _: &_| {
                    if let Ok(mut v) = s.lock() {
                        v.extend_from_slice(d);
                    }
                },
                err,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let s = samples.clone();
            dev.build_input_stream(
                &cfg.into(),
                move |d: &[i16], _: &_| {
                    if let Ok(mut v) = s.lock() {
                        v.extend(d.iter().map(|x| f32::from(*x) / 32768.0));
                    }
                },
                err,
                None,
            )
        }
        other => return Err(format!("unsupported microphone format {other:?}")),
    }
    .map_err(|e| format!("microphone: {e}"))?;
    stream.play().map_err(|e| format!("microphone: {e}"))?;
    Ok(Recording {
        _stream: stream,
        samples,
        rate,
        channels,
        started: Instant::now(),
        proxy,
    })
}

/// Hotkey / CLI / button: start dictating, or stop and transcribe.
pub fn toggle(model: &mut Model, proxy: EventLoopProxy<UserEvent>) {
    let recording = REC.with(|r| r.borrow().is_some());
    if recording {
        stop(model);
        return;
    }
    let spec = chosen();
    let Some(path) = model_path(&spec) else {
        model.flash("No cache folder for the speech model", Now::real());
        return;
    };
    if !path.exists() {
        let already = DOWNLOADING
            .lock()
            .map(|mut g| std::mem::replace(&mut *g, true))
            .unwrap_or(true);
        if already {
            model.flash("Still downloading the speech model", Now::real());
            return;
        }
        {
            let p = proxy.clone();
            let spawned = std::thread::Builder::new()
                .name("perch-model-download".into())
                .spawn(move || {
                    send(
                        &p,
                        SpeechMsg::Status(format!(
                            "Downloading speech model ({} MB)",
                            spec.size / 1_000_000
                        )),
                    );
                    let msg = match download(&spec, &path, &|m| send(&p, SpeechMsg::Status(m))) {
                        Ok(()) => "Dictation is ready; press the shortcut again".to_string(),
                        Err(e) => e,
                    };
                    if let Ok(mut g) = DOWNLOADING.lock() {
                        *g = false;
                    }
                    send(&p, SpeechMsg::Status(msg));
                });
            if spawned.is_err() {
                model.flash("Could not start the model download", Now::real());
            }
        }
        return;
    }
    match start_recording(proxy) {
        Ok(rec) => {
            REC.with(|r| *r.borrow_mut() = Some(rec));
            model.flash("Listening...", Now::real());
        }
        Err(e) => model.flash(e, Now::real()),
    }
}

/// Push-to-talk release: stop if the key was held (a quick tap keeps
/// recording, so a tap-tap also works).
pub fn release(model: &mut Model) {
    let held = REC.with(|r| {
        r.borrow()
            .as_ref()
            .is_some_and(|rec| rec.started.elapsed() > Duration::from_millis(400))
    });
    if held {
        stop(model);
    }
}

fn stop(model: &mut Model) {
    let Some(rec) = REC.with(|r| r.borrow_mut().take()) else {
        return;
    };
    let raw = rec.samples.lock().map(|v| v.clone()).unwrap_or_default();
    let (rate, channels, proxy) = (rec.rate, rec.channels, rec.proxy.clone());
    drop(rec); // closes the microphone
    let audio = to_16k_mono(&raw, rate, channels);
    if audio.len() < 16_000 / 4 {
        model.flash("Too short to transcribe", Now::real());
        return;
    }
    model.flash("Transcribing...", Now::real());
    let spec = chosen();
    let spawned = std::thread::Builder::new()
        .name("perch-transcribe".into())
        .spawn(move || {
            let msg = match transcribe(&spec, &audio) {
                Ok(t) if t.trim().is_empty() => SpeechMsg::Status("Heard nothing to type".into()),
                Ok(t) => SpeechMsg::Text(t.trim().to_string()),
                Err(e) => SpeechMsg::Status(e),
            };
            send(&proxy, msg);
        });
    if spawned.is_err() {
        model.flash("Could not start transcription", Now::real());
    }
}

fn transcribe(spec: &ModelSpec, audio: &[f32]) -> Result<String, String> {
    let path = model_path(spec).ok_or("no model folder")?;
    transcribe_path(&path, spec.file, audio)
}

/// Transcribe 16 kHz mono audio with the model at `path` (cached per `key`).
pub fn transcribe_path(path: &std::path::Path, key: &str, audio: &[f32]) -> Result<String, String> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};
    let mut guard = CTX.lock().map_err(|_| "speech engine busy")?;
    if guard.as_ref().map(|(f, _)| f.as_str()) != Some(key) {
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| format!("loading the speech model: {e}"))?;
        *guard = Some((key.to_string(), ctx));
    }
    let (_, ctx) = guard.as_ref().ok_or("speech model not loaded")?;
    let mut state = ctx
        .create_state()
        .map_err(|e| format!("speech engine: {e}"))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("auto"));
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(8);
    params.set_n_threads(threads as i32);
    state
        .full(params, audio)
        .map_err(|e| format!("transcription failed: {e}"))?;
    let mut text = String::new();
    for seg in state.as_iter() {
        if let Ok(s) = seg.to_str_lossy() {
            text.push_str(&s);
        }
    }
    Ok(text)
}
