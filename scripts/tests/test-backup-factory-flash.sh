#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0


set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../.." && pwd -P)"
backup_script="$repo_root/scripts/backup-factory-flash.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/note4c-backup-test.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT

fake_esptool="$test_root/fake-esptool"
fake_espefuse="$test_root/fake-espefuse"

cat >"$fake_esptool" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

: "${FAKE_ESPTOOL_STYLE:?}"
: "${FAKE_CLI_LOG:?}"
{
    printf 'esptool'
    printf '\t%q' "$@"
    printf '\n'
} >>"$FAKE_CLI_LOG"

for argument in "$@"; do
    if [[ "$argument" =~ (^|[-_])(write|erase|burn|force)([-_]|$) ]]; then
        printf 'forbidden esptool argument: %s\n' "$argument" >&2
        exit 90
    fi
done

if [[ "$FAKE_ESPTOOL_STYLE" == "hyphen" ]]; then
    read_command="read-flash"
    flash_id_command="flash-id"
    security_command="get-security-info"
else
    read_command="read_flash"
    flash_id_command="flash_id"
    security_command="get_security_info"
fi

if (($# == 2)) && [[ "$1" == "$read_command" && "$2" == "--help" ]]; then
    exit 0
fi

if (($# == 1)) && [[ "$1" == "version" ]]; then
    printf 'esptool fake-%s\n' "$FAKE_ESPTOOL_STYLE"
    exit 0
fi

if (($# == 7)) && [[ "$1" == "--chip" && "$2" == "esp32s3" && \
    "$3" == "--port" && "$4" == "/dev/null" && "$5" == "--baud" && \
    "$6" == "115200" && "$7" == "$flash_id_command" ]]; then
    printf '%s\n' 'Chip is ESP32-S3 (fake)' 'MAC: aa:bb:cc:dd:ee:ff' \
        'Manufacturer: ef' 'Device: 4018' 'Detected flash size: 16MB'
    exit 0
fi

if (($# == 7)) && [[ "$1" == "--chip" && "$2" == "esp32s3" && \
    "$3" == "--port" && "$4" == "/dev/null" && "$5" == "--baud" && \
    "$6" == "115200" && "$7" == "$security_command" ]]; then
    printf '%s\n' 'MAC: aa:bb:cc:dd:ee:ff' \
        'Secure Boot: Disabled' 'Flash Encryption: Disabled' \
        'Download Mode: Enabled'
    exit 0
fi

    if (($# == 10)) && [[ "$1" == "--chip" && "$2" == "esp32s3" && \
        "$3" == "--port" && "$4" == "/dev/null" && "$5" == "--baud" && \
        "$6" == "115200" && "$7" == "$read_command" && "$8" == "0x0" && \
        "$9" == "0x1000000" ]]; then
    if [[ -n "${FAKE_INTERRUPT_SIGNAL:-}" ]]; then
        dd if=/dev/zero of="${10}" bs=1048576 count=1 2>/dev/null
        kill "-$FAKE_INTERRUPT_SIGNAL" "$PPID"
        exit 92
    fi
    dd if=/dev/zero of="${10}" bs=1048576 count=16 2>/dev/null
    printf '%s\n' 'MAC: aa:bb:cc:dd:ee:ff' 'Read 16777216 bytes'
    exit 0
fi

printf 'unexpected esptool argv:' >&2
printf ' %q' "$@" >&2
printf '\n' >&2
exit 91
EOF

cat >"$fake_espefuse" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

: "${FAKE_CLI_LOG:?}"
{
    printf 'espefuse'
    printf '\t%q' "$@"
    printf '\n'
} >>"$FAKE_CLI_LOG"

for argument in "$@"; do
    if [[ "$argument" =~ (^|[-_])(write|erase|burn|force)([-_]|$) ]]; then
        printf 'forbidden espefuse argument: %s\n' "$argument" >&2
        exit 90
    fi
done

expected=(
    --chip esp32s3 --port /dev/null summary
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

if (($# != ${#expected[@]})); then
    printf 'unexpected espefuse argument count: %s\n' "$#" >&2
    exit 91
fi

for index in "${!expected[@]}"; do
    argument_number=$((index + 1))
    if [[ "${!argument_number}" != "${expected[$index]}" ]]; then
        printf 'unexpected espefuse argv at %s: %q\n' "$index" "${!argument_number}" >&2
        exit 91
    fi
done

printf '%s\n' \
    'MAC: aa:bb:cc:dd:ee:ff' \
    'Security fuses:' \
    'SPI_BOOT_CRYPT_CNT (BLOCK0) = Disable R/W (0b000)' \
    'SECURE_BOOT_EN (BLOCK0) = False R/W (0b0)' \
    'DIS_DOWNLOAD_MODE (BLOCK0) = False R/W (0b0)' \
    'DIS_USB_OTG (BLOCK0) = False R/W (0b0)' \
    'DIS_USB_SERIAL_JTAG_DOWNLOAD_MODE (BLOCK0) = False R/W (0b0)'
EOF

chmod 700 "$fake_esptool" "$fake_espefuse"
ln -s "$fake_esptool" "$test_root/esptool"
mkdir -m 700 "$test_root/output"

"$backup_script" --help >/dev/null

if "$backup_script" >/dev/null 2>&1; then
    printf '%s\n' 'missing required arguments unexpectedly succeeded' >&2
    exit 1
fi

if "$backup_script" \
    --port /dev/null \
    --output-dir "$repo_root" \
    --esptool "$fake_esptool" \
    --espefuse "$fake_espefuse" >/dev/null 2>&1; then
    printf '%s\n' 'repository-local output unexpectedly succeeded' >&2
    exit 1
fi

run_backup_test() {
    local style="$1"
    local output_dir="$test_root/output/$style"
    local cli_log="$test_root/$style-cli.log"
    mkdir -m 700 "$output_dir"

    FAKE_ESPTOOL_STYLE="$style" FAKE_CLI_LOG="$cli_log" \
        "$backup_script" \
        --port /dev/null \
        --output-dir "$output_dir" \
        --esptool "$fake_esptool" \
        --espefuse "$fake_espefuse" \
        --baud 115200 >/dev/null

    local backup_dirs=("$output_dir"/note4c-factory-backup-*)
    [[ "${#backup_dirs[@]}" == "1" ]]
    local backup_dir="${backup_dirs[0]}"
    local image="$backup_dir/factory-flash-16MiB.bin"

    [[ "$(wc -c <"$image" | tr -d '[:space:]')" == "16777216" ]]
    (
        cd "$backup_dir"
        if command -v sha256sum >/dev/null 2>&1; then
            sha256sum -c factory-flash-16MiB.bin.sha256 >/dev/null
        else
            shasum -a 256 -c factory-flash-16MiB.bin.sha256 >/dev/null
        fi
    )

    if grep -ER 'aa:bb:cc:dd:ee:ff|/dev/null' "$backup_dir" >/dev/null; then
        printf '%s\n' 'generated summaries leaked a mock device identifier or port' >&2
        exit 1
    fi

    grep -Fq '#   esptool' "$backup_dir/restore-command.txt"
    if awk 'NF && $0 !~ /^#/ { exit 1 }' "$backup_dir/restore-command.txt"; then
        :
    else
        printf '%s\n' 'restore template contains a non-comment line' >&2
        exit 1
    fi
    : >"$test_root/template-execution.log"
    PATH="$test_root:$PATH" FAKE_CLI_LOG="$test_root/template-execution.log" \
        bash "$backup_dir/restore-command.txt"
    [[ ! -s "$test_root/template-execution.log" ]]

    if grep -Eiq '(^|[[:space:]])(write|erase|burn|force)(-|_)' "$cli_log"; then
        printf '%s\n' 'backup path invoked a forbidden destructive operation' >&2
        exit 1
    fi

    if grep -Eq '^espefuse[[:space:]]+version([[:space:]]|$)' "$cli_log"; then
        printf '%s\n' 'backup path queried espefuse version without an explicit port' >&2
        exit 1
    fi
}

run_backup_test hyphen
run_backup_test underscore

for signal_name in HUP INT TERM; do
    interrupt_output="$test_root/output/interrupt-$signal_name"
    interrupt_log="$test_root/interrupt-$signal_name-cli.log"
    mkdir -m 700 "$interrupt_output"
    if FAKE_ESPTOOL_STYLE=hyphen \
        FAKE_CLI_LOG="$interrupt_log" \
        FAKE_INTERRUPT_SIGNAL="$signal_name" \
        "$backup_script" \
        --port /dev/null \
        --output-dir "$interrupt_output" \
        --esptool "$fake_esptool" \
        --espefuse "$fake_espefuse" \
        --baud 115200 >/dev/null 2>&1; then
        printf 'interrupted backup unexpectedly succeeded for %s\n' "$signal_name" >&2
        exit 1
    fi
    if find "$interrupt_output" -name 'factory-flash-16MiB.bin' -type f -print -quit |
        grep -q .; then
        printf 'partial image survived %s\n' "$signal_name" >&2
        exit 1
    fi
done

printf '%s\n' 'backup-factory-flash strict mock tests: PASS'
