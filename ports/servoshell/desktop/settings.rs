/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The Settings window, opened from the application menu (⌘,).
//!
//! Most of the window reports rather than configures: what this build is, where
//! it keeps your data, which web features it turns on that upstream Servo still
//! gates. The Appearance panel is the exception, and the settings it writes live
//! in [`StoredSettings`], next to the engine's `prefs.json` in the profile
//! folder. These are the browser's own settings, not the engine's, which is why
//! they are kept out of `prefs.json`.
//!
//! The models panel, macOS only, is the other exception: it manages the
//! on-device models — which one the chat panel talks to, which are downloaded,
//! and how much disk they take. Weights are gigabytes each and are shared with
//! the other Onde apps on this Mac, so deleting one is a real decision and gets
//! a confirmation step.
//!
//! Each panel is its own function taking `&mut egui::Ui`, and [`Settings::show`]
//! calls them in order. A new panel is a function plus one call.

use std::path::PathBuf;

use log::warn;
use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
use crate::desktop::inference::{Inference, Status};
use crate::prefs::{SMB_DAILY_WEB_PREFS, default_config_dir};

/// Width of the label column, wide enough for the longest label below without
/// letting a long value push the columns apart.
const LABEL_COLUMN_WIDTH: f32 = 140.0;

/// Where the settings below are kept, in the profile folder reported by the
/// About panel.
const SETTINGS_FILE_NAME: &str = "browser-settings.json";

/// The loading bar's colour when nothing has chosen one.
const DEFAULT_PROGRESS_BAR_COLOR: [u8; 3] = [66, 133, 244];

/// The settings the Settings window writes, as they are stored on disk.
///
/// Every field needs a `#[serde(default)]` so that a file written by an older
/// build, or one a user has hand-edited, still loads: a missing key should take
/// the default rather than throw the whole file away.
#[derive(Clone, Copy, Deserialize, Serialize)]
struct StoredSettings {
    /// The colour of the loading bar under the tab strip, as `[r, g, b]`.
    #[serde(default = "default_progress_bar_color")]
    progress_bar_color: [u8; 3],
}

fn default_progress_bar_color() -> [u8; 3] {
    DEFAULT_PROGRESS_BAR_COLOR
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            progress_bar_color: DEFAULT_PROGRESS_BAR_COLOR,
        }
    }
}

impl StoredSettings {
    fn path() -> Option<PathBuf> {
        Some(default_config_dir()?.join(SETTINGS_FILE_NAME))
    }

    /// Read the settings file, falling back to the defaults for anything that
    /// is missing, unreadable or malformed. A broken settings file should cost
    /// the user their customisation, not their browser.
    fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str(&contents) {
            Ok(settings) => settings,
            Err(error) => {
                warn!("Ignoring {}: {error}", path.display());
                Self::default()
            },
        }
    }

    fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        let write = || -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, serde_json::to_string_pretty(self)?)
        };
        if let Err(error) = write() {
            warn!("Could not write {}: {error}", path.display());
        }
    }
}

/// Write a path under the user's home directory the way people write it, so the
/// row reads `~/Library/…` rather than spelling out the account name.
fn abbreviate_home(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        },
        _ => path.to_owned(),
    }
}

pub(crate) struct Settings {
    open: bool,
    stored: StoredSettings,
    /// The model a delete has been asked for but not yet confirmed.
    #[cfg(target_os = "macos")]
    pending_delete: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            open: false,
            stored: StoredSettings::load(),
            #[cfg(target_os = "macos")]
            pending_delete: None,
        }
    }
}

impl Settings {
    /// Show the window, scanning the model cache on the way in so the list is
    /// current without polling the filesystem every frame.
    #[cfg(target_os = "macos")]
    pub(crate) fn open(&mut self, inference: &Inference) {
        if !self.open {
            inference.refresh_catalog();
        }
        self.open = true;
    }

    #[cfg(not(target_os = "macos"))]
    pub(crate) fn open(&mut self) {
        self.open = true;
    }

    /// The colour the loading bar draws itself in.
    pub(crate) fn progress_bar_color(&self) -> egui::Color32 {
        let [red, green, blue] = self.stored.progress_bar_color;
        egui::Color32::from_rgb(red, green, blue)
    }

