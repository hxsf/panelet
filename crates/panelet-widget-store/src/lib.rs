// SPDX-License-Identifier: MPL-2.0

#![forbid(unsafe_code)]

//! Flat-filesystem persistence for Panelet widgets and small text data.
//!
//! The physical names stay below SPIFFS' configured name limit. Logical
//! directories exist only inside the validated maps and widget package ZIPs.

pub mod web_assets;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use panelet_management::{WidgetCandidate, WidgetSettings};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const FORMAT_MARKER: &[u8] = b"NOTE4C-WIDGET-STORE\n1\n";
pub const PRIVATE_FILE_COUNT_MAX: usize = 16;
pub const PRIVATE_FILE_ITEM_MAX: usize = 16 * 1024;
pub const PRIVATE_TOTAL_MAX: usize = 64 * 1024;
pub const SHARED_FILE_COUNT_MAX: usize = 64;
pub const SHARED_FILE_ITEM_MAX: usize = 64 * 1024;
pub const SHARED_TOTAL_MAX: usize = 1024 * 1024;
pub const LOGICAL_PATH_MAX: usize = 96;
pub const KV_KEY_MAX: usize = 32;
pub const KV_VALUE_MAX: usize = 1024;
pub const KV_KEYS_MAX: usize = 8;
pub const KV_TOTAL_MAX: usize = 4 * 1024;

