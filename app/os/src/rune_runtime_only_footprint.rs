// SPDX-License-Identifier: MPL-2.0

//! ESP-IDF-specific heap sampling for the opt-in precompiled-Unit Rune probe.

use esp_idf_svc::sys;

use panelet_rune_runtime_only_poc::{self as probe, Stage};

const PSRAM_CONFIGURED: bool = cfg!(esp_idf_spiram);

const FIXTURE: &[u8] =
    include_bytes!("../../../experiments/rune-footprint-poc/fixtures/minimal-array-unit-v1.rnu");

pub fn run() {
    let payload_sha256_ok = probe::package_payload_sha256(FIXTURE).is_ok();
    log::info!(
        "{{\"event\":\"rune_runtime_only_footprint_poc_begin\",\"rune_version\":\"{}\",\"rune_features\":\"default-features=false,std\",\"unit_storage\":\"ArrayUnit\",\"container_format_version\":{},\"fixture_container_bytes\":{},\"fixture_header_valid\":{},\"decode_memory_limit_bytes\":{},\"psram_configured\":{}}}",
        probe::RUNE_VERSION,
        probe::CONTAINER_FORMAT_VERSION,
        FIXTURE.len(),
        payload_sha256_ok,
        probe::DECODE_MEMORY_LIMIT_BYTES,
        PSRAM_CONFIGURED,
    );

    let result = probe::run(FIXTURE, log_heap_snapshot);
    let failure_stage = result.failure.map_or("none", |stage| stage.as_str());
    let simple_value = result.simple_value.unwrap_or_default();

    log::info!(
        "{{\"event\":\"rune_runtime_only_footprint_poc_result\",\"rune_version\":\"{}\",\"simple_value_present\":{},\"simple_value\":{},\"instruction_budget\":{},\"instruction_budget_returned_error\":{},\"failure_stage\":\"{}\",\"psram_configured\":{}}}",
        probe::RUNE_VERSION,
        result.simple_value.is_some(),
        simple_value,
        probe::INSTRUCTION_BUDGET,
        result.instruction_budget_returned_error,
        failure_stage,
        PSRAM_CONFIGURED,
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
        // `minimum` is a boot-lifetime low watermark, not an instantaneous
        // per-stage delta. The values are only emitted after a deliberately
        // safe board setup and are not a claim of a board-run result until
        // this firmware is explicitly flashed.
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
        "{{\"event\":\"rune_runtime_only_footprint_heap\",\"stage\":\"{}\",\"psram_configured\":{},\"system_default_heap\":{{\"free_bytes\":{},\"minimum_free_bytes\":{}}},\"all_8bit\":{{\"free_bytes\":{},\"minimum_free_bytes\":{},\"largest_free_block_bytes\":{}}},\"internal\":{{\"free_bytes\":{},\"minimum_free_bytes\":{},\"largest_free_block_bytes\":{}}},\"psram\":{{\"free_bytes\":{},\"minimum_free_bytes\":{},\"largest_free_block_bytes\":{}}}}}",
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
        psram.largest,
    );
}
