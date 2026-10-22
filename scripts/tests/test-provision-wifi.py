#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0


from __future__ import annotations

import json
import os
import pathlib
import pty
import subprocess
import threading
import time


ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "provision-wifi.py"
HELLO = b"N4C-HELLO/1 "
REQUEST = b"N4C-PROVISION/1 "
RESULT = b"N4C-RESULT/1 "


def response(request_id: str, state: object, **extra) -> bytes:
    value = {"id": request_id, "state": state, **extra}
    return RESULT + json.dumps(value, separators=(",", ":")).encode() + b"\n"


def simulate(master: int, seen: dict[str, str], scenario: dict[str, object]) -> None:
    buffer = bytearray()
    while True:
        chunk = os.read(master, 1024)
        if not chunk:
            return
        buffer.extend(chunk)
        while b"\n" in buffer:
            line, _, remainder = buffer.partition(b"\n")
            buffer[:] = remainder
            line = line.rstrip(b"\r")
            if line.startswith(HELLO):
                request_id = line[len(HELLO) :].decode()
                os.write(master, b"ordinary PRIVATE-NET private-password boot log\r\n")
                ready = response(request_id, "ready")
                os.write(master, ready[:9])
                time.sleep(0.01)
                os.write(master, ready[9:])
            elif line.startswith(REQUEST):
                value = json.loads(line[len(REQUEST) :])
                seen["ssid"] = value["ssid"]
                seen["password"] = value["password"]
                request_id = value["id"]
                os.write(master, response("ffffffffffffffff", "failed", code="wrong_id"))
                if "request_code" in scenario:
                    os.write(
                        master,
                        response(
                            request_id,
                            "failed",
                            code=scenario["request_code"],
                            message=scenario.get("message", ""),
                        ),
                    )
                    return
                if not scenario.get("skip_accepted"):
                    os.write(
                        master,
                        response(
                            request_id,
                            "accepted",
                            job_id=scenario.get("accepted_job_id", 7),
                        ),
                    )
                os.write(
                    master,
                    b"post-accept PRIVATE-NET private-password diagnostic log\r\n",
                )
                time.sleep(0.02)
                if scenario.get("malformed_state"):
                    os.write(
                        master,
                        response(
                            request_id,
                            {"PRIVATE-NET": "private-password"},
                            job_id=7,
                        ),
                    )
                if scenario.get("oversized_integer"):
                    os.write(
                        master,
                        RESULT
                        + b'{"id":"'
                        + request_id.encode()
                        + b'","state":"diagnostic","number":'
                        + b"1" * 5000
                        + b"}\n",
                    )
                if scenario.get("mismatched_job"):
                    os.write(
                        master,
                        response(
                            request_id,
                            "failed",
                            job_id=8,
                            code="wifi_authentication_failed",
                            message="PRIVATE-NET private-password",
                        ),
                    )
                if scenario.get("no_terminal"):
                    return
                fields = dict(scenario.get("terminal_fields", {}))
                fields.setdefault("job_id", 7)
                terminal = response(request_id, scenario["state"], **fields)
                if scenario.get("fragment_terminal"):
                    split = len(terminal) // 2
                    os.write(master, terminal[:split])
                    time.sleep(0.01)
                    os.write(master, terminal[split:])
                else:
                    os.write(master, terminal)
                return


def run_case(
    scenario: dict[str, object],
) -> tuple[subprocess.CompletedProcess[str], dict[str, str]]:
    master, slave = pty.openpty()
    port = os.ttyname(slave)
    seen: dict[str, str] = {}
    worker = threading.Thread(
        target=simulate, args=(master, seen, scenario), daemon=True
    )
    worker.start()
    ssid = "PRIVATE-NET"
    password = "private-password"
    completed = subprocess.run(
        [
            str(SCRIPT),
            "--port",
            port,
            "--ssid",
            ssid,
            "--password",
            password,
            "--timeout",
            str(scenario.get("timeout", 5)),
        ],
        text=True,
        capture_output=True,
        check=False,
    )
    worker.join(timeout=1)
    os.close(slave)
    os.close(master)
    return completed, seen


def assert_no_credentials(completed: subprocess.CompletedProcess[str]) -> None:
    combined = completed.stdout + completed.stderr
    assert "PRIVATE-NET" not in combined
    assert "private-password" not in combined