#[derive(Debug)]
pub enum StoreError {
    NotInitialized,
    AlreadyInitialized,
    InvalidPath,
    InvalidKey,
    Limit(&'static str),
    Collision,
    Corrupt(&'static str),
    Io(std::io::Error),
    Json(serde_json::Error),
    Package(panelet_widget_package::PackageError),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized => write!(f, "widget store is not initialized"),
            Self::AlreadyInitialized => write!(f, "widget store is already initialized"),
            Self::InvalidPath => write!(f, "invalid logical path"),
            Self::InvalidKey => write!(f, "invalid KV key"),
            Self::Limit(limit) => write!(f, "storage limit exceeded: {limit}"),
            Self::Collision => write!(f, "widget identifier hash collision"),
            Self::Corrupt(what) => write!(f, "corrupt widget store: {what}"),
            Self::Io(error) => write!(f, "storage I/O failed: {error}"),
            Self::Json(error) => write!(f, "storage JSON failed: {error}"),
            Self::Package(error) => write!(f, "widget package failed: {error}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<panelet_widget_package::PackageError> for StoreError {
    fn from(value: panelet_widget_package::PackageError) -> Self {
        Self::Package(value)
    }
}

#[derive(Clone, Debug)]
pub struct LoadIssue {
    pub physical_name: String,
    pub message: String,
}

#[derive(Default, Debug)]
pub struct LoadReport {
    pub widgets: Vec<WidgetCandidate>,
    pub issues: Vec<LoadIssue>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PrivateState {
    #[serde(default)]
    settings: WidgetSettings,
    #[serde(default)]
    owner_id: String,
    #[serde(default)]
    kv: BTreeMap<String, String>,
    #[serde(default)]
    files: BTreeMap<String, String>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SharedState {
    #[serde(default)]
    files: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct WidgetStore {
    root: PathBuf,
}

impl WidgetStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let store = Self { root: root.into() };
        let marker = fs::read(store.root.join("format.v1")).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                StoreError::NotInitialized
            } else {
                error.into()
            }
        })?;
        if marker != FORMAT_MARKER {
            return Err(StoreError::NotInitialized);
        }
        Ok(store)
    }

    /// Initialize an empty directory-backed store. Formatting flash is the
    /// platform owner's responsibility and must require explicit user action.
    pub fn initialize_empty(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        if !root.exists() {
            fs::create_dir_all(&root)?;
        }
        Self::initialize_mounted(root)
    }

    /// Initialize a store at an already mounted filesystem root.
    ///
    /// Unlike [`Self::initialize_empty`], this never tries to create the root.
    /// Filesystems such as ESP-IDF SPIFFS expose a VFS mount point but do not
    /// implement `mkdir`, so probing the mount point with `Path::exists` and
    /// then creating it can fail with `ENOTSUP`.
    pub fn initialize_mounted(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        let store = Self { root };
        store.atomic_write("format.v1", FORMAT_MARKER)?;
        Ok(store)
    }

    pub fn load_widgets(&self) -> Result<LoadReport, StoreError> {
        self.require_initialized()?;
        self.recover_packages()?;
        self.recover_orphan_blobs()?;
        let mut report = LoadReport::default();
        for entry in fs::read_dir(&self.root)? {
            storage_checkpoint();
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_package_name(&name) {
                continue;
            }
            let result = fs::read(entry.path())
                .map_err(StoreError::from)
                .and_then(|bytes| {
                    panelet_widget_package::decode_n4w(&bytes).map_err(StoreError::from)
                })
                .and_then(|candidate| {
                    if package_name(&candidate.manifest.id) != name {
                        return Err(StoreError::Collision);
                    }
                    Ok(candidate)
                });
            match result {
                Ok(candidate) => report.widgets.push(candidate),
                Err(error) => report.issues.push(LoadIssue {
                    physical_name: name,
                    message: error.to_string(),
                }),
            }
        }
        Ok(report)
    }

    pub fn put_widget(&self, candidate: &WidgetCandidate) -> Result<(), StoreError> {
        self.require_initialized()?;
        let name = package_name(&candidate.manifest.id);
        if let Ok(existing) = fs::read(self.root.join(&name)) {
            let existing_id = panelet_widget_package::package_owner_id(&existing)?;
            if existing_id != candidate.manifest.id {
                return Err(StoreError::Collision);
            }
        }
        let bytes = panelet_widget_package::encode_n4w(candidate);
        let temp = temp_package_name(&candidate.manifest.id);
        self.write_synced(&temp, &bytes)?;
        // Re-read before publishing so a short flash write can never replace a
        // previously valid package.
        let verified = panelet_widget_package::decode_n4w(&fs::read(self.root.join(&temp))?)?;
        if verified != *candidate {
            return Err(StoreError::Corrupt("package verification mismatch"));
        }
        replace_file(&self.root.join(&temp), &self.root.join(&name))?;
        Ok(())
    }

    pub fn delete_widget(&self, widget_id: &str) -> Result<(), StoreError> {
        self.require_initialized()?;
        let tombstone = tombstone_name(widget_id);
        self.write_synced(&tombstone, widget_id.as_bytes())?;
        let state = self.private_state(widget_id)?;
        for (path, physical) in &state.files {
            self.verify_private_blob(widget_id, path, physical)?;
            // The tombstone and index are authoritative. A failed obsolete
            // blob removal is recovered by boot-time orphan GC.
            let _ = remove_if_exists(&self.root.join(physical));
        }
        remove_if_exists(&self.root.join(package_name(widget_id)))?;
        remove_if_exists(&self.root.join(temp_package_name(widget_id)))?;
        remove_if_exists(&self.root.join(private_name(widget_id)))?;
        remove_if_exists(&self.root.join(private_temp_name(widget_id)))?;
        self.recover_orphan_blobs()?;
        remove_if_exists(&self.root.join(tombstone))?;
        Ok(())
    }

    pub fn active_widget(&self) -> Result<Option<String>, StoreError> {
        self.require_initialized()?;
        self.recover_active_file()?;
        match fs::read_to_string(self.root.join("active.txt")) {
            Ok(value) if value.is_empty() => Ok(None),
            Ok(value) if value.len() <= 48 => Ok(Some(value)),
            Ok(_) => Err(StoreError::Corrupt("active widget")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn set_active_widget(&self, widget_id: &str) -> Result<(), StoreError> {
        self.require_initialized()?;
        if widget_id.len() > 48 {
            return Err(StoreError::InvalidPath);
        }
        self.atomic_write("active.txt", widget_id.as_bytes())
    }

    pub fn private_exists(&self, widget_id: &str, path: &str) -> Result<bool, StoreError> {
        Ok(self.private_read_bytes(widget_id, path)?.is_some())
    }

    pub fn private_read(&self, widget_id: &str, path: &str) -> Result<Option<String>, StoreError> {
        self.private_read_bytes(widget_id, path)?
            .map(|bytes| {
                String::from_utf8(bytes)
                    .map_err(|_| StoreError::Corrupt("private text is not UTF-8"))
            })
            .transpose()
    }

    pub fn private_read_bytes(
        &self,
        widget_id: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let path = normalize_path(path)?;
        let state = self.private_state(widget_id)?;
        state
            .files
            .get(&path)
            .map(|name| self.verify_private_blob(widget_id, &path, name))
            .transpose()
    }

    pub fn private_list(&self, widget_id: &str, prefix: &str) -> Result<Vec<String>, StoreError> {
        let prefix = normalize_prefix(prefix)?;
        Ok(self
            .private_state(widget_id)?
            .files
            .keys()
            .filter(|path| path.starts_with(&prefix))
            .cloned()
            .collect())
    }

    pub fn private_write(&self, widget_id: &str, path: &str, text: &str) -> Result<(), StoreError> {
        self.private_write_bytes(widget_id, path, text.as_bytes())
    }

    pub fn private_write_bytes(
        &self,
        widget_id: &str,
        path: &str,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        let path = normalize_path(path)?;
        if bytes.len() > PRIVATE_FILE_ITEM_MAX {
            return Err(StoreError::Limit("private item"));
        }
        let mut state = self.private_state(widget_id)?;
        if !state.files.contains_key(&path) && state.files.len() >= PRIVATE_FILE_COUNT_MAX {
            return Err(StoreError::Limit("private file count"));
        }
        let old_size = state.files.get(&path).map_or(Ok(0), |name| {
            self.verify_private_blob(widget_id, &path, name)
                .map(|bytes| bytes.len())
        })?;
        let used = state
            .files
            .values()
            .filter_map(|name| fs::metadata(self.root.join(name)).ok())
            .map(|meta| meta.len() as usize)
            .sum::<usize>();
        if used.saturating_sub(old_size).saturating_add(bytes.len()) > PRIVATE_TOTAL_MAX {
            return Err(StoreError::Limit("private bytes"));
        }
        let physical = blob_name(b'b', &[widget_id.as_bytes(), path.as_bytes(), bytes]);
        let created = self.put_blob(&physical, bytes)?;
        let old = state.files.insert(path, physical.clone());
        if let Err(error) = self.write_private_state(widget_id, &state) {
            if created {
                let _ = remove_if_exists(&self.root.join(physical));
            }
            return Err(error);
        }
        if let Some(old) = old {
            if old != physical {
                let _ = remove_if_exists(&self.root.join(old));
            }
        }
        Ok(())
    }

    pub fn private_delete(&self, widget_id: &str, path: &str) -> Result<(), StoreError> {
        let path = normalize_path(path)?;
        let mut state = self.private_state(widget_id)?;
        let removed = state.files.get(&path).cloned();
        if let Some(name) = &removed {
            self.verify_private_blob(widget_id, &path, name)?;
        }
        state.files.remove(&path);
        self.write_private_state(widget_id, &state)?;
        if let Some(name) = removed {
            let _ = remove_if_exists(&self.root.join(name));
        }
        Ok(())
    }

    pub fn shared_exists(&self, path: &str) -> Result<bool, StoreError> {
        Ok(self.shared_read_bytes(path)?.is_some())
    }

    pub fn shared_read(&self, path: &str) -> Result<Option<String>, StoreError> {
        self.shared_read_bytes(path)?
            .map(|bytes| {
                String::from_utf8(bytes)
                    .map_err(|_| StoreError::Corrupt("shared text is not UTF-8"))
            })
            .transpose()
    }

    pub fn shared_read_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let path = normalize_path(path)?;
        let state = self.shared_state()?;
        state
            .files
            .get(&path)
            .map(|name| self.verify_shared_blob(&path, name))
            .transpose()
    }

    pub fn shared_list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        let prefix = normalize_prefix(prefix)?;
        Ok(self
            .shared_state()?
            .files
            .keys()
            .filter(|path| path.starts_with(&prefix))
            .cloned()
            .collect())
    }

    pub fn shared_write(&self, path: &str, text: &str) -> Result<(), StoreError> {
        self.shared_write_bytes(path, text.as_bytes())
    }

    pub fn shared_write_bytes(&self, path: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let path = normalize_path(path)?;
        if bytes.len() > SHARED_FILE_ITEM_MAX {
            return Err(StoreError::Limit("shared item"));
        }
        let mut state = self.shared_state()?;
        if !state.files.contains_key(&path) && state.files.len() >= SHARED_FILE_COUNT_MAX {
            return Err(StoreError::Limit("shared file count"));
        }
        let old_size = state.files.get(&path).map_or(Ok(0), |name| {
            self.verify_shared_blob(&path, name)
                .map(|bytes| bytes.len())
        })?;
        let used = state
            .files
            .values()
            .filter_map(|name| fs::metadata(self.root.join(name)).ok())
            .map(|meta| meta.len() as usize)
            .sum::<usize>();
        if used.saturating_sub(old_size).saturating_add(bytes.len()) > SHARED_TOTAL_MAX {
            return Err(StoreError::Limit("shared bytes"));
        }
        let physical = blob_name(b's', &[path.as_bytes(), bytes]);
        let created = self.put_blob(&physical, bytes)?;
        let old = state.files.insert(path, physical.clone());
        if let Err(error) = self.atomic_json("shared.json", &state) {
            if created {
                let _ = remove_if_exists(&self.root.join(physical));
            }
            return Err(error);
        }
        if let Some(old) = old {
            if old != physical {
                let _ = remove_if_exists(&self.root.join(old));
            }
        }
        Ok(())
    }

    pub fn shared_delete(&self, path: &str) -> Result<(), StoreError> {
        let path = normalize_path(path)?;
        let mut state = self.shared_state()?;
        let removed = state.files.get(&path).cloned();
        if let Some(name) = &removed {
            self.verify_shared_blob(&path, name)?;
        }
        state.files.remove(&path);
        self.atomic_json("shared.json", &state)?;
        if let Some(name) = removed {
            let _ = remove_if_exists(&self.root.join(name));
        }
        Ok(())
    }

    pub fn widget_settings(&self, widget_id: &str) -> Result<WidgetSettings, StoreError> {
        let settings = self.private_state(widget_id)?.settings;
        settings.validate().map_err(StoreError::Corrupt)?;
        Ok(settings)
    }

    pub fn set_widget_settings(
        &self,
        widget_id: &str,
        settings: &WidgetSettings,
    ) -> Result<(), StoreError> {
        settings.validate().map_err(StoreError::Limit)?;
        let mut state = self.private_state(widget_id)?;
        state.settings = settings.clone();
        self.write_private_state(widget_id, &state)
    }

    pub fn kv_has(&self, widget_id: &str, key: &str) -> Result<bool, StoreError> {
        validate_key(key)?;
        Ok(self.private_state(widget_id)?.kv.contains_key(key))
    }

    pub fn kv_get(&self, widget_id: &str, key: &str) -> Result<Option<String>, StoreError> {
        validate_key(key)?;
        Ok(self.private_state(widget_id)?.kv.get(key).cloned())
    }

    pub fn kv_set(&self, widget_id: &str, key: &str, value: &str) -> Result<(), StoreError> {
        validate_key(key)?;
        if value.len() > KV_VALUE_MAX {
            return Err(StoreError::Limit("KV item"));
        }
        let mut state = self.private_state(widget_id)?;
        if !state.kv.contains_key(key) && state.kv.len() >= KV_KEYS_MAX {
            return Err(StoreError::Limit("KV key count"));
        }
        state.kv.insert(key.to_owned(), value.to_owned());
        let used: usize = state
            .kv
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum();
        if used > KV_TOTAL_MAX {
            return Err(StoreError::Limit("KV bytes"));
        }
        self.write_private_state(widget_id, &state)
    }

    pub fn kv_delete(&self, widget_id: &str, key: &str) -> Result<(), StoreError> {
        validate_key(key)?;
        let mut state = self.private_state(widget_id)?;
        state.kv.remove(key);
        self.write_private_state(widget_id, &state)
    }

    fn require_initialized(&self) -> Result<(), StoreError> {
        storage_checkpoint();
        if fs::read(self.root.join("format.v1"))? == FORMAT_MARKER {
            Ok(())
        } else {
            Err(StoreError::NotInitialized)
        }
    }

    fn private_state(&self, widget_id: &str) -> Result<PrivateState, StoreError> {
        let mut state: PrivateState =
            self.read_json_recover(&private_name(widget_id), &private_temp_name(widget_id))?;
        if state.owner_id.is_empty() && state.kv.is_empty() && state.files.is_empty() {
            state.owner_id = widget_id.to_owned();
        }
        if state.owner_id != widget_id || private_name(&state.owner_id) != private_name(widget_id) {
            return Err(StoreError::Collision);
        }
        validate_index_files(&state.files, b'b')?;
        Ok(state)
    }

    fn write_private_state(&self, widget_id: &str, state: &PrivateState) -> Result<(), StoreError> {
        if state.owner_id != widget_id {
            return Err(StoreError::Collision);
        }
        self.atomic_json(&private_name(widget_id), state)
    }

    fn shared_state(&self) -> Result<SharedState, StoreError> {
        let state: SharedState = self.read_json_recover("shared.json", "shared.tmp")?;
        validate_index_files(&state.files, b's')?;
        Ok(state)
    }

    fn read_json_recover<T>(&self, name: &str, temp: &str) -> Result<T, StoreError>
    where
        T: Default + for<'de> Deserialize<'de>,
    {
        self.require_initialized()?;
        let temp_path = self.root.join(temp);
        if temp_path.exists() {
            match fs::read(&temp_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<T>(&bytes).ok())
            {
                Some(_) => replace_file(&temp_path, &self.root.join(name))?,
                None => remove_if_exists(&temp_path)?,
            }
        }
        match fs::read(self.root.join(name)) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(error) => Err(error.into()),
        }
    }

    fn atomic_json<T: Serialize>(&self, name: &str, value: &T) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(value)?;
        let temp = if name == "shared.json" {
            "shared.tmp".to_owned()
        } else {
            name.replace(".json", ".tmp")
        };
        self.write_synced(&temp, &bytes)?;
        replace_file(&self.root.join(temp), &self.root.join(name))
    }

