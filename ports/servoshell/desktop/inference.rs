/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! On-device chat inference, backed by the [`onde`] crate.
//!
//! Everything here runs locally. There is no server round trip and no page
//! content or user text leaves the machine.
//!
//! The engine is deliberately lazy: constructing [`Inference`] costs nothing
//! but an idle tokio runtime, and no model is fetched until the user asks for
//! one, either from the chat panel or from Settings. A first load downloads a
//! couple of gigabytes, which is not something to do on behalf of someone who
//! never opened the panel.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use log::{info, warn};
use onde::hf_cache::{
    ModelDownloadProgress, SupportedHfModel, delete_local_hf_model, download_model,
    list_local_hf_models, list_supported_hf_models,
};
use onde::inference::{ChatEngine, ChatRole, GgufModelConfig, models};

/// The model offered before the user picks one in Settings.
///
/// This is deliberately not [`GgufModelConfig::platform_default`]. That
/// returns Qwen 2.5 Coder 3B on desktop, which is a coding model and, more
/// to the point, is missing from onde's `SUPPORTED_MODELS`, so
/// [`download_model`] refuses it. Anything named here has to be in the
/// catalog that [`list_supported_hf_models`] returns.
const DEFAULT_MODEL_ID: &str = models::BARTOWSKI_QWEN3_4B_INSTRUCT_2507_GGUF;

/// The Onde shared model cache App Group.
///
/// Every Onde-based app on this team points at this group, so weights
/// downloaded by one are already present for the next. Models run to a couple
/// of gigabytes each, which makes the difference between one download and one
/// download per app.
const ONDE_APP_GROUP: &str = "group.com.ondeinference.apps";

/// Where the chosen model id is remembered between launches.
const SELECTED_MODEL_FILE: &str = "selected-model";

/// Base directory handed to onde for model storage.
///
/// Returns the *container root*, not a subdirectory: onde appends `models/` and
/// `models/hub` itself and seeds `HF_HOME` and `HF_HUB_CACHE` from them.
/// Passing an already-suffixed path would bury the cache at `models/models/hub`
/// and quietly miss anything another app had downloaded.
///
/// Falls back to app-private Application Support when the group container is
/// unavailable, which is what happens if the entitlement is missing from the
/// signature or the profile does not carry the group. That failure is silent by
/// design on Apple's side, so the log line is the only way to notice that
/// sharing is off and the app is about to download its own copy.
pub(crate) fn model_cache_dir() -> Option<PathBuf> {
    use objc2_foundation::{NSFileManager, NSString};

    let group = NSString::from_str(ONDE_APP_GROUP);
    let container =
        NSFileManager::defaultManager().containerURLForSecurityApplicationGroupIdentifier(&group);

    if let Some(url) = container &&
        let Some(path) = url.path()
    {
        let path = PathBuf::from(path.to_string());
        info!("Onde model cache: shared group container at {}", path.display());
        return Some(path);
    }

    warn!(
        "Onde App Group {ONDE_APP_GROUP} is unavailable; falling back to a private model cache. \
         Models will not be shared with other Onde apps."
    );
    crate::prefs::default_config_dir()
}

/// Point onde's HuggingFace cache lookups at [`model_cache_dir`].
///
/// [`download_model`] seeds these variables itself, but everything that only
/// *reads* the cache — the Settings model list, deletion, the "already
/// downloaded" check — resolves the directory straight from the environment.
/// Without this, the first launch would report an empty cache even when the
/// group container is full of weights another Onde app downloaded.
///
/// Call this once, before any thread that might read the environment exists.
pub(crate) fn seed_model_cache_env() {
    let Some(root) = model_cache_dir() else {
        return;
    };

    // The layout onde itself creates in `download_model`. Diverging from it
    // here would split the cache in two.
    let hf_home = root.join("models");
    let hf_hub_cache = hf_home.join("hub");
    if let Err(error) = std::fs::create_dir_all(&hf_hub_cache) {
        warn!("Could not create the model cache at {}: {error}", hf_hub_cache.display());
        return;
    }

    // SAFETY: called from `main` before any other thread is started, so no
    // thread can be reading the environment concurrently.
    unsafe {
        std::env::set_var("HF_HOME", &hf_home);
        std::env::set_var("HF_HUB_CACHE", &hf_hub_cache);
    }
}

