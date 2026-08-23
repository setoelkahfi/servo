/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The Settings window, opened from the application menu (⌘,).
//!
//! Everything here is read-only. The window reports what this build is, where
//! it keeps your data, and which web features it turns on that upstream Servo
//! still gates. Nothing in it writes a preference yet.
//!
//! Each panel is its own function taking `&mut egui::Ui`, and [`Settings::show`]
//! calls them in order. A new panel is a function plus one call, which is what
//! keeps this file mergeable while the model catalog lives on another branch.

use crate::prefs::{SMB_DAILY_WEB_PREFS, default_config_dir};

/// Width of the label column, wide enough for the longest label below without
/// letting a long value push the columns apart.
const LABEL_COLUMN_WIDTH: f32 = 140.0;

#[derive(Default)]
pub(crate) struct Settings {
    open: bool,
}

impl Settings {
    pub(crate) fn open(&mut self) {
        self.open = true;
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }

        // `Window::open` needs its own borrow, so the rest of the state is
        // taken apart first.
        let Self { open } = self;

        egui::Window::new("Settings")
            .open(open)
            .collapsible(false)
            .default_size([560.0, 480.0])
            .min_width(420.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    Self::about_panel(ui);
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(12.0);
                    Self::web_platform_panel(ui);
                });
            });
    }

    /// What this build is and where it puts things.
    fn about_panel(ui: &mut egui::Ui) {
        ui.heading("About");
        ui.label(
            egui::RichText::new("Read-only for now. Nothing on this page changes a setting.")
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
            .map(|path| path.display().to_string())
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
}
