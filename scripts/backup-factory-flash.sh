#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0


set -euo pipefail

readonly FLASH_START="0x0"
readonly FLASH_SIZE_HEX="0x1000000"
readonly FLASH_SIZE_BYTES="16777216"

usage() {
    cat <<'EOF'
Back up the complete 16 MiB factory Flash of a NOTE4C (ESP32-S3).

This script is deliberately read-only. It never erases Flash, writes Flash,
writes eFuses, or restores an image.

Usage:
  scripts/backup-factory-flash.sh \
    --port /dev/cu.usbmodemXXXX \
    --output-dir /absolute/path/outside/the/repository

Required:
  --port PATH          Explicit serial device under /dev; auto-detection is forbidden.
  --output-dir DIR     Existing, writable directory outside this repository.

Optional:
  --baud RATE          Read baud rate (default: 460800).
  --esptool PATH       Explicit esptool executable.
  --espefuse PATH      Explicit espefuse executable.
  -h, --help           Show this help.

The output is a new mode-0700 directory containing:
  factory-flash-16MiB.bin          Exact 16 MiB Flash image (sensitive)
  factory-flash-16MiB.bin.sha256   SHA-256 checksum
  manifest.txt                     Capture metadata without device identifiers
  flash-summary.txt                Curated, non-unique Flash information
  security-summary.txt             Curated esptool security status, if available
  efuse-security-summary.txt       Selected security eFuses only, if available
  restore-command.txt              A review-only restore command template

Before running, manually prove BOOT+RESET ROM-download recovery as described in
docs/bringup-and-recovery.md. Do not commit or share the generated backup.
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 2
}

port=""
output_dir=""
baud="460800"
esptool_override=""
espefuse_override=""

