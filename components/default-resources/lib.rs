/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::path::PathBuf;

use embedder_traits::resources::{Resource, ResourceReaderMethods};

/// A default resource reader that provides baked in resources.
pub struct DefaultResourceReader;

impl ResourceReaderMethods for DefaultResourceReader {
    fn sandbox_access_files(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn sandbox_access_files_dirs(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn read(&self, file: Resource) -> Vec<u8> {
        match file {
            Resource::BluetoothBlocklist => &include_bytes!("resources/gatt_blocklist.txt")[..],
            Resource::DomainList => &include_bytes!("resources/public_domains.txt")[..],
            Resource::HstsPreloadList => &include_bytes!("resources/hsts_preload.fstmap")[..],
            Resource::BadCertHTML => &include_bytes!("resources/badcert.html")[..],
            Resource::NetErrorHTML => &include_bytes!("resources/neterror.html")[..],
            Resource::BrokenImageIcon => &include_bytes!("resources/rippy.png")[..],
            Resource::CrashHTML => &include_bytes!("resources/crash.html")[..],
            Resource::DirectoryListingHTML => {
                &include_bytes!("resources/directory-listing.html")[..]
            },
            Resource::AboutMemoryHTML => &include_bytes!("resources/about-memory.html")[..],
            Resource::DebuggerJS => &include_bytes!("resources/debugger.js")[..],
            Resource::JsonViewerHTML => &include_bytes!("resources/json-viewer.html")[..],
        }
        .to_owned()
    }
}

embedder_traits::submit_resource_reader!(&DefaultResourceReader);

/// Keeps this crate's inventory constructor in native static-library embeds.
///
/// A constructor-only archive member is not selected by Apple's linker during
/// the final application link. Embedders that expose Servo through a Rust
/// `staticlib` can call this symbol to retain the member without force-loading
/// the entire archive (which can introduce duplicate bundled C symbols).
#[unsafe(no_mangle)]
pub extern "C" fn servo_force_link_default_resources() {}
