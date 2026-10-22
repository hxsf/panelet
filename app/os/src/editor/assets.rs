// SPDX-License-Identifier: MPL-2.0

//! Explicit local integration mode for the firmware's resource protocol.
//!
//! This uses real files and WebAssetStore publication/recovery, but the capacity
//! is a simulated 8 MiB partition. It never discovers or writes an ESP device.
use super::transport::{editor_css, Reply, Request};
use panelet_widget_store::web_assets::{
    WebAssetEncoding, WebAssetError, WebAssetManifest, WebAssetStore, WEB_ASSET_CHUNK_SIZE_MAX,
    WEB_ASSET_MANIFEST_SIZE_MAX,
};
use serde_json::{json, Value};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const CAPACITY: u64 = 8 * 1024 * 1024;
const SAFE_CAPACITY: u64 = CAPACITY * 3 / 4;
const TOKEN: &str = "preview-token";

pub(super) struct LocalAssets {
    root: PathBuf,
    api_version: u32,
    store: Mutex<WebAssetStore>,
}

impl LocalAssets {
    pub(super) fn open(root: PathBuf) -> io::Result<Self> {
        // Unlike the firmware's mounted SPIFFS root, this is an explicitly
        // supplied local development directory and may be created here.
        fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let api_version = include_str!("../../web-assets-api-version")
            .trim()
            .parse::<u32>()
            .map_err(io::Error::other)?;
        let store = WebAssetStore::open(&root, api_version).map_err(io::Error::other)?;
        Ok(Self {
            root,
            api_version,
            store: Mutex::new(store),
        })
    }

    pub(super) fn route(&self, request: Request) -> Reply {
        let path = request.path.split('?').next().unwrap_or("");
        match (request.method.as_str(), path) {
            ("GET", "/" | "/editor") => {
                let page = include_str!("editor.html").replacen(
                    "<html lang=\"zh-CN\">",
                    &format!(
                        "<html lang=\"zh-CN\" data-preview-engine=\"wasm\" data-assets-api=\"{}\">",
                        self.api_version
                    ),
                    1,
                );
                return Reply::bytes(200, "text/html; charset=utf-8", page.into_bytes());
            }
            ("GET", "/editor.css") => return editor_css(),
            ("GET", "/editor-worker.js") => {
                return Reply::bytes(
                    200,
                    "application/javascript",
                    include_bytes!("preview-worker.js").to_vec(),
                );
            }
            _ => {}
        }
        let protected = path == "/web/v1/status"
            || (request.method == "POST"
                && matches!(
                    path,
                    "/web/v1/begin" | "/web/v1/chunk" | "/web/v1/commit" | "/web/v1/rollback"
                ));
        if protected && request.headers.get("x-note4c-token").map(String::as_str) != Some(TOKEN) {
            return failure(
                401,
                "unauthorized",
                "Development token preview-token required",
            );
        }
        if request.method == "POST" && protected {
            let limit = if path == "/web/v1/chunk" {
                WEB_ASSET_CHUNK_SIZE_MAX
            } else {
                WEB_ASSET_MANIFEST_SIZE_MAX
            };
            if request.body.is_empty() || request.body.len() > limit {
                return failure(413, "request_size", "Invalid resource request size");
            }
        }
        let mut store = match self.store.lock() {
            Ok(store) => store,
            Err(_) => return failure(500, "storage_io", "Local resource store is unavailable"),
        };
        let result = match (request.method.as_str(), path) {
            ("GET", "/web/v1/status") => self.status(&store),
            ("GET", "/web/v1/manifest") => self.manifest(&store, &request.path),
            ("POST", "/web/v1/begin") => self.begin(&mut store, &request.body),
            ("POST", "/web/v1/chunk") => self.chunk(&mut store, &request.path, &request.body),
            ("POST", "/web/v1/commit") => self.commit(&mut store, &request.body),
            ("POST", "/web/v1/rollback") => {
                if serde_json::from_slice::<Value>(&request.body).ok() != Some(json!({})) {
                    Err(WebAssetError::Invalid("rollback requires an empty object"))
                } else {
                    self.safe_available()
                        .and_then(|available| store.rollback_with_capacity(available))
                        .and_then(|value| Ok(serde_json::to_value(value)?))
                }
            }
            ("GET", _) if path.starts_with("/web/") => return Self::asset(&store, path, &request),
            // In this mode no request can reach the native render worker or
            // package operations. The browser executes those WASM exports.
            _ => return failure(404, "not_found", "Unknown browser assets endpoint"),
        };
        match result {
            Ok(value) => Reply::json(200, value),
            Err(error) => asset_error(error),
        }
    }

