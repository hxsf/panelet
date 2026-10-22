// SPDX-License-Identifier: MPL-2.0

#![cfg_attr(target_os = "espidf", feature(allocator_api))]
//! Rune module sets and the Panelet widget host ABI.

use rune::{Context, ContextError};

pub mod clock;
pub mod display;
pub mod lifecycle;
mod mvp;
pub mod sync;

pub use self::mvp::{
    context_with_os, mvp_context, BatteryReading, Canvas, CanvasCommandOwned, CanvasFrame,
    DeviceApi, DeviceInfo, FsApi, FsPrivateApi, FsSharedApi, HostError, HostFuture, HostIo,
    HostResult, HttpApi, HttpHeader, HttpHeaders, HttpRequest, HttpResponse, JsonApi, JsonValue,
    LocalTimeReading, LogApi, LunarReading, OsBindings, PowerApi, ResourceApi, RtcReading,
    RuntimeApi, RuntimeLimits, ScreenApi, Std, StdConfig, StorageApi, SubmissionReceipt, TimeApi,
    WifiApi, WifiStatus, ABI_ID, ABI_VERSION, CANVAS_FRAME_BYTES, CANVAS_HEIGHT, CANVAS_WIDTH,
    CAP_DEVICE_READ, CAP_FS_PRIVATE, CAP_FS_SHARED_READ, CAP_FS_SHARED_WRITE, CAP_HTTP_FETCH,
    CAP_HTTP_INSECURE_TLS, CAP_LOG_WRITE, CAP_POWER_READ, CAP_SCREEN_PRESENT, CAP_STORAGE_KV,
    CAP_TIME_READ, CAP_WIFI_READ, FS_LIST_MAX_ENTRIES, FS_MAX_CALLS, FS_PATH_MAX_BYTES,
    FS_SINGLE_MAX_BYTES, FS_TOTAL_MAX_BYTES, HTTP_BODY_MAX_BYTES, HTTP_HEADERS_MAX_BYTES,
    HTTP_REDIRECT_MAX, HTTP_REQUEST_HEADERS_MAX, HTTP_TIMEOUT_DEFAULT_MS, HTTP_TIMEOUT_MAX_MS,
    HTTP_URL_MAX_BYTES, JSON_MAX_DEPTH, JSON_MAX_NODES, KV_KEY_MAX_BYTES, KV_VALUE_MAX_BYTES,
    LOG_MESSAGE_MAX_BYTES, MAX_CANVAS_COMMANDS, MAX_CANVAS_TEXT_BYTES, MAX_SLEEP_MS,
    MAX_STARTED_CALLS, MAX_WAIT_ALL, RESOURCE_COUNT_MAX, RESOURCE_ITEM_MAX_BYTES,
    RESOURCE_PATH_MAX_BYTES, RESOURCE_TOTAL_MAX_BYTES,
};

pub const RUNE_VERSION: &str = "0.14.2";
pub const HOST_ABI_DESCRIPTION: &str = "note4c.rune/3 os namespace with cold Futures";
pub const MODULE_SET_DESCRIPTION: &str =
    "core + clone + cmp + vec + i64 + u64 + iter + option + ops + string + result + future + object + note4c";

/// Construct the only context accepted by this PoC's package generator and
/// runtime.
///
/// This deliberately does not use `Context::with_default_modules()`: its
/// broader stdlib would invalidate a minimum-runtime comparison.
pub fn minimal_context() -> Result<Context, ContextError> {
    let mut context = Context::new();
    context.install(rune::modules::core::module()?)?;
    context.install(rune::modules::clone::module()?)?;
    context.install(rune::modules::cmp::module()?)?;
    context.install(rune::modules::vec::module()?)?;
    Ok(context)
}

#[cfg(test)]
mod v3_tests;
