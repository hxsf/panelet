// SPDX-License-Identifier: MPL-2.0

#![forbid(unsafe_code)]

//! Strict STORE-only `.pnw` packages for Panelet widgets.
//!
//! `.pnw` is the public extension; legacy `.n4w` files use the same ZIP format
//! and remain readable. This byte-oriented codec does not inspect filenames.
//!
//! A package is a deliberately small ZIP archive containing exactly one
//! `manifest.json`, one `main.rn`, and zero to eight UTF-8 resource files below
//! `resources/`.  The reader implements the small subset that the device needs
//! instead of handing an untrusted archive to a general-purpose extractor:
//! entries are uncompressed, non-encrypted, non-ZIP64, contiguous, CRC checked
//! and bounded before any JSON or string allocation is performed.

use std::fmt;

use panelet_management::{WidgetCandidate, WidgetManifest, WidgetResource};

/// The management HTTP endpoint and the package codec share this transport
/// ceiling.  The bound is checked before parsing the ZIP directory.
pub const MAX_PACKAGE_BYTES: usize = 64 * 1024;
/// `manifest.json` is much smaller than the transport limit, but bounding it
/// separately keeps malformed JSON from becoming an allocation primitive.
pub const MAX_MANIFEST_BYTES: usize = 8 * 1024;
pub const MAX_SOURCE_BYTES: usize = 5 * 1024;
pub const MAX_RESOURCE_COUNT: usize = 8;
pub const MAX_RESOURCE_ITEM_BYTES: usize = 1024;
/// Resource accounting includes the logical path bytes, matching management's
/// admission check.
pub const MAX_RESOURCE_TOTAL_BYTES: usize = 4 * 1024;
pub const MAX_RESOURCE_PATH_BYTES: usize = 64;
pub const MAX_ZIP_ENTRY_COUNT: usize = MAX_RESOURCE_COUNT + 2;
pub const MAX_ZIP_ENTRY_NAME_BYTES: usize = 128;

const LOCAL_FILE_HEADER_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_FILE_HEADER_SIGNATURE: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0605_4b50;
const LOCAL_FILE_HEADER_BYTES: usize = 30;
const CENTRAL_FILE_HEADER_BYTES: usize = 46;
const END_OF_CENTRAL_DIRECTORY_BYTES: usize = 22;
const UTF8_FLAG: u16 = 0x0800;
const ZIP64_SENTINEL_U16: u16 = u16::MAX;
const ZIP64_SENTINEL_U32: u32 = u32::MAX;

/// Errors deliberately expose a short reason while retaining enough context
/// for management logs and host-side tests.  No error contains package data.
#[derive(Debug)]
pub enum PackageError {
    TooLarge {
        actual: usize,
        maximum: usize,
    },
    Truncated(&'static str),
    InvalidZip(&'static str),
    Unsupported(&'static str),
    MissingEntry(&'static str),
    DuplicatePath(String),
    InvalidPath(String),
    EntryTooLarge {
        path: String,
        actual: usize,
        maximum: usize,
    },
    TooManyEntries {
        actual: usize,
        maximum: usize,
    },
    CrcMismatch {
        path: String,
        expected: u32,
        actual: u32,
    },
    InvalidUtf8(&'static str),
    ManifestJson(serde_json::Error),
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { actual, maximum } => {
                write!(f, "widget package is {actual} bytes; maximum is {maximum}")
            }
            Self::Truncated(what) => write!(f, "truncated ZIP {what}"),
            Self::InvalidZip(what) => write!(f, "invalid ZIP: {what}"),
            Self::Unsupported(what) => write!(f, "unsupported ZIP feature: {what}"),
            Self::MissingEntry(name) => write!(f, "widget package is missing {name}"),
            Self::DuplicatePath(path) => write!(f, "duplicate widget package path: {path}"),
            Self::InvalidPath(path) => write!(f, "invalid widget package path: {path}"),
            Self::EntryTooLarge {
                path,
                actual,
                maximum,
            } => {
                write!(
                    f,
                    "widget package entry {path} is {actual} bytes; maximum is {maximum}"
                )
            }
            Self::TooManyEntries { actual, maximum } => {
                write!(
                    f,
                    "widget package has {actual} entries; maximum is {maximum}"
                )
            }
            Self::CrcMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "CRC mismatch for {path}: expected {expected:08x}, got {actual:08x}"
            ),
            Self::InvalidUtf8(what) => write!(f, "widget package {what} is not valid UTF-8"),
            Self::ManifestJson(error) => write!(f, "invalid widget manifest JSON: {error}"),
        }
    }
}

impl std::error::Error for PackageError {}

/// Backward-compatible name for [`decode_pnw`]; the package bytes are unchanged.
pub use decode_pnw as decode_n4w;
/// Backward-compatible name for [`encode_pnw`]; the package bytes are unchanged.
pub use encode_pnw as encode_n4w;

/// Decode one bounded, STORE-only `.pnw` archive into the management-layer
/// candidate.  The complete archive is checked before any candidate is
/// returned, including every entry's CRC and local/central-header agreement.
pub fn decode_pnw(bytes: &[u8]) -> Result<WidgetCandidate, PackageError> {
    decode_pnw_inner(bytes, false)
}

/// Verify an archive and recover its owner for an in-place ABI migration.
/// ABI 2 is accepted here only for identity: never return its executable source.
pub fn package_owner_id(bytes: &[u8]) -> Result<String, PackageError> {
    Ok(decode_pnw_inner(bytes, true)?.manifest.id)
}

fn decode_pnw_inner(bytes: &[u8], migration_identity: bool) -> Result<WidgetCandidate, PackageError> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(PackageError::TooLarge {
            actual: bytes.len(),
            maximum: MAX_PACKAGE_BYTES,
        });
    }
    if bytes.len() < END_OF_CENTRAL_DIRECTORY_BYTES {
        return Err(PackageError::Truncated("end of central directory"));
    }