/// The load configuration for a catalog model id.
///
/// onde keeps this mapping in `GgufModelConfig::from_supported_model_id`, which
/// is crate-private, so it is repeated here. A model that
/// [`list_supported_hf_models`] offers but that is missing below can be
/// downloaded and never loaded, so the two lists have to move together.
fn config_for(model_id: &str) -> Option<GgufModelConfig> {
    Some(match model_id {
        models::BARTOWSKI_QWEN25_0_5B_INSTRUCT_GGUF => GgufModelConfig::qwen25_0_5b(),
        models::BARTOWSKI_QWEN25_1_5B_INSTRUCT_GGUF => GgufModelConfig::qwen25_1_5b(),
        models::BARTOWSKI_QWEN25_3B_INSTRUCT_GGUF => GgufModelConfig::qwen25_3b(),
        models::BARTOWSKI_QWEN25_CODER_7B_INSTRUCT_GGUF => GgufModelConfig::qwen25_coder_7b(),
        models::BARTOWSKI_QWEN3_0_6B_GGUF => GgufModelConfig::qwen3_0_6b(),
        models::BARTOWSKI_QWEN3_1_7B_GGUF => GgufModelConfig::qwen3_1_7b(),
        models::BARTOWSKI_QWEN3_4B_GGUF => GgufModelConfig::qwen3_4b(),
        models::BARTOWSKI_QWEN3_8B_GGUF => GgufModelConfig::qwen3_8b(),
        models::BARTOWSKI_QWEN3_14B_GGUF => GgufModelConfig::qwen3_14b(),
        models::BARTOWSKI_QWEN3_32B_GGUF => GgufModelConfig::qwen3_32b(),
        models::BARTOWSKI_QWEN3_4B_INSTRUCT_2507_GGUF => GgufModelConfig::qwen3_4b_instruct_2507(),
        models::BARTOWSKI_QWEN3_4B_THINKING_2507_GGUF => GgufModelConfig::qwen3_4b_thinking_2507(),
        models::BARTOWSKI_QWEN3_30B_A3B_INSTRUCT_2507_GGUF => {
            GgufModelConfig::qwen3_30b_a3b_instruct_2507()
        },
        models::THEBLOKE_DEEPSEEK_CODER_6_7B_INSTRUCT_GGUF => {
            GgufModelConfig::deepseek_coder_6_7b()
        },
        _ => return None,
    })
}

fn selected_model_path() -> Option<PathBuf> {
    crate::prefs::default_config_dir().map(|directory| directory.join(SELECTED_MODEL_FILE))
}

/// A single line of the visible conversation.
///
/// This mirrors the engine's own history rather than owning it. The engine
/// tracks history for prompt construction; this is what the panel draws, and
/// it also holds entries the engine never sees, such as failures.
pub(crate) struct Turn {
    pub(crate) role: ChatRole,
    pub(crate) text: String,
}

/// What the engine is doing, as far as the UI is concerned.
pub(crate) enum Status {
    /// No model requested yet.
    Idle,
    /// Fetching weights from HuggingFace.
    Downloading {
        fraction: f32,
        detail: String,
    },
    /// Weights are local; mistral.rs is building the model.
    Loading {
        model: String,
    },
    Ready {
        model: String,
    },
    Failed(String),
}

impl Status {
    /// Whether a new request would be accepted right now.
    pub(crate) fn is_ready(&self) -> bool {
        matches!(self, Status::Ready { .. })
    }

    /// Whether the engine is mid-flight, so the panel can disable input
    /// without having to enumerate every busy variant at the call site.
    pub(crate) fn is_busy(&self) -> bool {
        matches!(self, Status::Downloading { .. } | Status::Loading { .. })
    }
}

