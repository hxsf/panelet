#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0


set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../.." && pwd -P)"
runner="$repo_root/scripts/flash-server.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/note4c-flash-runner-test.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

touch "$test_root/application.elf"
touch "$test_root/bootloader.bin"

if "$runner" "$test_root/application.elf" >/dev/null 2>&1; then
    printf '%s\n' 'runner bypassed the factory-backup gate' >&2
    exit 1
fi

if NOTE4C_FACTORY_BACKUP_VERIFIED=YES \
    "$runner" "$test_root/application.elf" >/dev/null 2>&1; then
    printf '%s\n' 'runner bypassed the security-state gate' >&2
    exit 1
fi

if NOTE4C_FACTORY_BACKUP_VERIFIED=YES \
    NOTE4C_SECURITY_STATE_COMPATIBLE=YES \
    "$runner" "$test_root/application.elf" >/dev/null 2>&1; then
    printf '%s\n' 'runner bypassed the partition-overwrite gate' >&2
    exit 1
fi

if NOTE4C_FACTORY_BACKUP_VERIFIED=YES \
    NOTE4C_SECURITY_STATE_COMPATIBLE=YES \
    NOTE4C_ALLOW_PARTITION_OVERWRITE=YES \
    "$runner" "$test_root/application.elf" >/dev/null 2>&1; then
    printf '%s\n' 'runner accepted an implicit serial port' >&2
    exit 1
fi

cat >"$test_root/espflash" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
: "${ESPFLASH_TEST_LOG:?}"
printf 'port=%s\n' "${ESPFLASH_PORT:-}" >"$ESPFLASH_TEST_LOG"
printf 'argv=' >>"$ESPFLASH_TEST_LOG"
printf '%q ' "$@" >>"$ESPFLASH_TEST_LOG"
printf '\n' >>"$ESPFLASH_TEST_LOG"
EOF
chmod 700 "$test_root/espflash"

ESPFLASH_TEST_LOG="$test_root/espflash.log" \
    NOTE4C_FACTORY_BACKUP_VERIFIED=YES \
    NOTE4C_SECURITY_STATE_COMPATIBLE=YES \
    NOTE4C_ALLOW_PARTITION_OVERWRITE=YES \
    NOTE4C_PORT=/dev/null \
    PATH="$test_root:$PATH" \
    "$runner" "$test_root/application.elf" >/dev/null

grep -Fqx 'port=/dev/null' "$test_root/espflash.log"
grep -Fq 'argv=flash --monitor --flash-size 16mb --bootloader ' "$test_root/espflash.log"
grep -Fq -- '--partition-table-offset 0x8000 --target-app-partition ota_0 --erase-parts otadata ' "$test_root/espflash.log"
grep -Fq "$test_root/application.elf" "$test_root/espflash.log"

# App-only updates require a separately verified existing layout, preserve
# data/boot selections, and cannot write a merged or oversized image.
if NOTE4C_FACTORY_BACKUP_VERIFIED=YES NOTE4C_SECURITY_STATE_COMPATIBLE=YES \
    NOTE4C_FLASH_MODE=app-only NOTE4C_PORT=/dev/null \
    "$runner" "$test_root/application.elf" >/dev/null 2>&1; then
    printf '%s\n' 'app-only bypassed existing-layout gate' >&2; exit 1
fi
cat >"$test_root/espflash" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == save-image ]]; then
    image="${@: -1}"
    case "${ESPFLASH_TEST_IMAGE:-valid}" in
        valid) printf '\351TEST-APP' > "$image" ;;
        invalid) printf 'NOT-ESP' > "$image" ;;
        oversized) printf '\351' > "$image"; dd if=/dev/zero of="$image" bs=1 count=1 seek=4128768 conv=notrunc 2>/dev/null ;;
    esac
else
    printf 'port=%s\n' "${ESPFLASH_PORT:-}" > "$ESPFLASH_TEST_LOG"
    printf '%s\n' "$*" >> "$ESPFLASH_TEST_LOG"
fi
EOF
for variant in valid invalid oversized; do
    rm -f "$test_root/app-only.log"
    if ESPFLASH_TEST_LOG="$test_root/app-only.log" ESPFLASH_TEST_IMAGE="$variant" \
        NOTE4C_FACTORY_BACKUP_VERIFIED=YES NOTE4C_SECURITY_STATE_COMPATIBLE=YES \
        NOTE4C_FLASH_MODE=app-only NOTE4C_EXISTING_OTA0_LAYOUT_VERIFIED=YES \
        NOTE4C_PORT=/dev/null PATH="$test_root:$PATH" \
        "$runner" "$test_root/application.elf" >/dev/null 2>&1; then
        [[ "$variant" == valid ]] || { printf '%s\n' 'invalid app image was accepted' >&2; exit 1; }
        grep -Fqx 'port=/dev/null' "$test_root/app-only.log"
        grep -Fq 'write-bin --chip esp32s3 --non-interactive --after hard-reset 0x20000 ' "$test_root/app-only.log"
        if grep -Eq 'erase|bootloader|partition-table' "$test_root/app-only.log"; then exit 1; fi
    else
        [[ "$variant" != valid ]] || { printf '%s\n' 'valid app-only image was rejected' >&2; exit 1; }
        [[ ! -e "$test_root/app-only.log" ]] || { printf '%s\n' 'invalid image reached device' >&2; exit 1; }
    fi
done

printf '%s\n' 'flash-server mock tests: PASS'