    // `.pnw` intentionally has no archive comment.  Requiring EOCD to be the
    // final 22 bytes makes appended data and multiple plausible EOCD records
    // unambiguous failures instead of parser-dependent behavior.
    let eocd_offset = bytes.len() - END_OF_CENTRAL_DIRECTORY_BYTES;
    if read_u32(bytes, eocd_offset)? != END_OF_CENTRAL_DIRECTORY_SIGNATURE {
        return Err(PackageError::InvalidZip(
            "end of central directory position",
        ));
    }
    let disk_number = read_u16(bytes, eocd_offset + 4)?;
    let central_disk = read_u16(bytes, eocd_offset + 6)?;
    let entries_on_disk = read_u16(bytes, eocd_offset + 8)?;
    let entries_total = read_u16(bytes, eocd_offset + 10)?;
    let central_size = read_u32(bytes, eocd_offset + 12)?;
    let central_offset = read_u32(bytes, eocd_offset + 16)?;
    let comment_len = read_u16(bytes, eocd_offset + 20)?;

    if comment_len != 0 {
        return Err(PackageError::Unsupported("archive comment"));
    }
    if disk_number != 0 || central_disk != 0 || entries_on_disk != entries_total {
        return Err(PackageError::Unsupported("multi-disk ZIP"));
    }
    if entries_total == ZIP64_SENTINEL_U16
        || central_size == ZIP64_SENTINEL_U32
        || central_offset == ZIP64_SENTINEL_U32
    {
        return Err(PackageError::Unsupported("ZIP64"));
    }
    let entry_count = usize::from(entries_total);
    if entry_count == 0 {
        return Err(PackageError::InvalidZip("empty archive"));
    }
    if entry_count > MAX_ZIP_ENTRY_COUNT {
        return Err(PackageError::TooManyEntries {
            actual: entry_count,
            maximum: MAX_ZIP_ENTRY_COUNT,
        });
    }

    let central_offset = usize::try_from(central_offset)
        .map_err(|_| PackageError::InvalidZip("central directory offset"))?;
    let central_size = usize::try_from(central_size)
        .map_err(|_| PackageError::InvalidZip("central directory size"))?;
    let central_end = central_offset
        .checked_add(central_size)
        .ok_or(PackageError::InvalidZip("central directory overflow"))?;
    if central_end != eocd_offset || central_end > bytes.len() {
        return Err(PackageError::InvalidZip("central directory boundary"));
    }
    if central_size < entry_count.saturating_mul(CENTRAL_FILE_HEADER_BYTES) {
        return Err(PackageError::Truncated("central directory"));
    }