    fn atomic_write(&self, name: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let temp = format!("{name}.tmp");
        self.write_synced(&temp, bytes)?;
        replace_file(&self.root.join(temp), &self.root.join(name))
    }

    fn write_synced(&self, name: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let mut file = File::create(self.root.join(name))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }

    fn put_blob(&self, name: &str, bytes: &[u8]) -> Result<bool, StoreError> {
        let path = self.root.join(name);
        if let Ok(existing) = fs::read(&path) {
            if existing == bytes {
                return Ok(false);
            }
            return Err(StoreError::Collision);
        }
        self.write_synced(name, bytes)?;
        Ok(true)
    }

    fn verify_private_blob(
        &self,
        widget_id: &str,
        logical_path: &str,
        physical: &str,
    ) -> Result<Vec<u8>, StoreError> {
        if !is_typed_blob_name(physical, b'b') {
            return Err(StoreError::Corrupt("private blob name"));
        }
        let bytes = fs::read(self.root.join(physical))?;
        let expected = blob_name(
            b'b',
            &[widget_id.as_bytes(), logical_path.as_bytes(), &bytes],
        );
        if expected != physical {
            return Err(StoreError::Collision);
        }
        Ok(bytes)
    }

    fn verify_shared_blob(
        &self,
        logical_path: &str,
        physical: &str,
    ) -> Result<Vec<u8>, StoreError> {
        if !is_typed_blob_name(physical, b's') {
            return Err(StoreError::Corrupt("shared blob name"));
        }
        let bytes = fs::read(self.root.join(physical))?;
        let expected = blob_name(b's', &[logical_path.as_bytes(), &bytes]);
        if expected != physical {
            return Err(StoreError::Collision);
        }
        Ok(bytes)
    }

