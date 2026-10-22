// SPDX-License-Identifier: MPL-2.0

//! ESP-IDF-specific heap sampling for the opt-in Rune footprint firmware.

use esp_idf_svc::sys;

use panelet_rune_footprint_poc::{self as probe, Stage};

const PSRAM_CONFIGURED: bool = cfg!(esp_idf_spiram);

pub fn run() {
    log::info!(
        "{{\"event\":\"rune_footprint_poc_begin\",\"rune_version\":\"{}\",\"rune_features\":\"default-features=false,std\",\"psram_configured\":{}}}",
        probe::RUNE_VERSION,
        PSRAM_CONFIGURED
    );

    let result = probe::run(log_heap_snapshot);
    let failure_stage = result.failure.map_or("none", |stage| stage.as_str());
    let simple_value = result.simple_value.unwrap_or_default();

    log::info!(
        "{{\"event\":\"rune_footprint_poc_result\",\"rune_version\":\"{}\",\"instruction_budget\":{},\"instruction_budget_returned_error\":{},\"memory_limit_bytes\":{},\"memory_limit_returned_error\":{},\"simple_value_present\":{},\"simple_value\":{},\"failure_stage\":\"{}\",\"psram_configured\":{}}}",
        probe::RUNE_VERSION,
        probe::INSTRUCTION_BUDGET,
        result.instruction_budget_returned_error,
        probe::MEMORY_LIMIT_BYTES,
        result.memory_limit_returned_error,
        result.simple_value.is_some(),
        simple_value,
        failure_stage,
        PSRAM_CONFIGURED
    );
}

#[derive(Clone, Copy)]
struct HeapCaps {
    free: usize,
    minimum: usize,
    largest: usize,
}

impl HeapCaps {
    fn sample(caps: u32) -> Self {
        // ESP-IDF heap-capability APIs expose both current capacity and the
        // lowest free value seen since boot. The latter is deliberately
        // monotonic, so it is not a per-stage instantaneous delta.
        unsafe {
            Self {
                free: sys::heap_caps_get_free_size(caps),
                minimum: sys::heap_caps_get_minimum_free_size(caps),
                largest: sys::heap_caps_get_largest_free_block(caps),
            }
        }
    }
}

fn log_heap_snapshot(stage: Stage) {
    let all_8bit = HeapCaps::sample(sys::MALLOC_CAP_8BIT);
    let internal = HeapCaps::sample(sys::MALLOC_CAP_INTERNAL);
    let psram = HeapCaps::sample(sys::MALLOC_CAP_SPIRAM);
    let system_free = unsafe { sys::esp_get_free_heap_size() };
    let system_minimum = unsafe { sys::esp_get_minimum_free_heap_size() };

    log::info!(
        "{{\"event\":\"rune_footprint_heap\",\"stage\":\"{}\",\"psram_configured\":{},\"system_default_heap\":{{\"free_bytes\":{},\"minimum_free_bytes\":{}}},\"all_8bit\":{{\"free_bytes\":{},\"minimum_free_bytes\":{},\"largest_free_block_bytes\":{}}},\"internal\":{{\"free_bytes\":{},\"minimum_free_bytes\":{},\"largest_free_block_bytes\":{}}},\"psram\":{{\"free_bytes\":{},\"minimum_free_bytes\":{},\"largest_free_block_bytes\":{}}}}}",
        stage.as_str(),
        PSRAM_CONFIGURED,
        system_free,
        system_minimum,
        all_8bit.free,
        all_8bit.minimum,
        all_8bit.largest,
        internal.free,
        internal.minimum,
        internal.largest,
        psram.free,
        psram.minimum,
        psram.largest
    );
}
