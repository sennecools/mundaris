#!/usr/bin/env python3
"""Exercise the MCP stdio bridge against an already-running native session.

The session itself is launched by the caller. This script never starts or stops
the engine; it only starts mundaris_dev in MCP mode and connects through the
session descriptor's registry.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time
from typing import Any


MODERN_PROTOCOL_VERSION = "2026-06-18"
NEGOTIATED_PROTOCOL_VERSION = "2025-11-25"
REQUIRED_TOOLS = {
    "mundaris_sessions",
    "mundaris_capabilities",
    "mundaris_inspect",
    "mundaris_control",
    "mundaris_action",
    "mundaris_receipt",
    "mundaris_capture",
    "mundaris_wait",
}


class CheckFailure(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise CheckFailure(message)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def jsonable(value: Any) -> Any:
    """Drop image payloads from saved RPC logs, retaining reproducible hashes."""
    if isinstance(value, dict):
        result = {}
        for key, child in value.items():
            if key == "data" and value.get("type") == "image" and isinstance(child, str):
                try:
                    raw = base64.b64decode(child, validate=True)
                    result["data_sha256"] = sha256(raw)
                    result["data_bytes"] = len(raw)
                except (ValueError, binascii.Error):
                    result["data_sha256"] = None
                    result["data_bytes"] = None
            else:
                result[key] = jsonable(child)
        return result
    if isinstance(value, list):
        return [jsonable(child) for child in value]
    return value


class McpProcess:
    def __init__(self, binary: Path, output: Path, registry: Path):
        self.process = subprocess.Popen(
            [str(binary), "mcp"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
            env={**os.environ, "MUNDARIS_DEV_REGISTRY": str(registry)},
        )
        self.messages: queue.Queue[dict[str, Any] | None] = queue.Queue()
        self.stderr_lines: list[str] = []
        self.responses: dict[str, dict[str, Any]] = {}
        self.calls: list[dict[str, Any]] = []
        self.next_id = 1
        self.output = output
        threading.Thread(target=self._read_stdout, daemon=True).start()
        threading.Thread(target=self._read_stderr, daemon=True).start()

    def _read_stdout(self) -> None:
        assert self.process.stdout is not None
        try:
            for line in self.process.stdout:
                try:
                    self.messages.put(json.loads(line))
                except (UnicodeDecodeError, json.JSONDecodeError):
                    self.messages.put({"_invalid_stdout": line[:512].decode("utf-8", "replace")})
        finally:
            self.messages.put(None)

    def _read_stderr(self) -> None:
        assert self.process.stderr is not None
        for line in self.process.stderr:
            self.stderr_lines.append(line.decode("utf-8", "replace").rstrip())
            del self.stderr_lines[:-100]

    def send(self, message: dict[str, Any]) -> None:
        assert self.process.stdin is not None
        require(self.process.poll() is None, "MCP child exited before request")
        data = (json.dumps(message, separators=(",", ":")) + "\n").encode("utf-8")
        self.process.stdin.write(data)
        self.process.stdin.flush()

    def receive(self, request_id: int | str, timeout: float = 10.0) -> dict[str, Any]:
        key = str(request_id)
        if key in self.responses:
            return self.responses.pop(key)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                message = self.messages.get(timeout=max(0.01, deadline - time.monotonic()))
            except queue.Empty:
                break
            require(message is not None, f"MCP child closed stdout awaiting response {key}")
            require("_invalid_stdout" not in message, f"invalid MCP stdout: {message}")
            mid = message.get("id")
            if mid is not None and str(mid) == key:
                return message
            if mid is not None:
                self.responses[str(mid)] = message
        raise CheckFailure(f"timed out waiting for MCP response {key}")

    def request(self, method: str, params: dict[str, Any] | None = None, timeout: float = 10.0) -> dict[str, Any]:
        request_id = self.next_id
        self.next_id += 1
        message = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            message["params"] = params
        self.send(message)
        response = self.receive(request_id, timeout)
        self.calls.append({"request": message, "response": jsonable(response)})
        return response

    def notify(self, method: str, params: dict[str, Any] | None = None) -> None:
        message = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            message["params"] = params
        self.send(message)
        self.calls.append({"request": message})

    def tool(self, name: str, arguments: dict[str, Any], timeout: float = 15.0) -> dict[str, Any]:
        response = self.request("tools/call", {"name": name, "arguments": arguments}, timeout)
        result = response.get("result")
        require(isinstance(result, dict), f"{name} returned no MCP result: {response}")
        return result

    def close(self) -> None:
        if self.process.stdin is not None:
            self.process.stdin.close()
        try:
            code = self.process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            self.process.kill()
            code = self.process.wait(timeout=3)
            raise CheckFailure("MCP child did not exit after orderly stdin EOF")
        require(code == 0, f"MCP child exited {code}; stderr tail: {self.stderr_lines[-15:]}")


def extract_structured(result: dict[str, Any], label: str) -> dict[str, Any]:
    require(result.get("isError") is not True, f"{label} MCP tool error: {result.get('content')}")
    structured = result.get("structuredContent")
    require(isinstance(structured, dict), f"{label} omitted structuredContent")
    return structured


def tool_structured(client: McpProcess, name: str, args: dict[str, Any], label: str | None = None) -> dict[str, Any]:
    return extract_structured(client.tool(name, args), label or name)


def response_data(value: dict[str, Any], expected: set[str], label: str) -> dict[str, Any]:
    require(value.get("status") in expected, f"{label} status {value.get('status')!r}; response={value}")
    data = value.get("data")
    require(isinstance(data, dict), f"{label} omitted data")
    return data


def run(args: argparse.Namespace) -> dict[str, Any]:
    output = args.output.expanduser().resolve()
    require(not output.exists(), f"output directory must be fresh: {output}")
    output.mkdir(parents=True)
    descriptor_path = args.session.expanduser().resolve()
    binary = args.binary.expanduser().resolve()
    registry = args.registry.expanduser().resolve() if args.registry else descriptor_path.parent
    session_id: str | None = None
    client: McpProcess | None = None
    lease: str | None = None
    failures: list[str] = []
    result: dict[str, Any] = {
        "status": "running",
        "session_id": None,
        "descriptor": str(descriptor_path),
        "registry": str(registry),
        "binary": str(binary),
        "output_directory": str(output),
        "checks": [],
    }
    try:
        descriptor = json.loads(descriptor_path.read_text(encoding="utf-8"))
        session_id = descriptor.get("session_id")
        require(isinstance(session_id, str) and session_id, "descriptor has no session_id")
        result["session_id"] = session_id
        require(binary.is_file(), f"MCP executable not found: {binary}")
        client = McpProcess(binary, output, registry)
        initialized = client.request(
            "initialize",
            {"protocolVersion": MODERN_PROTOCOL_VERSION, "capabilities": {}, "clientInfo": {"name": "mundaris-mcp-check", "version": "1"}},
        )
        negotiated = initialized.get("result", {}).get("protocolVersion")
        require(negotiated == NEGOTIATED_PROTOCOL_VERSION, f"unexpected negotiated protocol: {negotiated!r}")
        client.notify("notifications/initialized")
        require("result" in client.request("ping"), "MCP ping failed")
        listed = client.request("tools/list").get("result", {}).get("tools", [])
        names = {tool.get("name") for tool in listed if isinstance(tool, dict)}
        require(REQUIRED_TOOLS <= names, f"MCP tool list missing {sorted(REQUIRED_TOOLS - names)}")
        for tool in listed:
            require(isinstance(tool.get("inputSchema"), dict), f"tool lacks inputSchema: {tool.get('name')}")
        result["checks"].append("initialize-2026-to-2025-negotiation-ping-tool-schemas")

        sessions_data = tool_structured(client, "mundaris_sessions", {"registry": str(registry)})
        sessions = sessions_data.get("sessions", [])
        live = next((s for s in sessions if isinstance(s, dict) and s.get("session_id") == session_id), None)
        require(live is not None, f"session {session_id!r} is not confirmed live by registry discovery")
        require(live.get("pid") == descriptor.get("pid") and live.get("endpoint") == descriptor.get("endpoint"), "live descriptor identity differs from selected descriptor")
        result["checks"].append("registry-session-is-live-and-matches-descriptor")

        inspect = response_data(tool_structured(client, "mundaris_inspect", {"session": session_id}), {"ok"}, "inspect")
        require(inspect.get("session", {}).get("session_id") == session_id, "inspect session identity mismatch")
        bodies = inspect.get("bodies")
        require(isinstance(bodies, list) and bodies, "inspect returned no body inventory")
        earth = next((b for b in bodies if isinstance(b, dict) and (str(b.get("semantic_identity", "")).lower() == "earth" or str(b.get("name", "")).lower() == "earth")), None)
        require(earth is not None and isinstance(earth.get("handle"), str), "inspect did not expose an Earth handle")
        result["initial_snapshot"] = jsonable(inspect.get("snapshot"))
        result["checks"].append("inspect-session-snapshot-and-earth-handle")

        control = tool_structured(client, "mundaris_control", {"session": session_id, "operation": "acquire", "owner": "mcp-integration-check"})
        control_data = response_data(control, {"ok"}, "acquire control")
        lease = control_data.get("lease")
        require(isinstance(lease, str) and lease, "acquire did not return lease")
        result["checks"].append("acquire-control")

        for label, command in [
            ("focus-earth", {"action": "focus", "body": earth["handle"]}),
            ("disable-clouds", {"action": "layer", "layer": "clouds", "enabled": False}),
            ("set-sky-intensity", {"action": "sky_setting", "setting": "intensity", "value": 0.8}),
        ]:
            accepted = extract_structured(client.tool("mundaris_action", {"session": session_id, "lease": lease, "command": command}), label)
            data = response_data(accepted, {"accepted"}, label)
            command_id = data.get("command_id")
            require(isinstance(command_id, str), f"{label} returned no command_id")
            receipt = tool_structured(client, "mundaris_receipt", {"session": session_id, "command_id": command_id})
            receipt_data = response_data(receipt, {"ok"}, f"{label} receipt")
            require(receipt_data.get("status") in {"applied", "completed"}, f"{label} did not apply: {receipt_data}")
            result.setdefault("actions", []).append({"name": label, "command_id": command_id, "receipt": jsonable(receipt_data)})
            result["checks"].append(label)

        focused = tool_structured(client, "mundaris_wait", {"session": session_id, "timeout_s": 30, "predicate": {"path": "general.focused_body.name", "equals": "Earth"}})
        require(focused.get("snapshot", {}).get("general", {}).get("focused_body", {}).get("name") == "Earth", "focus did not reach Earth before capture")
        tool_structured(client, "mundaris_wait", {"session": session_id, "timeout_s": 30, "predicate": {"path": "camera.navigation.transitioning", "equals": False}})
        result["checks"].append("focus-earth-checkpoint-before-capture")

        captured = client.tool("mundaris_capture", {"session": session_id, "lease": lease, "name": "mcp-smoke"}, timeout=35.0)
        capture = extract_structured(captured, "capture")
        receipt_envelope = capture.get("result")
        require(isinstance(receipt_envelope, dict), "capture omitted terminal receipt")
        capture_data = response_data(receipt_envelope, {"ok"}, "capture receipt")
        require(capture_data.get("status") in {"applied", "completed"}, f"capture did not complete: {capture_data}")
        receipt_payload = capture_data.get("data")
        require(isinstance(receipt_payload, dict), "capture receipt omitted data")
        manifest_path = Path(receipt_payload.get("manifest", "")).resolve()
        full_path = Path(receipt_payload.get("full_image", "")).resolve()
        viewport_path = Path(receipt_payload.get("viewport_image", "")).resolve()
        snapshot_path = Path(receipt_payload.get("snapshot", "")).resolve()
        for path in [manifest_path, full_path, viewport_path, snapshot_path]:
            require(path.is_file(), f"capture evidence file missing: {path}")
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        require(manifest.get("full_image") == receipt_payload.get("full_image"), "manifest full-image path mismatch")
        require(manifest.get("viewport_image") == receipt_payload.get("viewport_image"), "manifest viewport path mismatch")
        require(manifest.get("snapshot") == receipt_payload.get("snapshot"), "manifest snapshot path mismatch")
        for field in ["capture_id", "source_frame", "prepared_frame", "world_revision", "width", "height"]:
            require(manifest.get(field) == receipt_payload.get(field), f"manifest {field} differs from capture receipt")
        image_blocks = [block for block in captured.get("content", []) if isinstance(block, dict) and block.get("type") == "image"]
        require(len(image_blocks) == 2, f"expected paired full/viewport PNG blocks, got {len(image_blocks)}")
        returned_images = []
        for block, path, name in zip(image_blocks, [full_path, viewport_path], ["full_image", "viewport_image"]):
            raw = base64.b64decode(block.get("data", ""), validate=True)
            saved = path.read_bytes()
            require(block.get("mimeType") == "image/png", f"{name} block is not image/png")
            require(raw == saved, f"{name} MCP image bytes differ from the manifest-listed PNG")
            require(raw.startswith(b"\x89PNG\r\n\x1a\n"), f"{name} is not a PNG")
            destination = output / f"{name}.png"
            destination.write_bytes(raw)
            returned_images.append({"name": name, "path": str(destination), "bytes": len(raw), "sha256": sha256(raw)})
        snap = json.loads(snapshot_path.read_text(encoding="utf-8"))
        require(snap.get("general", {}).get("frame_number") == receipt_payload.get("prepared_frame"), "capture snapshot frame differs from receipt")
        result["capture"] = {"manifest": str(manifest_path), "snapshot": str(snapshot_path), "images": returned_images, "frame": receipt_payload.get("prepared_frame"), "submission_frame": receipt_payload.get("source_frame")}
        result["checks"].append("paired-capture-images-match-manifest-and-snapshot")

        malformed = client.tool("mundaris_action", {"session": session_id, "lease": lease, "command": {"action": "definitely_not_an_action"}})
        require(malformed.get("isError") is True, f"unknown action was not an MCP tool error: {malformed}")
        result["checks"].append("unknown-action-is-error")

        for invalid in [
            {"timeout_ms": 30000, "predicate": {"field": "general.frame_number", "operator": "at_least", "value": 3}},
            {"predicate": {"path": "general.frame_number", "at_least": 10, "at_most": 1}},
            {"session": 123, "predicate": {"path": "general.paused", "equals": True}},
        ]:
            started = time.monotonic()
            rejected = client.tool("mundaris_wait", {"session": session_id, **invalid})
            require(rejected.get("isError") is True, f"malformed wait arguments were accepted: {rejected}")
            require(time.monotonic() - started < 2, "malformed wait arguments did not fail immediately")
        result["checks"].append("malformed-unknown-typed-and-inverted-wait-arguments-rejected-immediately")

        cancel_id = client.next_id
        client.next_id += 1
        wait_request = {"jsonrpc": "2.0", "id": cancel_id, "method": "tools/call", "params": {"name": "mundaris_wait", "arguments": {"session": session_id, "timeout_s": 30, "predicate": {"path": "general.frame_number", "at_least": 1e100}}}}
        client.send(wait_request)
        time.sleep(0.25)
        client.notify("notifications/cancelled", {"requestId": cancel_id, "reason": "integration check"})
        cancelled = client.receive(cancel_id, timeout=4.0)
        cancelled_result = cancelled.get("result", {})
        require(cancelled_result.get("isError") is True, f"predicate wait cancellation did not fail boundedly: {cancelled}")
        texts = [c.get("text", "") for c in cancelled_result.get("content", []) if isinstance(c, dict)]
        require(any("cancel" in text.lower() for text in texts), f"wait cancellation response did not identify cancellation: {texts}")
        client.calls.append({"request": wait_request, "response": jsonable(cancelled), "cancellation": True})
        result["checks"].append("predicate-wait-cancelled-within-4-seconds")

        released = tool_structured(client, "mundaris_control", {"session": session_id, "operation": "release", "lease": lease})
        response_data(released, {"ok"}, "release control")
        lease = None
        result["checks"].append("release-control")
        scenario_id = client.next_id
        client.next_id += 1
        scenario_request = {"jsonrpc":"2.0", "id":scenario_id, "method":"tools/call", "params":{
            "name":"mundaris_scenarios", "arguments":{"session":session_id,
                "output_directory":str(output / "cancelled-scenario"),
                "scenario":{"schema":1,"preset":descriptor["preset"],"initial_settings":[{"action":"pause","paused":True}],
                    "steps":[{"action":{"action":"overview"},"duration_s":10}, {"action":{"action":"seek","seconds":42}}]}}}}
        client.send(scenario_request)
        time.sleep(.4)
        client.notify("notifications/cancelled", {"requestId":scenario_id, "reason":"native scenario cancellation check"})
        scenario_cancelled = client.receive(scenario_id, timeout=4)
        require(scenario_cancelled.get("result", {}).get("isError") is True, "cancelled scenario must report an error")
        client.calls.append({"request":scenario_request,"response":jsonable(scenario_cancelled),"cancellation":True})
        after = response_data(tool_structured(client,"mundaris_inspect",{"session":session_id}), {"ok"}, "inspect after scenario cancellation")
        require(after.get("control_owner") is None, "scenario cancellation did not release control")
        require(after.get("snapshot",{}).get("motion",{}).get("published_time_s") != 42, "scenario issued an action after cancellation")
        result["checks"].append("native-scenario-cancelled-within-4-seconds-releases-control-and-skips-later-actions")
        result["status"] = "passed"
    except Exception as exc:
        result["status"] = "failed"
        result["error"] = str(exc)
        raise
    finally:
        if client is not None and lease is not None and client.process.poll() is None:
            try:
                tool_structured(client, "mundaris_control", {"session": session_id, "operation": "release", "lease": lease}, "cleanup release")
            except Exception as exc:  # preserve primary failure; record cleanup evidence
                failures.append(f"lease cleanup failed: {exc}")
        if client is not None:
            try:
                client.close()
            except Exception as exc:
                failures.append(str(exc))
        result["rpc_log"] = str(output / "mcp-rpc.json")
        result["stderr_tail"] = client.stderr_lines[-20:] if client is not None else []
        result["cleanup_failures"] = failures
        if failures and result["status"] == "passed":
            result["status"] = "failed"
            result["error"] = "; ".join(failures)
        (output / "mcp-rpc.json").write_text(json.dumps(client.calls if client is not None else [], indent=2) + "\n", encoding="utf-8")
        (output / "result.json").write_text(json.dumps(jsonable(result), indent=2) + "\n", encoding="utf-8")
    require(not failures, "; ".join(failures))
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", type=Path, required=True, help="path to the existing native session descriptor JSON")
    parser.add_argument("--output", type=Path, required=True, help="fresh directory for result JSON, RPC metadata, and returned PNGs")
    parser.add_argument("--binary", type=Path, default=Path("../target/release/mundaris_dev.exe"), help="mundaris_dev executable (default: ../target/release/mundaris_dev.exe)")
    parser.add_argument("--registry", type=Path, help="session registry directory (default: descriptor's parent directory)")
    args = parser.parse_args()
    try:
        result = run(args)
        print(json.dumps(jsonable(result), indent=2))
        return 0
    except Exception as exc:
        print(json.dumps({"status": "failed", "error": str(exc)}), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