    let mut entries = Vec::with_capacity(entry_count);
    let mut cursor = central_offset;
    for _ in 0..entry_count {
        let header_end = cursor
            .checked_add(CENTRAL_FILE_HEADER_BYTES)
            .ok_or(PackageError::InvalidZip("central header overflow"))?;
        if header_end > central_end {
            return Err(PackageError::Truncated("central file header"));
        }
        if read_u32(bytes, cursor)? != CENTRAL_FILE_HEADER_SIGNATURE {
            return Err(PackageError::InvalidZip("central file header signature"));
        }

        let version_made_by = read_u16(bytes, cursor + 4)?;
        let version_needed = read_u16(bytes, cursor + 6)?;
        let flags = read_u16(bytes, cursor + 8)?;
        let method = read_u16(bytes, cursor + 10)?;
        let crc = read_u32(bytes, cursor + 16)?;
        let compressed_size = read_u32(bytes, cursor + 20)?;
        let uncompressed_size = read_u32(bytes, cursor + 24)?;
        let name_len = usize::from(read_u16(bytes, cursor + 28)?);
        let extra_len = usize::from(read_u16(bytes, cursor + 30)?);
        let comment_len = usize::from(read_u16(bytes, cursor + 32)?);
        let disk_start = read_u16(bytes, cursor + 34)?;
        let internal_attributes = read_u16(bytes, cursor + 36)?;
        let external_attributes = read_u32(bytes, cursor + 38)?;
        let local_offset = read_u32(bytes, cursor + 42)?;

        if compressed_size == ZIP64_SENTINEL_U32
            || uncompressed_size == ZIP64_SENTINEL_U32
            || local_offset == ZIP64_SENTINEL_U32
        {
            return Err(PackageError::Unsupported("ZIP64"));
        }
        validate_zip_fields(
            flags,
            method,
            version_needed,
            version_made_by,
            disk_start,
            internal_attributes,
            external_attributes,
        )?;
        if name_len == 0 || name_len > MAX_ZIP_ENTRY_NAME_BYTES {
            return Err(PackageError::InvalidZip("entry name length"));
        }
        // `.pnw` has no extra fields or per-entry comments.  In particular,
        // this rejects ZIP64 extra fields even if their 32-bit header values
        // happen to be small.
        if extra_len != 0 {
            return Err(PackageError::Unsupported("entry extra field"));
        }
        if comment_len != 0 {
            return Err(PackageError::Unsupported("entry comment"));
        }
        let variable_end = header_end
            .checked_add(name_len)
            .and_then(|end| end.checked_add(extra_len))
            .and_then(|end| end.checked_add(comment_len))
            .ok_or(PackageError::InvalidZip("central entry overflow"))?;
        if variable_end > central_end {
            return Err(PackageError::Truncated("central entry name"));
        }
        let raw_name = bytes[header_end..header_end + name_len].to_vec();
        let name = decode_entry_name(&raw_name, flags)?;
        let path = classify_entry_path(&name)?;
        if entries
            .iter()
            .any(|entry: &CentralEntry| entry.name == name)
        {
            return Err(PackageError::DuplicatePath(path));
        }

        entries.push(CentralEntry {
            name,
            raw_name,
            flags,
            method,
            version_needed,
            crc,
            compressed_size,
            uncompressed_size,
            local_offset,
        });
        cursor = variable_end;
    }
    if cursor != central_end {
        return Err(PackageError::InvalidZip("central directory trailing bytes"));
    }

    // Every local record must cover exactly the bytes before the central
    // directory.  This rejects unreferenced records, gaps, overlaps and hidden
    // trailing data in the local-file area.
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_unstable_by_key(|index| entries[*index].local_offset);
    let mut expected_local_offset = 0usize;
    let mut payloads = Vec::with_capacity(entries.len());
    for index in order {
        let entry = &entries[index];
        let local_offset = usize::try_from(entry.local_offset)
            .map_err(|_| PackageError::InvalidZip("local header offset"))?;
        if local_offset != expected_local_offset {
            return Err(PackageError::InvalidZip("local records are not contiguous"));
        }
        let local_header_end = local_offset
            .checked_add(LOCAL_FILE_HEADER_BYTES)
            .ok_or(PackageError::InvalidZip("local header overflow"))?;
        if local_header_end > central_offset {
            return Err(PackageError::Truncated("local file header"));
        }
        if read_u32(bytes, local_offset)? != LOCAL_FILE_HEADER_SIGNATURE {
            return Err(PackageError::InvalidZip("local file header signature"));
        }
        let local_version_needed = read_u16(bytes, local_offset + 4)?;
        let local_flags = read_u16(bytes, local_offset + 6)?;
        let local_method = read_u16(bytes, local_offset + 8)?;
        let local_crc = read_u32(bytes, local_offset + 14)?;
        let local_compressed_size = read_u32(bytes, local_offset + 18)?;
        let local_uncompressed_size = read_u32(bytes, local_offset + 22)?;
        let local_name_len = usize::from(read_u16(bytes, local_offset + 26)?);
        let local_extra_len = usize::from(read_u16(bytes, local_offset + 28)?);
        if local_compressed_size == ZIP64_SENTINEL_U32
            || local_uncompressed_size == ZIP64_SENTINEL_U32
        {
            return Err(PackageError::Unsupported("ZIP64"));
        }
        if local_version_needed != entry.version_needed
            || local_flags != entry.flags
            || local_method != entry.method
            || local_crc != entry.crc
            || local_compressed_size != entry.compressed_size
            || local_uncompressed_size != entry.uncompressed_size
        {
            return Err(PackageError::InvalidZip(
                "local and central header mismatch",
            ));
        }
        if local_extra_len != 0 {
            return Err(PackageError::Unsupported("local extra field"));
        }
        let local_name_end = local_header_end
            .checked_add(local_name_len)
            .and_then(|end| end.checked_add(local_extra_len))
            .ok_or(PackageError::InvalidZip("local entry overflow"))?;
        if local_name_len != entry.raw_name.len()
            || local_name_end > central_offset
            || bytes[local_header_end..local_name_end - local_extra_len] != entry.raw_name
        {
            return Err(PackageError::InvalidZip("local and central name mismatch"));
        }

        let data_start = local_name_end;
        let compressed_size = usize::try_from(entry.compressed_size)
            .map_err(|_| PackageError::InvalidZip("entry size"))?;
        let data_end = data_start
            .checked_add(compressed_size)
            .ok_or(PackageError::InvalidZip("entry size overflow"))?;
        if data_end > central_offset {
            return Err(PackageError::Truncated("entry data"));
        }
        if entry.compressed_size != entry.uncompressed_size {
            return Err(PackageError::Unsupported("compressed entry"));
        }
        let actual_crc = crc32(&bytes[data_start..data_end]);
        if actual_crc != entry.crc {
            return Err(PackageError::CrcMismatch {
                path: entry.name.clone(),
                expected: entry.crc,
                actual: actual_crc,
            });
        }
        expected_local_offset = data_end;
        payloads.push(Payload {
            entry_index: index,
            bytes: data_start..data_end,
        });
    }
    if expected_local_offset != central_offset {
        return Err(PackageError::InvalidZip(
            "local records do not reach central directory",
        ));
    }

