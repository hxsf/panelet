// SPDX-License-Identifier: MPL-2.0

//! Small, versioned-by-resource-bundle browser ABI. This library is deliberately
//! empty unless both the browser feature and bare WASM target are selected.
#![cfg(all(
    target_arch = "wasm32",
    target_os = "unknown",
    feature = "browser-preview"
))]

mod runtime;

use serde_json::{json, Value};
use std::sync::Mutex;

const MAX_INPUT: usize = 65_536;
const MAX_ALLOCATIONS: usize = 8;
// Retain ownership so malformed pointers/lengths and repeated release calls
// cannot turn the public ABI into unchecked Rust pointer operations.
static ALLOCATIONS: Mutex<Vec<Box<[u8]>>> = Mutex::new(Vec::new());

fn retain(bytes: Box<[u8]>) -> u64 {
    let mut allocations = ALLOCATIONS.lock().unwrap();
    if allocations.len() >= MAX_ALLOCATIONS {
        return 0;
    }
    let descriptor = (bytes.as_ptr() as u64) | ((bytes.len() as u64) << 32);
    allocations.push(bytes);
    descriptor
}

/// Allocate a zeroed input buffer. Zero or oversized lengths return null.
#[no_mangle]
pub extern "C" fn alloc(len: u32) -> u32 {
    if len == 0 || len as usize > MAX_INPUT {
        return 0;
    }
    retain(vec![0; len as usize].into_boxed_slice()) as u32
}

/// Release an exact input or output allocation. Invalid descriptors are ignored.
#[no_mangle]
pub extern "C" fn release(ptr: u32, len: u32) {
    let mut allocations = ALLOCATIONS.lock().unwrap();
    if let Some(index) = allocations
        .iter()
        .position(|bytes| bytes.as_ptr() as u32 == ptr && bytes.len() == len as usize)
    {
        allocations.swap_remove(index);
    }
}

fn error(code: &str, message: &str) -> Value {
    json!({"ok": false, "error": {"code": code, "message": message}})
}

fn dispatch(op: u32, ptr: u32, len: u32) -> Value {
    if len as usize > MAX_INPUT {
        return error("request_too_large", "Preview input exceeds 64 KiB");
    }
    let input = {
        let allocations = ALLOCATIONS.lock().unwrap();
        let Some(input) = allocations
            .iter()
            .find(|bytes| bytes.as_ptr() as u32 == ptr && bytes.len() == len as usize)
        else {
            return error("invalid_buffer", "Input must be an exact live allocation");
        };
        input.to_vec()
    };
    match op {
        0 => match serde_json::from_slice(&input) {
            Ok(request) => runtime::render(request),
            Err(_) => error("invalid_json", "Invalid preview JSON"),
        },
        1 => runtime::encode_package(&input),
        2 => runtime::decode_package(&input),
        _ => error("invalid_operation", "Unknown browser preview operation"),
    }
}

/// Input remains owned by the caller until release. Output is a fresh buffer
/// descriptor: pointer in the low 32 bits, byte length in the high 32 bits.
/// The caller releases both buffers, including error outputs. A zero descriptor
/// means the caller exhausted its eight live allocations.
#[no_mangle]
pub extern "C" fn call(op: u32, ptr: u32, len: u32) -> u64 {
    let response = dispatch(op, ptr, len);
    retain(serde_json::to_vec(&response).unwrap().into_boxed_slice())
}