    fn recover_active_file(&self) -> Result<(), StoreError> {
        let temp = self.root.join("active.txt.tmp");
        if !temp.exists() {
            return Ok(());
        }
        let valid = fs::metadata(&temp)
            .ok()
            .filter(|metadata| metadata.len() <= 48)
            .and_then(|_| fs::read_to_string(&temp).ok())
            .is_some_and(|id| valid_widget_id(&id));
        if valid {
            replace_file(&temp, &self.root.join("active.txt"))
        } else {
            remove_if_exists(&temp)
        }
    }

    fn recover_private_temps(&self) -> Result<(), StoreError> {
        for entry in fs::read_dir(&self.root)? {
            storage_checkpoint();
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_private_temp_name(&name) {
                continue;
            }
            let state = fs::read(entry.path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<PrivateState>(&bytes).ok());
            match state {
                Some(state)
                    if valid_widget_id(&state.owner_id)
                        && private_temp_name(&state.owner_id) == name
                        && validate_index_files(&state.files, b'b').is_ok() =>
                {
                    for (path, physical) in &state.files {
                        self.verify_private_blob(&state.owner_id, path, physical)?;
                    }
                    replace_file(
                        &entry.path(),
                        &self.root.join(private_name(&state.owner_id)),
                    )?
                }
                _ => remove_if_exists(&entry.path())?,
            }
        }
        Ok(())
    }