def main() -> None:
    completed, seen = run_case({"state": "succeeded"})
    assert completed.returncode == 0, completed.stderr
    assert "provisioning succeeded" in completed.stdout
    assert seen == {"ssid": "PRIVATE-NET", "password": "private-password"}
    assert_no_credentials(completed)

    pending, seen = run_case(
        {
            "state": "pending",
            "terminal_fields": {"code": "device_timeout", "job_id": 7},
        }
    )
    assert pending.returncode == 3, pending.stderr
    assert "still pending and may still change Wi-Fi" in pending.stderr
    assert seen == {"ssid": "PRIVATE-NET", "password": "private-password"}
    assert_no_credentials(pending)

    auth, _ = run_case(
        {
            "state": "failed",
            "terminal_fields": {"code": "wifi_authentication_failed"},
            "fragment_terminal": True,
        }
    )
    assert auth.returncode == 1, auth.stderr
    assert "wifi_authentication_failed" in auth.stderr
    assert "check the password and AP security mode" in auth.stderr
    assert_no_credentials(auth)

    for code, hint in [
        ("wifi_ap_not_found", "check the exact SSID"),
        ("wifi_authentication_timeout", "did not answer authentication"),
        ("wifi_dhcp_timeout", "did not obtain an IP address"),
        ("wifi_security_incompatible", "check WPA2/WPA3 and PMF settings"),
    ]:
        diagnostic, _ = run_case(
            {"state": "failed", "terminal_fields": {"code": code}}
        )
        assert diagnostic.returncode == 1, diagnostic.stderr
        assert code in diagnostic.stderr
        assert hint in diagnostic.stderr
        assert_no_credentials(diagnostic)

    malicious, _ = run_case(
        {
            "state": "failed",
            "terminal_fields": {
                "code": "private-password",
                "message": "PRIVATE-NET private-password",
            },
        }
    )
    assert malicious.returncode == 1, malicious.stderr
    assert "unrecognized device failure" in malicious.stderr
    assert_no_credentials(malicious)

    rejected, _ = run_case(
        {"state": "unused", "request_code": "invalid_credentials"}
    )
    assert rejected.returncode == 1, rejected.stderr
    assert "invalid_credentials" in rejected.stderr
    assert_no_credentials(rejected)

    invalid_accepted, _ = run_case(
        {
            "state": "unused",
            "accepted_job_id": "PRIVATE-NET private-password",
            "no_terminal": True,
        }
    )
    assert invalid_accepted.returncode == 1, invalid_accepted.stderr
    assert "without a valid job ID" in invalid_accepted.stderr
    assert_no_credentials(invalid_accepted)

    malicious_rejected, _ = run_case(
        {
            "state": "unused",
            "request_code": "PRIVATE-NET private-password",
            "message": "PRIVATE-NET private-password",
        }
    )
    assert malicious_rejected.returncode == 1, malicious_rejected.stderr
    assert "device rejected the provisioning request" in malicious_rejected.stderr
    assert_no_credentials(malicious_rejected)

    noisy_success, _ = run_case(
        {
            "state": "succeeded",
            "malformed_state": True,
            "oversized_integer": True,
        }
    )
    assert noisy_success.returncode == 0, noisy_success.stderr
    assert "provisioning succeeded" in noisy_success.stdout
    assert_no_credentials(noisy_success)

    direct_terminal, _ = run_case(
        {
            "state": "succeeded",
            "skip_accepted": True,
            "fragment_terminal": True,
        }
    )
    assert direct_terminal.returncode == 0, direct_terminal.stderr
    assert "provisioning succeeded" in direct_terminal.stdout
    assert_no_credentials(direct_terminal)

    mismatched, _ = run_case(
        {
            "state": "succeeded",
            "mismatched_job": True,
        }
    )
    assert mismatched.returncode == 0, mismatched.stderr
    assert "provisioning succeeded" in mismatched.stdout
    assert "wifi_authentication_failed" not in mismatched.stderr
    assert_no_credentials(mismatched)

    malicious_pending, _ = run_case(
        {
            "state": "pending",
            "terminal_fields": {
                "code": "device_timeout",
                "job_id": "PRIVATE-NET private-password",
            },
            "timeout": 1,
        }
    )
    assert malicious_pending.returncode == 3, malicious_pending.stderr
    assert "device job 7 is still pending" in malicious_pending.stderr
    assert_no_credentials(malicious_pending)

    accepted_timeout, _ = run_case(
        {
            "state": "unused",
            "no_terminal": True,
            "timeout": 1,
        }
    )
    assert accepted_timeout.returncode == 3, accepted_timeout.stderr
    assert (
        "device job 7 is still pending and may still change Wi-Fi"
        in accepted_timeout.stderr
    )
    assert "device response timed out" not in accepted_timeout.stderr
    assert_no_credentials(accepted_timeout)
    print("provision-wifi pseudo-terminal test: PASS")


if __name__ == "__main__":
    main()
