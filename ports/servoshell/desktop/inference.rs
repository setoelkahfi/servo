/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! On-device chat inference, backed by the [`onde`] crate.
//!
//! Everything here runs locally. There is no server round trip and no page
//! content or user text leaves the machine.
//!
//! The engine is deliberately lazy: constructing [`Inference`] costs nothing
//! but an idle tokio runtime, and no model is fetched until the user opens the
//! chat panel and asks for one. A first load downloads roughly 2 GB, which is
//! not something to do on behalf of someone who never opened the panel.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use onde::hf_cache::{ModelDownloadProgress, download_model};
use onde::inference::{ChatEngine, ChatRole, GgufModelConfig};

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
    Loading,
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
        matches!(self, Status::Downloading { .. } | Status::Loading)
    }
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
    /// Draft text in the input box. UI-only state, kept here so the panel
    /// stays a pure function of `Inference`.
    pub(crate) draft: String,
}

impl Inference {
    pub(crate) fn new(cache_dir: Option<PathBuf>) -> Self {
        // A single worker is enough. Generation is one long-lived blocking
        // task at a time, not a fan-out workload, and extra threads would
        // compete with the model's own Metal work for cores.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("onde-inference")
            .enable_all()
            .build()
            .expect("Failed to build the inference runtime");

        Self {
            runtime,
            engine: Arc::new(ChatEngine::new()),
            status: Arc::new(Mutex::new(Status::Idle)),
            transcript: Arc::new(Mutex::new(Vec::new())),
            generating: Arc::new(AtomicBool::new(false)),
            cache_dir,
            draft: String::new(),
        }
    }

    pub(crate) fn status(&self) -> std::sync::MutexGuard<'_, Status> {
        self.status.lock().unwrap()
    }

    pub(crate) fn transcript(&self) -> std::sync::MutexGuard<'_, Vec<Turn>> {
        self.transcript.lock().unwrap()
    }

    pub(crate) fn is_generating(&self) -> bool {
        self.generating.load(Ordering::Relaxed)
    }

    /// Download the platform default model if needed, then load it.
    ///
    /// Returns immediately. Progress is published through [`Self::status`].
    pub(crate) fn ensure_model(&self) {
        {
            let status = self.status.lock().unwrap();
            if status.is_ready() || status.is_busy() {
                return;
            }
        }

        let config = GgufModelConfig::platform_default();
        let display_name = config.display_name.clone();
        let model_id = config.model_id.clone();
        let engine = Arc::clone(&self.engine);
        let status = Arc::clone(&self.status);
        let cache_dir = self.cache_dir.clone();

        *self.status.lock().unwrap() = Status::Downloading {
            fraction: 0.0,
            detail: format!("Preparing {display_name}"),
        };

        self.runtime.spawn(async move {
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

            // Downloading separately from loading is what makes a progress bar
            // possible: load_gguf_model would fetch the same weights on its
            // own, but only reports once it is finished, which reads as a
            // multi-minute hang the first time.
            if let Err(error) = download_model(model_id, on_progress, cache_dir).await {
                *status.lock().unwrap() = Status::Failed(error);
                return;
            }

            *status.lock().unwrap() = Status::Loading;

            let system_prompt = Some(
                "You are a helpful assistant inside a web browser. Keep answers brief."
                    .to_string(),
            );
            match engine.load_gguf_model(config, system_prompt, None).await {
                Ok(_) => {
                    *status.lock().unwrap() = Status::Ready {
                        model: display_name,
                    }
                },
                Err(error) => *status.lock().unwrap() = Status::Failed(error.to_string()),
            }
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
}