/// What is on disk, as of the last [`Inference::refresh_catalog`].
///
/// Scanning the cache means walking a directory of multi-gigabyte files, so it
/// happens on user action rather than on every frame the Settings view is
/// drawn.
#[derive(Default)]
pub(crate) struct Catalog {
    /// Every model this build can download, downloaded or not.
    pub(crate) models: Vec<SupportedHfModel>,
    /// Where the weights live, shown so people can find them in Finder.
    pub(crate) cache_path: String,
    /// Total size of the cache, including models this build does not offer.
    pub(crate) total_size_display: String,
    /// Whether a scan has completed. Distinguishes "no models" from "not
    /// looked yet", which otherwise read the same in an empty list.
    pub(crate) scanned: bool,
}

pub(crate) struct Inference {
    /// Inference gets its own runtime rather than borrowing Servo's. Servo's
    /// lives in `components/net` and serves page loads; a multi-second
    /// generation parked on one of its workers would be felt as network
    /// latency by every tab.
    runtime: tokio::runtime::Runtime,
    engine: Arc<ChatEngine>,
    status: Arc<Mutex<Status>>,
    transcript: Arc<Mutex<Vec<Turn>>>,
    /// Set while a generation is outstanding. The panel is drawn every frame
    /// from the winit thread, so this has to be readable without blocking.
    generating: Arc<AtomicBool>,
    /// Where HuggingFace weights land.
    cache_dir: Option<PathBuf>,
    /// The model the user picked, which is not necessarily the loaded one:
    /// picking a model that still has to be downloaded leaves the previous one
    /// serving the chat panel until the new weights are ready.
    selected: Arc<Mutex<String>>,
    /// The model the engine currently holds in memory, if any.
    loaded: Arc<Mutex<Option<String>>>,
    catalog: Arc<Mutex<Catalog>>,
    /// The last failed management action, shown in Settings until the next one
    /// succeeds. Load failures live in [`Status`] instead.
    last_error: Arc<Mutex<Option<String>>>,
    /// Draft text in the input box. UI-only state, kept here so the panel
    /// stays a pure function of `Inference`.
    pub(crate) draft: String,
}

impl Inference {
    pub(crate) fn new() -> Self {
        // A single worker is enough. Generation is one long-lived blocking
        // task at a time, not a fan-out workload, and extra threads would
        // compete with the model's own Metal work for cores.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("onde-inference")
            .enable_all()
            .build()
            .expect("Failed to build the inference runtime");

        let selected = selected_model_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|contents| contents.trim().to_string())
            .filter(|model_id| config_for(model_id).is_some())
            .unwrap_or_else(|| DEFAULT_MODEL_ID.to_string());

