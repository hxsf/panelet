// SPDX-License-Identifier: MPL-2.0

//! Firmware-owned pages and independently versioned browser-engine resources.
//! This module never compiles or executes a widget; the browser does that.
use super::*;
use crate::storage::WEB_ASSETS_API_VERSION;
use panelet_widget_store::web_assets::{
    WebAssetEncoding, WebAssetError, WebAssetManifest, WEB_ASSET_CHUNK_SIZE_MAX,
    WEB_ASSET_MANIFEST_SIZE_MAX,
};

const EDITOR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/editor.html.gz"));
const EDITOR_CSS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/editor.css.gz"));
const EDITOR_WORKER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/editor-worker.js.gz"));
const EDITOR_CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; worker-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'";
// Public assets are data, not trusted management documents. An uploaded HTML or
// SVG must not gain access to the device origin's saved Admin credential.
const ASSET_CSP: &str = "default-src 'none'; sandbox; frame-ancestors 'none'; base-uri 'none'";

pub(super) fn register(
    server: &mut EspHttpServer<'static>,
    broker: Arc<Mutex<Broker>>,
    storage: Arc<AssetStorage>,
) -> Result<(), esp_idf_svc::io::EspIOError> {
    // Pages follow the running firmware/API, unlike the large WASM resource.
    for (path, content_type, bytes) in [
        ("/editor", "text/html; charset=utf-8", EDITOR),
        ("/editor.css", "text/css; charset=utf-8", EDITOR_CSS),
        ("/editor-worker.js", "application/javascript", EDITOR_WORKER),
    ] {
        server.fn_handler(path, Method::Get, move |request| {
            let mut response = request.into_response(
                200,
                Some("OK"),
                &[
                    ("Content-Type", content_type),
                    ("Content-Encoding", "gzip"),
                    ("Cache-Control", "no-store"),
                    ("X-Content-Type-Options", "nosniff"),
                    ("Content-Security-Policy", EDITOR_CSP),
                ],
            )?;
            response.connection().write_all(bytes)
        })?;
    }

    let status_storage = Arc::clone(&storage);
    let status_broker = Arc::clone(&broker);
    server.fn_handler("/web/v1/status", Method::Get, move |request| {
        if !authorized(&request, &status_broker) {
            return failure(request, 401, "unauthorized", "Admin token required");
        }
        let capacity = status_storage.capacity().ok();
        let mut capacity = capacity.and_then(|value| serde_json::to_value(value).ok())
            .unwrap_or_else(|| json!({"total_bytes":null,"used_bytes":null,"free_bytes":null,"safe_available_bytes":null}));
        capacity["ready"] = json!(status_storage.status() == StorageStatus::Ready);
        let web = match status_storage.with_web_assets(|web| web.status()) {
            Ok(status) => serde_json::to_value(status).unwrap_or_default(),
            Err(error) => json!({"api_version":WEB_ASSETS_API_VERSION,"active":null,"previous":null,"installed":[],"pending":null,"issues":[error.to_string()]}),
        };
        let body = serde_json::to_vec(&json!({"api_version":WEB_ASSETS_API_VERSION,"storage":capacity,"web":web})).unwrap_or_default();
        write_json_response(request, 200, &body)
    })?;

    let manifest_storage = Arc::clone(&storage);
    server.fn_handler("/web/v1/manifest", Method::Get, move |request| {
        let query = match query(request.uri()) {
            Some(query) if query.len() <= 1 && query.iter().all(|(k, _)| *k == "api") => query,
            _ => return failure(request, 400, "invalid_query", "Invalid resource version"),
        };
        let api = query.iter().find(|(k, _)| *k == "api")
            .map_or(Some(WEB_ASSETS_API_VERSION), |(_, v)| v.parse::<u32>().ok());
        let Some(api) = api.filter(|api| *api != 0) else {
            return failure(request, 400, "invalid_query", "Invalid resource version");
        };
        let result = manifest_storage.with_web_assets(|web| {
            let status = web.status()?;
            let selected = status.installed.into_iter().find(|v| v.api_version == api)
                .and_then(|version| version.active).ok_or(WebAssetError::NotFound)?;
            Ok(json!({"api_version":api,"bundle_id":selected.bundle_id,"files":selected.manifest.files}))
        });
        match result {
            Ok(value) => write_json_response(request, 200, &serde_json::to_vec(&value).unwrap_or_default()),
            Err(_) => failure(request, 404, "web_assets_missing", "Compatible preview assets are not installed; use Files and storage to install a resource bundle"),
        }
    })?;

    for operation in ["begin", "chunk", "commit", "rollback"] {
        let path = format!("/web/v1/{operation}");
        let storage = Arc::clone(&storage);
        let broker = Arc::clone(&broker);
        server.fn_handler(&path, Method::Post, move |mut request| {
            if !request_same_origin(&request)
                || request.header("Sec-Fetch-Site") == Some("cross-site")
            {
                return failure(request, 403, "cross_origin", "Cross-origin upload denied");
            }
            if !authorized(&request, &broker) {
                return failure(request, 401, "unauthorized", "Admin token required");
            }
            if storage.status() != StorageStatus::Ready {
                return failure(
                    request,
                    409,
                    "storage_not_initialized",
                    "Assets storage is not ready; uploading resources never formats storage",
                );
            }
            let expected = if operation == "chunk" {
                "application/octet-stream"
            } else {
                "application/json"
            };
            if request
                .header("Content-Type")
                .and_then(|v| v.split(';').next())
                .is_none_or(|value| !value.trim().eq_ignore_ascii_case(expected))
            {
                return failure(request, 415, "content_type", "Unsupported Content-Type");
            }
            let limit = if operation == "chunk" {
                WEB_ASSET_CHUNK_SIZE_MAX
            } else {
                WEB_ASSET_MANIFEST_SIZE_MAX
            };
            let length = content_length(&request);
            if length == 0 || length > limit || request.header("Transfer-Encoding").is_some() {
                return failure(
                    request,
                    413,
                    "request_size",
                    "Invalid resource request size",
                );
            }
            let uri = request.uri().to_owned();
            let body = match read_request_body(&mut request, length) {
                Ok(body) if body.len() == length => body,
                _ => {
                    return failure(
                        request,
                        400,
                        "truncated_request",
                        "Incomplete resource request",
                    )
                }
            };
            let result = apply_request(&storage, operation, &uri, &body);
            match result {
                Ok(value) => write_json_response(
                    request,
                    200,
                    &serde_json::to_vec(&value).unwrap_or_default(),
                ),
                Err(error) => {
                    let (status, code) = match &error {
                        WebAssetError::Capacity { .. } => (507, "storage_capacity"),
                        WebAssetError::Offset { .. } => (409, "upload_offset"),
                        WebAssetError::NotFound => (404, "resource_not_found"),
                        WebAssetError::Io(_) => (500, "storage_io"),
                        _ => (400, "web_assets_invalid"),
                    };
                    failure(request, status, code, &error.to_string())
                }
            }
        })?;
    }

    // Register after the fixed /web/v1/... routes. Hash URLs stay pinned to a
    // compatible retained bundle, including while another version is installed.
    server.fn_handler("/web/*", Method::Get, move |request| {
        let path = request.uri().split('?').next().unwrap_or("");
        let Some((bundle, asset)) = path.strip_prefix("/web/").and_then(|v| v.split_once('/'))
        else {
            return failure(request, 404, "not_found", "Unknown web asset");
        };
        if !valid_bundle(bundle) || asset.is_empty() || asset.contains('%') {
            return failure(request, 404, "not_found", "Unknown web asset");
        }
        let result = storage.with_web_assets(|web| web.open_asset(bundle, &format!("/{asset}")));
        let (metadata, mut file) = match result {
            Ok(value) => value,
            Err(_) => {
                return failure(
                    request,
                    404,
                    "not_found",
                    "Web asset is missing or incompatible",
                )
            }
        };
        let etag = format!("\"{}\"", metadata.sha256);
        if request.header("If-None-Match") == Some(etag.as_str()) {
            let mut response = request.into_response(
                304,
                Some("Not Modified"),
                &[
                    ("ETag", &etag),
                    ("Cache-Control", "public, max-age=31536000, immutable"),
                    ("Content-Security-Policy", ASSET_CSP),
                ],
            )?;
            return response.connection().write_all(&[]);
        }
        let mut headers = vec![
            ("Content-Type", metadata.content_type.as_str()),
            ("ETag", etag.as_str()),
            ("Cache-Control", "public, max-age=31536000, immutable"),
            ("X-Content-Type-Options", "nosniff"),
            ("Content-Security-Policy", ASSET_CSP),
        ];
        if metadata.encoding == WebAssetEncoding::Gzip {
            headers.push(("Content-Encoding", "gzip"));
        }
        let mut response = request.into_response(200, Some("OK"), &headers)?;
        let mut buffer = [0; 1024];
        loop {
            let count = file.read(&mut buffer).map_err(|_| {
                esp_idf_svc::io::EspIOError::from(EspError::from_infallible::<
                    { esp_idf_svc::sys::ESP_FAIL },
                >())
            })?;
            if count == 0 {
                break;
            }
            response.connection().write_all(&buffer[..count])?;
        }
        Ok(())
    })?;
    Ok(())
}

