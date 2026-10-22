// SPDX-License-Identifier: MPL-2.0

use std::ffi::CString;
use std::path::Path;
use std::sync::Mutex;

use esp_idf_svc::sys::EspError;
use panelet_widget_store::web_assets::{WebAssetError, WebAssetStore};
use panelet_widget_store::{StoreError, WidgetStore};

const PARTITION_LABEL: &str = "assets";
const MOUNT_PATH: &str = "/assets";
const MAX_OPEN_FILES: usize = 8;
// Resource compatibility is independent of the firmware version/OTA slot.
// Compatible firmware releases share this version; incompatible releases
// select a new version while retaining the old assets for rollback.
include!(concat!(env!("OUT_DIR"), "/web_assets_version.rs"));

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct StorageCapacity {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    /// Keep 25% of SPIFFS capacity available for GC and other persistent data.
    pub safe_available_bytes: u64,
}

pub struct AssetStorage {
    state: Mutex<State>,
}

enum State {
    Unmounted,
    Mounted {
        _mount: MountedAssets,
        store: Option<WidgetStore>,
        web: Option<WebAssetStore>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageStatus {
    Ready,
    NeedsInitialization,
    MountFailed,
}

impl AssetStorage {
    pub fn probe() -> Self {
        let state = match mount() {
            Ok(mounted) => {
                let store = WidgetStore::open(MOUNT_PATH).ok();
                let web = if store.is_some() {
                    open_web_assets()
                } else {
                    None
                };
                State::Mounted {
                    _mount: mounted,
                    store,
                    web,
                }
            }
            Err(error) => {
                log::warn!("assets SPIFFS mount failed without formatting: {}", error);
                State::Unmounted
            }
        };
        Self {
            state: Mutex::new(state),
        }
    }

    pub fn status(&self) -> StorageStatus {
        match self.state.lock() {
            Ok(state) => match &*state {
                State::Mounted { store: Some(_), .. } => StorageStatus::Ready,
                State::Mounted { store: None, .. } => StorageStatus::NeedsInitialization,
                State::Unmounted => StorageStatus::MountFailed,
            },
            Err(_) => StorageStatus::MountFailed,
        }
    }

    /// Destructive and intentionally separate from `probe`: callers must
    /// obtain an exact user confirmation before invoking this method.
    pub fn initialize(&self) -> Result<(), StoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| StoreError::Corrupt("storage lock poisoned"))?;
        let previous = std::mem::replace(&mut *state, State::Unmounted);
        match previous {
            mounted @ State::Mounted { store: Some(_), .. } => {
                *state = mounted;
                return Err(StoreError::AlreadyInitialized);
            }
            State::Mounted {
                _mount: mounted,
                store: None,
                ..
            } => match mounted.unmount() {
                Ok(()) => {}
                Err((mounted, error)) => {
                    *state = State::Mounted {
                        _mount: mounted,
                        store: None,
                        web: None,
                    };
                    return Err(esp_io(error));
                }
            },
            State::Unmounted => {}
        }

        let label =
            CString::new(PARTITION_LABEL).map_err(|_| StoreError::Corrupt("partition label"))?;
        // SAFETY: the previous mount was dropped above. ESP-IDF creates and
        // frees temporary SPIFFS state when formatting an unmounted partition.
        esp_idf_svc::sys::esp!(unsafe { esp_idf_svc::sys::esp_spiffs_format(label.as_ptr()) })
            .map_err(esp_io)?;
        let mounted = mount().map_err(esp_io)?;
        // SPIFFS is already mounted at MOUNT_PATH. Its VFS intentionally does
        // not implement mkdir, so never try to create the mount point here.
        let store = WidgetStore::initialize_mounted(MOUNT_PATH)?;
        *state = State::Mounted {
            _mount: mounted,
            store: Some(store),
            web: open_web_assets(),
        };
        Ok(())
    }

    pub fn with_store<T>(
        &self,
        f: impl FnOnce(&WidgetStore) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let state = self
            .state
            .lock()
            .map_err(|_| StoreError::Corrupt("storage lock poisoned"))?;
        match &*state {
            State::Mounted {
                store: Some(store), ..
            } => f(store),
            _ => Err(StoreError::NotInitialized),
        }
    }