        Self {
            runtime,
            engine: Arc::new(ChatEngine::new()),
            status: Arc::new(Mutex::new(Status::Idle)),
            transcript: Arc::new(Mutex::new(Vec::new())),
            generating: Arc::new(AtomicBool::new(false)),
            cache_dir: model_cache_dir(),
            selected: Arc::new(Mutex::new(selected)),
            loaded: Arc::new(Mutex::new(None)),
            catalog: Arc::new(Mutex::new(Catalog::default())),
            last_error: Arc::new(Mutex::new(None)),
            draft: String::new(),
        }
    }

    pub(crate) fn status(&self) -> std::sync::MutexGuard<'_, Status> {
        self.status.lock().unwrap()
    }

    pub(crate) fn transcript(&self) -> std::sync::MutexGuard<'_, Vec<Turn>> {
        self.transcript.lock().unwrap()
    }

    pub(crate) fn catalog(&self) -> std::sync::MutexGuard<'_, Catalog> {
        self.catalog.lock().unwrap()
    }

    pub(crate) fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }

    pub(crate) fn is_generating(&self) -> bool {
        self.generating.load(Ordering::Relaxed)
    }

    pub(crate) fn selected_model_id(&self) -> String {
        self.selected.lock().unwrap().clone()
    }

    pub(crate) fn loaded_model_id(&self) -> Option<String> {
        self.loaded.lock().unwrap().clone()
    }

    /// Human-readable name of the selected model, for status copy.
    pub(crate) fn selected_model_name(&self) -> String {
        let model_id = self.selected_model_id();
        config_for(&model_id)
            .map(|config| config.display_name)
            .unwrap_or(model_id)
    }

    /// Rescan the model cache in the background.
    ///
    /// Cheap to call repeatedly: the scan is metadata-only, but it still
    /// touches the filesystem, so it runs off the UI thread and the view
    /// keeps drawing the previous result until it lands.
    pub(crate) fn refresh_catalog(&self) {
        let catalog = Arc::clone(&self.catalog);
        self.runtime.spawn_blocking(move || {
            let supported = list_supported_hf_models();
            let local = list_local_hf_models();
            *catalog.lock().unwrap() = Catalog {
                models: supported.models,
                cache_path: local.cache_path,
                total_size_display: local.total_size_display,
                scanned: true,
            };
        });
    }

    /// Download the selected model if needed, then load it.
    ///
    /// Returns immediately. Progress is published through [`Self::status`].
    pub(crate) fn ensure_model(&self) {
        self.use_model(&self.selected_model_id());
    }

    /// Make `model_id` the model the chat panel talks to, downloading it first
    /// if the weights are not already in the cache.
    ///
    /// Returns immediately, and does nothing while another load is in flight.
    pub(crate) fn use_model(&self, model_id: &str) {
        {
            let status = self.status.lock().unwrap();
            if status.is_busy() {
                return;
            }
        }

        let Some(config) = config_for(model_id) else {
            *self.last_error.lock().unwrap() = Some(format!("{model_id} cannot be loaded."));
            return;
        };

        self.select(model_id);
        if self.loaded_model_id().as_deref() == Some(model_id) &&
            self.status.lock().unwrap().is_ready()
        {
            return;
        }

        let display_name = config.display_name.clone();
        let model_id = config.model_id.clone();
        let engine = Arc::clone(&self.engine);
        let status = Arc::clone(&self.status);
        let loaded = Arc::clone(&self.loaded);
        let catalog = Arc::clone(&self.catalog);
        let cache_dir = self.cache_dir.clone();

        *self.status.lock().unwrap() = Status::Downloading {
            fraction: 0.0,
            detail: format!("Preparing {display_name}"),
        };

        self.runtime.spawn(async move {
            // Skip the fetch when the weights are already complete. Onde's
            // `download_model` builds the model to verify it, so running it
            // over a full cache would load the weights twice for one request.
            let already_downloaded = list_supported_hf_models()
                .models
                .iter()
                .any(|model| model.model_id == model_id && model.is_downloaded);

            if !already_downloaded {
                let progress_status = Arc::clone(&status);
                let on_progress = move |progress: ModelDownloadProgress| {
                    *progress_status.lock().unwrap() = Status::Downloading {
                        fraction: progress.progress as f32,
                        detail: format!(
                            "{} of {}",
                            progress.downloaded_display, progress.total_display
                        ),
                    };
                };

                // Downloading separately from loading is what makes a progress
                // bar possible: load_gguf_model would fetch the same weights on
                // its own, but only reports once it is finished, which reads as
                // a multi-minute hang the first time.
                if let Err(error) = download_model(model_id.clone(), on_progress, cache_dir).await {
                    *status.lock().unwrap() = Status::Failed(error);
                    return;
                }
            }

            *status.lock().unwrap() = Status::Loading {
                model: display_name.clone(),
            };

            let system_prompt = Some(
                "You are a helpful assistant inside a web browser. Keep answers brief."
                    .to_string(),
            );
            match engine.load_gguf_model(config, system_prompt, None).await {
                Ok(_) => {
                    *loaded.lock().unwrap() = Some(model_id);
                    *status.lock().unwrap() = Status::Ready {
                        model: display_name,
                    };
                },
                Err(error) => *status.lock().unwrap() = Status::Failed(error.to_string()),
            }

            // The download changed what is on disk, and a load says nothing
            // about the rest of the cache, so rescan either way.
            let supported = list_supported_hf_models();
            let local = list_local_hf_models();
            *catalog.lock().unwrap() = Catalog {
                models: supported.models,
                cache_path: local.cache_path,
                total_size_display: local.total_size_display,
                scanned: true,
            };
        });
    }

    /// Delete `model_id`'s weights, unloading it first if it is the model in
    /// memory.
    ///
    /// Deleting the loaded model leaves the chat panel with no model rather
    /// than silently keeping one that no longer exists on disk.
    pub(crate) fn delete_model(&self, model_id: &str) {
        let model_id = model_id.to_string();
        let engine = Arc::clone(&self.engine);
        let status = Arc::clone(&self.status);
        let loaded = Arc::clone(&self.loaded);
        let catalog = Arc::clone(&self.catalog);
        let transcript = Arc::clone(&self.transcript);
        let last_error = Arc::clone(&self.last_error);

        self.runtime.spawn(async move {
            if loaded.lock().unwrap().as_deref() == Some(model_id.as_str()) {
                engine.unload_model().await;
                *loaded.lock().unwrap() = None;
                transcript.lock().unwrap().clear();
                *status.lock().unwrap() = Status::Idle;
            }

            match delete_local_hf_model(model_id.clone()) {
                Ok(()) => {
                    info!("Deleted on-device model {model_id}");
                    *last_error.lock().unwrap() = None;
                },
                Err(error) => {
                    warn!("Could not delete on-device model {model_id}: {error}");
                    *last_error.lock().unwrap() = Some(error);
                },
            }

            let supported = list_supported_hf_models();
            let local = list_local_hf_models();
            *catalog.lock().unwrap() = Catalog {
                models: supported.models,
                cache_path: local.cache_path,
                total_size_display: local.total_size_display,
                scanned: true,
            };
        });
    }

    /// Send `message` to the model and append both sides to the transcript.
    ///
    /// Dropped silently when no model is loaded or a reply is already in
    /// flight, so the panel does not have to guard every send path.
    pub(crate) fn send(&self, message: String) {
        if message.trim().is_empty() || self.is_generating() {
            return;
        }
        if !self.status.lock().unwrap().is_ready() {
            return;
        }

        self.transcript.lock().unwrap().push(Turn {
            role: ChatRole::User,
            text: message.clone(),
        });

        let engine = Arc::clone(&self.engine);
        let transcript = Arc::clone(&self.transcript);
        let generating = Arc::clone(&self.generating);

        generating.store(true, Ordering::Relaxed);
        self.runtime.spawn(async move {
            let text = match engine.send_message(message).await {
                Ok(result) => result.text,
                Err(error) => format!("Inference failed: {error}"),
            };
            transcript.lock().unwrap().push(Turn {
                role: ChatRole::Assistant,
                text,
            });
            generating.store(false, Ordering::Relaxed);
        });
    }

    /// Drop the conversation without unloading the model, which is the
    /// expensive part to rebuild.
    pub(crate) fn clear(&self) {
        self.transcript.lock().unwrap().clear();
        let engine = Arc::clone(&self.engine);
        self.runtime.spawn(async move {
            engine.clear_history().await;
        });
    }

    /// Remember `model_id` as the model to load on the next launch.
    fn select(&self, model_id: &str) {
        *self.selected.lock().unwrap() = model_id.to_string();

        let Some(path) = selected_model_path() else {
            return;
        };
        if let Some(directory) = path.parent() &&
            let Err(error) = std::fs::create_dir_all(directory)
        {
            warn!("Could not create {}: {error}", directory.display());
            return;
        }
        if let Err(error) = std::fs::write(&path, model_id) {
            warn!("Could not save the model choice to {}: {error}", path.display());
        }
    }
}
