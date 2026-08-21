/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! smb Browser's additions to the macOS application menu.
//!
//! winit builds the default menubar during launch — About, Services, Hide,
//! Quit — and offers no way to extend it, so the Settings item is inserted
//! into the finished menu instead. That has to happen after launch and on the
//! main thread, which [`install_settings_item`] enforces through
//! [`MainThreadMarker`].
//!
//! AppKit delivers the click on the main thread while winit is inside its own
//! run loop, so the handler cannot touch the event loop or a window. It sets a
//! flag, and the GUI picks it up on its next frame through
//! [`take_settings_request`].

use std::cell::OnceCell;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use log::warn;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSMenuItem};
use objc2_foundation::{MainThreadMarker, ns_string};

/// Set when Settings is chosen, cleared when the GUI acts on it.
static SETTINGS_REQUESTED: AtomicBool = AtomicBool::new(false);

static INSTALLED: Once = Once::new();

thread_local! {
    /// `NSMenuItem`'s target is a weak reference, so the object that answers
    /// the action has to outlive the menu. It is created once on the main
    /// thread and kept here for the life of the process.
    static MENU_TARGET: OnceCell<Retained<MenuTarget>> = const { OnceCell::new() };
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `MenuTarget` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SMBBrowserMenuTarget"]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(smbOpenSettings:))]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            SETTINGS_REQUESTED.store(true, Ordering::Relaxed);
        }
    }
);

/// Add "Settings…" (⌘,) to the application menu, in the place macOS apps put
/// it: below About, above Services.
///
/// Does nothing if called more than once, or before winit has built the menu.
pub fn install_settings_item() {
    let Some(mtm) = MainThreadMarker::new() else {
        warn!("The Settings menu item can only be installed from the main thread.");
        return;
    };

    INSTALLED.call_once(|| {
        let app = NSApplication::sharedApplication(mtm);
        let Some(app_menu) = app
            .mainMenu()
            .and_then(|menubar| menubar.itemAtIndex(0))
            .and_then(|first_item| first_item.submenu())
        else {
            warn!("No application menu to add Settings to.");
            return;
        };

        let target = MENU_TARGET.with(|cell| {
            // SAFETY: `MenuTarget` inherits NSObject's designated initializer
            // and has no instance variables to set up.
            cell.get_or_init(|| unsafe { msg_send![MenuTarget::alloc(mtm), init] })
                .clone()
        });

        // SAFETY: `smbOpenSettings:` is defined above with the signature
        // AppKit invokes for a menu action, and `target` is kept alive for the
        // life of the process by `MENU_TARGET`.
        unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                ns_string!("Settings…"),
                Some(sel!(smbOpenSettings:)),
                ns_string!(","),
            );
            item.setTarget(Some(&target));

            // Index 1 is the separator winit put after About, so the separator
            // goes first and Settings lands between the two.
            app_menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), 1);
            app_menu.insertItem_atIndex(&item, 2);
        }
    });
}

/// Whether Settings was chosen since the last time this was asked.
///
/// Consumes the request, so the first caller wins and the window opens once.
pub fn take_settings_request() -> bool {
    SETTINGS_REQUESTED.swap(false, Ordering::Relaxed)
}