fn apply_request(
    storage: &AssetStorage,
    operation: &str,
    uri: &str,
    body: &[u8],
) -> Result<serde_json::Value, WebAssetError> {
    match operation {
        "begin" => {
            let manifest: WebAssetManifest = serde_json::from_slice(body)?;
            manifest.validate()?;
            storage.with_web_asset_budget(true, |web, free| {
                Ok(serde_json::to_value(web.begin(manifest, free)?)?)
            })
        }
        "chunk" => {
            let values = query(uri).ok_or(WebAssetError::Invalid("query"))?;
            if values.len() != 3
                || values
                    .iter()
                    .any(|(k, _)| !["bundle", "file", "offset"].contains(k))
            {
                return Err(WebAssetError::Invalid("chunk query"));
            }
            let value = |key| {
                values
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| *v)
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
            storage.with_web_asset_budget(false, |web, free| {
                web.write_chunk(bundle, index, offset, body, free)?;
                let pending = web.status()?.pending.ok_or(WebAssetError::NotFound)?;
                Ok(serde_json::to_value(pending)?)
            })
        }
        "commit" => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Commit {
                bundle_id: String,
            }
            let commit: Commit = serde_json::from_slice(body)?;
            if !valid_bundle(&commit.bundle_id) {
                return Err(WebAssetError::Invalid("bundle ID"));
            }
            storage.with_web_asset_budget(false, |web, free| {
                Ok(serde_json::to_value(
                    web.commit_with_capacity(&commit.bundle_id, free)?,
                )?)
            })
        }
        "rollback" => {
            if serde_json::from_slice::<serde_json::Value>(body)? != json!({}) {
                return Err(WebAssetError::Invalid("rollback requires an empty object"));
            }
            storage.with_web_asset_budget(false, |web, free| {
                Ok(serde_json::to_value(web.rollback_with_capacity(free)?)?)
            })
        }
        _ => Err(WebAssetError::Invalid("operation")),
    }
}

fn authorized<C: ServerConnection>(
    request: &ServerRequest<C>,
    broker: &Arc<Mutex<Broker>>,
) -> bool {
    broker_authorized(broker, request.header("X-Note4c-Token"))
}

fn failure<C: ServerConnection>(
    request: ServerRequest<C>,
    status: u16,
    code: &str,
    message: &str,
) -> Result<(), C::Error> {
    write_json_response(
        request,
        status,
        &serde_json::to_vec(&json!({"error":{"code":code,"message":message}})).unwrap_or_default(),
    )
}

fn valid_bundle(bundle: &str) -> bool {
    bundle.len() == 64
        && bundle
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// These endpoints use only decimal numbers and hex IDs; percent decoding is
/// deliberately unnecessary. Reject duplicate/unknown query keys at the caller.
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
