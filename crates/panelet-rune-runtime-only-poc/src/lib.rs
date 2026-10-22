// SPDX-License-Identifier: MPL-2.0

//! ESP-IDF-safe half of the precompiled Rune `Unit<ArrayUnit>` footprint PoC.
//!
//! This crate deliberately only consumes a fixed container. Program-text
//! construction and front-end work live in `tools/panelet-rune-unit-compiler-poc`;
//! the target validates and deserializes before building a `RuntimeContext` and
//! VM.

use std::sync::Arc;

use panelet_rune_abi_poc::minimal_context;
use panelet_rune_unit_format_poc::{Header, HeaderError};
use rune::alloc::limit;
use rune::runtime::budget;
use rune::runtime::unit::ArrayUnit;
use rune::{ContextError, Unit, Vm};

pub use panelet_rune_abi_poc::{HOST_ABI_DESCRIPTION, MODULE_SET_DESCRIPTION, RUNE_VERSION};
pub use panelet_rune_unit_format_poc::{
    ARRAY_UNIT_STORAGE, CBOR_CODEC, CONTAINER_FORMAT_VERSION, HEADER_LEN, MAX_UNIT_BLOB_BYTES,
    MAX_UNIT_PAYLOAD_BYTES, MODULE_SET_ID, RUNTIME_ABI_VERSION,
};

pub const INSTRUCTION_BUDGET: usize = 1_024;
/// Limits only allocations accounted by Rune's allocator while serde builds a
/// `Unit`. It is not a whole-process heap cap, hence the input/header cap and
/// trusted-package requirement remain necessary.
pub const DECODE_MEMORY_LIMIT_BYTES: usize = 192 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    PreDecode,
    DecodedUnit,
    ContextCreated,
    RuntimeContextCreated,
    VmCreated,
    ScriptExecuted,
    InstructionBudgetChecked,
    AllRuntimeObjectsDropped,
}

impl Stage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreDecode => "pre_decode",
            Self::DecodedUnit => "decoded_unit",
            Self::ContextCreated => "context_created",
            Self::RuntimeContextCreated => "runtime_context_created",
            Self::VmCreated => "vm_created",
            Self::ScriptExecuted => "script_executed",
            Self::InstructionBudgetChecked => "instruction_budget_checked",
            Self::AllRuntimeObjectsDropped => "all_runtime_objects_dropped",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureStage {
    Package,
    Context,
    RuntimeContext,
    SimpleScript,
}

impl FailureStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Package => "package",
            Self::Context => "context",
            Self::RuntimeContext => "runtime_context",
            Self::SimpleScript => "simple_script",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProbeResult {
    pub simple_value: Option<i64>,
    pub instruction_budget_returned_error: bool,
    pub failure: Option<FailureStage>,
}

impl ProbeResult {
    const fn failed(failure: FailureStage) -> Self {
        Self {
            simple_value: None,
            instruction_budget_returned_error: false,
            failure: Some(failure),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Header(HeaderError),
    Cbor,
    CborTrailingBytes,
}

/// Decode only a validated `Unit<ArrayUnit>`. The `limit` protects Rune-owned
/// allocations during decode, while `Header` rejects oversized, truncated,
/// mismatched and tampered packages before CBOR is invoked.
///
/// Rune 0.14.2 does not expose a complete hostile-Unit verifier. A production
/// device must therefore accept only signed output from a trusted package
/// producer
/// matching this exact Rune version and module ABI; SHA-256 here is integrity,
/// not uploader authentication.
pub fn decode_unit(blob: &[u8]) -> Result<Unit<ArrayUnit>, DecodeError> {
    let mut payload = Header::validate_blob(blob).map_err(DecodeError::Header)?;
    // Keep this temporary decoder scratch buffer explicit and small on ESP-IDF
    // tasks; Ciborium's convenience entry point would allocate 4 KiB on stack.
    let mut scratch = [0; 256];

    let unit = limit::with(DECODE_MEMORY_LIMIT_BYTES, || {
        ciborium::de::from_reader_with_buffer::<Unit<ArrayUnit>, _>(&mut payload, &mut scratch)
    })
    .call()
    .map_err(|_| DecodeError::Cbor)?;

    if !payload.is_empty() {
        return Err(DecodeError::CborTrailingBytes);
    }

    Ok(unit)
}

/// Execute the fixture sequence after decoding a supplied trusted container.
/// The callback owns observation so no ESP-IDF functionality enters this
/// portable crate.
pub fn run(blob: &[u8], mut snapshot: impl FnMut(Stage)) -> ProbeResult {
    snapshot(Stage::PreDecode);
    let unit = match decode_unit(blob) {
        Ok(unit) => Arc::new(unit),
        Err(_) => return ProbeResult::failed(FailureStage::Package),
    };
    snapshot(Stage::DecodedUnit);

    let context = match minimal_context() {
        Ok(context) => context,
        Err(_) => return ProbeResult::failed(FailureStage::Context),
    };
    snapshot(Stage::ContextCreated);

    let runtime = match context.runtime() {
        Ok(runtime) => Arc::new(runtime),
        Err(_) => return ProbeResult::failed(FailureStage::RuntimeContext),
    };
    // A `Context` is only a construction aid here. Do not retain build-side
    // metadata after turning it into the runtime context.
    drop(context);
    snapshot(Stage::RuntimeContextCreated);

    let mut vm = Vm::new(Arc::clone(&runtime), Arc::clone(&unit));
    snapshot(Stage::VmCreated);

    let simple_value = match vm.call(["simple"], ()) {
        Ok(value) => match rune::from_value::<i64>(value) {
            Ok(value) => value,
            Err(_) => return finish_after_simple_failure(&mut snapshot, vm, unit, runtime),
        },
        Err(_) => return finish_after_simple_failure(&mut snapshot, vm, unit, runtime),
    };
    snapshot(Stage::ScriptExecuted);

    let instruction_budget_returned_error =
        budget::with(INSTRUCTION_BUDGET, || vm.call(["while_true"], ()))
            .call()
            .is_err();
    snapshot(Stage::InstructionBudgetChecked);

    vm.clear();
    drop(vm);
    drop(unit);
    drop(runtime);
    snapshot(Stage::AllRuntimeObjectsDropped);

    ProbeResult {
        simple_value: Some(simple_value),
        instruction_budget_returned_error,
        failure: None,
    }
}

fn finish_after_simple_failure(
    snapshot: &mut impl FnMut(Stage),
    mut vm: Vm,
    unit: Arc<Unit<ArrayUnit>>,
    runtime: Arc<rune::runtime::RuntimeContext>,
) -> ProbeResult {
    snapshot(Stage::ScriptExecuted);
    vm.clear();
    drop(vm);
    drop(unit);
    drop(runtime);
    snapshot(Stage::AllRuntimeObjectsDropped);
    ProbeResult::failed(FailureStage::SimpleScript)
}

/// Exposes header validation metadata for board logs without deserializing.
pub fn package_payload_sha256(blob: &[u8]) -> Result<[u8; 32], HeaderError> {
    Header::payload_sha256_from_blob(blob)
}

/// Kept public so the host package generator can make its matching Context
/// explicit in its own source and tests.
pub fn runtime_context_contract() -> Result<rune::Context, ContextError> {
    minimal_context()
}
