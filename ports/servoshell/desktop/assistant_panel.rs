/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The assistant side panel: one conversation, answered on this Mac or in the
//! cloud. State lives in [`Inference`] (device) and [`Cloud`]; this file only
//! draws it and routes a send to the right brain.
//!
//! Reads state every frame rather than subscribing to it. Both brains publish
//! through mutexes held only for a field read or write, so polling costs less
//! than routing events back through the winit loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use onde::inference::ChatRole;
use servo::{JSValue, WebView};

use super::assistant::{
    Brain, Cloud, PAGE_BUDGET_CLOUD, PAGE_BUDGET_DEVICE, PendingPage, choose_brain, page_block,
};
use super::inference::{Inference, Status};
use super::settings::Settings;

/// Visible text of the page, which is all the assistant ever gets of it.
const PAGE_TEXT_SCRIPT: &str = "document.body ? document.body.innerText : ''";

fn send(inference: &Inference, cloud: &Cloud, brain: Brain, message: String, page: Option<String>) {
    match brain {
        Brain::Cloud => cloud.send(message, page),
        Brain::Device => match page {
            Some(block) => {
                inference.send_with_context(message.clone(), format!("{block}\n\n{message}"))
            },
            None => inference.send(message),
        },
    }
}

/// Ask the page for its text; the answer lands in `pending` and the next frame
/// sends the message.
fn start_page_read(cloud: &mut Cloud, webview: &WebView, message: String, brain: Brain) {
    let ready = Arc::new(AtomicBool::new(false));
    let text = Arc::new(Mutex::new(None));
    let title = webview.page_title().unwrap_or_default();
    let url = webview.url().map(|url| url.to_string()).unwrap_or_default();
    let (sink, flag) = (Arc::clone(&text), Arc::clone(&ready));
    webview.evaluate_javascript(PAGE_TEXT_SCRIPT, move |result| {
        let page = match result {
            Ok(JSValue::String(text)) => text,
            _ => String::new(),
        };
        *sink.lock().unwrap() = Some((title, url, page));
        flag.store(true, Ordering::SeqCst);
    });
    cloud.pending_page = Some(PendingPage {
        message,
        brain,
        ready,
        text,
    });
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    inference: &mut Inference,
    cloud: &mut Cloud,
    settings: &mut Settings,
    webview: Option<WebView>,
) {
    // A page read that finished: attach it and send.
    if let Some(pending) = cloud.pending_page.take() {
        if pending.ready.load(Ordering::SeqCst) {
            let read = pending.text.lock().unwrap().take();
            let budget = match pending.brain {
                Brain::Cloud => PAGE_BUDGET_CLOUD,
                Brain::Device => PAGE_BUDGET_DEVICE,
            };
            let block = read
                .filter(|(_, _, text)| !text.trim().is_empty())
                .map(|(title, url, text)| page_block(&title, &url, &text, budget));
            send(inference, cloud, pending.brain, pending.message, block);
        } else {
            cloud.pending_page = Some(pending);
            // Keep frames coming until the page answers.
            ui.ctx().request_repaint();
        }
    }
    if let Some(notice) = cloud.take_notice() {
        cloud.shown_notice = Some(notice);
    }

    let device_ready = inference.status().is_ready();
    let active = choose_brain(cloud.preferred(), device_ready, cloud.is_ready());

    ui.horizontal(|ui| {
        ui.heading("Assistant");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Clear").clicked() {
                inference.clear();
            }
            if ui.button("Models…").on_hover_text("Manage on-device models").clicked() {
                settings.open(inference);
            }
        });
    });

    // Which brain is preferred. The active one can differ while the preferred
    // one is not ready, which the privacy line below says out loud.
    ui.horizontal(|ui| {
        let preferred = cloud.preferred();
        if ui.selectable_label(preferred == Brain::Device, "On this Mac").clicked() {
            cloud.set_preferred(Brain::Device);
        }
        if ui.selectable_label(preferred == Brain::Cloud, "Cloud").clicked() {
            cloud.set_preferred(Brain::Cloud);
        }
    });
    ui.label(
        egui::RichText::new(match active {
            Brain::Device => "Answering on this Mac. Nothing is sent to a server.",
            Brain::Cloud => "Answering in the cloud through smbCloud. Your messages are sent.",
        })
        .small()
        .weak(),
    );
    ui.separator();

    if cloud.preferred() == Brain::Cloud || active == Brain::Cloud {
        cloud_section(ui, cloud);
        ui.separator();
    }

    let ready = match active {
        Brain::Cloud => true,
        Brain::Device => device_section(ui, inference),
    };
    if active == Brain::Device {
        ui.separator();
    }

    // Transcript. Reserve room for the input rows so a long conversation does
    // not push them off the bottom of the panel.
    let input_height = 92.0;
    let transcript_height = (ui.available_height() - input_height).max(0.0);
    egui::ScrollArea::vertical()
        .max_height(transcript_height)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for turn in inference.transcript().iter() {
                let (name, color) = match (&turn.role, turn.is_error) {
                    (_, true) => ("Error", ui.visuals().error_fg_color),
                    (ChatRole::User, _) => ("You", ui.visuals().strong_text_color()),
                    _ => ("Assistant", ui.visuals().text_color()),
                };
                let label = match (&turn.role, turn.brain) {
                    (ChatRole::Assistant, Brain::Cloud) => format!("{name} · cloud"),
                    (ChatRole::Assistant, Brain::Device) => format!("{name} · this Mac"),
                    _ => name.to_string(),
                };
                ui.label(egui::RichText::new(label).small().strong());
                ui.add(egui::Label::new(egui::RichText::new(&turn.text).color(color)).wrap());
                ui.add_space(6.0);
            }
            if inference.is_generating() || cloud.is_busy() || cloud.pending_page.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("Thinking…").small().weak());
                });
            }
        });

    if let Some(notice) = &cloud.shown_notice {
        ui.colored_label(ui.visuals().error_fg_color, notice.clone());
    }

    // Input rows, pinned to the bottom.
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        let busy = inference.is_generating() || cloud.is_busy() || cloud.pending_page.is_some();
        let can_send = ready && !busy;
        let hint = match (active, ready) {
            (Brain::Device, false) => "Load a model, or use Cloud",
            (Brain::Cloud, _) if !cloud.is_ready() => "Sign in and agree to use Cloud",
            _ => "Ask something",
        };
        ui.horizontal(|ui| {
            let send_button = ui.add_enabled(can_send, egui::Button::new("Send"));
            let field = ui.add_enabled(
                can_send,
                egui::TextEdit::singleline(&mut cloud.draft)
                    .desired_width(ui.available_width())
                    .hint_text(hint),
            );
            let submitted = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (send_button.clicked() || submitted) && !cloud.draft.trim().is_empty() {
                let message = std::mem::take(&mut cloud.draft);
                cloud.shown_notice = None;
                match (&webview, cloud.include_page) {
                    (Some(webview), true) => start_page_read(cloud, webview, message, active),
                    _ => send(inference, cloud, active, message, None),
                }
                field.request_focus();
            }
        });
        ui.add_enabled(
            webview.is_some(),
            egui::Checkbox::new(&mut cloud.include_page, "Include this page in the next message"),
        )
        .on_hover_text("Sends the page's title, address and visible text with your message.");
    });
}