    let manifest_payload = payload_for_name(bytes, &entries, &payloads, "manifest.json")?
        .ok_or(PackageError::MissingEntry("manifest.json"))?;
    let source_payload = payload_for_name(bytes, &entries, &payloads, "main.rn")?
        .ok_or(PackageError::MissingEntry("main.rn"))?;
    if manifest_payload.len() > MAX_MANIFEST_BYTES {
        return Err(PackageError::EntryTooLarge {
            path: "manifest.json".to_owned(),
            actual: manifest_payload.len(),
            maximum: MAX_MANIFEST_BYTES,
        });
    }
    if source_payload.is_empty() || source_payload.len() > MAX_SOURCE_BYTES {
        return Err(PackageError::EntryTooLarge {
            path: "main.rn".to_owned(),
            actual: source_payload.len(),
            maximum: MAX_SOURCE_BYTES,
        });
    }

    let mut manifest_value: serde_json::Value =
        serde_json::from_slice(manifest_payload).map_err(PackageError::ManifestJson)?;
    let abi = manifest_value.get("abi").and_then(serde_json::Value::as_str);
    if abi != Some(panelet_management::WIDGET_ABI) {
        if !migration_identity || abi != Some("note4c.rune/2") {
            return Err(PackageError::Unsupported("widget ABI"));
        }
        // ABI 2 used a fixed entry. Retain strict field validation after
        // removing that one known legacy field for the identity-only path.
        if let Some(entry) = manifest_value.get("entry") {
            if entry.as_str() != Some("main") {
                return Err(PackageError::Unsupported("legacy widget entry"));
            }
        }
        if let Some(object) = manifest_value.as_object_mut() { object.remove("entry"); }
    }
    let manifest: WidgetManifest =
        serde_json::from_value(manifest_value).map_err(PackageError::ManifestJson)?;
    let source = String::from_utf8(source_payload.to_vec())
        .map_err(|_| PackageError::InvalidUtf8("main.rn"))?;

    let mut resources = Vec::new();
    let mut resource_total = 0usize;
    for payload in &payloads {
        let entry = &entries[payload.entry_index];
        if entry.name == "manifest.json" || entry.name == "main.rn" {
            continue;
        }
        let resource_path = entry
            .name
            .strip_prefix("resources/")
            .ok_or_else(|| PackageError::InvalidPath(entry.name.clone()))?;
        if resources.len() >= MAX_RESOURCE_COUNT {
            return Err(PackageError::TooManyEntries {
                actual: resources.len() + 1,
                maximum: MAX_RESOURCE_COUNT,
            });
        }
        if payload.bytes.len() > MAX_RESOURCE_ITEM_BYTES {
            return Err(PackageError::EntryTooLarge {
                path: entry.name.clone(),
                actual: payload.bytes.len(),
                maximum: MAX_RESOURCE_ITEM_BYTES,
            });
        }
        resource_total = resource_total
            .checked_add(resource_path.len())
            .and_then(|total| total.checked_add(payload.bytes.len()))
            .ok_or(PackageError::InvalidZip("resource size overflow"))?;
        if resource_total > MAX_RESOURCE_TOTAL_BYTES {
            return Err(PackageError::EntryTooLarge {
                path: "resources/*".to_owned(),
                actual: resource_total,
                maximum: MAX_RESOURCE_TOTAL_BYTES,
            });
        }
        let text = String::from_utf8(bytes_for_payload(bytes, payload).to_vec())
            .map_err(|_| PackageError::InvalidUtf8("resource"))?;
        resources.push(WidgetResource {
            path: resource_path.to_owned(),
            text,
        });
    }

    Ok(WidgetCandidate {
        manifest,
        source,
        resources,
    })
}

