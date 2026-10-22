#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0


set -euo pipefail

die() {
    printf 'error: %s\n' "$*" >&2
    exit 2
}

[[ "${NOTE4C_FACTORY_BACKUP_VERIFIED:-}" == "YES" ]] || die \
    "set NOTE4C_FACTORY_BACKUP_VERIFIED=YES only after completing docs/devices/note4c/bringup-and-recovery.md"
[[ "${NOTE4C_SECURITY_STATE_COMPATIBLE:-}" == "YES" ]] || die \
    "set NOTE4C_SECURITY_STATE_COMPATIBLE=YES only after confirming the device security state permits this unsigned demo"
flash_mode="${NOTE4C_FLASH_MODE:-full}"
case "$flash_mode" in
    full)
        [[ "${NOTE4C_ALLOW_PARTITION_OVERWRITE:-}" == "YES" ]] || die \
            "set NOTE4C_ALLOW_PARTITION_OVERWRITE=YES only after accepting bootloader/partition/ota_0 overwrite and otadata erase"
        ;;
    app-only)
        [[ "${NOTE4C_EXISTING_OTA0_LAYOUT_VERIFIED:-}" == "YES" ]] || die \
            "app-only requires verification of the existing ota_0 at 0x20000 with size 0x3f0000 and active boot selection"
        ;;
    *) die "NOTE4C_FLASH_MODE must be full or app-only" ;;
esac

port="${NOTE4C_PORT:-}"
[[ -n "$port" ]] || die "NOTE4C_PORT is required; serial-port auto-detection is forbidden"
[[ "$port" =~ ^/dev/[A-Za-z0-9._/+:-]+$ ]] || die \
    "NOTE4C_PORT must be an explicit device path under /dev"
[[ -c "$port" ]] || die "NOTE4C_PORT is not a character device: $port"

(($# >= 1)) || die "Cargo did not pass an application ELF to the runner"
app_elf="$1"
shift
[[ -f "$app_elf" ]] || die "application ELF does not exist: $app_elf"
command -v espflash >/dev/null 2>&1 || die "espflash is not installed"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"
partition_table="$repo_root/app/os/partitions-note4c-16m-ota.csv"
if [[ "$flash_mode" == "app-only" ]]; then
    (($# == 0)) || die "app-only mode does not accept extra flash arguments"
    app_image_dir="$(mktemp -d "${TMPDIR:-/tmp}/note4c-app-only.XXXXXX")"
    trap 'rm -rf -- "$app_image_dir"' EXIT
    app_image="$app_image_dir/application.bin"
    espflash save-image --chip esp32s3 --flash-size 16mb \
        --partition-table "$partition_table" "$app_elf" "$app_image"
    [[ -f "$app_image" ]] || die "application image was not generated"
    image_bytes="$(wc -c < "$app_image" | tr -d '[:space:]')"
    ((image_bytes > 0 && image_bytes <= 0x3f0000)) || die "application image does not fit ota_0"
    image_magic="$(od -An -tu1 -N1 "$app_image" | tr -d '[:space:]')"
    [[ "$image_magic" == "233" ]] || die "application image has an invalid ESP header"
    printf 'Writing only ota_0 at 0x20000 on the explicitly selected NOTE4C.\n'
    ESPFLASH_PORT="$port" espflash write-bin --chip esp32s3 --non-interactive \
        --after hard-reset 0x20000 "$app_image"
    exit 0
fi

bootloader="$(dirname "$app_elf")/bootloader.bin"
[[ -f "$partition_table" ]] || die "partition table does not exist: $partition_table"
[[ -f "$bootloader" ]] || die "bootloader does not exist beside application ELF: $bootloader"

printf 'Flashing the explicitly selected NOTE4C port.\n'
printf 'The backup, security-state, and partition-overwrite gates were acknowledged.\n'
printf 'This writes the bootloader, partition table, and ota_0 app, and erases OTA selection data; it uses no eFuse command.\n'

# ESPFLASH_PORT has the highest documented connection precedence. Passing it
# here prevents espflash from auto-selecting a different serial device.
ESPFLASH_PORT="$port" exec espflash flash \
    --monitor \
    --flash-size 16mb \
    --bootloader "$bootloader" \
    --partition-table "$partition_table" \
    --partition-table-offset 0x8000 \
    --target-app-partition ota_0 \
    --erase-parts otadata \
    "$@" \
    "$app_elf"
