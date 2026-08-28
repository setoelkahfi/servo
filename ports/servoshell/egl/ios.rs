/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::NonNull;
use std::rc::Rc;

use euclid::{Rect, Scale, Size2D};
use log::error;
use raw_window_handle::{DisplayHandle, RawWindowHandle, UiKitWindowHandle, WindowHandle};
use servo::{
    DevicePixel, EventLoopWaker, InputMethodControl, LoadStatus, MediaSessionPlaybackState,
    Preferences, SelectElement, WebViewId,
};

use super::app::{App, AppInitOptions};
use super::host_trait::HostTrait;
use crate::prefs::ServoShellPreferences;
use crate::{init_crypto, init_tracing};

type WakeCallback = extern "C" fn(*mut c_void);

#[cfg(feature = "bundled")]
unsafe extern "C" {
    fn servo_force_link_default_resources();
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy)]
struct WakeupCallback {
    callback: Option<WakeCallback>,
    context: usize,
}

impl EventLoopWaker for WakeupCallback {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(*self)
    }

    fn wake(&self) {
        if let Some(callback) = self.callback {
            callback(self.context as *mut c_void);
        }
    }
}

struct HostCallbacks;

impl HostTrait for HostCallbacks {
    fn show_alert(&self, message: String) {
        log::warn!("Ignoring alert on iOS: {message}");
    }

    fn notify_load_status_changed(&self, _load_status: LoadStatus) {}
    fn on_title_changed(&self, _title: Option<String>) {}
    fn on_url_changed(&self, _url: String) {}
    fn on_history_changed(&self, _can_go_back: bool, _can_go_forward: bool) {}
    fn on_shutdown_complete(&self) {}
    fn on_ime_show(&self, _input_method_control: InputMethodControl) {}
    fn on_ime_hide(&self) {}
    fn on_media_session_metadata(&self, _title: String, _artist: String, _album: String) {}
    fn on_media_session_playback_state_change(&self, _state: MediaSessionPlaybackState) {}
    fn on_media_session_set_position_state(
        &self,
        _duration: f64,
        _position: f64,
        _playback_rate: f64,
    ) {
    }
    fn on_show_select_element(&self, _webview_id: WebViewId, _prompt: SelectElement) {}
    fn on_panic(&self, reason: String, backtrace: Option<String>) {
        error!("Servo panic: {reason}");
        if let Some(backtrace) = backtrace {
            error!("{backtrace}");
        }
    }
}

fn c_string(value: *const c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }

    unsafe { CStr::from_ptr(value) }
        .to_str()
        .ok()
        .map(ToOwned::to_owned)
}

fn viewport_rect(width: i32, height: i32) -> Rect<i32, DevicePixel> {
    Rect::new(Default::default(), Size2D::new(width.max(1), height.max(1)))
}

fn window_handle(view: *mut c_void) -> Option<RawWindowHandle> {
    NonNull::new(view).map(|view| RawWindowHandle::UiKit(UiKitWindowHandle::new(view)))
}

#[unsafe(no_mangle)]
pub extern "C" fn servoshell_ios_init(
    view: *mut c_void,
    width: i32,
    height: i32,
    scale: f32,
    url: *const c_char,
    wake_callback: Option<WakeCallback>,
    wake_context: *mut c_void,
) -> bool {
    let Some(raw_window_handle) = window_handle(view) else {
        return false;
    };

    // Rust static libraries are linked as native archives. Keep the baked-in
    // resource reader's constructor-bearing member in the final iOS binary.
    #[cfg(feature = "bundled")]
    unsafe {
        servo_force_link_default_resources();
    }

    init_crypto();
    init_tracing(None);

    // The BrowserEngineKit JIT entitlement has not been granted to this app.
    // SpiderMonkey crashes when it attempts executable mappings without that
    // entitlement, so the iOS bootstrap must use its interpreter-only path.
    let mut preferences = Preferences::default();
    preferences.js_disable_jit = true;

    let app = App::new(AppInitOptions {
        host: Rc::new(HostCallbacks),
        event_loop_waker: Box::new(WakeupCallback {
            callback: wake_callback,
            context: wake_context as usize,
        }),
        initial_url: c_string(url),
        opts: Default::default(),
        preferences,
        servoshell_preferences: ServoShellPreferences {
            homepage: "about:blank".to_owned(),
            ..Default::default()
        },
        #[cfg(feature = "webxr")]
        xr_discovery: None,
    });

    let window_handle = unsafe { WindowHandle::borrow_raw(raw_window_handle) };
    app.add_platform_window(
        DisplayHandle::uikit(),
        window_handle,
        viewport_rect(width, height),
        Scale::new(scale.max(1.0)),
        None,
    );
    app.spin_event_loop();

    APP.with(|slot| {
        *slot.borrow_mut() = Some(app);
    });
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn servoshell_ios_spin_event_loop() {
    APP.with(|slot| {
        if let Some(app) = slot.borrow().as_ref() {
            app.spin_event_loop();
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn servoshell_ios_notify_vsync() {
    APP.with(|slot| {
        if let Some(app) = slot.borrow().as_ref() {
            // The embedded window uses VsyncRefreshDriver, so merely draining
            // Servo's event queue does not make a frame eligible to paint.
            // UIKit's CADisplayLink is the platform vsync source.
            app.notify_vsync();
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn servoshell_ios_resize(width: i32, height: i32, _scale: f32) {
    APP.with(|slot| {
        if let Some(app) = slot.borrow().as_ref() {
            app.resize(viewport_rect(width, height));
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn servoshell_ios_load_url(url: *const c_char) {
    let Some(url) = c_string(url) else {
        return;
    };
    APP.with(|slot| {
        if let Some(app) = slot.borrow().as_ref() {
            app.load_uri(&url);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn servoshell_ios_touch_down(x: f32, y: f32, pointer_id: i32) {
    APP.with(|slot| {
        if let Some(app) = slot.borrow().as_ref() {
            app.touch_down(x, y, pointer_id);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn servoshell_ios_touch_move(x: f32, y: f32, pointer_id: i32) {
    APP.with(|slot| {
        if let Some(app) = slot.borrow().as_ref() {
            app.touch_move(x, y, pointer_id);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn servoshell_ios_touch_up(x: f32, y: f32, pointer_id: i32) {
    APP.with(|slot| {
        if let Some(app) = slot.borrow().as_ref() {
            app.touch_up(x, y, pointer_id);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn servoshell_ios_touch_cancel(x: f32, y: f32, pointer_id: i32) {
    APP.with(|slot| {
        if let Some(app) = slot.borrow().as_ref() {
            app.touch_cancel(x, y, pointer_id);
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn servoshell_ios_shutdown() {
    APP.with(|slot| {
        if let Some(app) = slot.borrow_mut().take() {
            app.state.schedule_exit();
            app.spin_event_loop();
        }
    });
}
