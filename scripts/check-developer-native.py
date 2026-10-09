#!/usr/bin/env python3
"""Explicit Windows native integration checks against a selected development session.

The caller owns lifecycle. This script acquires control, sends logical operations
and native window messages, then releases control. Pillow verifies paired pixels.
"""
import argparse
import concurrent.futures
import ctypes
from ctypes import wintypes
import json
from pathlib import Path
import socket
import subprocess
import time
import uuid
from PIL import Image


def run(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    descriptor = json.loads(args.session.read_text(encoding="utf-8-sig"))
    descriptor = descriptor.get("session", descriptor)
    host, port = descriptor["endpoint"].rsplit(":", 1)
    records, checks, lease = [], [], None

    def request(operation, raw=None):
        if raw is None:
            result = subprocess.run([str(args.binary.resolve()), "request", json.dumps(operation),
                                     "--session", descriptor["session_id"], "--registry", str(args.session.parent.resolve())],
                                    capture_output=True, text=True, timeout=40)
            if not result.stdout.strip():
                raise RuntimeError(result.stderr.strip())
            response = json.loads(result.stdout)
            records.append({"operation": operation, "response": response})
            return response
        body = {"protocol_version": 1, "session_id": descriptor["session_id"],
                "request_id": uuid.uuid4().hex, "operation": operation}
        with socket.create_connection((host, int(port)), timeout=35) as connection:
            connection.sendall(raw if raw is not None else json.dumps(body, separators=(",", ":")).encode() + b"\n")
            data = bytearray()
            while not data.endswith(b"\n"):
                chunk = connection.recv(65536)
                if not chunk:
                    raise RuntimeError("session closed before a complete response")
                data.extend(chunk)
                if len(data) > 4 * 1024 * 1024:
                    raise RuntimeError("response exceeded 4 MiB")
            response = json.loads(data)
        records.append({"operation": operation, "response": response})
        return response

    def acquire():
        response = request({"op": "acquire_control", "owner": "native-integration-check"})
        assert response["status"] == "ok", response
        return response["data"]["lease"]

    def action(command):
        response = request({"op": "action", "lease": lease, "command": command})
        assert response["status"] == "accepted", response
        return response["data"]["command_id"]

    def fresh(predicate=lambda s: True):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            response = request({"op": "inspect"})
            snapshot = response["data"]["snapshot"]
            if snapshot and not snapshot["development"]["stale"] and predicate(snapshot):
                return snapshot
            time.sleep(.02)
        raise AssertionError("fresh observation predicate timed out")

    def terminal(command_id):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            response = request({"op": "receipt", "command_id": command_id})
            assert response["status"] == "ok", response
            if response["data"]["status"] != "accepted":
                return response["data"]
            time.sleep(.02)
        raise AssertionError("capture receipt timed out")

    def capture(name):
        response = request({"op": "capture", "lease": lease, "name": name})
        assert response["status"] == "accepted", response
        receipt = terminal(response["data"]["command_id"])
        assert receipt["status"] == "applied", receipt
        evidence = receipt["data"]
        manifest = json.loads(Path(evidence["manifest"]).read_text())
        snapshot = json.loads(Path(evidence["snapshot"]).read_text())
        assert manifest == evidence
        assert snapshot["development"]["capture_id"] == evidence["capture_id"]
        assert snapshot["development"]["submitted_frame"] == evidence["source_frame"]
        assert snapshot["general"]["world_revision"] == evidence["world_revision"]
        with Image.open(evidence["full_image"]) as full, Image.open(evidence["viewport_image"]) as crop:
            x, y = snapshot["camera"]["viewport_origin_pixels"]
            w, h = snapshot["camera"]["viewport_size_pixels"]
            assert full.size == (evidence["width"], evidence["height"])
            assert full.crop((x, y, x+w, y+h)).tobytes() == crop.tobytes()
            assert len(full.getcolors(full.width * full.height)) > 20
            assert len(full.crop((0, 0, min(x, 300), full.height)).getcolors(full.width * full.height)) > 20
        return evidence

    result = {"session_id": descriptor["session_id"], "binary_sha256": descriptor["binary_sha256"], "checks": checks}
    try:
        idle = fresh()
        assert idle["performance"]["gpu_source_frame"] is None, idle["performance"]
        checks.append("idle-session-has-no-timestamp-readback")
        assert request({"op": "action", "lease": "not-acquired", "command": {"action": "overview"}})["status"] == "failed"
        lease = acquire()
        assert request({"op": "acquire_control", "owner": "second-controller"})["status"] == "failed"
        checks.append("read-only-attachment-and-exclusive-lease")
        inventory = request({"op": "inspect"})["data"]["bodies"]
        earth = next(b for b in inventory if b["semantic_identity"] == "solar:earth")
        for command in [{"action": "pause", "paused": True}, {"action": "seek", "seconds": 0},
                        {"action": "focus", "body": earth["handle"]}, {"action": "render_mode", "mode": "natural"}]:
            action(command)
        fresh(lambda s: s["general"]["focused_body"]["name"] == "Earth" and not s["camera"]["navigation"]["transitioning"])
        failure = request({"op": "action", "lease": lease, "command": {"action": "select", "body": "stale-handle"}})
        assert failure["status"] == "failed"
        action({"action": "layer", "layer": "clouds", "enabled": False})
        action({"action": "sky_setting", "setting": "intensity", "value": .8})
        assert terminal(failure["data"]["command_id"])["status"] == "failed"
        checks.append("failure-retained-after-success-and-invalid-handle-rejected")
        if not args.skip_raw:
            assert request({}, b"\xff\n")["status"] == "failed"
            assert request({}, b"x" * 65537 + b"\n")["status"] == "failed"
            checks.append("malformed-and-oversized-input")
        result["capture"] = capture("native-check")
        checks.append("native-client-ui-and-exact-viewport-crop-associated")
        sequence = fresh()["development"]["command_sequence"]
        guarded = Path(descriptor["output_directory"]) / f"{descriptor['session_id']}-{sequence+1}-publication-refusal"
        guarded.mkdir()
        marker = guarded / "preserved.txt"
        marker.write_text("existing evidence must be preserved")
        refused = request({"op": "capture", "lease": lease, "name": "publication-refusal"})
        assert refused["status"] == "accepted", refused
        failed = terminal(refused["data"]["command_id"])
        assert failed["status"] == "failed", failed
        assert marker.read_text() == "existing evidence must be preserved" and not (guarded / "complete.json").exists()
        checks.append("publication-refusal-retained-without-overwriting-evidence")
        operations = [{"op": "capture", "lease": lease, "name": f"busy-{i}"} for i in range(4)]
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
            responses = list(executor.map(request, operations))
        assert any(r["status"] == "failed" and r["data"].get("error") == "busy" for r in responses), responses
        request({"op": "cancel", "lease": lease})
        lease = acquire()
        capture("after-cancel")
        checks.append("busy-slot-cancellation-and-retry")

        user = ctypes.WinDLL("user32", use_last_error=True)
        user.EnumWindows.argtypes = [ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM), wintypes.LPARAM]
        user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
        user.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
        user.SetWindowPos.argtypes = [wintypes.HWND,wintypes.HWND,ctypes.c_int,ctypes.c_int,ctypes.c_int,ctypes.c_int,ctypes.c_uint]
        user.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
        user.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
        windows = []
        callback_type = user.EnumWindows.argtypes[0]
        @callback_type
        def callback(window, _):
            process_id = wintypes.DWORD()
            user.GetWindowThreadProcessId(window, ctypes.byref(process_id))
            title=ctypes.create_unicode_buffer(256)
            user.GetWindowTextW(window,title,256)
            if process_id.value == descriptor["pid"] and title.value == "Astrum":
                windows.append(window)
            return True
        user.EnumWindows(callback, 0)
        assert windows, "owned native window unavailable"
        window = windows[0]
        assert user.SetWindowPos(window, None, 0, 0, 1100, 760, 0x16)
        time.sleep(.25)
        fresh()
        result["resized_capture"] = capture("after-resize")
        user.ShowWindow(window, 6)
        time.sleep(.25)
        stale = request({"op": "inspect"})["data"]["snapshot"]
        assert stale["development"]["stale"] and not stale["development"]["drawable"]
        diagnostic = request({"op": "diagnostics", "scope": "performance"})["data"]["freshness"]
        assert diagnostic["stale"] and not diagnostic["drawable"] and diagnostic["observation_age_ms"] > 0
        wait = subprocess.run([str(args.binary.resolve()), "wait", "--session", descriptor["session_id"],
                               "--registry", str(args.session.parent.resolve()), "--timeout", ".2", "--predicate",
                               json.dumps({"path":"general.paused", "equals":True})], capture_output=True,text=True,timeout=5)
        assert json.loads(wait.stdout)["status"] == "timeout", wait.stdout
        not_drawable = request({"op": "capture", "lease": lease, "name": "minimized"})
        assert not_drawable["status"] == "failed" and not_drawable["data"]["error"] == "not_drawable"
        user.ShowWindow(window, 9)
        time.sleep(.25)
        fresh()
        result["restored_capture"] = capture("after-restore")
        timed = fresh(lambda s: s["performance"]["gpu_source_frame"] is not None)
        assert timed["performance"]["gpu_source_frame"] <= timed["development"]["submitted_frame"]
        checks.append("requested-gpu-timing-retains-source-frame")
        checks.append("native-resize-minimize-stale-query-diagnostics-wait-expiry-not-drawable-restore")
        user.PostMessageW(window, 0x020A, 120 << 16, 0)
        time.sleep(.1)
        lost = request({"op": "action", "lease": lease, "command": {"action": "overview"}})
        assert lost["status"] == "failed" and lost["data"]["error"] == "lost_ownership"
        lease = None
        checks.append("native-wheel-interrupts-control")
        disconnected = {"protocol_version":1,"session_id":descriptor["session_id"],"request_id":uuid.uuid4().hex,"operation":{"op":"inspect"}}
        with socket.create_connection((host,int(port)),timeout=5) as connection:
            connection.sendall(json.dumps(disconnected).encode()+b"\n")
        assert request({"op":"inspect"})["status"] == "ok"
        checks.append("inspector-disconnect-keeps-session-usable")
        if args.lease_expiry:
            lease = acquire()
            time.sleep(31)
            expired = request({"op":"action","lease":lease,"command":{"action":"overview"}})
            assert expired["status"] == "failed" and expired["data"]["error"] == "lost_ownership", expired
            lease = None
            checks.append("renewable-30-second-lease-expires-with-lost-ownership")
        result["status"] = "passed"
    except BaseException as error:
        result.update(status="failed", error=str(error))
        raise
    finally:
        try:
            if lease:
                request({"op": "release_control", "lease": lease})
        except Exception as error:
            result["release_error"] = str(error)
        (output / "result.json").write_text(json.dumps(result, indent=2))
        (output / "requests.json").write_text(json.dumps(records, indent=2))
    print(json.dumps({"status": result["status"], "checks": checks, "result": str(output / "result.json")}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=Path("../target/release/astrum_dev.exe"))
    parser.add_argument("--skip-raw", action="store_true", help="Check malformed transport separately with the Rust transport tests")
    parser.add_argument("--lease-expiry", action="store_true", help="Also wait 31 seconds to check actual lease expiry")
    run(parser.parse_args())
