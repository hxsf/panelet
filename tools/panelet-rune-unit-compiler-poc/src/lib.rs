// SPDX-License-Identifier: MPL-2.0

//! macOS/host-only compiler for the precompiled Rune footprint fixture.
//!
//! It shares `minimal_context()` with the device runtime. This is the only
//! package in the runtime-only experiment allowed to reference Rune source or
//! compiler APIs.

use panelet_rune_abi_poc::minimal_context;
use panelet_rune_unit_format_poc::{Header, HeaderError};
use rune::runtime::unit::ArrayUnit;
use rune::{Source, Sources, Unit};

pub const FIXTURE_SOURCE: &str = r#"
pub fn simple() {
    40 + 2
}

pub fn while_true() {
    while true {}
}
"#;

pub fn compile_fixture() -> Result<Unit<ArrayUnit>, String> {
    compile_source(FIXTURE_SOURCE)
}

pub fn compile_source(source_text: &str) -> Result<Unit<ArrayUnit>, String> {
    let mut sources = Sources::new();
    let source = Source::memory(source_text).map_err(|error| format!("source: {error:?}"))?;
    sources
        .insert(source)
        .map_err(|error| format!("source insert: {error:?}"))?;

    let context = minimal_context().map_err(|error| format!("context: {error:?}"))?;
    rune::prepare(&mut sources)
        .with_context(&context)
        .build()
        .map_err(|error| format!("compile: {error:?}"))
}

pub fn serialize_unit(unit: &Unit<ArrayUnit>) -> Result<Vec<u8>, String> {
    // Rune CLI 0.14.2 has a bincode cache call site, but its Unit's fallible
    // HashMap serializer supplies an unknown map length and bincode 1.3.3
    // rejects the actual Unit at runtime. CBOR permits unknown map lengths and
    // yields a binary payload. The local experimental rune-core patch is
    // separately required to make ItemBuf decode symmetrically.
    let mut payload = Vec::new();
    ciborium::ser::into_writer(unit, &mut payload)
        .map_err(|error| format!("serialize: {error}"))?;
    let header = Header::for_payload(&payload).map_err(header_error)?;
    let mut package = Vec::with_capacity(panelet_rune_unit_format_poc::HEADER_LEN + payload.len());
    package.extend_from_slice(&header.encode());
    package.extend_from_slice(&payload);
    Ok(package)
}

pub fn fixture_package() -> Result<Vec<u8>, String> {
    serialize_unit(&compile_fixture()?)
}

fn header_error(error: HeaderError) -> String {
    format!("container: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use panelet_rune_runtime_only_poc::{decode_unit, run, FailureStage, Stage};
    use panelet_rune_unit_format_poc::{HeaderError, HEADER_LEN};

    #[test]
    fn host_compiles_serializes_decodes_and_executes_fixture() {
        let package = fixture_package().expect("compile and serialize fixture");
        let _unit = decode_unit(&package).expect("runtime must deserialize host package");
        let mut stages = Vec::new();
        let result = run(&package, |stage| stages.push(stage));

        assert_eq!(result.failure, None);
        assert_eq!(result.simple_value, Some(42));
        assert!(result.instruction_budget_returned_error);
        assert_eq!(stages.first(), Some(&Stage::PreDecode));
        assert_eq!(stages.last(), Some(&Stage::AllRuntimeObjectsDropped));
    }

    #[test]
    fn corrupt_truncated_and_wrong_version_packages_are_rejected() {
        let package = fixture_package().expect("compile fixture");

        let mut corrupt = package.clone();
        corrupt[HEADER_LEN] ^= 0x01;
        assert!(matches!(
            decode_unit(&corrupt),
            Err(panelet_rune_runtime_only_poc::DecodeError::Header(
                HeaderError::Hash
            ))
        ));

        let truncated = &package[..package.len() - 1];
        assert!(matches!(
            decode_unit(truncated),
            Err(panelet_rune_runtime_only_poc::DecodeError::Header(
                HeaderError::LengthMismatch { .. }
            ))
        ));

        let mut wrong_version = package;
        wrong_version[8] ^= 0x01;
        assert!(matches!(
            decode_unit(&wrong_version),
            Err(panelet_rune_runtime_only_poc::DecodeError::Header(
                HeaderError::ContainerVersion { .. }
            ))
        ));
    }

    #[test]
    fn generated_fixture_matches_checked_in_target_blob() {
        let generated = fixture_package().expect("compile fixture");
        let checked_in = include_bytes!(
            "../../../experiments/rune-footprint-poc/fixtures/minimal-array-unit-v1.rnu"
        );
        assert_eq!(generated, checked_in);
    }

    #[test]
    fn malformed_package_does_not_run_a_vm() {
        let result = run(b"not a package", |_| {});
        assert_eq!(result.failure, Some(FailureStage::Package));
    }
}