while (($# > 0)); do
    case "$1" in
        --port)
            (($# >= 2)) || die "--port requires a value"
            port="$2"
            shift 2
            ;;
        --output-dir)
            (($# >= 2)) || die "--output-dir requires a value"
            output_dir="$2"
            shift 2
            ;;
        --baud)
            (($# >= 2)) || die "--baud requires a value"
            baud="$2"
            shift 2
            ;;
        --esptool)
            (($# >= 2)) || die "--esptool requires a value"
            esptool_override="$2"
            shift 2
            ;;
        --espefuse)
            (($# >= 2)) || die "--espefuse requires a value"
            espefuse_override="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
done

[[ -n "$port" ]] || die "--port is required; serial-port auto-detection is forbidden"
[[ -n "$output_dir" ]] || die "--output-dir is required"
[[ "$port" =~ ^/dev/[A-Za-z0-9._/+:-]+$ ]] || die "--port must be an explicit device path under /dev"
[[ -c "$port" ]] || die "serial port does not exist or is not a character device: $port"
[[ "$baud" =~ ^[0-9]+$ ]] || die "--baud must be a positive integer"
((baud > 0)) || die "--baud must be greater than zero"
[[ -d "$output_dir" ]] || die "output directory does not exist: $output_dir"
[[ -w "$output_dir" ]] || die "output directory is not writable: $output_dir"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"
output_dir_abs="$(cd "$output_dir" && pwd -P)"

[[ "$output_dir_abs" != "/" ]] || die "refusing to use the filesystem root as an output directory"
case "$output_dir_abs/" in
    "$repo_root/"*)
        die "output directory must be outside the repository: $repo_root"
        ;;
esac

if [[ -n "$esptool_override" ]]; then
    [[ -x "$esptool_override" ]] || die "esptool override is not executable: $esptool_override"
    ESPTOOL=("$esptool_override")
elif command -v esptool >/dev/null 2>&1; then
    ESPTOOL=("$(command -v esptool)")
elif command -v esptool.py >/dev/null 2>&1; then
    ESPTOOL=("$(command -v esptool.py)")
elif command -v python3 >/dev/null 2>&1 && python3 -c 'import esptool' >/dev/null 2>&1; then
    ESPTOOL=("$(command -v python3)" -m esptool)
else
    die "esptool was not found; install Espressif esptool before connecting the board"
fi

if [[ -n "$espefuse_override" ]]; then
    [[ -x "$espefuse_override" ]] || die "espefuse override is not executable: $espefuse_override"
    ESPEFUSE=("$espefuse_override")
elif command -v espefuse >/dev/null 2>&1; then
    ESPEFUSE=("$(command -v espefuse)")
elif command -v espefuse.py >/dev/null 2>&1; then
    ESPEFUSE=("$(command -v espefuse.py)")
elif command -v python3 >/dev/null 2>&1 && python3 -c 'import espefuse' >/dev/null 2>&1; then
    ESPEFUSE=("$(command -v python3)" -m espefuse)
else
    die "espefuse was not found; it is required for the read-only security summary"
fi

if "${ESPTOOL[@]}" read-flash --help >/dev/null 2>&1; then
    read_flash_command="read-flash"
    flash_id_command="flash-id"
    security_info_command="get-security-info"
elif "${ESPTOOL[@]}" read_flash --help >/dev/null 2>&1; then
    read_flash_command="read_flash"
    flash_id_command="flash_id"
    security_info_command="get_security_info"
else
    die "unsupported esptool: neither read-flash nor read_flash is available"
fi

umask 077
temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/note4c-backup.XXXXXX")"
backup_dir=""
image=""
backup_complete=0
cleanup() {
    if [[ "$backup_complete" != "1" && -n "$image" && -f "$image" ]]; then
        rm -f -- "$image"
    fi
    rm -rf -- "$temp_dir"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

port_sed_pattern="$(printf '%s' "$port" | sed 's/[][\\.^$*+?{}|()]/\\&/g')"

redact_stream() {
    sed -E \
        -e "s|$port_sed_pattern|[REDACTED-PORT]|g" \
        -e 's/([[:xdigit:]]{2}:){5}[[:xdigit:]]{2}/[REDACTED-MAC]/g' \
        -e 's/((MAC|Chip UID|Unique ID|Serial number)[^:=]*[:=][[:space:]]*)[^[:space:]]+/\1[REDACTED]/g'
}

run_captured() {
    local name="$1"
    shift
    local raw_log="$temp_dir/$name.raw.log"

    if "$@" >"$raw_log" 2>&1; then
        return 0
    fi

    printf 'read-only command failed: %s\n' "$name" >&2
    redact_stream <"$raw_log" >&2
    return 1
}

printf 'Checking that the connected Flash reports exactly 16 MiB...\n'
if ! run_captured flash-id \
    "${ESPTOOL[@]}" --chip esp32s3 --port "$port" --baud "$baud" "$flash_id_command"; then
    die "could not read Flash identification; no backup was attempted"
fi

if ! grep -Eiq '([Dd]etected )?[Ff]lash size:[[:space:]]*16[[:space:]]*(MB|MiB)' "$temp_dir/flash-id.raw.log"; then
    printf 'Flash identification (redacted):\n' >&2
    redact_stream <"$temp_dir/flash-id.raw.log" >&2
    die "Flash did not report 16 MiB; refusing the fixed 16 MiB read"
fi

timestamp="$(date -u '+%Y%m%dT%H%M%SZ')"
backup_dir="$output_dir_abs/note4c-factory-backup-$timestamp"
[[ ! -e "$backup_dir" ]] || die "backup directory already exists: $backup_dir"
mkdir -m 700 -- "$backup_dir"

flash_summary="$backup_dir/flash-summary.txt"
{
    printf 'NOTE4C curated Flash summary\n'
    printf 'Device identifiers and serial port are intentionally omitted.\n\n'
    grep -Ei 'Chip is|Features:|Crystal is|Flash (size|type|voltage)|Manufacturer:|Device:|Detected flash size:|SPI Flash Size' \
        "$temp_dir/flash-id.raw.log" || true
} | redact_stream >"$flash_summary"

security_summary="$backup_dir/security-summary.txt"
if run_captured security-info \
    "${ESPTOOL[@]}" --chip esp32s3 --port "$port" --baud "$baud" "$security_info_command"; then
    {
        printf 'NOTE4C curated esptool security summary\n'
        printf 'Device identifiers and serial port are intentionally omitted.\n\n'
        grep -Ei 'Secure Boot|Flash Encryption|Download Mode|JTAG|Crypt|Security' \
            "$temp_dir/security-info.raw.log" || true
    } | redact_stream >"$security_summary"
else
    printf '%s\n' 'UNAVAILABLE: esptool could not return its read-only security summary.' >"$security_summary"
fi

efuse_summary="$backup_dir/efuse-security-summary.txt"
efuse_fields=(
    SPI_BOOT_CRYPT_CNT
    SECURE_BOOT_KEY_REVOKE0
    SECURE_BOOT_KEY_REVOKE1
    SECURE_BOOT_KEY_REVOKE2
    SECURE_BOOT_EN
    SECURE_BOOT_AGGRESSIVE_REVOKE
    DIS_FORCE_DOWNLOAD
    DIS_DOWNLOAD_MODE
    ENABLE_SECURITY_DOWNLOAD
    SECURE_VERSION
    SOFT_DIS_JTAG
    DIS_PAD_JTAG
    DIS_USB_JTAG
    DIS_USB_SERIAL_JTAG
    DIS_USB_SERIAL_JTAG_DOWNLOAD_MODE
    DIS_USB_OTG
    DIS_USB_OTG_DOWNLOAD_MODE
)
efuse_pattern="$(IFS='|'; printf '%s' "${efuse_fields[*]}")"
if run_captured efuse-security \
    "${ESPEFUSE[@]}" --chip esp32s3 --port "$port" summary "${efuse_fields[@]}"; then
    {
        printf 'NOTE4C selected read-only eFuse security summary\n'
        printf 'MAC, custom MAC, optional unique ID, keys, and user blocks were not requested.\n\n'
        grep -E "^([[:space:]]*($efuse_pattern)[[:space:]]|Security fuses:|Jtag fuses:|Usb fuses:|EFUSE_NAME|[-]{5,})" \
            "$temp_dir/efuse-security.raw.log" || true
    } | redact_stream >"$efuse_summary"
else
    printf '%s\n' 'UNAVAILABLE: espefuse could not return the selected read-only fields.' >"$efuse_summary"
fi

image="$backup_dir/factory-flash-16MiB.bin"
printf 'Reading 16 MiB from %s through %s; this may take several minutes...\n' \
    "$FLASH_START" "$FLASH_SIZE_HEX"
if ! run_captured read-flash \
    "${ESPTOOL[@]}" --chip esp32s3 --port "$port" --baud "$baud" \
    "$read_flash_command" "$FLASH_START" "$FLASH_SIZE_HEX" "$image"; then
    rm -f -- "$image"
    die "Flash read failed; the incomplete image was removed"
fi

actual_size="$(wc -c <"$image" | tr -d '[:space:]')"
if [[ "$actual_size" != "$FLASH_SIZE_BYTES" ]]; then
    rm -f -- "$image"
    die "backup length was $actual_size bytes, expected $FLASH_SIZE_BYTES; invalid image removed"
fi

if command -v sha256sum >/dev/null 2>&1; then
    image_hash="$(sha256sum "$image" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
    image_hash="$(shasum -a 256 "$image" | awk '{print $1}')"
else
    rm -f -- "$image"
    die "neither sha256sum nor shasum is available; unverified image removed"
fi

checksum_file="$backup_dir/factory-flash-16MiB.bin.sha256"
printf '%s  %s\n' "$image_hash" "$(basename "$image")" >"$checksum_file"

esptool_version="$("${ESPTOOL[@]}" version 2>&1 | head -n 1 || true)"
manifest="$backup_dir/manifest.txt"
{
    printf 'Artifact: NOTE4C factory Flash backup\n'
    printf 'Captured UTC: %s\n' "$timestamp"
    printf 'Chip constraint: ESP32-S3\n'
    printf 'Board scope: NOTE4C PCB V1.0, ESP32-S3 N16R8\n'
    printf 'Serial port: [omitted]\n'
    printf 'Flash range: %s + %s\n' "$FLASH_START" "$FLASH_SIZE_HEX"
    printf 'Expected bytes: %s\n' "$FLASH_SIZE_BYTES"
    printf 'Actual bytes: %s\n' "$actual_size"
    printf 'SHA-256: %s\n' "$image_hash"
    printf 'esptool: %s\n' "$esptool_version"
    printf 'espefuse: version not queried (avoids implicit serial-device access)\n'
    printf 'Contains sensitive factory/NVS data: yes\n'
    printf 'Safe to commit or share: no\n'
} | redact_stream >"$manifest"

restore_template="$backup_dir/restore-command.txt"
printf -v image_shell_quoted '%q' "$image"
cat >"$restore_template" <<EOF
# REVIEW-ONLY RESTORE TEMPLATE. Every line is intentionally a shell comment.
#
# Do not run a restore until security-summary.txt and efuse-security-summary.txt
# have been reviewed. If Flash Encryption, Secure Boot, or security status is
# enabled/unknown, stop: a raw write may not be a valid or safe restoration path.
#
# Verify the image checksum first from inside this directory:
#   macOS: shasum -a 256 -c factory-flash-16MiB.bin.sha256
#   Linux: sha256sum -c factory-flash-16MiB.bin.sha256
#
# Candidate whole-image commands; choose syntax only after checking your
# installed esptool version, then replace <EXPLICIT_PORT> yourself:
#   esptool --chip esp32s3 --port '<EXPLICIT_PORT>' write-flash 0x0 $image_shell_quoted
#   esptool --chip esp32s3 --port '<EXPLICIT_PORT>' write_flash 0x0 $image_shell_quoted
#
# This template intentionally contains no erase-flash, eFuse, force, encryption,
# or automatic reset/retry operation.
EOF

chmod 600 "$image" "$checksum_file" "$manifest" "$flash_summary" \
    "$security_summary" "$efuse_summary" "$restore_template"

backup_complete=1

printf '\nBackup completed and verified.\n'
printf 'Directory: %s\n' "$backup_dir"
printf 'Bytes: %s\n' "$actual_size"
printf 'SHA-256: %s\n' "$image_hash"
printf 'The backup is sensitive and must remain outside version control.\n'
printf 'The restore command was recorded as a review-only template and was not run.\n'