/// Encode a candidate as a deterministic STORE-only `.pnw` ZIP archive.  The
/// resource order supplied by the candidate is retained so a persistent-store
/// write can verify an exact encode/decode round trip without changing the
/// caller's vector ordering.
///
/// The public API is intentionally infallible for the valid, management-layer
/// candidate type.  Candidates with malformed paths or fields are still
/// serialized so that the same device decoder can reject them; callers that
/// need a user-facing validation error should call `decode_pnw` on the result
/// or pass through management admission first.
pub fn encode_pnw(candidate: &WidgetCandidate) -> Vec<u8> {
    let manifest = serde_json::to_vec(&candidate.manifest)
        .expect("WidgetManifest contains only serializable strings and vectors");
    let mut entries = Vec::with_capacity(2 + candidate.resources.len());
    entries.push(EncodedEntry {
        name: b"manifest.json".to_vec(),
        data: manifest,
    });
    entries.push(EncodedEntry {
        name: b"main.rn".to_vec(),
        data: candidate.source.as_bytes().to_vec(),
    });
    for resource in &candidate.resources {
        let mut name = b"resources/".to_vec();
        name.extend_from_slice(resource.path.as_bytes());
        entries.push(EncodedEntry {
            name,
            data: resource.text.as_bytes().to_vec(),
        });
    }
    encode_entries(&entries)
}

#[derive(Debug)]
struct EncodedEntry {
    name: Vec<u8>,
    data: Vec<u8>,
}

fn encode_entries(entries: &[EncodedEntry]) -> Vec<u8> {
    assert!(
        entries.len() <= usize::from(u16::MAX),
        "too many ZIP entries"
    );
    let mut local = Vec::new();
    let mut central = Vec::new();
    let mut offsets = Vec::with_capacity(entries.len());

    for entry in entries {
        assert!(
            entry.name.len() <= usize::from(u16::MAX),
            "ZIP entry name is too long"
        );
        assert!(
            entry.data.len() <= u32::MAX as usize,
            "ZIP entry is too large"
        );
        assert!(local.len() <= u32::MAX as usize, "ZIP archive is too large");
        let offset = local.len() as u32;
        let size = entry.data.len() as u32;
        let crc = crc32(&entry.data);
        offsets.push(offset);

        push_u32(&mut local, LOCAL_FILE_HEADER_SIGNATURE);
        push_u16(&mut local, 20); // version needed: STORE + UTF-8
        push_u16(&mut local, UTF8_FLAG);
        push_u16(&mut local, 0); // STORE
        push_u16(&mut local, 0); // modification time
        push_u16(&mut local, 0); // modification date
        push_u32(&mut local, crc);
        push_u32(&mut local, size);
        push_u32(&mut local, size);
        push_u16(&mut local, entry.name.len() as u16);
        push_u16(&mut local, 0); // no extra fields
        local.extend_from_slice(&entry.name);
        local.extend_from_slice(&entry.data);
    }

    for (entry, offset) in entries.iter().zip(offsets) {
        let size = entry.data.len() as u32;
        let crc = crc32(&entry.data);
        push_u32(&mut central, CENTRAL_FILE_HEADER_SIGNATURE);
        push_u16(&mut central, 20); // made by version
        push_u16(&mut central, 20); // version needed
        push_u16(&mut central, UTF8_FLAG);
        push_u16(&mut central, 0); // STORE
        push_u16(&mut central, 0); // modification time
        push_u16(&mut central, 0); // modification date
        push_u32(&mut central, crc);
        push_u32(&mut central, size);
        push_u32(&mut central, size);
        push_u16(&mut central, entry.name.len() as u16);
        push_u16(&mut central, 0); // no extra fields
        push_u16(&mut central, 0); // no comment
        push_u16(&mut central, 0); // disk number
        push_u16(&mut central, 0); // internal attributes
        push_u32(&mut central, 0); // external attributes
        push_u32(&mut central, offset);
        central.extend_from_slice(&entry.name);
    }

    let central_offset = local.len();
    assert!(
        central_offset <= u32::MAX as usize,
        "ZIP archive is too large"
    );
    assert!(
        central.len() <= u32::MAX as usize,
        "ZIP central directory is too large"
    );
    let mut output = local;
    output.extend_from_slice(&central);
    push_u32(&mut output, END_OF_CENTRAL_DIRECTORY_SIGNATURE);
    push_u16(&mut output, 0); // disk number
    push_u16(&mut output, 0); // central directory disk
    push_u16(&mut output, entries.len() as u16);
    push_u16(&mut output, entries.len() as u16);
    push_u32(&mut output, central.len() as u32);
    push_u32(&mut output, central_offset as u32);
    push_u16(&mut output, 0); // no archive comment
    output
}

#[derive(Debug)]
struct CentralEntry {
    name: String,
    raw_name: Vec<u8>,
    flags: u16,
    method: u16,
    version_needed: u16,
    crc: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    local_offset: u32,
}

#[derive(Debug)]
struct Payload {
    entry_index: usize,
    bytes: std::ops::Range<usize>,
}