/// Sign-in and consent for the cloud brain.
fn cloud_section(ui: &mut egui::Ui, cloud: &mut Cloud) {
    if cloud.is_signed_in() {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Signed in as {}",
                    cloud.email().unwrap_or_else(|| "your smbCloud account".into())
                ))
                .small(),
            );
            if ui.small_button("Sign out").clicked() {
                cloud.sign_out();
            }
        });
    } else if cloud.sign_in_available() {
        ui.label(egui::RichText::new("Sign in with your smbCloud account").small().strong());
        ui.add(
            egui::TextEdit::singleline(&mut cloud.email_draft)
                .hint_text("Email")
                .desired_width(f32::INFINITY),
        );
        let password = ui.add(
            egui::TextEdit::singleline(&mut cloud.password_draft)
                .password(true)
                .hint_text("Password")
                .desired_width(f32::INFINITY),
        );
        let signing_in = cloud.signing_in.load(Ordering::Relaxed);
        let submit = ui.add_enabled(!signing_in, egui::Button::new("Sign in")).clicked() ||
            (password.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if submit {
            cloud.shown_notice = None;
            cloud.sign_in();
        }
        if signing_in {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Signing in…").small().weak());
            });
            ui.ctx().request_repaint();
        }
    } else {
        ui.label(
            egui::RichText::new("Cloud sign-in is not set up in this build.")
                .small()
                .weak(),
        );
    }

    let mut consent = cloud.consented();
    if ui
        .checkbox(&mut consent, "Send my messages to the cloud assistant")
        .on_hover_text(
            "Your message and the last few turns of this chat are sent to smbCloud, which \
             forwards them to a model provider. A page is sent only when you tick \
             \"Include this page\".",
        )
        .changed()
    {
        cloud.set_consent(consent);
    }
}

/// The on-device model's status and its load button. Returns whether it can
/// answer now.
fn device_section(ui: &mut egui::Ui, inference: &mut Inference) -> bool {
    let status = inference.status();
    match &*status {
        Status::Idle => {
            drop(status);
            let model = inference.selected_model_name();
            if ui.button(format!("Load {model}")).clicked() {
                inference.ensure_model();
            }
            ui.label(
                egui::RichText::new(
                    "Downloads a few gigabytes the first time. Choose a different model in \
                     Settings.",
                )
                .small()
                .weak(),
            );
            false
        },
        Status::Downloading { fraction, detail } => {
            ui.add(egui::ProgressBar::new(*fraction).text(detail.clone()));
            false
        },
        Status::Loading { model } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Loading {model}…"));
            });
            false
        },
        Status::Ready { model } => {
            ui.label(egui::RichText::new(model.clone()).small().weak());
            true
        },
        Status::Failed(error) => {
            ui.colored_label(ui.visuals().error_fg_color, error.clone());
            drop(status);
            if ui.button("Try again").clicked() {
                inference.ensure_model();
            }
            false
        },
    }
}