    fn status(&self, store: &WebAssetStore) -> Result<Value, WebAssetError> {
        let used = directory_bytes(&self.root)?;
        Ok(json!({
            "api_version": self.api_version,
            "simulated": true,
            "storage": {
                "total_bytes": CAPACITY, "used_bytes": used,
                "free_bytes": CAPACITY.saturating_sub(used),
                "safe_available_bytes": SAFE_CAPACITY.saturating_sub(used),
                "ready": true, "simulated": true
            },
            "web": store.status()?
        }))
    }

    fn manifest(&self, store: &WebAssetStore, uri: &str) -> Result<Value, WebAssetError> {
        let values = query(uri).ok_or(WebAssetError::Invalid("query"))?;
        if values.len() > 1 || values.iter().any(|(key, _)| *key != "api") {
            return Err(WebAssetError::Invalid("resource API version"));
        }
        let api = values
            .first()
            .map_or(Ok(self.api_version), |(_, value)| value.parse::<u32>())
            .map_err(|_| WebAssetError::Invalid("resource API version"))?;
        if api == 0 {
            return Err(WebAssetError::Invalid("resource API version"));
        }
        let selected = store
            .status()?
            .installed
            .into_iter()
            .find(|version| version.api_version == api)
            .and_then(|version| version.active)
            .ok_or(WebAssetError::NotFound)?;
        Ok(
            json!({"api_version": api, "bundle_id": selected.bundle_id, "files": selected.manifest.files}),
        )
    }

    fn begin(&self, store: &mut WebAssetStore, body: &[u8]) -> Result<Value, WebAssetError> {
        let manifest: WebAssetManifest = serde_json::from_slice(body)?;
        manifest.validate()?;
        store.garbage_collect()?;
        Ok(serde_json::to_value(
            store.begin(manifest, self.safe_available()?)?,
        )?)
    }

    fn chunk(
        &self,
        store: &mut WebAssetStore,
        uri: &str,
        body: &[u8],
    ) -> Result<Value, WebAssetError> {
        let values = query(uri).ok_or(WebAssetError::Invalid("query"))?;
        if values.len() != 3
            || values
                .iter()
                .any(|(key, _)| !["bundle", "file", "offset"].contains(key))
        {
            return Err(WebAssetError::Invalid("chunk query"));
        }
        let value = |key| {
            values
                .iter()
                .find(|(current, _)| *current == key)
                .map(|(_, value)| *value)
                .ok_or(WebAssetError::Invalid("chunk query"))
        };
        let bundle = value("bundle")?;
        if !valid_bundle(bundle) {
            return Err(WebAssetError::Invalid("bundle ID"));
        }
        let index = value("file")?
            .parse()
            .map_err(|_| WebAssetError::Invalid("file index"))?;
        let offset = value("offset")?
            .parse()
            .map_err(|_| WebAssetError::Invalid("offset"))?;
        store.write_chunk(bundle, index, offset, body, self.safe_available()?)?;
        Ok(serde_json::to_value(
            store.status()?.pending.ok_or(WebAssetError::NotFound)?,
        )?)
    }

