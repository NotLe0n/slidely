//! Loading the Whisper and VAD models, and keeping them loaded between
//! transcriptions.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context as _, bail};
use whisper_rs::{WhisperContext, WhisperContextParameters};

use super::speech::Vad;

/// Overrides where the Whisper model is loaded from.
const MODEL_ENV_VAR: &str = "SLIDELY_WHISPER_MODEL";
/// Where the Whisper model is looked for, relative to the working directory, unless
/// `MODEL_ENV_VAR` is set. The first one that exists is used: small is several times
/// faster than large-v3-turbo and plenty to match the speech to the slides.
const DEFAULT_MODEL_PATHS: &[&str] = &[
    "models/ggml-small-q5_1.bin",
    "models/ggml-small.bin",
    "models/ggml-large-v3-turbo-q5_0.bin",
];

/// The Whisper model, kept loaded between transcriptions: loading it reads hundreds
/// of MB and sets up the GPU, which takes seconds. The parts share it, each with a
/// state of its own.
static MODEL: Mutex<Option<(PathBuf, Arc<WhisperContext>)>> = Mutex::new(None);

/// Voice activity detection models not in use, kept loaded like `MODEL`. Each part
/// needs one of its own, since they run at the same time.
static VADS: Mutex<Vec<Vad>> = Mutex::new(Vec::new());

/// Starts loading the Whisper and VAD models in the background, so they load while
/// the first block of audio decodes.
pub(super) fn preload_models() {
    std::thread::spawn(|| match take_vad() {
        Ok(vad) => return_vad(vad),
        Err(err) => eprintln!("Failed to preload the VAD model: {err:#}"),
    });
    std::thread::spawn(|| {
        if let Err(err) = load_model() {
            eprintln!("Failed to preload the Whisper model: {err:#}");
        }
    });
}

/// A loaded VAD, loading another one if they're all in use.
pub(super) fn take_vad() -> anyhow::Result<Vad> {
    let vad = VADS.lock().unwrap_or_else(PoisonError::into_inner).pop();
    match vad {
        Some(vad) => Ok(vad),
        None => Vad::load(),
    }
}

/// Keeps `vad` loaded for the next part that needs one.
pub(super) fn return_vad(vad: Vad) {
    VADS.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(vad);
}

/// The loaded Whisper model, loading it unless that's done already. Waits for a load
/// running on another thread instead of loading it twice.
pub(super) fn load_model() -> anyhow::Result<Arc<WhisperContext>> {
    let model_path = find_whisper_model()?;
    let mut model = MODEL.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((path, context)) = &*model
        && *path == model_path
    {
        return Ok(context.clone());
    }
    // Frees the GPU memory of a different model before loading this one.
    *model = None;

    let context_params = WhisperContextParameters {
        use_gpu: true,
        flash_attn: true,
        ..Default::default()
    };
    let model_path_str = model_path
        .to_str()
        .context("model path isn't valid UTF-8")?;
    let context = Arc::new(
        WhisperContext::new_with_params(model_path_str, context_params)
            .context("failed to load the Whisper model")?,
    );
    *model = Some((model_path, context.clone()));
    Ok(context)
}

/// The model at `MODEL_ENV_VAR`, or else the first of `DEFAULT_MODEL_PATHS` there is.
pub(super) fn find_whisper_model() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os(MODEL_ENV_VAR) {
        let path = PathBuf::from(path);
        if !path.is_file() {
            bail!(
                "Whisper model not found at {} ({MODEL_ENV_VAR}).",
                path.display()
            );
        }
        return Ok(path);
    }
    DEFAULT_MODEL_PATHS
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .with_context(|| {
            format!(
                "Whisper model not found at any of {}. Download a ggml model (e.g. from \
                 huggingface.co/ggerganov/whisper.cpp) there, or set {MODEL_ENV_VAR}.",
                DEFAULT_MODEL_PATHS.join(", ")
            )
        })
}
