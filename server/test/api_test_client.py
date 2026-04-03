#!/usr/bin/env python3
"""CI smoke client that exercises many backend API calls."""

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from typing import Any, Dict, Optional, Tuple


def call_api(
    base_url: str,
    path: str,
    method: str = "GET",
    payload: Optional[Dict[str, Any]] = None,
    timeout: float = 5.0,
) -> Tuple[int, str]:
    url = f"{base_url.rstrip('/')}{path}"
    data = None
    headers = {}
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"

    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = resp.read().decode("utf-8", errors="replace")
            return resp.status, body
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        return e.code, body


def parse_json(body: str) -> Any:
    try:
        return json.loads(body)
    except json.JSONDecodeError as exc:
        raise AssertionError(f"Expected JSON response, got: {body[:220]}") from exc


def assert_status(status: int, expected: int, context: str) -> None:
    if status != expected:
        raise AssertionError(f"{context}: expected status {expected}, got {status}")


def run_contract_tests(base_url: str, timeout: float) -> None:
    status, body = call_api(base_url, "/ping", timeout=timeout)
    assert_status(status, 200, "GET /ping")
    if body.strip() != "pong":
        raise AssertionError(f"GET /ping: expected 'pong', got {body!r}")

    status, body = call_api(base_url, "/version", timeout=timeout)
    assert_status(status, 200, "GET /version")
    version = parse_json(body)
    if not isinstance(version, dict):
        raise AssertionError("GET /version: expected object response")

    status, body = call_api(
        base_url,
        "/version/check",
        method="POST",
        payload={
            "plugin_version": "0.0.0-ci",
            "plugin_release_tag": "v0.0.0-ci",
            "plugin_build": 0,
        },
        timeout=timeout,
    )
    assert_status(status, 200, "POST /version/check")
    if not isinstance(parse_json(body), dict):
        raise AssertionError("POST /version/check: expected object response")

    status, body = call_api(base_url, "/db/stats", timeout=timeout)
    assert_status(status, 200, "GET /db/stats")
    stats = parse_json(body)
    if not isinstance(stats, dict):
        raise AssertionError("GET /db/stats: expected object response")

    status, body = call_api(base_url, "/get/ids", timeout=timeout)
    assert_status(status, 200, "GET /get/ids")
    ids = parse_json(body)
    if not isinstance(ids, list):
        raise AssertionError("GET /get/ids: expected list response")

    status, body = call_api(
        base_url,
        "/index/check-unprocessed",
        method="POST",
        payload={"photo_ids": []},
        timeout=timeout,
    )
    assert_status(status, 200, "POST /index/check-unprocessed")
    unprocessed = parse_json(body)
    if not isinstance(unprocessed, dict) or "photo_ids" not in unprocessed:
        raise AssertionError("POST /index/check-unprocessed: malformed response")

    status, _ = call_api(
        base_url,
        "/sync/cleanup",
        method="POST",
        payload={"photo_ids": []},
        timeout=timeout,
    )
    assert_status(status, 400, "POST /sync/cleanup invalid payload")

    status, _ = call_api(
        base_url,
        "/sync/claim",
        method="POST",
        payload={"catalog_id": "ci-catalog"},
        timeout=timeout,
    )
    assert_status(status, 400, "POST /sync/claim invalid payload")


def run_load_probe(base_url: str, timeout: float, iterations: int) -> None:
    for i in range(iterations):
        status, body = call_api(base_url, "/ping", timeout=timeout)
        assert_status(status, 200, f"GET /ping load iteration {i + 1}")
        if body.strip() != "pong":
            raise AssertionError(f"GET /ping load iteration {i + 1}: unexpected body")

    for i in range(max(1, iterations // 2)):
        status, body = call_api(base_url, "/version", timeout=timeout)
        assert_status(status, 200, f"GET /version load iteration {i + 1}")
        if not isinstance(parse_json(body), dict):
            raise AssertionError(f"GET /version load iteration {i + 1}: malformed JSON")

        status, body = call_api(base_url, "/db/stats", timeout=timeout)
        assert_status(status, 200, f"GET /db/stats load iteration {i + 1}")
        if not isinstance(parse_json(body), dict):
            raise AssertionError(f"GET /db/stats load iteration {i + 1}: malformed JSON")


def main() -> int:
    parser = argparse.ArgumentParser(description="Run backend API smoke/load checks.")
    parser.add_argument("--base-url", default="http://127.0.0.1:19819", help="Backend base URL")
    parser.add_argument("--timeout", type=float, default=5.0, help="HTTP timeout in seconds")
    parser.add_argument("--iterations", type=int, default=120, help="Number of repeated load requests")
    args = parser.parse_args()

    start = time.time()
    run_contract_tests(args.base_url, args.timeout)
    run_load_probe(args.base_url, args.timeout, max(1, args.iterations))
    elapsed = time.time() - start

    total_requests = 8 + args.iterations + max(1, args.iterations // 2) * 2
    print(
        f"API smoke/load checks passed: {total_requests} requests in {elapsed:.2f}s "
        f"(base={args.base_url}, iterations={args.iterations})"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as exc:
        print(f"API test client failed: {exc}", file=sys.stderr)
        raise SystemExit(1)
