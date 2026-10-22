// SPDX-License-Identifier: MPL-2.0

//! A portable, deliberately small Rune 0.14.2 probe used by the Panelet
//! firmware footprint experiment. The ESP-IDF app supplies the heap sampler;
//! keeping Rune execution here makes the safety assertions host-testable.

use std::sync::Arc;

use rune::alloc::limit;
use rune::runtime::budget;
use rune::{Context, Source, Sources, Vm};

pub const RUNE_VERSION: &str = "0.14.2";
pub const INSTRUCTION_BUDGET: usize = 1_024;
pub const MEMORY_LIMIT_BYTES: usize = 4 * 1024;

const SCRIPT: &str = r#"
pub fn simple() {
    40 + 2
}

pub fn while_true() {
    while true {}
}

pub fn memory_pressure() {
    let values = [];

    while values.len() < 4096 {
        values.push(1);
    }

    values.len()
}
"#;

/// Each point where the caller should record a heap-capability sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    PreRune,
    SourceCreated,
    ContextCreated,
    RuntimeContextCreated,
    UnitCompiled,
    VmCreated,
    SimpleScriptExecuted,
    InstructionBudgetChecked,
    MemoryLimitChecked,
    VmCleared,
    AllRuneObjectsDropped,
}

impl Stage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreRune => "pre_rune_after_safe_board_init",
            Self::SourceCreated => "source_created",
            Self::ContextCreated => "context_created",
            Self::RuntimeContextCreated => "runtime_context_created",
            Self::UnitCompiled => "unit_compiled",
            Self::VmCreated => "vm_created",
            Self::SimpleScriptExecuted => "simple_script_executed",
            Self::InstructionBudgetChecked => "instruction_budget_checked",
            Self::MemoryLimitChecked => "memory_limit_checked",
            Self::VmCleared => "vm_cleared",
            Self::AllRuneObjectsDropped => "all_rune_objects_dropped",
        }
    }
}

/// A setup failure is distinct from the two expected sandbox errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureStage {
    Source,
    Context,
    RuntimeContext,
    Unit,
    SimpleScript,
}

impl FailureStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Context => "context",
            Self::RuntimeContext => "runtime_context",
            Self::Unit => "unit",
            Self::SimpleScript => "simple_script",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProbeResult {
    pub simple_value: Option<i64>,
    pub instruction_budget_returned_error: bool,
    pub memory_limit_returned_error: bool,
    pub failure: Option<FailureStage>,
}

impl ProbeResult {
    const fn failed(failure: FailureStage) -> Self {
        Self {
            simple_value: None,
            instruction_budget_returned_error: false,
            memory_limit_returned_error: false,
            failure: Some(failure),
        }
    }
}