    fn recover_orphan_blobs(&self) -> Result<(), StoreError> {
        self.recover_private_temps()?;
        let mut referenced = BTreeSet::new();
        let shared = self.shared_state()?;
        for (path, physical) in &shared.files {
            self.verify_shared_blob(path, physical)?;
            referenced.insert(physical.clone());
        }
        for entry in fs::read_dir(&self.root)? {
            storage_checkpoint();
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_private_state_name(&name) {
                continue;
            }
            let state: PrivateState = serde_json::from_slice(&fs::read(entry.path())?)?;
            if !valid_widget_id(&state.owner_id)
                || private_name(&state.owner_id) != name
                || validate_index_files(&state.files, b'b').is_err()
            {
                return Err(StoreError::Collision);
            }
            for (path, physical) in &state.files {
                self.verify_private_blob(&state.owner_id, path, physical)?;
                referenced.insert(physical.clone());
            }
        }
        for entry in fs::read_dir(&self.root)? {
            storage_checkpoint();
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_blob_name(&name) && !referenced.contains(&name) {
                remove_if_exists(&entry.path())?;
            }
        }
        Ok(())
    }

    fn purge_tombstoned_hash(&self, tombstone: &str) -> Result<(), StoreError> {
        let hash = std::str::from_utf8(
            tombstone
                .as_bytes()
                .get(1..25)
                .ok_or(StoreError::Corrupt("tombstone name"))?,
        )
        .map_err(|_| StoreError::Corrupt("tombstone name"))?;
        let private = self.root.join(format!("d{hash}.json"));
        if let Ok(bytes) = fs::read(&private) {
            if let Ok(state) = serde_json::from_slice::<PrivateState>(&bytes) {
                if valid_widget_id(&state.owner_id)
                    && private_name(&state.owner_id) == format!("d{hash}.json")
                    && validate_index_files(&state.files, b'b').is_ok()
                {
                    for (path, physical) in &state.files {
                        if self
                            .verify_private_blob(&state.owner_id, path, physical)
                            .is_ok()
                        {
                            let _ = remove_if_exists(&self.root.join(physical));
                        }
                    }
                }
            }
        }
        for name in [
            format!("p{hash}.n4w"),
            format!("t{hash}.n4w"),
            format!("d{hash}.json"),
            format!("d{hash}.tmp"),
        ] {
            remove_if_exists(&self.root.join(name))?;
        }
        self.recover_orphan_blobs()?;
        remove_if_exists(&self.root.join(tombstone))?;
        Ok(())
    }

    fn recover_packages(&self) -> Result<(), StoreError> {
        for entry in fs::read_dir(&self.root)? {
            storage_checkpoint();
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_tombstone_name(&name) {
                let valid = fs::metadata(entry.path())
                    .ok()
                    .filter(|metadata| (3..=48).contains(&metadata.len()))
                    .and_then(|_| fs::read_to_string(entry.path()).ok())
                    .is_some_and(|id| valid_widget_id(&id) && tombstone_name(&id) == name);
                // The marker filename is the deletion authority. Even a torn
                // payload must not let the package reappear after reset.
                let _ = valid;
                self.purge_tombstoned_hash(&name)?;
            }
        }
        for entry in fs::read_dir(&self.root)? {
            storage_checkpoint();
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_temp_package_name(&name) {
                continue;
            }
            let bytes = fs::read(entry.path())?;
            match panelet_widget_package::package_owner_id(&bytes) {
                Ok(owner_id) if temp_package_name(&owner_id) == name => {
                    replace_file(
                        &entry.path(),
                        &self.root.join(package_name(&owner_id)),
                    )?;
                }
                _ => remove_if_exists(&entry.path())?,
            }
        }
        Ok(())
    }
}