    pub(crate) fn show(
        &mut self,
        ctx: &egui::Context,
        #[cfg(target_os = "macos")] inference: &Inference,
    ) {
        if !self.open {
            return;
        }

        // `Window::open` needs its own borrow, so the rest of the state is
        // taken apart first.
        let Self {
            open,
            stored,
            #[cfg(target_os = "macos")]
            pending_delete,
        } = self;

        #[cfg(target_os = "macos")]
        let was_open = *open;

        egui::Window::new("Settings")
            .open(open)
            .collapsible(false)
            .default_size([560.0, 480.0])
            .min_width(420.0)
            // Centred on first open. Left at egui's default the window lands in
            // the top-left corner, over the toolbar it was just opened from.
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    Self::about_panel(ui);
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(12.0);
                    Self::appearance_panel(ui, stored);
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(12.0);
                    #[cfg(target_os = "macos")]
                    {
                        Self::models_panel(ui, inference, pending_delete);
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(12.0);
                    }
                    Self::web_platform_panel(ui);
                });
            });

        // Closing with the title-bar button should also forget a half-finished
        // delete, so reopening does not present a stale confirmation.
        #[cfg(target_os = "macos")]
        if was_open && !*open {
            *pending_delete = None;
        }
    }

    /// What this build is and where it puts things.
    fn about_panel(ui: &mut egui::Ui) {
        ui.heading("About");
        ui.label(
            egui::RichText::new("What this build is, and where it keeps your data.")
                .small()
                .weak(),
        );
        ui.add_space(8.0);

        // The engine version is servoshell's own crate version, which is the
        // honest answer for "what is rendering this page". The bundle's
        // marketing version lives in Info.plist and is not visible from here.
        let engine_version = env!("CARGO_PKG_VERSION");
        let user_agent = servo::prefs::get().user_agent.clone();
        let profile_directory = default_config_dir()
            .map(|path| abbreviate_home(&path.display().to_string()))
            .unwrap_or_else(|| "Unavailable".to_owned());

        egui::Grid::new("settings-about-grid")
            .num_columns(2)
            .min_col_width(LABEL_COLUMN_WIDTH)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                Self::row(ui, "Application", "smbCloud Browser");
                Self::row(ui, "Engine", &format!("Servo {engine_version}"));
                Self::row(ui, "User agent", &user_agent);
                Self::row(ui, "Profile folder", &profile_directory);
            });
    }

    /// The parts of the browser's own chrome the user can choose.
    fn appearance_panel(ui: &mut egui::Ui, stored: &mut StoredSettings) {
        ui.heading("Appearance");
        ui.add_space(8.0);

        egui::Grid::new("settings-appearance-grid")
            .num_columns(2)
            .min_col_width(LABEL_COLUMN_WIDTH)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Loading bar").small().weak());
                ui.horizontal(|ui| {
                    // The picker edits the stored bytes directly, so the bar
                    // follows the pointer while the user drags. Only a finished
                    // edit reaches the disk -- writing a file per frame of a
                    // drag would be hundreds of writes for one colour.
                    let mut color = stored.progress_bar_color;
                    if ui.color_edit_button_srgb(&mut color).changed() {
                        stored.progress_bar_color = color;
                    }
                    if ui.button("Reset").clicked() {
                        stored.progress_bar_color = DEFAULT_PROGRESS_BAR_COLOR;
                        stored.save();
                    }
                });
                ui.end_row();
            });

        // `color_edit_button_srgb` reports `changed` throughout a drag, so the
        // save is hung off the popup closing instead.
        if ui.ctx().input(|input| input.pointer.any_released()) {
            stored.save();
        }
    }

    /// The features this build turns on that upstream Servo still keeps behind
    /// a preference. Worth surfacing: it is the difference between a page
    /// working here and the same page working in servoshell.
    fn web_platform_panel(ui: &mut egui::Ui) {
        ui.heading("Web platform");
        ui.label(
            egui::RichText::new(
                "Enabled in every window, rather than left behind the experimental toggle.",
            )
            .small()
            .weak(),
        );
        ui.add_space(8.0);

        for pref in SMB_DAILY_WEB_PREFS {
            ui.label(format!("• {}", Self::humanize_pref(pref)));
        }
    }

    /// Turns a preference path into something readable. These are the five
    /// names in [`SMB_DAILY_WEB_PREFS`]; anything unrecognised falls back to
    /// the raw path rather than guessing at a title.
    fn humanize_pref(pref: &str) -> &str {
        match pref {
            "dom_adoptedstylesheet_enabled" => "Adopted style sheets",
            "dom_fontface_enabled" => "FontFace",
            "dom_indexeddb_enabled" => "IndexedDB",
            "dom_intersection_observer_enabled" => "IntersectionObserver",
            "layout_container_queries_enabled" => "CSS container queries",
            other => other,
        }
    }

    fn row(ui: &mut egui::Ui, label: &str, value: &str) {
        ui.label(egui::RichText::new(label).small().weak());
        ui.add(egui::Label::new(value).wrap());
        ui.end_row();
    }

    /// The on-device models: which one the chat panel talks to, which are
    /// downloaded, and how much disk they take.
    #[cfg(target_os = "macos")]
    fn models_panel(
        ui: &mut egui::Ui,
        inference: &Inference,
        pending_delete: &mut Option<String>,
    ) {
        ui.heading("On-device models");
        ui.label(
            egui::RichText::new(
                "Models run on this Mac. Downloading one uses the network; chatting with it \
                 does not.",
            )
            .small()
            .weak(),
        );

        ui.add_space(8.0);

        let catalog = inference.catalog();
        ui.horizontal(|ui| {
            if ui.button("Rescan").clicked() {
                inference.refresh_catalog();
            }
            if catalog.scanned {
                ui.label(
                    egui::RichText::new(format!("{} in use", catalog.total_size_display))
                        .small()
                        .weak(),
                );
            }
        });
        if catalog.scanned && !catalog.cache_path.is_empty() {
            ui.label(
                egui::RichText::new(catalog.cache_path.clone())
                    .small()
                    .weak(),
            );
        }

        if let Some(error) = inference.last_error() {
            ui.add_space(4.0);
            ui.colored_label(ui.visuals().error_fg_color, error);
        }

        ui.add_space(8.0);
        ui.separator();

        if !catalog.scanned {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the model cache…");
            });
            return;
        }

        let busy = inference.status().is_busy();
        let loaded = inference.loaded_model_id();
        let selected = inference.selected_model_id();
        let mut requested: Option<String> = None;
        let mut deleted: Option<String> = None;

        for model in catalog.models.iter() {
            let is_loaded = loaded.as_deref() == Some(model.model_id.as_str());
            let is_selected = selected == model.model_id;

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.strong(&model.name);
                if is_loaded {
                    ui.label(egui::RichText::new("In use").small().strong());
                } else if is_selected {
                    ui.label(egui::RichText::new("Selected").small().weak());
                }
            });
            ui.label(
                egui::RichText::new(format!("{} · {}", model.org, model.description))
                    .small()
                    .weak(),
            );

            // The in-flight download or load of this model, if any. Every
            // other model shows what it costs on disk instead.
            let mut showed_progress = false;
            if is_selected {
                match &*inference.status() {
                    Status::Downloading { fraction, detail } => {
                        ui.add(egui::ProgressBar::new(*fraction).text(detail.clone()));
                        showed_progress = true;
                    },
                    Status::Loading { .. } => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new("Loading…").small().weak());
                        });
                        showed_progress = true;
                    },
                    Status::Failed(error) => {
                        ui.colored_label(ui.visuals().error_fg_color, error.clone());
                    },
                    Status::Idle | Status::Ready { .. } => {},
                }
            }

            if !showed_progress {
                ui.horizontal(|ui| {
                    if model.is_downloaded {
                        ui.label(
                            egui::RichText::new(format!(
                                "Downloaded · {}",
                                model.local_size_display
                            ))
                            .small()
                            .weak(),
                        );
                    } else if model.is_incomplete {
                        ui.label(
                            egui::RichText::new(format!(
                                "Partly downloaded · {} of {}",
                                model.local_size_display, model.expected_size_display
                            ))
                            .small()
                            .weak(),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new(format!(
                                "Not downloaded · {}",
                                model.expected_size_display
                            ))
                            .small()
                            .weak(),
                        );
                    }
                });

                ui.horizontal(|ui| {
                    let action = if model.is_downloaded {
                        "Use"
                    } else if model.is_incomplete {
                        "Resume download"
                    } else {
                        "Download"
                    };
                    let can_act = !busy && !is_loaded;
                    if ui
                        .add_enabled(can_act, egui::Button::new(action))
                        .on_disabled_hover_text(if is_loaded {
                            "Already loaded"
                        } else {
                            "Another model is downloading or loading"
                        })
                        .clicked()
                    {
                        requested = Some(model.model_id.clone());
                    }

                    let on_disk = model.is_downloaded || model.is_incomplete;
                    if pending_delete.as_deref() == Some(model.model_id.as_str()) {
                        ui.label(
                            egui::RichText::new(format!(
                                "Delete {}?",
                                model.local_size_display
                            ))
                            .small(),
                        );
                        if ui.button("Delete").clicked() {
                            deleted = Some(model.model_id.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            *pending_delete = None;
                        }
                    } else if ui
                        .add_enabled(on_disk && !busy, egui::Button::new("Delete"))
                        .clicked()
                    {
                        *pending_delete = Some(model.model_id.clone());
                    }
                });
            }

            ui.add_space(6.0);
            ui.separator();
        }

        // Both calls take the catalog lock, so the guard has to go first.
        drop(catalog);
        if let Some(model_id) = requested {
            inference.use_model(&model_id);
        }
        if let Some(model_id) = deleted {
            *pending_delete = None;
            inference.delete_model(&model_id);
        }
    }
}
