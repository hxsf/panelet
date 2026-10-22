#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Provision NOTE4C Wi-Fi over its native USB Serial/JTAG console."""

from __future__ import annotations

import argparse
import fcntl
import json
import os
import secrets
import select
import stat
import sys
import termios
import time


HELLO_PREFIX = b"N4C-HELLO/1 "
REQUEST_PREFIX = b"N4C-PROVISION/1 "
RESULT_PREFIX = b"N4C-RESULT/1 "
MAX_BUFFER = 64 * 1024
REQUEST_FAILURES = {
    "invalid_credentials": "SSID or password length is invalid",
    "command_queue_full": "device command queue is full; retry later",
    "job_capacity_full": "device job ledger is full; retry later",
    "job_id_exhausted": "device could not allocate a job ID",
    "broker_unavailable": "device management broker is unavailable",
}
WIFI_FAILURES = {
    "wifi_ap_not_found": (
        "access point not found or did not match the supplied credentials/security; "
        "check the exact SSID, 2.4 GHz availability, range, password, and security mode"
    ),
    "wifi_authentication_timeout": (
        "the AP did not answer authentication; check range, AP load, and security mode"
    ),
    "wifi_authentication_failed": "authentication or key exchange failed; check the password and AP security mode",
    "wifi_security_incompatible": "AP security is incompatible; check WPA2/WPA3 and PMF settings",
    "wifi_association_failed": "association failed; check AP capacity and security settings",
    "wifi_signal_too_weak": "access point signal is below the connection threshold",
    "wifi_beacon_timeout": "access point beacon timed out",
    "wifi_dhcp_timeout": "associated with the AP but did not obtain an IP address",
    "wifi_connection_timeout": "connection timed out without a specific disconnect reason",
    "wifi_connection_failed": "connection failed for an unclassified Wi-Fi reason",
    "wifi_driver_error": "the device Wi-Fi driver rejected the operation",
    "network_rejected": "connection was rejected without a detailed reason",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Configure NOTE4C Wi-Fi through an explicit USB serial port."
    )
    parser.add_argument("--port", required=True, help="explicit /dev character device")
    parser.add_argument("--ssid", required=True, help="target Wi-Fi SSID")
    parser.add_argument("--password", required=True, help="target Wi-Fi password")
    parser.add_argument(
        "--timeout",
        type=float,
        default=60.0,
        help="total seconds to wait (default: 60)",
    )
    return parser.parse_args()


def validate_args(args: argparse.Namespace) -> None:
    if not args.port.startswith("/dev/"):
        raise ValueError("--port must be an explicit path under /dev")
    try:
        mode = os.stat(args.port).st_mode
    except OSError as error:
        raise ValueError(f"cannot access serial port: {error.strerror}") from error
    if not stat.S_ISCHR(mode):
        raise ValueError("--port is not a character device")
    ssid_bytes = args.ssid.encode("utf-8")
    password_bytes = args.password.encode("utf-8")
    if not 1 <= len(ssid_bytes) <= 32:
        raise ValueError("--ssid must contain 1..32 UTF-8 bytes")
    if not (len(password_bytes) == 0 or 8 <= len(password_bytes) <= 63):
        raise ValueError("--password must be empty or contain 8..63 UTF-8 bytes")
    if not 1 <= args.timeout <= 300:
        raise ValueError("--timeout must be between 1 and 300 seconds")


def configure_serial(fd: int) -> None:
    attributes = termios.tcgetattr(fd)
    attributes[0] = termios.IGNBRK
    attributes[1] = 0
    attributes[2] = termios.CLOCAL | termios.CREAD | termios.CS8
    attributes[3] = 0
    attributes[4] = termios.B115200
    attributes[5] = termios.B115200
    attributes[6][termios.VMIN] = 0
    attributes[6][termios.VTIME] = 0
    termios.tcsetattr(fd, termios.TCSANOW, attributes)
    termios.tcflush(fd, termios.TCIFLUSH)
    flags = fcntl.fcntl(fd, fcntl.F_GETFL)
    fcntl.fcntl(fd, fcntl.F_SETFL, flags | os.O_NONBLOCK)


def write_all(fd: int, payload: bytes, deadline: float) -> None:
    offset = 0
    while offset < len(payload):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("serial write timed out")
        _, writable, _ = select.select([], [fd], [], min(remaining, 1.0))
        if not writable:
            continue
        try:
            offset += os.write(fd, payload[offset:])
        except BlockingIOError:
            continue


