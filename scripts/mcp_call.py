#!/usr/bin/env python3
"""Call one tool on LrGeniusAI's MCP endpoint from a terminal.

For testing and scripting without an MCP client (see
.claude/skills/lrgenius-mcp-full-test/SKILL.md). Standard library only.

    LRGENIUS_MCP_TOKEN=<agent token> python3 scripts/mcp_call.py get_status
    python3 scripts/mcp_call.py search_photos '{"query": "dog on a beach"}'
    python3 scripts/mcp_call.py view_photo '{"photo_id": "..."}' --image-out /tmp/p.jpg
    python3 scripts/mcp_call.py --list

The token is the agent token from Plug-in Manager > LrGeniusAI > AI Agents
(Copy token). LRG_MCP_URL points elsewhere than http://127.0.0.1:19819/mcp.

Prints the tool's text answer on stdout; progress notifications go to stderr.
Exit code 0 for success, 1 when the tool answered with an error, 2 when the
call itself failed (backend not running, wrong token, protocol error).
"""

import argparse
import base64
import hashlib
import json
import os
import sys
import tempfile
import urllib.error
import urllib.request

URL = os.environ.get("LRG_MCP_URL", "http://127.0.0.1:19819/mcp")
PROTOCOL_VERSION = "2025-06-18"
# One cached session per endpoint, outside the repository.
SESSION_FILE = os.path.join(
    tempfile.gettempdir(),
    "lrgenius-mcp-session-" + hashlib.sha256(URL.encode()).hexdigest()[:12],
)


def fail(message, code=2):
    print(message, file=sys.stderr)
    sys.exit(code)


def token():
    value = os.environ.get("LRGENIUS_MCP_TOKEN", "").strip()
    if not value:
        fail(
            "Set LRGENIUS_MCP_TOKEN to the agent token "
            "(Plug-in Manager > LrGeniusAI > AI Agents > Copy token)."
        )
    return value


def post(message, session=None):
    """POST one JSON-RPC message; returns (status, messages, session id, raw body)."""
    request = urllib.request.Request(URL, data=json.dumps(message).encode(), method="POST")
    request.add_header("content-type", "application/json")
    request.add_header("accept", "application/json, text/event-stream")
    request.add_header("authorization", "Bearer " + token())
    request.add_header("mcp-protocol-version", PROTOCOL_VERSION)
    if session:
        request.add_header("mcp-session-id", session)
    try:
        with urllib.request.urlopen(request, timeout=4 * 3600) as response:
            body = response.read().decode()
            session_id = response.headers.get("mcp-session-id")
    except urllib.error.HTTPError as e:
        return e.code, [], None, e.read().decode()
    except urllib.error.URLError as e:
        fail(f"Cannot reach {URL} ({e.reason}). Is the LrGeniusAI backend running?")
    messages = []
    if body.lstrip().startswith("{"):
        messages.append(json.loads(body))
    else:
        data = []
        for line in body.splitlines() + [""]:
            if not line:
                if data:
                    messages.append(json.loads("\n".join(data)))
                    data = []
            elif line.startswith("data:"):
                chunk = line[5:].lstrip(" ")
                if chunk:
                    data.append(chunk)
    return 200, messages, session_id, body


def explain(status, body):
    try:
        return json.loads(body)["error"]["message"]
    except (ValueError, KeyError, TypeError):
        return f"HTTP {status}: {body[:300]}"


def session(fresh=False):
    if not fresh and os.path.exists(SESSION_FILE):
        with open(SESSION_FILE) as f:
            return f.read().strip()
    status, _, session_id, body = post({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "lrgenius-mcp-call", "version": "1"},
        },
    })
    if status != 200 or not session_id:
        fail(explain(status, body))
    post({"jsonrpc": "2.0", "method": "notifications/initialized"}, session_id)
    with open(SESSION_FILE, "w") as f:
        f.write(session_id)
    return session_id


def request(method, params, retried=False):
    """One request with a fresh id (ids must be unique per session)."""
    sid = session(fresh=retried)
    request_id = int.from_bytes(os.urandom(4), "big")
    status, messages, _, body = post(
        {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}, sid
    )
    if status == 404 and not retried:  # session gone: the backend restarted
        return request(method, params, retried=True)
    if status != 200:
        fail(explain(status, body))
    result = None
    for message in messages:
        if message.get("method") == "notifications/progress":
            p = message.get("params", {})
            print(
                f"[progress {p.get('progress')}/{p.get('total')}] {p.get('message', '')}",
                file=sys.stderr,
            )
        elif message.get("id") == request_id:
            if "error" in message:
                fail(f"Protocol error: {json.dumps(message['error'])}")
            result = message.get("result")
    if result is None:
        fail("The backend closed the stream without an answer.")
    return result


def main():
    parser = argparse.ArgumentParser(description="Call a tool on LrGeniusAI's MCP endpoint.")
    parser.add_argument("tool", nargs="?", help="tool name, e.g. get_status")
    parser.add_argument("arguments", nargs="?", default="{}", help="tool arguments as JSON")
    parser.add_argument("--list", action="store_true", help="list the tools and exit")
    parser.add_argument("--image-out", help="where to save an image the tool returns")
    args = parser.parse_args()

    if args.list:
        for tool in request("tools/list", {}).get("tools", []):
            description = (tool.get("description") or "").strip().splitlines()[0:1]
            print(f"{tool['name']:28} {description[0] if description else ''}")
        return
    if not args.tool:
        parser.error("name a tool, or use --list")
    try:
        arguments = json.loads(args.arguments)
    except json.JSONDecodeError as e:
        fail(f"The arguments are not JSON: {e}")

    result = request(
        "tools/call",
        {"name": args.tool, "arguments": arguments, "_meta": {"progressToken": "cli"}},
    )
    is_error = bool(result.get("isError"))
    for block in result.get("content", []):
        if block.get("type") == "text":
            print(block["text"])
        elif block.get("type") == "image":
            suffix = ".jpg" if "jpeg" in block.get("mimeType", "") else ".bin"
            path = args.image_out or os.path.join(tempfile.gettempdir(), "lrgenius-view" + suffix)
            with open(path, "wb") as f:
                f.write(base64.b64decode(block["data"]))
            print(f"[image {block.get('mimeType')} saved to {path}]")
    sys.exit(1 if is_error else 0)


if __name__ == "__main__":
    main()