fn payload_for_name<'a>(
    bytes: &'a [u8],
    entries: &[CentralEntry],
    payloads: &'a [Payload],
    name: &str,
) -> Result<Option<&'a [u8]>, PackageError> {
    let Some(entry_index) = entries.iter().position(|entry| entry.name == name) else {
        return Ok(None);
    };
    let payload = payloads
        .iter()
        .find(|payload| payload.entry_index == entry_index)
        .ok_or(PackageError::InvalidZip("entry payload index"))?;
    Ok(Some(bytes_for_payload(bytes, payload)))
}

fn bytes_for_payload<'a>(bytes: &'a [u8], payload: &Payload) -> &'a [u8] {
    &bytes[payload.bytes.clone()]
}

fn classify_entry_path(name: &str) -> Result<String, PackageError> {
    if name == "manifest.json" || name == "main.rn" {
        return Ok(name.to_owned());
    }
    let resource_path = name
        .strip_prefix("resources/")
        .ok_or_else(|| PackageError::InvalidPath(name.to_owned()))?;
    validate_resource_path(resource_path)?;
    Ok(name.to_owned())
}

fn validate_resource_path(path: &str) -> Result<(), PackageError> {
    if path.is_empty()
        || path.len() > MAX_RESOURCE_PATH_BYTES
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.chars().any(|ch| ch.is_control())
    {
        return Err(PackageError::InvalidPath(path.to_owned()));
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(PackageError::InvalidPath(path.to_owned()));
        }
    }
    Ok(())
}

fn decode_entry_name(raw_name: &[u8], flags: u16) -> Result<String, PackageError> {
    if flags & UTF8_FLAG == 0 && !raw_name.iter().all(u8::is_ascii) {
        return Err(PackageError::Unsupported("non-UTF-8 entry name"));
    }
    String::from_utf8(raw_name.to_vec()).map_err(|_| PackageError::InvalidUtf8("entry path"))
}