    fn commit(&self, store: &mut WebAssetStore, body: &[u8]) -> Result<Value, WebAssetError> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Commit {
            bundle_id: String,
        }
        let commit: Commit = serde_json::from_slice(body)?;
        if !valid_bundle(&commit.bundle_id) {
            return Err(WebAssetError::Invalid("bundle ID"));
        }
        Ok(serde_json::to_value(store.commit_with_capacity(
            &commit.bundle_id,
            self.safe_available()?,
        )?)?)
    }

    fn asset(store: &WebAssetStore, path: &str, request: &Request) -> Reply {
        let Some((bundle, asset)) = path
            .strip_prefix("/web/")
            .and_then(|value| value.split_once('/'))
        else {
            return failure(404, "not_found", "Unknown web asset");
        };
        if !valid_bundle(bundle) || asset.is_empty() || asset.contains('%') {
            return failure(404, "not_found", "Unknown web asset");
        }
        let (metadata, file) = match store.open_asset(bundle, &format!("/{asset}")) {
            Ok(value) => value,
            Err(_) => return failure(404, "not_found", "Web asset is missing or incompatible"),
        };
        let etag = format!("\"{}\"", metadata.sha256);
        let headers = |reply: Reply| {
            reply
                .with_header("ETag", &etag)
                .with_header("Cache-Control", "public, max-age=31536000, immutable")
                .with_header(
                    "Content-Security-Policy",
                    "default-src 'none'; sandbox; frame-ancestors 'none'; base-uri 'none'",
                )
        };
        if request.headers.get("if-none-match") == Some(&etag) {
            return headers(Reply::bytes(304, &metadata.content_type, Vec::new()));
        }
        // A local HTTP response is bounded by the same 1.5 MiB file cap. The
        // firmware adapter streams chunks instead; this does not model RAM.
        let mut body = Vec::with_capacity(metadata.size as usize);
        if let Err(error) = file.take(metadata.size + 1).read_to_end(&mut body) {
            return asset_error(error.into());
        }
        if body.len() as u64 != metadata.size {
            return asset_error(WebAssetError::Corrupt("asset length"));
        }
        let mut reply = headers(Reply::bytes(200, metadata.content_type, body));
        if metadata.encoding == WebAssetEncoding::Gzip {
            reply = reply.with_header("Content-Encoding", "gzip");
        }
        reply
    }

    fn safe_available(&self) -> Result<u64, WebAssetError> {
        Ok(SAFE_CAPACITY.saturating_sub(directory_bytes(&self.root)?))
    }
}

fn directory_bytes(root: &Path) -> io::Result<u64> {
    let mut size = 0u64;
    for item in fs::read_dir(root)? {
        let item = item?;
        let kind = item.file_type()?;
        let bytes = if kind.is_dir() {
            directory_bytes(&item.path())?
        } else if kind.is_file() {
            item.metadata()?.len()
        } else {
            0
        };
        size = size
            .checked_add(bytes)
            .ok_or_else(|| io::Error::other("Local file size overflow"))?;
    }
    Ok(size)
}

fn valid_bundle(bundle: &str) -> bool {
    bundle.len() == 64
        && bundle
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn query(uri: &str) -> Option<Vec<(&str, &str)>> {
    let Some((_, query)) = uri.split_once('?') else {
        return Some(Vec::new());
    };
    if query.is_empty() || query.len() > 256 {
        return None;
    }
    let mut pairs = Vec::new();
    for item in query.split('&') {
        let (key, value) = item.split_once('=')?;
        if key.is_empty() || value.is_empty() || pairs.iter().any(|(old, _)| *old == key) {
            return None;
        }
        pairs.push((key, value));
    }
    Some(pairs)
}

fn failure(status: u16, code: &str, message: &str) -> Reply {
    Reply::json(status, json!({"error": {"code": code, "message": message}}))
}

fn asset_error(error: WebAssetError) -> Reply {
    let (status, code) = match &error {
        WebAssetError::Capacity { .. } => (507, "storage_capacity"),
        WebAssetError::Offset { .. } => (409, "upload_offset"),
        WebAssetError::NotFound => (404, "resource_not_found"),
        WebAssetError::Io(_) => (500, "storage_io"),
        _ => (400, "web_assets_invalid"),
    };
    failure(status, code, &error.to_string())
}