/// Execute the minimal compile/VM/sandbox sequence.
///
/// `snapshot` deliberately owns all ESP-specific observation. This function
/// contains no hardware I/O and can be tested on the host. The memory limit
/// wraps only the `memory_pressure` execution, matching the future per-script
/// execution boundary rather than claiming to limit Context or Unit creation.
pub fn run(mut snapshot: impl FnMut(Stage)) -> ProbeResult {
    snapshot(Stage::PreRune);

    let mut sources = Sources::new();
    let source = match Source::memory(SCRIPT) {
        Ok(source) => source,
        Err(_) => return ProbeResult::failed(FailureStage::Source),
    };

    if sources.insert(source).is_err() {
        return ProbeResult::failed(FailureStage::Source);
    }
    snapshot(Stage::SourceCreated);

    // Do not use `with_default_modules()`: its broad standard-library module
    // set would make this a "Rune plus full stdlib" measurement. The script
    // only needs Rune's dynamic Vec methods plus the traits that its module
    // registers (`Clone`, `PartialEq`, `Eq`, `PartialOrd`, and `Ord`).
    let mut context = Context::new();

    let install_result = rune::modules::core::module()
        .and_then(|module| context.install(module))
        .and_then(|_| rune::modules::clone::module())
        .and_then(|module| context.install(module))
        .and_then(|_| rune::modules::cmp::module())
        .and_then(|module| context.install(module))
        .and_then(|_| rune::modules::vec::module())
        .and_then(|module| context.install(module));

    if install_result.is_err() {
        return ProbeResult::failed(FailureStage::Context);
    }
    snapshot(Stage::ContextCreated);

    let runtime = match context.runtime() {
        Ok(runtime) => Arc::new(runtime),
        Err(_) => return ProbeResult::failed(FailureStage::RuntimeContext),
    };
    snapshot(Stage::RuntimeContextCreated);

    let unit = match rune::prepare(&mut sources).with_context(&context).build() {
        Ok(unit) => Arc::new(unit),
        Err(_) => return ProbeResult::failed(FailureStage::Unit),
    };
    snapshot(Stage::UnitCompiled);

    let mut vm = Vm::new(Arc::clone(&runtime), Arc::clone(&unit));
    snapshot(Stage::VmCreated);

    let simple_value = match vm.call(["simple"], ()) {
        Ok(value) => match rune::from_value::<i64>(value) {
            Ok(value) => value,
            Err(_) => {
                return finish_after_simple_failure(
                    &mut snapshot,
                    vm,
                    unit,
                    runtime,
                    context,
                    sources,
                )
            }
        },
        Err(_) => {
            return finish_after_simple_failure(&mut snapshot, vm, unit, runtime, context, sources)
        }
    };
    snapshot(Stage::SimpleScriptExecuted);

    // `while true {}` must become an ordinary VM error after at most this many
    // Rune instructions. It is intentionally not a wall-clock watchdog.
    let instruction_budget_returned_error =
        budget::with(INSTRUCTION_BUDGET, || vm.call(["while_true"], ()))
            .call()
            .is_err();
    snapshot(Stage::InstructionBudgetChecked);

    // The transient VM makes the pressure allocation independent of the
    // normal VM's retained stack. `limit` tracks Rune allocator payloads, not
    // ESP-IDF allocator metadata or Rust allocations outside Rune.
    let memory_limit_returned_error = limit::with(MEMORY_LIMIT_BYTES, || {
        let mut pressure_vm = Vm::new(Arc::clone(&runtime), Arc::clone(&unit));
        pressure_vm.call(["memory_pressure"], ())
    })
    .call()
    .is_err();
    snapshot(Stage::MemoryLimitChecked);

    vm.clear();
    snapshot(Stage::VmCleared);
    drop(vm);
    drop(unit);
    drop(runtime);
    drop(context);
    drop(sources);
    snapshot(Stage::AllRuneObjectsDropped);

    ProbeResult {
        simple_value: Some(simple_value),
        instruction_budget_returned_error,
        memory_limit_returned_error,
        failure: None,
    }
}

fn finish_after_simple_failure(
    snapshot: &mut impl FnMut(Stage),
    mut vm: Vm,
    unit: Arc<rune::Unit>,
    runtime: Arc<rune::runtime::RuntimeContext>,
    context: Context,
    sources: Sources,
) -> ProbeResult {
    snapshot(Stage::SimpleScriptExecuted);
    vm.clear();
    snapshot(Stage::VmCleared);
    drop(vm);
    drop(unit);
    drop(runtime);
    drop(context);
    drop(sources);
    snapshot(Stage::AllRuneObjectsDropped);
    ProbeResult::failed(FailureStage::SimpleScript)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_script_and_both_sandbox_guards_complete() {
        let mut stages = Vec::new();
        let result = run(|stage| stages.push(stage));

        assert_eq!(result.failure, None);
        assert_eq!(result.simple_value, Some(42));
        assert!(result.instruction_budget_returned_error);
        assert!(result.memory_limit_returned_error);
        assert_eq!(stages.first(), Some(&Stage::PreRune));
        assert_eq!(stages.last(), Some(&Stage::AllRuneObjectsDropped));
    }
}
