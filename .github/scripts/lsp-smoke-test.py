#!/usr/bin/env python3
"""Minimal end-to-end smoke test for the ``lockpick-lsp`` binary.

Sends a standard LSP initialize -> initialized -> shutdown -> exit sequence
over stdio using the ``Content-Length`` framing the server implements, then
checks that the handshake response and a clean exit were produced.
"""

import json
import subprocess
import sys


def frame(message: dict) -> bytes:
    body = json.dumps(message, separators=(",", ":")).encode("utf-8")
    return b"Content-Length: %d\r\n\r\n" % len(body) + body


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: lsp-smoke-test.py <path-to-lockpick-lsp>", file=sys.stderr)
        return 2

    messages = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"capabilities": {}},
        },
        {"jsonrpc": "2.0", "method": "initialized", "params": {}},
        {"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": None},
        {"jsonrpc": "2.0", "method": "exit", "params": None},
    ]
    payload = b"".join(frame(message) for message in messages)

    try:
        proc = subprocess.run(
            [sys.argv[1]],
            input=payload,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=60,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        print(f"failed to run {sys.argv[1]}: {exc}", file=sys.stderr)
        return 1

    if proc.returncode != 0:
        print(
            f"lockpick-lsp exited with {proc.returncode}\n"
            f"stderr:\n{proc.stderr.decode('utf-8', 'replace')}",
            file=sys.stderr,
        )
        return 1

    output = proc.stdout.decode("utf-8", "replace")
    if '"capabilities"' not in output or '"serverInfo"' not in output:
        print(
            "lockpick-lsp did not return a valid initialize response\n"
            f"stdout:\n{output}\n"
            f"stderr:\n{proc.stderr.decode('utf-8', 'replace')}",
            file=sys.stderr,
        )
        return 1

    print("lockpick-lsp initialize/shutdown handshake OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