pub fn normalize_path(path: &str) -> Result<String, StoreError> {
    if path.is_empty()
        || path.len() > LOGICAL_PATH_MAX
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.bytes().any(|b| b.is_ascii_control())
    {
        return Err(StoreError::InvalidPath);
    }
    let mut out = String::new();
    for part in path.split('/') {
        if part.is_empty() || part == ".." {
            return Err(StoreError::InvalidPath);
        }
        if part == "." {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    if out.is_empty() || out.len() > LOGICAL_PATH_MAX {
        Err(StoreError::InvalidPath)
    } else {
        Ok(out)
    }
}

fn normalize_prefix(prefix: &str) -> Result<String, StoreError> {
    if prefix.is_empty() {
        Ok(String::new())
    } else {
        normalize_path(prefix)
    }
}

fn validate_key(key: &str) -> Result<(), StoreError> {
    if key.is_empty()
        || key.len() > KV_KEY_MAX
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        Err(StoreError::InvalidKey)
    } else {
        Ok(())
    }
}

fn digest_hex(parts: &[&[u8]], bytes: usize) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u32).to_le_bytes());
        hasher.update(part);
    }
    let digest = hasher.finalize();
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes * 2);
    for byte in &digest[..bytes] {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

fn blob_name(prefix: u8, parts: &[&[u8]]) -> String {
    format!("{}{}", char::from(prefix), digest_hex(parts, 15))
}

fn package_name(id: &str) -> String {
    format!("p{}.n4w", digest_hex(&[id.as_bytes()], 12))
}
fn temp_package_name(id: &str) -> String {
    format!("t{}.n4w", digest_hex(&[id.as_bytes()], 12))
}
fn tombstone_name(id: &str) -> String {
    format!("x{}.del", digest_hex(&[id.as_bytes()], 12))
}
fn private_name(id: &str) -> String {
    format!("d{}.json", digest_hex(&[id.as_bytes()], 12))
}
fn private_temp_name(id: &str) -> String {
    format!("d{}.tmp", digest_hex(&[id.as_bytes()], 12))
}
fn is_lower_hex(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
fn is_hashed_name(name: &str, prefix: u8, suffix: &[u8]) -> bool {
    let bytes = name.as_bytes();
    bytes.first() == Some(&prefix)
        && bytes.ends_with(suffix)
        && bytes.len() == 1 + 24 + suffix.len()
        && is_lower_hex(&bytes[1..25])
}
fn is_package_name(name: &str) -> bool {
    is_hashed_name(name, b'p', b".n4w")
}
fn is_temp_package_name(name: &str) -> bool {
    is_hashed_name(name, b't', b".n4w")
}
fn is_tombstone_name(name: &str) -> bool {
    is_hashed_name(name, b'x', b".del")
}
fn is_private_state_name(name: &str) -> bool {
    is_hashed_name(name, b'd', b".json")
}
fn is_private_temp_name(name: &str) -> bool {
    is_hashed_name(name, b'd', b".tmp")
}
fn is_blob_name(name: &str) -> bool {
    is_typed_blob_name(name, b'b') || is_typed_blob_name(name, b's')
}
fn is_typed_blob_name(name: &str, prefix: u8) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 31 && bytes.first() == Some(&prefix) && is_lower_hex(&bytes[1..])
}
fn validate_index_files(files: &BTreeMap<String, String>, prefix: u8) -> Result<(), StoreError> {
    for (logical, physical) in files {
        if normalize_path(logical)? != *logical {
            return Err(StoreError::InvalidPath);
        }
        if !is_typed_blob_name(physical, prefix) {
            return Err(StoreError::Corrupt("blob index name"));
        }
    }
    Ok(())
}
fn valid_widget_id(id: &str) -> bool {
    (3..=48).contains(&id.len())
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

fn remove_if_exists(path: &Path) -> Result<(), StoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn replace_file(source: &Path, target: &Path) -> Result<(), StoreError> {
    remove_if_exists(target)?;
    fs::rename(source, target)?;
    Ok(())
}

// SPIFFS metadata scans can consume the whole main-task watchdog window when
// restoring several packages. Block briefly so the idle task can run; merely
// yielding to an equal-priority task does not service the idle watchdog.
fn storage_checkpoint() {
    #[cfg(target_os = "espidf")]
    std::thread::sleep(std::time::Duration::from_millis(10));
}

#[cfg(test)]
mod tests {
    use super::*;
    use panelet_management::{WidgetManifest, WIDGET_ABI};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "panelet-store-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn candidate(id: &str) -> WidgetCandidate {
        WidgetCandidate {
            manifest: WidgetManifest {
                id: id.into(),
                name: "TEST".into(),
                version: "0.1.0".into(),
                abi: WIDGET_ABI.into(),
                capabilities: vec![],
                host_permissions: vec![],
            },
            source: "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { os::runtime::ok(()) } }".into(),
            resources: vec![],
        }
    }

    #[test]
    fn legacy_packages_are_retained_and_upgrade_preserves_private_configuration_and_selection() {
        let root = temp_root();
        let store = WidgetStore::initialize_empty(&root).unwrap();
        let current = candidate("sample.migrate");
        let mut legacy = current.clone();
        legacy.manifest.abi = "note4c.rune/2".into();
        legacy.source = "pub async fn main(std) { std.runtime.ok(()) }".into();
        let bytes = panelet_widget_package::encode_n4w(&legacy);
        // A pending legacy write is also retained through startup recovery.
        fs::write(root.join(temp_package_name(&legacy.manifest.id)), &bytes).unwrap();
        store.private_write(&legacy.manifest.id, "cache.txt", "PRIVATE").unwrap();
        store.kv_set(&legacy.manifest.id, "page", "2").unwrap();
        store.set_active_widget(&legacy.manifest.id).unwrap();
        let settings = WidgetSettings {
            interval_seconds: 300,
            host_permissions: Some(vec!["https://example.test/feed".into()]),
            values: serde_json::json!({"token": "PRIVATE"}),
        };
        store.set_widget_settings(&legacy.manifest.id, &settings).unwrap();
        let report = store.load_widgets().unwrap();
        assert!(report.widgets.is_empty());
        assert_eq!(report.issues.len(), 1);
        assert!(report.issues[0].message.contains("widget ABI"));
        assert_eq!(fs::read(root.join(package_name(&legacy.manifest.id))).unwrap(), bytes);
        store.put_widget(&current).unwrap();
        let reopened = WidgetStore::open(&root).unwrap();
        assert_eq!(reopened.load_widgets().unwrap().widgets, vec![current]);
        assert_eq!(reopened.private_read(&legacy.manifest.id, "cache.txt").unwrap().as_deref(), Some("PRIVATE"));
        assert_eq!(reopened.kv_get(&legacy.manifest.id, "page").unwrap().as_deref(), Some("2"));
        assert_eq!(reopened.widget_settings(&legacy.manifest.id).unwrap(), settings);
        assert_eq!(reopened.active_widget().unwrap().as_deref(), Some(legacy.manifest.id.as_str()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleared_foreground_survives_reopen() {
        let root = temp_root();
        let store = WidgetStore::initialize_empty(&root).unwrap();
        store.set_active_widget("sample.foreground").unwrap();
        store.set_active_widget("").unwrap();
        assert_eq!(WidgetStore::open(&root).unwrap().active_widget().unwrap(), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn initialization_is_explicit() {
        let root = temp_root();
        assert!(matches!(
            WidgetStore::open(&root),
            Err(StoreError::NotInitialized)
        ));
        let store = WidgetStore::initialize_empty(&root).unwrap();
        assert!(WidgetStore::open(&root).is_ok());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mounted_initialization_writes_marker_without_creating_root() {
        let root = temp_root();
        let store = WidgetStore::initialize_mounted(&root).unwrap();
        assert!(WidgetStore::open(&root).is_ok());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configuration_survives_updates_and_is_isolated_and_deleted_with_widget() {
        let root = temp_root();
        let store = WidgetStore::initialize_empty(&root).unwrap();
        let package = candidate("test.todo");
        store.put_widget(&package).unwrap();
        assert_eq!(
            store.widget_settings("test.todo").unwrap(),
            WidgetSettings::default()
        );
        let settings = WidgetSettings {
            interval_seconds: 300,
            host_permissions: None,
            values: serde_json::json!({"bearer_token":"TEST_ONLY"}),
        };
        store.set_widget_settings("test.todo", &settings).unwrap();
        store.kv_set("test.todo", "page", "2").unwrap();
        store.put_widget(&package).unwrap();
        let reopened = WidgetStore::open(&root).unwrap();
        assert_eq!(reopened.widget_settings("test.todo").unwrap(), settings);
        assert_eq!(
            reopened.widget_settings("test.other").unwrap(),
            WidgetSettings::default()
        );
        assert_eq!(
            reopened.kv_get("test.todo", "page").unwrap().as_deref(),
            Some("2")
        );
        let mut invalid = settings.clone();
        invalid.interval_seconds = 3;
        assert!(reopened.set_widget_settings("test.todo", &invalid).is_err());
        assert_eq!(reopened.widget_settings("test.todo").unwrap(), settings);
        reopened.delete_widget("test.todo").unwrap();
        reopened.put_widget(&package).unwrap();
        assert_eq!(
            reopened.widget_settings("test.todo").unwrap(),
            WidgetSettings::default()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_private_shared_and_kv_survive_reopen() {
        let root = temp_root();
        let store = WidgetStore::initialize_empty(&root).unwrap();
        let package = candidate("sample.persist");
        store.put_widget(&package).unwrap();
        store
            .private_write(&package.manifest.id, "cache/value.txt", "PRIVATE")
            .unwrap();
        store.shared_write("photos/index.txt", "SHARED").unwrap();
        store.kv_set(&package.manifest.id, "page", "2").unwrap();
        store.set_active_widget(&package.manifest.id).unwrap();
        let reopened = WidgetStore::open(&root).unwrap();
        assert_eq!(
            reopened.load_widgets().unwrap().widgets,
            vec![package.clone()]
        );
        assert_eq!(
            reopened
                .private_read(&package.manifest.id, "cache/value.txt")
                .unwrap()
                .as_deref(),
            Some("PRIVATE")
        );
        assert_eq!(
            reopened.shared_read("photos/index.txt").unwrap().as_deref(),
            Some("SHARED")
        );
        assert_eq!(
            reopened
                .kv_get(&package.manifest.id, "page")
                .unwrap()
                .as_deref(),
            Some("2")
        );
        assert_eq!(
            reopened.active_widget().unwrap().as_deref(),
            Some("sample.persist")
        );
        reopened.delete_widget(&package.manifest.id).unwrap();
        assert!(reopened.load_widgets().unwrap().widgets.is_empty());
        assert_eq!(
            reopened
                .private_read(&package.manifest.id, "cache/value.txt")
                .unwrap(),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn logical_paths_and_limits_are_enforced() {
        let root = temp_root();
        let store = WidgetStore::initialize_empty(&root).unwrap();
        assert!(matches!(
            store.shared_write("../secret", "x"),
            Err(StoreError::InvalidPath)
        ));
        assert!(matches!(
            store.private_write("sample.test", "x", &"a".repeat(PRIVATE_FILE_ITEM_MAX + 1)),
            Err(StoreError::Limit(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_package_publish_and_tombstone_recover() {
        let root = temp_root();
        let store = WidgetStore::initialize_empty(&root).unwrap();
        let package = candidate("sample.recover");
        let bytes = panelet_widget_package::encode_n4w(&package);
        store
            .write_synced(&temp_package_name(&package.manifest.id), &bytes)
            .unwrap();

        let reopened = WidgetStore::open(&root).unwrap();
        assert_eq!(
            reopened.load_widgets().unwrap().widgets,
            vec![package.clone()]
        );
        assert!(root.join(package_name(&package.manifest.id)).exists());
        assert!(!root.join(temp_package_name(&package.manifest.id)).exists());

        reopened
            .private_write(&package.manifest.id, "state.txt", "kept until delete")
            .unwrap();
        reopened
            .write_synced(
                &tombstone_name(&package.manifest.id),
                package.manifest.id.as_bytes(),
            )
            .unwrap();
        let after_reset = WidgetStore::open(&root).unwrap();
        assert!(after_reset.load_widgets().unwrap().widgets.is_empty());
        assert_eq!(
            after_reset
                .private_read(&package.manifest.id, "state.txt")
                .unwrap(),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shared_binary_survives_but_text_api_rejects_non_utf8() {
        let root = temp_root();
        let store = WidgetStore::initialize_empty(&root).unwrap();
        let bytes = [0xff, 0x00, 0x52, 0x49, 0x46, 0x46];
        store.shared_write_bytes("photos/test.bin", &bytes).unwrap();
        let reopened = WidgetStore::open(&root).unwrap();
        assert_eq!(
            reopened.shared_read_bytes("photos/test.bin").unwrap(),
            Some(bytes.to_vec())
        );
        assert!(matches!(
            reopened.shared_read("photos/test.bin"),
            Err(StoreError::Corrupt("shared text is not UTF-8"))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reused_blobs_survive_index_write_failure() {
        let root = temp_root();
        let store = WidgetStore::initialize_empty(&root).unwrap();
        let widget_id = "sample.rollback";
        store.private_write(widget_id, "same.txt", "OLD").unwrap();
        fs::create_dir(root.join(private_temp_name(widget_id))).unwrap();
        assert!(store.private_write(widget_id, "same.txt", "OLD").is_err());
        fs::remove_dir(root.join(private_temp_name(widget_id))).unwrap();
        assert_eq!(
            store
                .private_read(widget_id, "same.txt")
                .unwrap()
                .as_deref(),
            Some("OLD")
        );

        store.shared_write("same.txt", "SHARED").unwrap();
        fs::create_dir(root.join("shared.tmp")).unwrap();
        assert!(store.shared_write("same.txt", "SHARED").is_err());
        fs::remove_dir(root.join("shared.tmp")).unwrap();
        assert_eq!(
            store.shared_read("same.txt").unwrap().as_deref(),
            Some("SHARED")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn torn_tombstone_is_fail_closed_and_orphans_are_collected() {
        let root = temp_root();
        let store = WidgetStore::initialize_empty(&root).unwrap();
        let package = candidate("sample.torn-delete");
        store.put_widget(&package).unwrap();
        store
            .private_write(&package.manifest.id, "state.txt", "SECRET")
            .unwrap();
        let orphan = blob_name(b'b', &[b"orphan"]);
        store.write_synced(&orphan, b"ORPHAN").unwrap();
        store
            .write_synced(&tombstone_name(&package.manifest.id), b"")
            .unwrap();

        let reopened = WidgetStore::open(&root).unwrap();
        assert!(reopened.load_widgets().unwrap().widgets.is_empty());
        assert!(!root.join(orphan).exists());
        assert!(!root.join(tombstone_name(&package.manifest.id)).exists());
        assert_eq!(
            reopened
                .private_read(&package.manifest.id, "state.txt")
                .unwrap(),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn active_temp_recovers_and_private_owner_is_verified() {
        let root = temp_root();
        let store = WidgetStore::initialize_empty(&root).unwrap();
        store.set_active_widget("sample.old").unwrap();
        store.write_synced("active.txt.tmp", b"sample.new").unwrap();
        assert_eq!(
            store.active_widget().unwrap().as_deref(),
            Some("sample.new")
        );

        store.kv_set("sample.owner-a", "key", "value").unwrap();
        fs::copy(
            root.join(private_name("sample.owner-a")),
            root.join(private_name("sample.owner-b")),
        )
        .unwrap();
        assert!(matches!(
            store.kv_get("sample.owner-b", "key"),
            Err(StoreError::Collision)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_index_cannot_escape_root_or_cross_owner() {
        let root = temp_root();
        let store = WidgetStore::initialize_empty(&root).unwrap();
        store
            .private_write("sample.owner-b", "secret.txt", "SECRET")
            .unwrap();
        let owner_b = store.private_state("sample.owner-b").unwrap();
        let foreign_blob = owner_b.files["secret.txt"].clone();

        let mut crossed = PrivateState {
            owner_id: "sample.owner-a".into(),
            ..PrivateState::default()
        };
        crossed.files.insert("leak.txt".into(), foreign_blob);
        store
            .write_synced(
                &private_name("sample.owner-a"),
                &serde_json::to_vec(&crossed).unwrap(),
            )
            .unwrap();
        assert!(matches!(
            store.private_read("sample.owner-a", "leak.txt"),
            Err(StoreError::Collision)
        ));

        crossed.files.clear();
        crossed
            .files
            .insert("safe.txt".into(), "../../victim".into());
        store
            .write_synced(
                &private_name("sample.owner-a"),
                &serde_json::to_vec(&crossed).unwrap(),
            )
            .unwrap();
        assert!(matches!(
            store.private_read("sample.owner-a", "safe.txt"),
            Err(StoreError::Corrupt("blob index name"))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_hex_and_unicode_tombstones_are_ignored_without_panicking() {
        let root = temp_root();
        let store = WidgetStore::initialize_empty(&root).unwrap();
        let package = candidate("sample.stays");
        store.put_widget(&package).unwrap();
        for name in [
            format!("x{}.del", "g".repeat(24)),
            format!("x{}é.del", "a".repeat(22)),
        ] {
            store.write_synced(&name, b"sample.stays").unwrap();
        }
        assert_eq!(store.load_widgets().unwrap().widgets, vec![package]);
        fs::remove_dir_all(root).unwrap();
    }
}