fn validate_zip_fields(
    flags: u16,
    method: u16,
    version_needed: u16,
    version_made_by: u16,
    disk_start: u16,
    internal_attributes: u16,
    external_attributes: u32,
) -> Result<(), PackageError> {
    if flags & 0x0001 != 0 {
        return Err(PackageError::Unsupported("encrypted entry"));
    }
    if flags & 0x0008 != 0 {
        return Err(PackageError::Unsupported("data descriptor"));
    }
    if flags & !UTF8_FLAG != 0 {
        return Err(PackageError::Unsupported("ZIP flags"));
    }
    if method != 0 {
        return Err(PackageError::Unsupported("compressed entry"));
    }
    // Version 1.0 is the lowest version used by ordinary STORE archives.  A
    // version above 2.0 may indicate features this narrow reader does not
    // understand, so reject it before interpreting the rest of the record.
    if !(10..=20).contains(&version_needed) {
        return Err(PackageError::Unsupported("ZIP version"));
    }
    if disk_start != 0 || internal_attributes != 0 {
        return Err(PackageError::Unsupported("split or attributed entry"));
    }

    // External attributes vary between ZIP writers.  Accept ordinary regular
    // files, but reject DOS directories and POSIX directories/symlinks/special
    // files.  A directory name would be rejected by path validation as well;
    // checking mode bits closes the symlink/special-file escape hatch.
    if external_attributes & 0x10 != 0 {
        return Err(PackageError::Unsupported("directory entry"));
    }
    let _made_by_os = (version_made_by >> 8) as u8;
    let mode = (external_attributes >> 16) as u16;
    let file_type = mode & 0xf000;
    if file_type == 0xa000 {
        return Err(PackageError::Unsupported("symlink entry"));
    }
    if file_type == 0x4000 {
        return Err(PackageError::Unsupported("directory entry"));
    }
    // Reject every non-regular POSIX file type (FIFO, socket and device nodes
    // included), even if a malicious archive changed its creator OS byte to
    // DOS.  Ordinary DOS/Windows entries leave these upper mode bits at zero;
    // regular Unix/Windows entries use the 0x8000 type.
    if file_type != 0 && file_type != 0x8000 {
        return Err(PackageError::Unsupported("special entry"));
    }
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, PackageError> {
    let end = offset
        .checked_add(2)
        .ok_or(PackageError::InvalidZip("offset overflow"))?;
    let raw = bytes
        .get(offset..end)
        .ok_or(PackageError::Truncated("header"))?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, PackageError> {
    let end = offset
        .checked_add(4)
        .ok_or(PackageError::InvalidZip("offset overflow"))?;
    let raw = bytes
        .get(offset..end)
        .ok_or(PackageError::Truncated("header"))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn push_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use panelet_management::{WidgetManifest, WIDGET_ABI};

    fn candidate() -> WidgetCandidate {
        WidgetCandidate {
            manifest: WidgetManifest {
                id: "sample.calendar".to_owned(),
                name: "Calendar".to_owned(),
                version: "0.1.0".to_owned(),
                abi: WIDGET_ABI.to_owned(),
                capabilities: vec!["screen.present".to_owned()],
                host_permissions: vec![],
            },
            source: "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { os::runtime::ok(()) } }".to_owned(),
            resources: vec![
                WidgetResource {
                    path: "labels/title.txt".to_owned(),
                    text: "CALENDAR".to_owned(),
                },
                WidgetResource {
                    path: "data.json".to_owned(),
                    text: "{}".to_owned(),
                },
            ],
        }
    }

    #[test]
    fn legacy_abi_is_rejected_for_execution_but_identity_is_verified_for_migration() {
        let package = candidate();
        let mut manifest = serde_json::to_value(&package.manifest).unwrap();
        manifest["abi"] = serde_json::json!("note4c.rune/2");
        manifest["entry"] = serde_json::json!("main");
        let bytes = raw_zip(&[
            ("manifest.json", &serde_json::to_vec(&manifest).unwrap()),
            ("main.rn", b"pub async fn main(std) { std.runtime.ok(()) }")
        ], &[]);
        assert!(matches!(decode_pnw(&bytes), Err(PackageError::Unsupported("widget ABI"))));
        assert_eq!(package_owner_id(&bytes).unwrap(), package.manifest.id);
        let mut corrupt = bytes.clone();
        let offset = corrupt.windows(3).position(|w| w == b"pub").unwrap();
        corrupt[offset] = b'x';
        assert!(package_owner_id(&corrupt).is_err());
        manifest["abi"] = serde_json::json!(WIDGET_ABI);
        let v3_with_entry = raw_zip(&[
            ("manifest.json", &serde_json::to_vec(&manifest).unwrap()),
            ("main.rn", package.source.as_bytes())
        ], &[]);
        assert!(decode_pnw(&v3_with_entry).is_err());
        assert!(package_owner_id(&v3_with_entry).is_err());
    }

    #[test]
    fn store_round_trip_is_deterministic_and_utf8() {
        let package = encode_pnw(&candidate());
        let decoded = decode_pnw(&package).unwrap();
        assert_eq!(decoded, candidate());
        assert_eq!(package, encode_pnw(&decoded));
        assert_eq!(package, encode_n4w(&decoded));
        assert_eq!(decode_n4w(&package).unwrap(), decoded);
        assert!(package.windows(4).any(|window| window == b"PK\x03\x04"));
        assert!(!package.windows(4).any(|window| window == b"PK\x07\x08"));
    }

    #[test]
    fn rejects_trailing_data_and_archive_comments() {
        let mut package = encode_pnw(&candidate());
        package.extend_from_slice(b"tail");
        assert!(decode_pnw(&package).is_err());

        let mut commented = encode_pnw(&candidate());
        let eocd = commented.len() - 22;
        commented[eocd + 20..eocd + 22].copy_from_slice(&1u16.to_le_bytes());
        commented.push(b'x');
        assert!(decode_pnw(&commented).is_err());
    }

    #[test]
    fn rejects_compression_encryption_and_zip64() {
        let mut compressed = encode_pnw(&candidate());
        let local = 8;
        compressed[local..local + 2].copy_from_slice(&8u16.to_le_bytes());
        let central = find_signature(&compressed, CENTRAL_FILE_HEADER_SIGNATURE).unwrap();
        compressed[central + 10..central + 12].copy_from_slice(&8u16.to_le_bytes());
        assert!(decode_pnw(&compressed).is_err());

        let mut encrypted = encode_pnw(&candidate());
        encrypted[6..8].copy_from_slice(&0x0801u16.to_le_bytes());
        let central = find_signature(&encrypted, CENTRAL_FILE_HEADER_SIGNATURE).unwrap();
        encrypted[central + 8..central + 10].copy_from_slice(&0x0801u16.to_le_bytes());
        assert!(decode_pnw(&encrypted).is_err());

        let mut zip64 = encode_pnw(&candidate());
        let eocd = zip64.len() - 22;
        zip64[eocd + 10..eocd + 12].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(decode_pnw(&zip64).is_err());
    }

    #[test]
    fn rejects_crc_mismatch() {
        let mut package = encode_pnw(&candidate());
        let local = find_signature(&package, LOCAL_FILE_HEADER_SIGNATURE).unwrap();
        let name_len = u16::from_le_bytes([package[local + 26], package[local + 27]]) as usize;
        let data_start = local + LOCAL_FILE_HEADER_BYTES + name_len;
        package[data_start] ^= 1;
        assert!(matches!(
            decode_pnw(&package),
            Err(PackageError::CrcMismatch { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_and_traversal_paths() {
        let package = candidate();
        let manifest = serde_json::to_vec(&package.manifest).unwrap();
        let entries = vec![
            ("manifest.json", manifest.as_slice()),
            ("main.rn", package.source.as_bytes()),
            ("resources/a.txt", b"a".as_slice()),
            ("resources/a.txt", b"b".as_slice()),
        ];
        assert!(decode_pnw(&raw_zip(&entries, &[])).is_err());

        let entries = vec![
            ("manifest.json", manifest.as_slice()),
            ("main.rn", package.source.as_bytes()),
            ("resources/../secret", b"x".as_slice()),
        ];
        assert!(decode_pnw(&raw_zip(&entries, &[])).is_err());
    }

    #[test]
    fn rejects_directory_and_symlink_entries() {
        let package = candidate();
        let manifest = serde_json::to_vec(&package.manifest).unwrap();
        let entries = vec![
            ("manifest.json", manifest.as_slice()),
            ("main.rn", package.source.as_bytes()),
            ("resources/dir/", b"".as_slice()),
        ];
        let attrs = [0x0000_0010, 0, 0];
        assert!(decode_pnw(&raw_zip(&entries, &attrs)).is_err());

        let entries = vec![
            ("manifest.json", manifest.as_slice()),
            ("main.rn", package.source.as_bytes()),
            ("resources/link", b"target".as_slice()),
        ];
        let attrs = [0, 0, 0xA000_0000];
        assert!(decode_pnw(&raw_zip(&entries, &attrs)).is_err());
    }

    #[test]
    fn enforces_source_resource_and_entry_limits() {
        let mut oversized_source = candidate();
        oversized_source.source = "x".repeat(MAX_SOURCE_BYTES + 1);
        assert!(decode_pnw(&encode_pnw(&oversized_source)).is_err());

        let mut oversized_resource = candidate();
        oversized_resource.resources[0].text = "x".repeat(MAX_RESOURCE_ITEM_BYTES + 1);
        assert!(decode_pnw(&encode_pnw(&oversized_resource)).is_err());

        let mut too_many = candidate();
        too_many.resources = (0..MAX_RESOURCE_COUNT + 1)
            .map(|index| WidgetResource {
                path: format!("{index}.txt"),
                text: "x".to_owned(),
            })
            .collect();
        assert!(decode_pnw(&encode_pnw(&too_many)).is_err());

        let mut too_large_package = candidate();
        too_large_package.source = "x".repeat(MAX_PACKAGE_BYTES);
        assert!(decode_pnw(&encode_pnw(&too_large_package)).is_err());
    }

    fn find_signature(bytes: &[u8], signature: u32) -> Option<usize> {
        let needle = signature.to_le_bytes();
        bytes.windows(4).position(|window| window == needle)
    }

    /// Small test-only ZIP writer. `external_attributes` is indexed by entry;
    /// zero means an ordinary regular file.
    fn raw_zip(entries: &[(&str, &[u8])], external_attributes: &[u32]) -> Vec<u8> {
        let mut encoded = Vec::new();
        let mut central = Vec::new();
        let mut offsets = Vec::new();
        for (name, data) in entries {
            let name = name.as_bytes();
            let offset = encoded.len() as u32;
            offsets.push(offset);
            push_u32(&mut encoded, LOCAL_FILE_HEADER_SIGNATURE);
            push_u16(&mut encoded, 20);
            push_u16(&mut encoded, UTF8_FLAG);
            push_u16(&mut encoded, 0);
            push_u16(&mut encoded, 0);
            push_u16(&mut encoded, 0);
            push_u32(&mut encoded, crc32(data));
            push_u32(&mut encoded, data.len() as u32);
            push_u32(&mut encoded, data.len() as u32);
            push_u16(&mut encoded, name.len() as u16);
            push_u16(&mut encoded, 0);
            encoded.extend_from_slice(name);
            encoded.extend_from_slice(data);
        }
        for (index, ((name, data), offset)) in entries.iter().zip(offsets).enumerate() {
            let name = name.as_bytes();
            push_u32(&mut central, CENTRAL_FILE_HEADER_SIGNATURE);
            push_u16(&mut central, 20);
            push_u16(&mut central, 20);
            push_u16(&mut central, UTF8_FLAG);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, crc32(data));
            push_u32(&mut central, data.len() as u32);
            push_u32(&mut central, data.len() as u32);
            push_u16(&mut central, name.len() as u16);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(
                &mut central,
                external_attributes.get(index).copied().unwrap_or_default(),
            );
            push_u32(&mut central, offset);
            central.extend_from_slice(name);
        }
        let central_offset = encoded.len() as u32;
        encoded.extend_from_slice(&central);
        push_u32(&mut encoded, END_OF_CENTRAL_DIRECTORY_SIGNATURE);
        push_u16(&mut encoded, 0);
        push_u16(&mut encoded, 0);
        push_u16(&mut encoded, entries.len() as u16);
        push_u16(&mut encoded, entries.len() as u16);
        push_u32(&mut encoded, central.len() as u32);
        push_u32(&mut encoded, central_offset);
        push_u16(&mut encoded, 0);
        encoded
    }
}