    pub fn with_web_assets<T>(
        &self,
        f: impl FnOnce(&mut WebAssetStore) -> Result<T, WebAssetError>,
    ) -> Result<T, WebAssetError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| WebAssetError::Io(std::io::Error::other("assets lock poisoned")))?;
        match &mut *state {
            State::Mounted {
                store: Some(_),
                web: Some(web),
                ..
            } => f(web),
            _ => Err(WebAssetError::Invalid(
                "assets not initialized or unavailable",
            )),
        }
    }

    pub fn capacity(&self) -> Result<StorageCapacity, StoreError> {
        let state = self
            .state
            .lock()
            .map_err(|_| StoreError::Corrupt("storage lock poisoned"))?;
        if !matches!(&*state, State::Mounted { .. }) {
            return Err(StoreError::NotInitialized);
        }
        mounted_capacity()
    }

    /// Query capacity and mutate resources under the same lock as all widget,
    /// private and shared writes, which cannot spend a stale resource budget.
    pub fn with_web_asset_budget<T>(
        &self,
        collect: bool,
        f: impl FnOnce(&mut WebAssetStore, u64) -> Result<T, WebAssetError>,
    ) -> Result<T, WebAssetError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| WebAssetError::Io(std::io::Error::other("assets lock poisoned")))?;
        match &mut *state {
            State::Mounted {
                store: Some(_),
                web: Some(web),
                ..
            } => {
                if collect {
                    web.garbage_collect()?;
                }
                let capacity = mounted_capacity().map_err(|_| {
                    WebAssetError::Io(std::io::Error::other("storage capacity unavailable"))
                })?;
                f(web, capacity.safe_available_bytes)
            }
            _ => Err(WebAssetError::Invalid(
                "assets not initialized or unavailable",
            )),
        }
    }
}

/// Called only while AssetStorage owns the mount and holds its state lock.
fn mounted_capacity() -> Result<StorageCapacity, StoreError> {
    let label =
        CString::new(PARTITION_LABEL).map_err(|_| StoreError::Corrupt("partition label"))?;
    let (mut total, mut used) = (0usize, 0usize);
    // SAFETY: the mount remains owned/locked during this read-only query.
    esp_idf_svc::sys::esp!(unsafe {
        esp_idf_svc::sys::esp_spiffs_info(label.as_ptr(), &mut total, &mut used)
    })
    .map_err(esp_io)?;
    Ok(StorageCapacity {
        total_bytes: total as u64,
        used_bytes: used as u64,
        free_bytes: total.saturating_sub(used) as u64,
        safe_available_bytes: (total / 4 * 3).saturating_sub(used) as u64,
    })
}

fn open_web_assets() -> Option<WebAssetStore> {
    match WebAssetStore::open(MOUNT_PATH, WEB_ASSETS_API_VERSION) {
        Ok(store) => Some(store),
        Err(error) => {
            log::warn!("web assets unavailable; embedded management remains usable: {error}");
            None
        }
    }
}

struct MountedAssets {
    label: CString,
    registered: bool,
}

impl MountedAssets {
    fn unmount(mut self) -> Result<(), (Self, EspError)> {
        // SAFETY: this handle exists only after successful registration and is
        // uniquely owned by AssetStorage.
        match esp_idf_svc::sys::esp!(unsafe {
            esp_idf_svc::sys::esp_vfs_spiffs_unregister(self.label.as_ptr())
        }) {
            Ok(()) => {
                self.registered = false;
                Ok(())
            }
            Err(error) => Err((self, error)),
        }
    }
}

impl Drop for MountedAssets {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }
        // SAFETY: this handle exists only after successful registration and is
        // uniquely owned by AssetStorage.
        if let Err(error) = esp_idf_svc::sys::esp!(unsafe {
            esp_idf_svc::sys::esp_vfs_spiffs_unregister(self.label.as_ptr())
        }) {
            log::error!("assets SPIFFS unmount failed: {}", error);
        }
    }
}

fn mount() -> Result<MountedAssets, EspError> {
    let label = CString::new(PARTITION_LABEL)
        .map_err(|_| EspError::from_infallible::<{ esp_idf_svc::sys::ESP_ERR_INVALID_ARG }>())?;
    let path = CString::new(MOUNT_PATH)
        .map_err(|_| EspError::from_infallible::<{ esp_idf_svc::sys::ESP_ERR_INVALID_ARG }>())?;
    let configuration = esp_idf_svc::sys::esp_vfs_spiffs_conf_t {
        base_path: path.as_ptr(),
        partition_label: label.as_ptr(),
        max_files: MAX_OPEN_FILES,
        format_if_mount_failed: false,
    };
    // SAFETY: both C strings remain alive for the call. ESP-IDF copies the VFS
    // path and resolves the partition before returning.
    esp_idf_svc::sys::esp!(unsafe { esp_idf_svc::sys::esp_vfs_spiffs_register(&configuration) })?;
    Ok(MountedAssets {
        label,
        registered: true,
    })
}

fn esp_io(error: EspError) -> StoreError {
    StoreError::Io(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("ESP-IDF error {}", error.code()),
    ))
}

#[allow(dead_code)]
fn _assert_mount_path_is_absolute() {
    debug_assert!(Path::new(MOUNT_PATH).is_absolute());
}
