/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The Settings window, opened from the application menu (⌘,).
//!
//! Today it manages the on-device models: which one the chat panel talks to,
//! which are downloaded, and how much disk they take. Weights are gigabytes
//! each and are shared with the other Onde apps on this Mac, so deleting one
//! is a real decision and gets a confirmation step.

use crate::desktop::inference::{Inference, Status};

#[derive(Default)]
pub(crate) struct Settings {
    open: bool,
    /// The model a delete has been asked for but not yet confirmed.
    pending_delete: Option<String>,
}

impl Settings {
    /// Show the window, scanning the model cache on the way in so the list is
    /// current without polling the filesystem every frame.
    pub(crate) fn open(&mut self, inference: &Inference) {
        if !self.open {
            inference.refresh_catalog();
        }
        self.open = true;
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context, inference: &Inference) {
        if !self.open {
            return;
        }

        // `Window::open` needs its own borrow, so the rest of the state is
        // taken apart first.
        let Self {
            open,
            pending_delete,
        } = self;

        let was_open = *open;
        egui::Window::new("Settings")
            .open(open)
            .collapsible(false)
            .default_size([560.0, 480.0])
            .min_width(420.0)
            .show(ctx, |ui| {
                Self::models_section(ui, inference, pending_delete);
            });

        // Closing with the title-bar button should also forget a half-finished
        // delete, so reopening does not present a stale confirmation.
        if was_open && !*open {
            *pending_delete = None;
        }
    }

    fn models_section(
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

        egui::ScrollArea::vertical().show(ui, |ui| {
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
        });

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
