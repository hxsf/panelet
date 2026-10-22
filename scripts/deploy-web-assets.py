#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Install a versioned .pna resource bundle without formatting or flashing apps.

Use the device's management page for normal installation. This CLI is useful for
development/automation and requires an explicit origin and a private token file.
Legacy .n4a files remain accepted; validation uses the unchanged N4A1 contents.
"""
import argparse
import hashlib
import http.client
import json
from pathlib import Path
import struct
import time
import urllib.error
import urllib.parse
import urllib.request

CHUNK = 32 * 1024
MANIFEST_MAX = 16 * 1024
FILE_MAX = 1536 * 1024
BUNDLE_MAX = 2 * 1024 * 1024


def parse_bundle(path):
    """Validate all stored bytes before making any network request."""
    with Path(path).open("rb") as stream:
        header = stream.read(8)
        if len(header) != 8 or header[:4] != b"N4A1":
            raise ValueError("Not an N4A1 resource bundle")
        length = struct.unpack("<I", header[4:])[0]
        if not 0 < length <= MANIFEST_MAX:
            raise ValueError("Invalid manifest length")
        manifest = json.loads(stream.read(length))
        if not isinstance(manifest, dict) or set(manifest) != {"schema_version", "api_version", "files"}:
            raise ValueError("Invalid manifest fields")
        api = manifest["api_version"]
        if type(manifest["schema_version"]) is not int or manifest["schema_version"] != 1 or type(api) is not int or not 0 < api <= 0xFFFFFFFF:
            raise ValueError("Invalid resource compatibility version")
        files = manifest["files"]
        if not isinstance(files, list) or not 0 < len(files) <= 24:
            raise ValueError("Invalid resource file count")
        total, previous, offsets = 0, "", []
        for item in files:
            if not isinstance(item, dict) or set(item) != {"path", "content_type", "encoding", "size", "sha256"}:
                raise ValueError("Invalid resource metadata")
            name = item["path"]
            if (not isinstance(name, str) or not name.startswith("/") or len(name) > 128
                    or name <= previous or any(part in ("", ".", "..") for part in name[1:].split("/"))
                    or any(not (c.isascii() and (c.isalnum() or c in "/._-")) for c in name)):
                raise ValueError("Resource paths must be safe, unique and sorted")
            previous = name
            mime, encoding, size, digest = (item[k] for k in ("content_type", "encoding", "size", "sha256"))
            if (not isinstance(mime, str) or not 0 < len(mime) <= 96 or any(not 32 <= ord(c) <= 126 for c in mime)
                    or encoding not in ("gzip", "identity") or type(size) is not int or not 0 <= size <= FILE_MAX
                    or not isinstance(digest, str) or len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest)):
                raise ValueError("Invalid resource type, size or hash")
            total += size
            if total > BUNDLE_MAX:
                raise ValueError("Resource bundle exceeds 2 MiB")
            offsets.append(stream.tell())
            remaining, checksum = size, hashlib.sha256()
            while remaining:
                data = stream.read(min(CHUNK, remaining))
                if not data:
                    raise ValueError("Truncated resource bundle")
                checksum.update(data)
                remaining -= len(data)
            if checksum.hexdigest() != digest:
                raise ValueError("Resource SHA-256 mismatch")
        if stream.read(1):
            raise ValueError("Unexpected trailing resource data")
    # Match Rust's declared struct field order regardless of input JSON ordering.
    manifest = {"schema_version": 1, "api_version": api, "files": [
        {key: item[key] for key in ("path", "content_type", "encoding", "size", "sha256")} for item in files]}
    canonical = json.dumps(manifest, separators=(",", ":"), ensure_ascii=False).encode()
    return manifest, offsets, hashlib.sha256(canonical).hexdigest()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        return None


class Client:
    def __init__(self, origin, token, timeout=30):
        parsed = urllib.parse.urlsplit(origin)
        if (parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username or parsed.password
                or parsed.path not in ("", "/") or parsed.query or parsed.fragment):
            raise ValueError("Use the device's http(s) origin without credentials or a path")
        self.origin = urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, "", "", ""))
        if not token or "\r" in token or "\n" in token:
            raise ValueError("Invalid Admin token file")
        self.token, self.timeout = token, timeout
        self.opener = urllib.request.build_opener(NoRedirect())

    def request(self, route, data=None, binary=False):
        body = data if binary else None if data is None else json.dumps(data, separators=(",", ":")).encode()
        headers = {"X-Note4c-Token": self.token, "Origin": self.origin}
        if body is not None:
            headers["Content-Type"] = "application/octet-stream" if binary else "application/json"
        request = urllib.request.Request(self.origin + route, data=body, headers=headers)
        for attempt in range(3):
            try:
                with self.opener.open(request, timeout=self.timeout) as response:
                    result = json.loads(response.read(256 * 1024 + 1))
                if isinstance(result, dict) and result.get("error"):
                    raise ValueError("Resource API returned an error")
                return result
            except urllib.error.HTTPError as error:
                # Never follow a redirect carrying the saved Admin credential.
                code = "request_rejected"
                try:
                    detail = json.loads(error.read(16 * 1024)).get("error", {})
                    candidate = detail.get("code", code)
                    if isinstance(candidate, str) and candidate.replace("_", "").isalnum():
                        code = candidate[:64]
                except (ValueError, AttributeError, TypeError, OSError, http.client.HTTPException):
                    pass
                finally:
                    error.close()
                raise ValueError(f"Resource request rejected: HTTP {error.code} ({code})") from None
            except (urllib.error.URLError, TimeoutError, ConnectionError, http.client.IncompleteRead):
                if attempt == 2:
                    raise ValueError("Device connection unavailable; pending upload can be resumed") from None
                # begin/chunk/commit are idempotent; retry the exact request.
                time.sleep(.2 * (attempt + 1))


def install(client, bundle, progress=print):
    manifest, offsets, bundle_id = parse_bundle(bundle)
    status = client.request("/web/v1/status")
    if not status.get("storage", {}).get("ready"):
        raise ValueError("Assets storage is not ready; this installer never initializes or formats it")
    for installed in status.get("web", {}).get("installed", []):
        active = installed.get("active") or {}
        if active.get("bundle_id") == bundle_id:
            progress("This compatible resource bundle is already installed; no upload needed")
            return active
    state = client.request("/web/v1/begin", manifest)
    if state.get("bundle_id") != bundle_id:
        raise ValueError("Device returned a different bundle ID")
    with Path(bundle).open("rb") as stream:
        for index, item in enumerate(manifest["files"]):
            current = state["files"][index]
            if current.get("path") != item["path"] or current.get("size") != item["size"]:
                raise ValueError("Device returned inconsistent upload metadata")
            offset = current["received"]
            if type(offset) is not int or not 0 <= offset <= item["size"]:
                raise ValueError("Device returned an invalid upload offset")
            while offset < item["size"]:
                stream.seek(offsets[index] + offset)
                block = stream.read(min(CHUNK, item["size"] - offset))
                if not block:
                    raise ValueError("Bundle changed or was truncated during upload")
                route = f"/web/v1/chunk?bundle={bundle_id}&file={index}&offset={offset}"
                state = client.request(route, block, binary=True)
                if state.get("bundle_id") != bundle_id:
                    raise ValueError("Upload session changed")
                received = state["files"][index]["received"]
                if type(received) is not int or received < offset + len(block) or received > item["size"]:
                    raise ValueError("Device did not acknowledge the uploaded chunk")
                offset = received
            if not state["files"][index].get("complete"):
                raise ValueError("Device has not verified the resource hash")
            progress(f"Verified {item['path']} ({item['size']:,} stored bytes)")
    result = client.request("/web/v1/commit", {"bundle_id": bundle_id})
    if result.get("bundle_id") != bundle_id:
        raise ValueError("Resource publication was not confirmed")
    progress(f"Resource API v{manifest['api_version']} installed; other compatibility versions retained")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True, help="Explicit device origin, e.g. http://device-address")
    parser.add_argument("--token-file", required=True, type=Path, help="Private UTF-8 file containing the existing Admin token")
    parser.add_argument("--bundle", required=True, type=Path,
                        help="Panelet .pna resource bundle; legacy .n4a files are also accepted")
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    try:
        if not 0 < args.timeout <= 120:
            raise ValueError("Timeout must be between 0 and 120 seconds")
        client = Client(args.url, args.token_file.read_text().strip(), args.timeout)
        install(client, args.bundle)
    except (ValueError, OSError, KeyError, IndexError, TypeError) as error:
        parser.exit(1, f"Resource installation failed: {error}\n")


if __name__ == "__main__":
    main()
