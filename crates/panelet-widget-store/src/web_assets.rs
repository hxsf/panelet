// SPDX-License-Identifier: MPL-2.0

//! Versioned immutable web assets on the existing flat filesystem.
//!
//! API versions describe resource compatibility, not firmware builds or OTA
//! slots. Each API version keeps two checksummed commits; all installed API
//! versions retain their files. Uploads never touch widget or user-data names.
//! `sync_all` and readback protect the publication protocol, but do not establish
//! the physical filesystem's power-loss guarantees (SPIFFS needs hardware tests).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const WEB_ASSET_SCHEMA_VERSION: u32 = 1;
pub const WEB_ASSET_FILE_COUNT_MAX: usize = 24;
pub const WEB_ASSET_FILE_SIZE_MAX: u64 = 1536 * 1024;
pub const WEB_ASSET_BUNDLE_SIZE_MAX: u64 = 2 * 1024 * 1024;
pub const WEB_ASSET_MANIFEST_SIZE_MAX: usize = 16 * 1024;
pub const WEB_ASSET_CHUNK_SIZE_MAX: usize = 32 * 1024;
pub const WEB_ASSET_API_VERSIONS_MAX: usize = 4;
const RECORD_SIZE_MAX: usize = WEB_ASSET_MANIFEST_SIZE_MAX + 1024;
const PENDING_NAME: &str = "wa-p.json";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WebAssetEncoding {
    Identity,
    Gzip,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebAssetFile {
    pub path: String,
    pub content_type: String,
    pub encoding: WebAssetEncoding,
    /// Stored/transferred bytes, after gzip when encoding is gzip.
    pub size: u64,
    /// SHA-256 of the stored bytes; exactly 64 lowercase hexadecimal digits.
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebAssetManifest {
    pub schema_version: u32,
    pub api_version: u32,
    pub files: Vec<WebAssetFile>,
}

impl WebAssetManifest {
    /// Canonical JSON is compact UTF-8 serde JSON in declared field order,
    /// with files sorted by their logical path. No trailing newline is added.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, WebAssetError> {
        let mut manifest = self.clone();
        manifest.canonicalize()?;
        Ok(serde_json::to_vec(&manifest)?)
    }

    pub fn bundle_id(&self) -> Result<String, WebAssetError> {
        Ok(sha256(&self.canonical_bytes()?))
    }

    pub fn canonicalize(&mut self) -> Result<(), WebAssetError> {
        self.files.sort_by(|a, b| a.path.cmp(&b.path));
        self.validate()
    }

    pub fn validate(&self) -> Result<(), WebAssetError> {
        if self.schema_version != WEB_ASSET_SCHEMA_VERSION || self.api_version == 0 {
            return Err(WebAssetError::Invalid("schema or API version"));
        }
        if self.files.is_empty() || self.files.len() > WEB_ASSET_FILE_COUNT_MAX {
            return Err(WebAssetError::Invalid("file count"));
        }
        let mut paths = BTreeSet::new();
        let mut physical = BTreeMap::new();
        let mut total = 0u64;
        for file in &self.files {
            if !valid_logical_path(&file.path) || !paths.insert(&file.path) {
                return Err(WebAssetError::Invalid("file path"));
            }
            if file.content_type.is_empty()
                || file.content_type.len() > 96
                || !file.content_type.bytes().all(|c| (32..=126).contains(&c))
            {
                return Err(WebAssetError::Invalid("content type"));
            }
            if !valid_hash(&file.sha256) || file.size > WEB_ASSET_FILE_SIZE_MAX {
                return Err(WebAssetError::Invalid("file hash or size"));
            }
            if let Some((old_hash, old_size)) = physical.insert(
                content_name(&file.sha256),
                (file.sha256.as_str(), file.size),
            ) {
                if old_hash != file.sha256 || old_size != file.size {
                    return Err(WebAssetError::Collision);
                }
            }
            total = total
                .checked_add(file.size)
                .ok_or(WebAssetError::Invalid("bundle size"))?;
        }
        if total > WEB_ASSET_BUNDLE_SIZE_MAX {
            return Err(WebAssetError::Invalid("bundle size"));
        }
        if serde_json::to_vec(self)?.len() > WEB_ASSET_MANIFEST_SIZE_MAX {
            return Err(WebAssetError::Invalid("manifest size"));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum WebAssetError {
    Invalid(&'static str),
    Corrupt(&'static str),
    Collision,
    NotFound,
    Offset { expected: u64 },
    Capacity { needed: u64, available: u64 },
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for WebAssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(what) => write!(f, "invalid web assets: {what}"),
            Self::Corrupt(what) => write!(f, "corrupt web assets: {what}"),
            Self::Collision => write!(f, "web asset filename hash collision"),
            Self::NotFound => write!(f, "web asset or upload not found"),
            Self::Offset { expected } => write!(f, "unexpected upload offset; expected {expected}"),
            Self::Capacity { needed, available } => write!(
                f,
                "web assets need {needed} bytes; {available} safe bytes available"
            ),
            Self::Io(error) => write!(f, "web asset I/O failed: {error}"),
            Self::Json(error) => write!(f, "web asset JSON failed: {error}"),
        }
    }
}
impl std::error::Error for WebAssetError {}
impl From<std::io::Error> for WebAssetError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<serde_json::Error> for WebAssetError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FileProgress {
    pub path: String,
    pub size: u64,
    pub received: u64,
    pub complete: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UploadStatus {
    pub bundle_id: String,
    pub files: Vec<FileProgress>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BundleStatus {
    pub bundle_id: String,
    pub generation: u64,
    pub manifest: WebAssetManifest,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InstalledVersion {
    pub api_version: u32,
    pub active: Option<BundleStatus>,
    pub previous: Option<BundleStatus>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WebAssetStatus {
    /// API version requested by the running firmware; no cross-API fallback.
    pub api_version: u32,
    pub active: Option<BundleStatus>,
    pub previous: Option<BundleStatus>,
    pub installed: Vec<InstalledVersion>,
    pub pending: Option<UploadStatus>,
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CommitPayload {
    generation: u64,
    bundle_id: String,
    manifest: WebAssetManifest,
}
impl CommitPayload {
    fn status(&self) -> BundleStatus {
        BundleStatus {
            bundle_id: self.bundle_id.clone(),
            generation: self.generation,
            manifest: self.manifest.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PendingPayload {
    bundle_id: String,
    manifest: WebAssetManifest,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckedRecord<T> {
    payload: T,
    checksum: String,
}

#[derive(Debug)]
pub struct WebAssetStore {
    root: PathBuf,
    api_version: u32,
    commits: BTreeMap<u32, [Option<CommitPayload>; 2]>,
    issues: Vec<String>,
}

impl WebAssetStore {
    /// Opens an already mounted root, never creates directories or formats it.
    /// Both manifests and every referenced content hash are verified here.
    pub fn open(root: impl Into<PathBuf>, api_version: u32) -> Result<Self, WebAssetError> {
        if api_version == 0 {
            return Err(WebAssetError::Invalid("API version"));
        }
        let mut store = Self {
            root: root.into(),
            api_version,
            commits: BTreeMap::new(),
            issues: Vec::new(),
        };
        for item in fs::read_dir(&store.root)? {
            super::storage_checkpoint();
            let item = item?;
            let name = item.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some((api, slot)) = parse_commit_name(name) else {
                continue;
            };
            store.commits.entry(api).or_insert_with(|| [None, None]);
            let result = store.read_commit(name, api);
            match result {
                Ok(record) => {
                    store.commits.entry(api).or_insert_with(|| [None, None])[slot] = Some(record);
                }
                Err(error) => store.issues.push(format!("{name}: {error}")),
            }
        }
        Ok(store)
    }

    pub fn status(&self) -> Result<WebAssetStatus, WebAssetError> {
        let installed: Vec<_> = self
            .commits
            .keys()
            .map(|api| self.version_status(*api))
            .collect();
        let current = self.version_status(self.api_version);
        let mut issues = self.issues.clone();
        let pending = match self.pending() {
            Ok(Some(pending)) => match self.upload_status(&pending) {
                Ok(status) => Some(status),
                Err(error) => {
                    issues.push(format!("{PENDING_NAME}: {error}"));
                    None
                }
            },
            Ok(None) => None,
            Err(error) => {
                issues.push(format!("{PENDING_NAME}: {error}"));
                None
            }
        };
        Ok(WebAssetStatus {
            api_version: self.api_version,
            active: current.active,
            previous: current.previous,
            installed,
            pending,
            issues,
        })
    }

    /// `available_bytes` is the platform's safe remaining budget, already below
    /// its filesystem fullness threshold. Check all missing content and both
    /// small metadata writes before accepting the bundle. This does not reserve
    /// space against other filesystem users; subsequent I/O may still fail.
    pub fn begin(
        &mut self,
        mut manifest: WebAssetManifest,
        available_bytes: u64,
    ) -> Result<UploadStatus, WebAssetError> {
        manifest.canonicalize()?;
        self.check_collisions(&manifest)?;
        // Even a currently damaged installed API version occupies its slot:
        // installing another generation must never silently evict that API.
        let mut versions = BTreeSet::new();
        for item in fs::read_dir(&self.root)? {
            super::storage_checkpoint();
            let item = item?;
            if let Some((api, _)) = item.file_name().to_str().and_then(parse_commit_name) {
                versions.insert(api);
            }
        }
        versions.insert(manifest.api_version);
        if versions.len() > WEB_ASSET_API_VERSIONS_MAX {
            return Err(WebAssetError::Invalid("installed API version count"));
        }
        let pending = PendingPayload {
            bundle_id: manifest.bundle_id()?,
            manifest,
        };
        let pending_bytes = record_bytes(&pending)?;
        let commit_bytes = self.commit_bytes(&pending)?;
        let mut needed = pending_bytes.len() as u64 + commit_bytes.len() as u64;
        let mut counted = BTreeSet::new();
        for file in &pending.manifest.files {
            if counted.insert(&file.sha256) {
                // A crash may occur after the last chunk fsync but before its
                // rename. Resume promotes only a full hash-verified partial.
                let partial = self.root.join(partial_name(&file.sha256));
                if fs::metadata(&partial).is_ok_and(|m| m.len() >= file.size) {
                    if verify_file(&partial, file)? {
                        self.promote(file)?;
                    } else {
                        fs::remove_file(partial)?;
                    }
                }
                let progress = self.file_progress(file)?;
                needed += file.size - progress.received;
            }
        }
        require_capacity(needed, available_bytes)?;
        self.write_checked(PENDING_NAME, &pending_bytes)?;
        // Empty files have a real SHA-256 and must be published like other data.
        for file in &pending.manifest.files {
            if file.size == 0 && !self.root.join(content_name(&file.sha256)).exists() {
                if file.sha256 != sha256(&[]) {
                    return Err(WebAssetError::Corrupt("empty file hash"));
                }
                self.write_checked(&partial_name(&file.sha256), &[])?;
                self.promote(file)?;
            }
        }
        self.garbage_collect()?;
        self.upload_status(&pending)
    }

    /// Accepts consecutive chunks or an exact replay of already stored bytes.
    /// File indices refer to canonical (path-sorted) manifest order.
    pub fn write_chunk(
        &mut self,
        bundle_id: &str,
        file_index: usize,
        offset: u64,
        bytes: &[u8],
        available_bytes: u64,
    ) -> Result<FileProgress, WebAssetError> {
        if bytes.len() > WEB_ASSET_CHUNK_SIZE_MAX {
            return Err(WebAssetError::Invalid("chunk size"));
        }
        let pending = self.require_pending(bundle_id)?;
        let file = pending
            .manifest
            .files
            .get(file_index)
            .ok_or(WebAssetError::Invalid("file index"))?;
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or(WebAssetError::Invalid("chunk offset"))?;
        if end > file.size {
            return Err(WebAssetError::Invalid("chunk extends past file"));
        }
        let progress = self.file_progress(file)?;
        let physical = if progress.complete {
            content_name(&file.sha256)
        } else {
            partial_name(&file.sha256)
        };
        if offset < progress.received || progress.complete {
            if end > progress.received {
                return Err(WebAssetError::Offset {
                    expected: progress.received,
                });
            }
            let mut existing = File::open(self.root.join(physical))?;
            existing.seek(SeekFrom::Start(offset))?;
            let mut old = vec![0; bytes.len()];
            existing.read_exact(&mut old)?;
            if old != bytes {
                return Err(WebAssetError::Offset {
                    expected: progress.received,
                });
            }
            return Ok(progress);
        }
        if offset != progress.received {
            return Err(WebAssetError::Offset {
                expected: progress.received,
            });
        }
        require_capacity(
            bytes.len() as u64 + self.commit_bytes(&pending)?.len() as u64,
            available_bytes,
        )?;
        let path = self.root.join(physical);
        let mut target = OpenOptions::new().create(true).append(true).open(&path)?;
        target.write_all(bytes)?;
        target.sync_all()?;
        drop(target);
        // Read back every accepted chunk before acknowledging its offset.
        let mut check = File::open(&path)?;
        check.seek(SeekFrom::Start(offset))?;
        let mut stored = vec![0; bytes.len()];
        check.read_exact(&mut stored)?;
        if stored != bytes {
            return Err(WebAssetError::Corrupt("chunk readback"));
        }
        if end == file.size {
            if !verify_file(&path, file)? {
                fs::remove_file(&path)?;
                return Err(WebAssetError::Corrupt("uploaded file hash"));
            }
            self.promote(file)?;
        }
        self.file_progress(file)
    }

    /// Rechecks all full hashes, then overwrites only the older commit slot.
    /// The previous compatible bundle remains usable throughout publication.
    /// Platforms with a fullness threshold should use [`Self::commit_with_capacity`].
    pub fn commit(&mut self, bundle_id: &str) -> Result<BundleStatus, WebAssetError> {
        self.commit_with_capacity(bundle_id, u64::MAX)
    }

    /// Rechecks the current safe capacity immediately before publishing. The
    /// platform must obtain that capacity under the same lock as this operation.
    /// An already-active bundle needs no space unless pending metadata cleanup
    /// remains; that cleanup reserves one pending-record length conservatively.
    pub fn commit_with_capacity(
        &mut self,
        bundle_id: &str,
        available_bytes: u64,
    ) -> Result<BundleStatus, WebAssetError> {
        let active = self.commits.keys().find_map(|api| {
            self.version_status(*api)
                .active
                .filter(|active| active.bundle_id == bundle_id)
        });
        if let Some(active) = active {
            for file in &active.manifest.files {
                if !verify_file(&self.root.join(content_name(&file.sha256)), file)? {
                    return Err(WebAssetError::Corrupt("committed file hash"));
                }
            }
            if let Some(pending) = self
                .pending()
                .ok()
                .flatten()
                .filter(|p| p.bundle_id == bundle_id)
            {
                require_capacity(record_bytes(&pending)?.len() as u64, available_bytes)?;
                remove_if_present(&self.root.join(PENDING_NAME))?;
                self.garbage_collect()?;
            }
            return Ok(active);
        }
        let pending = self.require_pending(bundle_id)?;
        for file in &pending.manifest.files {
            if !verify_file(&self.root.join(content_name(&file.sha256)), file)? {
                return Err(WebAssetError::Corrupt("incomplete bundle"));
            }
        }
        let record = self.next_commit(&pending)?;
        self.publish(record, available_bytes)
    }

    /// Roll back only this firmware's compatible resource API version.
    /// Platforms with a fullness threshold should use [`Self::rollback_with_capacity`].
    pub fn rollback(&mut self) -> Result<BundleStatus, WebAssetError> {
        self.rollback_with_capacity(u64::MAX)
    }

    /// Publishes the previous compatible bundle only if its new commit record
    /// fits the platform's current safe capacity, checked under its storage lock.
    pub fn rollback_with_capacity(
        &mut self,
        available_bytes: u64,
    ) -> Result<BundleStatus, WebAssetError> {
        let previous = self
            .version_status(self.api_version)
            .previous
            .ok_or(WebAssetError::NotFound)?;
        for file in &previous.manifest.files {
            if !verify_file(&self.root.join(content_name(&file.sha256)), file)? {
                return Err(WebAssetError::Corrupt("rollback bundle"));
            }
        }
        let pending = PendingPayload {
            bundle_id: previous.bundle_id,
            manifest: previous.manifest,
        };
        let record = self.next_commit(&pending)?;
        self.publish(record, available_bytes)
    }

    /// Versioned URLs may request either retained commit of any installed API.
    /// Selection of the current firmware's compatible API is done via status().
    pub fn open_asset(
        &self,
        bundle_id: &str,
        path: &str,
    ) -> Result<(WebAssetFile, File), WebAssetError> {
        if !valid_hash(bundle_id) || !valid_logical_path(path) {
            return Err(WebAssetError::NotFound);
        }
        for record in self
            .commits
            .values()
            .flat_map(|slots| slots.iter().flatten())
        {
            if record.bundle_id != bundle_id {
                continue;
            }
            if let Some(metadata) = record.manifest.files.iter().find(|file| file.path == path) {
                let file = File::open(self.root.join(content_name(&metadata.sha256)))?;
                if file.metadata()?.len() != metadata.size {
                    return Err(WebAssetError::Corrupt("asset length"));
                }
                return Ok((metadata.clone(), file));
            }
        }
        Err(WebAssetError::NotFound)
    }

    /// Removes only exact names in this module's content/partial namespace.
    /// Retains both commits of EVERY API version and the current pending upload.
    pub fn garbage_collect(&mut self) -> Result<(), WebAssetError> {
        let mut keep = BTreeSet::new();
        for record in self
            .commits
            .values()
            .flat_map(|slots| slots.iter().flatten())
        {
            for file in &record.manifest.files {
                keep.insert(content_name(&file.sha256));
            }
        }
        // Preserve references from intact commit metadata even if one of its
        // data files is damaged. An unrelated API update must not destroy the
        // remaining data of that installed API version.
        for item in fs::read_dir(&self.root)? {
            super::storage_checkpoint();
            let item = item?;
            let name = item.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some((api, _)) = parse_commit_name(name) else {
                continue;
            };
            if let Ok(record) = self.read_commit_metadata(name, api) {
                for file in &record.manifest.files {
                    keep.insert(content_name(&file.sha256));
                }
            }
        }
        if let Ok(Some(pending)) = self.pending() {
            for file in &pending.manifest.files {
                keep.insert(content_name(&file.sha256));
                keep.insert(partial_name(&file.sha256));
            }
        }
        for item in fs::read_dir(&self.root)? {
            super::storage_checkpoint();
            let item = item?;
            let name = item.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if owned_content_name(name) && !keep.contains(name) {
                fs::remove_file(self.root.join(name))?;
            }
        }
        Ok(())
    }

    fn publish(
        &mut self,
        record: CommitPayload,
        available_bytes: u64,
    ) -> Result<BundleStatus, WebAssetError> {
        let bytes = record_bytes(&record)?;
        require_capacity(bytes.len() as u64, available_bytes)?;
        let api = record.manifest.api_version;
        let slots = self.commits.entry(api).or_insert_with(|| [None, None]);
        let slot = match (&slots[0], &slots[1]) {
            (None, _) => 0,
            (_, None) => 1,
            (Some(a), Some(b)) if a.generation <= b.generation => 0,
            _ => 1,
        };
        let name = commit_name(api, slot);
        self.write_checked(&name, &bytes)?;
        let verified = self.read_commit(&name, api)?;
        let status = verified.status();
        self.commits.get_mut(&api).expect("version inserted")[slot] = Some(verified);
        if self
            .pending()
            .ok()
            .flatten()
            .is_some_and(|p| p.bundle_id == record.bundle_id)
        {
            remove_if_present(&self.root.join(PENDING_NAME))?;
        }
        self.garbage_collect()?;
        Ok(status)
    }

    fn version_status(&self, api_version: u32) -> InstalledVersion {
        let mut records: Vec<_> = self
            .commits
            .get(&api_version)
            .into_iter()
            .flat_map(|slots| slots.iter().flatten())
            .collect();
        records.sort_by_key(|r| std::cmp::Reverse(r.generation));
        let active = records.first().map(|r| r.status());
        let previous = records
            .iter()
            .skip(1)
            .find(|r| active.as_ref().is_some_and(|a| a.bundle_id != r.bundle_id))
            .map(|r| r.status());
        InstalledVersion {
            api_version,
            active,
            previous,
        }
    }

    fn next_commit(&self, pending: &PendingPayload) -> Result<CommitPayload, WebAssetError> {
        let generation = self
            .commits
            .get(&pending.manifest.api_version)
            .into_iter()
            .flat_map(|slots| slots.iter().flatten())
            .map(|r| r.generation)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(WebAssetError::Invalid("generation exhausted"))?;
        Ok(CommitPayload {
            generation,
            bundle_id: pending.bundle_id.clone(),
            manifest: pending.manifest.clone(),
        })
    }

    fn commit_bytes(&self, pending: &PendingPayload) -> Result<Vec<u8>, WebAssetError> {
        record_bytes(&self.next_commit(pending)?)
    }

    fn require_pending(&self, bundle_id: &str) -> Result<PendingPayload, WebAssetError> {
        self.pending()?
            .filter(|p| p.bundle_id == bundle_id)
            .ok_or(WebAssetError::NotFound)
    }

    fn pending(&self) -> Result<Option<PendingPayload>, WebAssetError> {
        let path = self.root.join(PENDING_NAME);
        let bytes = match bounded_read(&path) {
            Ok(bytes) => bytes,
            Err(WebAssetError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None)
            }
            Err(error) => return Err(error),
        };
        let pending: PendingPayload = read_record(&bytes)?;
        validate_identity(&pending.bundle_id, &pending.manifest)?;
        Ok(Some(pending))
    }

    fn read_commit(&self, name: &str, api: u32) -> Result<CommitPayload, WebAssetError> {
        let record = self.read_commit_metadata(name, api)?;
        for file in &record.manifest.files {
            if !verify_file(&self.root.join(content_name(&file.sha256)), file)? {
                return Err(WebAssetError::Corrupt("committed file hash"));
            }
        }
        Ok(record)
    }

    fn read_commit_metadata(&self, name: &str, api: u32) -> Result<CommitPayload, WebAssetError> {
        let record: CommitPayload = read_record(&bounded_read(&self.root.join(name))?)?;
        validate_identity(&record.bundle_id, &record.manifest)?;
        if record.generation == 0 || record.manifest.api_version != api {
            return Err(WebAssetError::Corrupt("commit identity"));
        }
        Ok(record)
    }

    fn check_collisions(&self, manifest: &WebAssetManifest) -> Result<(), WebAssetError> {
        let mut known = BTreeMap::new();
        for record in self
            .commits
            .values()
            .flat_map(|slots| slots.iter().flatten())
        {
            for file in &record.manifest.files {
                known.insert(content_name(&file.sha256), file.sha256.clone());
            }
        }
        if let Ok(Some(pending)) = self.pending() {
            for file in &pending.manifest.files {
                known.insert(content_name(&file.sha256), file.sha256.clone());
            }
        }
        for file in &manifest.files {
            if known
                .get(&content_name(&file.sha256))
                .is_some_and(|hash| hash != &file.sha256)
            {
                return Err(WebAssetError::Collision);
            }
        }
        Ok(())
    }

    fn upload_status(&self, pending: &PendingPayload) -> Result<UploadStatus, WebAssetError> {
        Ok(UploadStatus {
            bundle_id: pending.bundle_id.clone(),
            files: pending
                .manifest
                .files
                .iter()
                .map(|file| self.file_progress(file))
                .collect::<Result<_, _>>()?,
        })
    }

    fn file_progress(&self, file: &WebAssetFile) -> Result<FileProgress, WebAssetError> {
        let complete_path = self.root.join(content_name(&file.sha256));
        let (received, complete) = match fs::metadata(&complete_path) {
            Ok(_) => {
                if !verify_file(&complete_path, file)? {
                    return Err(WebAssetError::Collision);
                }
                (file.size, true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let received = match fs::metadata(self.root.join(partial_name(&file.sha256))) {
                    Ok(metadata) => metadata.len(),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
                    Err(error) => return Err(error.into()),
                };
                if received > file.size {
                    return Err(WebAssetError::Corrupt("partial length"));
                }
                (received, false)
            }
            Err(error) => return Err(error.into()),
        };
        Ok(FileProgress {
            path: file.path.clone(),
            size: file.size,
            received,
            complete,
        })
    }

    fn promote(&self, file: &WebAssetFile) -> Result<(), WebAssetError> {
        let source = self.root.join(partial_name(&file.sha256));
        let target = self.root.join(content_name(&file.sha256));
        if !verify_file(&source, file)? {
            return Err(WebAssetError::Corrupt("partial hash"));
        }
        match fs::metadata(&target) {
            Ok(_) => {
                if !verify_file(&target, file)? {
                    return Err(WebAssetError::Collision);
                }
                remove_if_present(&source)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::rename(&source, &target)?
            }
            Err(error) => return Err(error.into()),
        }
        if !verify_file(&target, file)? {
            return Err(WebAssetError::Corrupt("content readback"));
        }
        Ok(())
    }

    fn write_checked(&self, name: &str, bytes: &[u8]) -> Result<(), WebAssetError> {
        let path = self.root.join(name);
        let mut file = File::create(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if bounded_read(&path)? != bytes {
            return Err(WebAssetError::Corrupt("metadata readback"));
        }
        Ok(())
    }
}

fn record_bytes<T: Serialize>(payload: &T) -> Result<Vec<u8>, WebAssetError> {
    Ok(serde_json::to_vec(&CheckedRecord {
        checksum: sha256(&serde_json::to_vec(payload)?),
        payload,
    })?)
}
fn read_record<T: serde::de::DeserializeOwned + Serialize>(
    bytes: &[u8],
) -> Result<T, WebAssetError> {
    let record: CheckedRecord<T> = serde_json::from_slice(bytes)?;
    if record.checksum != sha256(&serde_json::to_vec(&record.payload)?) {
        return Err(WebAssetError::Corrupt("manifest checksum"));
    }
    Ok(record.payload)
}
fn validate_identity(bundle_id: &str, manifest: &WebAssetManifest) -> Result<(), WebAssetError> {
    if !valid_hash(bundle_id) || manifest.bundle_id()? != bundle_id {
        return Err(WebAssetError::Corrupt("bundle identity"));
    }
    let mut sorted = manifest.clone();
    sorted.canonicalize()?;
    if &sorted != manifest {
        return Err(WebAssetError::Corrupt("manifest order"));
    }
    Ok(())
}
fn require_capacity(needed: u64, available: u64) -> Result<(), WebAssetError> {
    if needed > available {
        Err(WebAssetError::Capacity { needed, available })
    } else {
        Ok(())
    }
}
fn bounded_read(path: &Path) -> Result<Vec<u8>, WebAssetError> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(RECORD_SIZE_MAX as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > RECORD_SIZE_MAX {
        return Err(WebAssetError::Corrupt("metadata size"));
    }
    Ok(bytes)
}
fn verify_file(path: &Path, metadata: &WebAssetFile) -> Result<bool, WebAssetError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if file.metadata()?.len() != metadata.size {
        return Ok(false);
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 1024];
    let mut since_checkpoint = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        since_checkpoint += read;
        if since_checkpoint >= WEB_ASSET_CHUNK_SIZE_MAX {
            super::storage_checkpoint();
            since_checkpoint = 0;
        }
    }
    Ok(hex(&hash.finalize()) == metadata.sha256)
}
fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_logical_path(path: &str) -> bool {
    path.len() <= 160
        && path.starts_with('/')
        && path[1..].split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        })
}
fn content_name(hash: &str) -> String {
    format!("wa-f-{}", &hash[..24])
}
fn partial_name(hash: &str) -> String {
    format!("wa-u-{}", &hash[..24])
}
fn commit_name(api: u32, slot: usize) -> String {
    format!("wa-c-{api:08x}-{slot}.json")
}
fn parse_commit_name(name: &str) -> Option<(u32, usize)> {
    let middle = name.strip_prefix("wa-c-")?.strip_suffix(".json")?;
    let (api, slot) = middle.split_once('-')?;
    if api.len() != 8
        || !api
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let api = u32::from_str_radix(api, 16).ok()?;
    let slot = match slot {
        "0" => 0,
        "1" => 1,
        _ => return None,
    };
    (api != 0).then_some((api, slot))
}
fn owned_content_name(name: &str) -> bool {
    name.strip_prefix("wa-f-")
        .or_else(|| name.strip_prefix("wa-u-"))
        .is_some_and(|hash| {
            hash.len() == 24
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}
fn remove_if_present(path: &Path) -> Result<(), WebAssetError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "panelet-web-assets-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn open(&self, api: u32) -> WebAssetStore {
            WebAssetStore::open(&self.0, api).unwrap()
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn manifest(api: u32, data: &[u8]) -> WebAssetManifest {
        WebAssetManifest {
            schema_version: 1,
            api_version: api,
            files: vec![WebAssetFile {
                path: "/editor/index.html".into(),
                content_type: "text/html; charset=utf-8".into(),
                encoding: WebAssetEncoding::Identity,
                size: data.len() as u64,
                sha256: sha256(data),
            }],
        }
    }
    fn install(store: &mut WebAssetStore, api: u32, data: &[u8]) -> BundleStatus {
        let pending = store.begin(manifest(api, data), u64::MAX).unwrap();
        for (index, chunk) in data.chunks(WEB_ASSET_CHUNK_SIZE_MAX).enumerate() {
            store
                .write_chunk(
                    &pending.bundle_id,
                    0,
                    (index * WEB_ASSET_CHUNK_SIZE_MAX) as u64,
                    chunk,
                    u64::MAX,
                )
                .unwrap();
        }
        store.commit(&pending.bundle_id).unwrap()
    }

    #[test]
    fn canonical_manifest_rejects_unsafe_paths_and_metadata() {
        let mut m = manifest(1, b"a");
        let first = m.files[0].clone();
        let mut other = first.clone();
        other.path = "/a.js".into();
        m.files.push(other);
        let id = m.bundle_id().unwrap();
        m.files.reverse();
        assert_eq!(m.bundle_id().unwrap(), id);
        assert!(String::from_utf8(m.canonical_bytes().unwrap())
            .unwrap()
            .starts_with("{\"schema_version\":1,\"api_version\":1,\"files\":[{\"path\":\"/a.js\""));
        for path in ["", "/", "../a", "/../a", "/a//b", "/a?b", "/a%2fb", "/a\\b"] {
            let mut invalid = manifest(1, b"a");
            invalid.files[0].path = path.into();
            assert!(invalid.validate().is_err(), "{path}");
        }
        m.files[0].content_type = "text/html\r\nX: x".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn resume_replay_and_restart_preserve_exact_bytes() {
        let root = Temp::new();
        let mut store = root.open(1);
        let m = manifest(1, b"abcdef");
        let pending = store.begin(m.clone(), u64::MAX).unwrap();
        store
            .write_chunk(&pending.bundle_id, 0, 0, b"abc", u64::MAX)
            .unwrap();
        assert_eq!(
            store
                .write_chunk(&pending.bundle_id, 0, 0, b"abc", 0)
                .unwrap()
                .received,
            3
        );
        assert!(matches!(
            store.write_chunk(&pending.bundle_id, 0, 0, b"abd", u64::MAX),
            Err(WebAssetError::Offset { expected: 3 })
        ));
        assert!(store.commit(&pending.bundle_id).is_err());
        drop(store);
        let mut store = root.open(1);
        assert_eq!(store.begin(m, u64::MAX).unwrap().files[0].received, 3);
        assert!(store
            .write_chunk(&pending.bundle_id, 0, 4, b"ef", u64::MAX)
            .is_err());
        store
            .write_chunk(&pending.bundle_id, 0, 3, b"def", u64::MAX)
            .unwrap();
        let committed = store.commit(&pending.bundle_id).unwrap();
        drop(store);
        let store = root.open(1);
        assert_eq!(store.status().unwrap().active.unwrap(), committed);
        let (_, mut file) = store
            .open_asset(&pending.bundle_id, "/editor/index.html")
            .unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"abcdef");
    }

    #[test]
    fn bad_hash_is_never_committed_and_upload_can_restart() {
        let root = Temp::new();
        let mut store = root.open(1);
        let m = manifest(1, b"good");
        let upload = store.begin(m.clone(), u64::MAX).unwrap();
        assert!(matches!(
            store.write_chunk(&upload.bundle_id, 0, 0, b"evil", u64::MAX),
            Err(WebAssetError::Corrupt(_))
        ));
        assert!(store.commit(&upload.bundle_id).is_err());
        assert!(store.status().unwrap().active.is_none());
        assert_eq!(store.begin(m, u64::MAX).unwrap().files[0].received, 0);
        store
            .write_chunk(&upload.bundle_id, 0, 0, b"good", u64::MAX)
            .unwrap();
        store.commit(&upload.bundle_id).unwrap();
    }

    #[test]
    fn rollback_and_torn_manifest_keep_a_valid_previous_bundle() {
        let root = Temp::new();
        let mut store = root.open(1);
        let first = install(&mut store, 1, b"first");
        let second = install(&mut store, 1, b"second");
        assert_eq!(store.rollback().unwrap().bundle_id, first.bundle_id);
        assert_eq!(store.rollback().unwrap().bundle_id, second.bundle_id);
        let newest_slot = store.commits[&1]
            .iter()
            .position(|r| r.as_ref().unwrap().generation == 4)
            .unwrap();
        fs::write(root.0.join(commit_name(1, newest_slot)), b"{\"payload\":").unwrap();
        drop(store);
        let reopened = root.open(1);
        assert_eq!(
            reopened.status().unwrap().active.unwrap().bundle_id,
            first.bundle_id
        );
        assert_eq!(reopened.status().unwrap().issues.len(), 1);
    }

    #[test]
    fn corrupted_latest_data_falls_back_and_checksum_is_required() {
        let root = Temp::new();
        let mut store = root.open(1);
        let old = install(&mut store, 1, b"old");
        let latest = install(&mut store, 1, b"new");
        fs::write(
            root.0.join(content_name(&latest.manifest.files[0].sha256)),
            b"bad",
        )
        .unwrap();
        let reopened = root.open(1);
        assert_eq!(
            reopened.status().unwrap().active.unwrap().bundle_id,
            old.bundle_id
        );
        let name = root.0.join(commit_name(1, 0));
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&name).unwrap()).unwrap();
        value["payload"]["generation"] = 900.into();
        fs::write(name, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(root.open(1).status().unwrap().active.is_none());
    }

    #[test]
    fn independent_api_versions_share_data_and_survive_gc() {
        let root = Temp::new();
        let mut store = root.open(1);
        let v1 = install(&mut store, 1, b"shared");
        let v2 = install(&mut store, 2, b"shared");
        assert_ne!(v1.bundle_id, v2.bundle_id);
        let content_count = fs::read_dir(&root.0)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_str()
                    .unwrap()
                    .starts_with("wa-f-")
            })
            .count();
        assert_eq!(content_count, 1);
        install(&mut store, 2, b"v2-next");
        install(&mut store, 2, b"v2-latest");
        assert_eq!(
            store.status().unwrap().active.unwrap().bundle_id,
            v1.bundle_id
        );
        assert!(store
            .open_asset(&v1.bundle_id, "/editor/index.html")
            .is_ok());
        assert!(root.open(3).status().unwrap().active.is_none());
        install(&mut store, 3, b"three");
        install(&mut store, 4, b"four");
        assert!(store.begin(manifest(5, b"five"), u64::MAX).is_err());
        assert_eq!(root.open(1).status().unwrap().installed.len(), 4);
    }

    #[test]
    fn capacity_rejection_does_not_damage_pending_or_committed_data() {
        let root = Temp::new();
        let mut store = root.open(1);
        let old = install(&mut store, 1, b"old");
        assert!(matches!(
            store.begin(manifest(1, b"new"), 1),
            Err(WebAssetError::Capacity { .. })
        ));
        let upload = store.begin(manifest(1, b"new"), u64::MAX).unwrap();
        assert!(matches!(
            store.write_chunk(&upload.bundle_id, 0, 0, b"new", 1),
            Err(WebAssetError::Capacity { .. })
        ));
        assert_eq!(
            store.status().unwrap().pending.unwrap().files[0].received,
            0
        );
        assert_eq!(
            store.status().unwrap().active.unwrap().bundle_id,
            old.bundle_id
        );
    }

    #[test]
    fn gc_stays_in_namespace_and_retains_only_two_commits_per_api() {
        let root = Temp::new();
        let mut store = root.open(1);
        for name in [
            "format.v1",
            "active.txt",
            "widget.n4w",
            "private.json",
            "shared.json",
            "wa-f-user-data",
            "wa-p-user",
        ] {
            fs::write(root.0.join(name), b"user data").unwrap();
        }
        let first = install(&mut store, 1, b"first");
        let second = install(&mut store, 1, b"second");
        install(&mut store, 1, b"third");
        assert!(!root
            .0
            .join(content_name(&first.manifest.files[0].sha256))
            .exists());
        assert!(root
            .0
            .join(content_name(&second.manifest.files[0].sha256))
            .exists());
        for name in [
            "format.v1",
            "active.txt",
            "widget.n4w",
            "private.json",
            "shared.json",
            "wa-f-user-data",
            "wa-p-user",
        ] {
            assert_eq!(fs::read(root.0.join(name)).unwrap(), b"user data");
        }
        for item in fs::read_dir(&root.0).unwrap() {
            assert!(item.unwrap().file_name().len() <= 31);
        }
    }

    #[test]
    fn hash_prefix_collision_never_overwrites_existing_file() {
        let root = Temp::new();
        let mut store = root.open(1);
        let installed = install(&mut store, 1, b"original");
        let mut colliding = manifest(1, b"other");
        colliding.files[0].sha256 = format!(
            "{}{}",
            &installed.manifest.files[0].sha256[..24],
            "0".repeat(40)
        );
        assert!(matches!(
            store.begin(colliding, u64::MAX),
            Err(WebAssetError::Collision)
        ));
        assert!(store
            .open_asset(&installed.bundle_id, "/editor/index.html")
            .is_ok());
    }

    #[test]
    fn chunk_and_bundle_limits_are_enforced_and_empty_files_work() {
        let root = Temp::new();
        let mut store = root.open(1);
        let upload = store.begin(manifest(1, &[0; 40000]), u64::MAX).unwrap();
        assert!(store
            .write_chunk(
                &upload.bundle_id,
                0,
                0,
                &[0; WEB_ASSET_CHUNK_SIZE_MAX + 1],
                u64::MAX
            )
            .is_err());
        let mut m = manifest(1, b"x");
        m.files[0].size = WEB_ASSET_FILE_SIZE_MAX + 1;
        assert!(m.validate().is_err());
        m.files[0].size = WEB_ASSET_FILE_SIZE_MAX;
        let mut other = m.files[0].clone();
        other.path = "/other".into();
        m.files.push(other);
        assert!(m.validate().is_err());
        let empty = install(&mut store, 1, b"");
        assert!(store
            .open_asset(&empty.bundle_id, "/editor/index.html")
            .is_ok());
    }

    #[test]
    fn interrupted_pending_metadata_does_not_replace_active_commit() {
        let root = Temp::new();
        let mut store = root.open(1);
        let active = install(&mut store, 1, b"active");
        fs::write(root.0.join(PENDING_NAME), b"broken").unwrap();
        assert_eq!(
            root.open(1).status().unwrap().active.unwrap().bundle_id,
            active.bundle_id
        );
        assert!(root.open(1).status().unwrap().pending.is_none());
        store.begin(manifest(1, b"replacement"), u64::MAX).unwrap();
    }

    #[test]
    fn complete_partial_recovers_after_last_chunk_interruption() {
        let root = Temp::new();
        let mut store = root.open(1);
        let m = manifest(1, b"complete");
        let upload = store.begin(m.clone(), u64::MAX).unwrap();
        // Model power loss after writing the full partial and before rename.
        fs::write(root.0.join(partial_name(&m.files[0].sha256)), b"complete").unwrap();
        drop(store);
        let mut store = root.open(1);
        assert!(store.begin(m.clone(), u64::MAX).unwrap().files[0].complete);
        let first = store.commit(&upload.bundle_id).unwrap();
        store.begin(m, u64::MAX).unwrap();
        assert_eq!(store.commit(&upload.bundle_id).unwrap(), first);
        assert!(store.status().unwrap().pending.is_none());
    }

    #[test]
    fn malformed_complete_partial_is_discarded_and_can_be_reuploaded() {
        let root = Temp::new();
        let mut store = root.open(1);
        let m = manifest(1, b"valid");
        let upload = store.begin(m.clone(), u64::MAX).unwrap();
        fs::write(root.0.join(partial_name(&m.files[0].sha256)), b"wrong").unwrap();
        let resumed = store.begin(m, u64::MAX).unwrap();
        assert_eq!(resumed.files[0].received, 0);
        assert!(!resumed.files[0].complete);
        store
            .write_chunk(&upload.bundle_id, 0, 0, b"valid", u64::MAX)
            .unwrap();
        store.commit(&upload.bundle_id).unwrap();
    }

    #[test]
    fn unrelated_version_gc_preserves_damaged_versions_intact_files() {
        let root = Temp::new();
        let mut store = root.open(1);
        let mut m = manifest(1, b"first");
        let mut second = manifest(1, b"second").files.remove(0);
        second.path = "/other".into();
        m.files.push(second);
        m.canonicalize().unwrap();
        let upload = store.begin(m.clone(), u64::MAX).unwrap();
        store
            .write_chunk(&upload.bundle_id, 0, 0, b"first", u64::MAX)
            .unwrap();
        store
            .write_chunk(&upload.bundle_id, 1, 0, b"second", u64::MAX)
            .unwrap();
        store.commit(&upload.bundle_id).unwrap();
        fs::write(root.0.join(content_name(&m.files[0].sha256)), b"wrong").unwrap();
        let mut reopened = root.open(2);
        assert_eq!(reopened.status().unwrap().installed.len(), 1);
        assert!(reopened.status().unwrap().installed[0].active.is_none());
        install(&mut reopened, 2, b"v2");
        assert_eq!(
            fs::read(root.0.join(content_name(&m.files[1].sha256))).unwrap(),
            b"second"
        );
    }

    #[test]
    fn opening_missing_root_never_creates_a_directory() {
        let root = Temp::new();
        let missing = root.0.join("missing");
        assert!(WebAssetStore::open(&missing, 1).is_err());
        assert!(!missing.exists());
    }

    #[test]
    fn corrupt_pending_data_does_not_hide_the_committed_bundle() {
        let root = Temp::new();
        let mut store = root.open(1);
        let active = install(&mut store, 1, b"active");
        let pending_manifest = manifest(1, b"pending");
        store.begin(pending_manifest.clone(), u64::MAX).unwrap();
        fs::write(
            root.0.join(partial_name(&pending_manifest.files[0].sha256)),
            b"invalid-too-long",
        )
        .unwrap();
        let status = root.open(1).status().unwrap();
        assert_eq!(status.active.unwrap().bundle_id, active.bundle_id);
        assert!(status.pending.is_none());
        assert_eq!(status.issues.len(), 1);
    }

    #[test]
    fn bounded_commit_rechecks_capacity_and_preserves_every_installed_version() {
        let root = Temp::new();
        let mut store = root.open(1);
        install(&mut store, 1, b"api-one");
        install(&mut store, 2, b"api-two");
        let upload = store.begin(manifest(2, b"replacement"), u64::MAX).unwrap();
        store
            .write_chunk(&upload.bundle_id, 0, 0, b"replacement", u64::MAX)
            .unwrap();
        let before = store.status().unwrap();
        let required = store
            .commit_bytes(&store.require_pending(&upload.bundle_id).unwrap())
            .unwrap()
            .len() as u64;
        assert!(matches!(
            store.commit_with_capacity(&upload.bundle_id, required - 1),
            Err(WebAssetError::Capacity { needed, available }) if needed == required && available == required - 1
        ));
        assert_eq!(store.status().unwrap(), before);
        assert_eq!(root.open(1).status().unwrap(), before);
        let published = store
            .commit_with_capacity(&upload.bundle_id, required)
            .unwrap();
        assert_eq!(published.bundle_id, upload.bundle_id);
        assert_eq!(store.status().unwrap().active, before.active);
        assert_eq!(
            store.status().unwrap().installed[1].previous,
            before.installed[1].active
        );
    }

    #[test]
    fn bounded_rollback_preserves_active_previous_and_other_apis_when_full() {
        let root = Temp::new();
        let mut store = root.open(1);
        install(&mut store, 1, b"first");
        install(&mut store, 1, b"second");
        install(&mut store, 2, b"other-api");
        let before = store.status().unwrap();
        let previous = before.previous.clone().unwrap();
        let pending = PendingPayload {
            bundle_id: previous.bundle_id.clone(),
            manifest: previous.manifest.clone(),
        };
        let required = store.commit_bytes(&pending).unwrap().len() as u64;
        assert!(matches!(
            store.rollback_with_capacity(required - 1),
            Err(WebAssetError::Capacity { needed, available }) if needed == required && available == required - 1
        ));
        assert_eq!(store.status().unwrap(), before);
        assert_eq!(root.open(1).status().unwrap(), before);
        assert_eq!(
            store.rollback_with_capacity(required).unwrap().bundle_id,
            previous.bundle_id
        );
        assert_eq!(store.status().unwrap().installed[1], before.installed[1]);
    }

    #[test]
    fn bounded_idempotent_commit_needs_space_only_for_pending_cleanup() {
        let root = Temp::new();
        let mut store = root.open(1);
        let active = install(&mut store, 1, b"already-active");
        assert_eq!(
            store.commit_with_capacity(&active.bundle_id, 0).unwrap(),
            active
        );
        store.begin(active.manifest.clone(), u64::MAX).unwrap();
        let before = store.status().unwrap();
        let pending = store.require_pending(&active.bundle_id).unwrap();
        let required = record_bytes(&pending).unwrap().len() as u64;
        assert!(matches!(
            store.commit_with_capacity(&active.bundle_id, required - 1),
            Err(WebAssetError::Capacity { needed, .. }) if needed == required
        ));
        assert_eq!(store.status().unwrap(), before);
        assert_eq!(
            store
                .commit_with_capacity(&active.bundle_id, required)
                .unwrap(),
            active
        );
        assert!(store.status().unwrap().pending.is_none());
        assert_eq!(
            store.commit_with_capacity(&active.bundle_id, 0).unwrap(),
            active
        );
    }
}