def result_objects(buffer: bytearray):
    while b"\n" in buffer:
        raw_line, _, remainder = buffer.partition(b"\n")
        buffer[:] = remainder
        marker = raw_line.find(RESULT_PREFIX)
        if marker < 0:
            continue
        payload = raw_line[marker + len(RESULT_PREFIX) :].decode("utf-8", "ignore")
        try:
            value, _ = json.JSONDecoder().raw_decode(payload.lstrip())
        # Besides JSONDecodeError, Python can raise a plain ValueError for
        # bounded-but-pathological numbers (for example, an integer with more
        # digits than the interpreter limit). Treat every decode failure as
        # untrusted serial noise and never reflect its contents.
        except (ValueError, TypeError):
            continue
        if isinstance(value, dict):
            yield value


def validated_job_id(result: dict) -> int | None:
    job_id = result.get("job_id")
    if (
        isinstance(job_id, int)
        and not isinstance(job_id, bool)
        and 1 <= job_id <= 0xFFFFFFFF
    ):
        return job_id
    return None


def wait_for_state(
    fd: int,
    request_id: str,
    wanted: set[str],
    deadline: float,
    buffer: bytearray,
    expected_job_id: int | None = None,
) -> dict:
    while time.monotonic() < deadline:
        remaining = deadline - time.monotonic()
        readable, _, _ = select.select([fd], [], [], min(remaining, 0.5))
        if readable:
            try:
                chunk = os.read(fd, 4096)
            except BlockingIOError:
                chunk = b""
            if chunk:
                buffer.extend(chunk)
                if len(buffer) > MAX_BUFFER:
                    del buffer[: len(buffer) - MAX_BUFFER]
        for value in result_objects(buffer):
            state = value.get("state")
            if value.get("id") != request_id or not isinstance(state, str):
                continue
            if state not in wanted:
                continue
            if expected_job_id is not None and validated_job_id(value) != expected_job_id:
                continue
            return value
    raise TimeoutError("device response timed out")


def safe_failure_message(result: dict, allowlist: dict[str, str], fallback: str) -> str:
    code = result.get("code")
    if isinstance(code, str) and code in allowlist:
        return f"{code}: {allowlist[code]}"
    return fallback


def provision(args: argparse.Namespace) -> None:
    request_id = secrets.token_hex(8)
    deadline = time.monotonic() + args.timeout
    fd = os.open(args.port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    try:
        configure_serial(fd)
        buffer = bytearray()

        # Opening a native USB console can race a reset. Probe repeatedly and
        # send credentials only after this firmware version answers.
        while True:
            write_all(fd, HELLO_PREFIX + request_id.encode("ascii") + b"\n", deadline)
            probe_deadline = min(deadline, time.monotonic() + 1.0)
            try:
                wait_for_state(fd, request_id, {"ready"}, probe_deadline, buffer)
                break
            except TimeoutError:
                if time.monotonic() >= deadline:
                    raise

        request = {
            "id": request_id,
            "ssid": args.ssid,
            "password": args.password,
        }
        payload = REQUEST_PREFIX + json.dumps(
            request, ensure_ascii=False, separators=(",", ":")
        ).encode("utf-8") + b"\n"
        write_all(fd, payload, deadline)
        first = wait_for_state(
            fd,
            request_id,
            {"accepted", "succeeded", "pending", "failed"},
            deadline,
            buffer,
        )
        first_job_id = validated_job_id(first)
        if first["state"] == "failed" and first_job_id is None:
            reason = safe_failure_message(
                first, REQUEST_FAILURES, "device rejected the provisioning request"
            )
            raise RuntimeError(f"device rejected request: {reason}")
        accepted_job_id = first_job_id
        if accepted_job_id is None:
            raise RuntimeError("device accepted request without a valid job ID")
        if first["state"] == "accepted":
            try:
                result = wait_for_state(
                    fd,
                    request_id,
                    {"succeeded", "pending", "failed"},
                    deadline,
                    buffer,
                    expected_job_id=accepted_job_id,
                )
            except TimeoutError:
                raise TimeoutError(
                    f"device job {accepted_job_id} is still pending and may still change Wi-Fi"
                ) from None
        else:
            # A terminal frame can arrive even if an accepted frame was lost
            # or corrupted in a mixed serial log. It is still trustworthy when
            # it carries this request ID and a bounded numeric job ID.
            result = first
        if result["state"] == "pending":
            raise TimeoutError(
                f"device job {accepted_job_id} is still pending and may still change Wi-Fi"
            )
        if result["state"] != "succeeded":
            reason = safe_failure_message(
                result, WIFI_FAILURES, "unrecognized device failure"
            )
            raise RuntimeError(f"Wi-Fi connection failed: {reason}")
    finally:
        os.close(fd)


def main() -> int:
    args = parse_args()
    try:
        validate_args(args)
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(
        "warning: --password may be visible in shell history and the process list; "
        "close espflash monitor before continuing",
        file=sys.stderr,
    )
    try:
        provision(args)
    except TimeoutError as error:
        print(f"error: {error}", file=sys.stderr)
        return 3
    except (OSError, RuntimeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print("NOTE4C Wi-Fi provisioning succeeded.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
