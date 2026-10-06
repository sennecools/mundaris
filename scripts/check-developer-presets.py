#!/usr/bin/env python3
"""Check ordinary and developer-tools native presets on Windows.

The script launches only the supplied application executable. Each child gets an
isolated registry, evidence directory, and log files. It never uses the owned
launcher to build or start the app, and closes only windows owned by its child PID.
"""

from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from typing import Any


SNAPSHOT_SCHEMA = 5
BRIDGE_TIMEOUT_SECONDS = 40
OBSERVATION_TIMEOUT_SECONDS = 30
WM_CLOSE = 0x0010


class CheckFailure(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise CheckFailure(message)


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def cli_call(
    binary: Path,
    registry: Path,
    args: list[str],
    log: list[dict[str, Any]],
    *,
    allow_failure: bool = False,
    timeout: float = BRIDGE_TIMEOUT_SECONDS,
) -> tuple[subprocess.CompletedProcess[bytes], Any | None]:
    command = [str(binary), *args]
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        record = {
            "command": command,
            "timeout_seconds": timeout,
            "error": "bridge CLI timed out",
            "stdout": decode(exc.stdout),
            "stderr": decode(exc.stderr),
        }
        log.append(record)
        raise CheckFailure(f"bridge CLI timed out after {timeout:g}s: {command[1:]}") from exc
    stdout = decode(completed.stdout)
    stderr = decode(completed.stderr)
    try:
        value = json.loads(stdout) if stdout.strip() else None
    except json.JSONDecodeError:
        value = None
    record = {
        "command": command,
        "exit_code": completed.returncode,
        "stdout": stdout,
        "stderr": stderr,
    }
    log.append(record)
    if not allow_failure and completed.returncode != 0:
        raise CheckFailure(f"bridge CLI exited {completed.returncode}: {stderr or stdout}")
    if not allow_failure and value is None:
        raise CheckFailure(f"bridge CLI did not return JSON: {stderr or stdout}")
    return completed, value


def decode(value: bytes | str | None) -> str:
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    return value.decode("utf-8", "replace")


def request(
    bridge: Path,
    registry: Path,
    session_id: str,
    operation: dict[str, Any],
    log: list[dict[str, Any]],
) -> dict[str, Any]:
    # Route every app request through the Rust CLI. Direct Python TCP has caused
    # Windows socket 10053 failures in the native integration environment.
    _, value = cli_call(
        bridge,
        registry,
        [
            "request",
            json.dumps(operation, separators=(",", ":"), ensure_ascii=False),
            "--session",
            session_id,
            "--registry",
            str(registry),
        ],
        log,
    )
    require(isinstance(value, dict), f"request returned non-object JSON: {value!r}")
    return value


def response_data(response: dict[str, Any], expected: set[str], label: str) -> dict[str, Any]:
    require(
        response.get("status") in expected,
        f"{label} status {response.get('status')!r}: {response}",
    )
    data = response.get("data")
    require(isinstance(data, dict), f"{label} response lacks data: {response}")
    return data


def wait_for_descriptor(
    registry: Path,
    child: subprocess.Popen[bytes],
    expected_preset: str,
    timeout_s: float,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if child.poll() is not None:
            raise CheckFailure(
                f"{expected_preset} child exited before registering (code {child.returncode})"
            )
        for path in sorted(registry.glob("*.json")):
            try:
                value = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError, UnicodeDecodeError):
                continue
            if value.get("pid") == child.pid and value.get("preset") == expected_preset:
                return value
        time.sleep(0.1)
    raise CheckFailure(f"{expected_preset} child did not register within {timeout_s:g}s")


def windows_for_pid(pid: int, user32: Any) -> list[int]:
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    windows: list[int] = []

    def collect(hwnd: int, _parameter: int) -> bool:
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            windows.append(int(hwnd))
        return True

    callback = callback_type(collect)
    user32.EnumWindows(callback, 0)
    return windows


def window_pid(hwnd: int, user32: Any) -> int:
    owner = wintypes.DWORD()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
    return int(owner.value)


def win_api() -> Any:
    if os.name != "nt":
        raise CheckFailure("this preset check requires Windows")
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user32.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
    user32.EnumWindows.restype = wintypes.BOOL
    user32.GetWindowThreadProcessId.argtypes = [
        wintypes.HWND,
        ctypes.POINTER(wintypes.DWORD),
    ]
    user32.GetWindowThreadProcessId.restype = wintypes.DWORD
    user32.IsWindowVisible.argtypes = [wintypes.HWND]
    user32.IsWindowVisible.restype = wintypes.BOOL
    user32.PostMessageW.argtypes = [
        wintypes.HWND,
        wintypes.UINT,
        wintypes.WPARAM,
        wintypes.LPARAM,
    ]
    user32.PostMessageW.restype = wintypes.BOOL
    return user32


def close_owned_child(
    child: subprocess.Popen[bytes], user32: Any, timeout_s: float
) -> dict[str, Any]:
    if child.poll() is not None:
        return {"pid": child.pid, "exited": True, "exit_code": child.returncode, "wm_close": []}

    window_deadline = time.monotonic() + min(timeout_s, 15.0)
    hwnds: list[int] = []
    while time.monotonic() < window_deadline:
        if child.poll() is not None:
            return {"pid": child.pid, "exited": True, "exit_code": child.returncode, "wm_close": []}
        hwnds = windows_for_pid(child.pid, user32)
        if hwnds:
            break
        time.sleep(0.1)
    require(hwnds, f"owned PID {child.pid} has no visible top-level window; left running")

    sent: list[int] = []
    for hwnd in hwnds:
        # Recheck PID immediately before signaling to avoid acting on a stale HWND.
        if window_pid(hwnd, user32) == child.pid and user32.PostMessageW(hwnd, WM_CLOSE, 0, 0):
            sent.append(hwnd)
    require(sent, f"WM_CLOSE could not be sent to a visible window of owned PID {child.pid}")

    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        code = child.poll()
        if code is not None:
            return {"pid": child.pid, "exited": True, "exit_code": code, "wm_close": sent}
        time.sleep(0.1)
    raise CheckFailure(
        f"WM_CLOSE was sent only to owned PID {child.pid} windows {sent}, "
        f"but it did not exit within {timeout_s:g}s; process was not force-killed"
    )


def create_run_directories(root: Path) -> tuple[Path, Path, Path]:
    root.mkdir(parents=True, exist_ok=False)
    registry = root / "registry"
    evidence = root / "evidence"
    logs = root / "logs"
    registry.mkdir()
    evidence.mkdir()
    logs.mkdir()
    return registry, evidence, logs


def start_app(
    binary: Path,
    root: Path,
    argument: str,
    developer_interface: bool,
) -> tuple[subprocess.Popen[bytes], Path]:
    registry, evidence, logs = create_run_directories(root)
    env = os.environ.copy()
    env["MUNDARIS_DEV_REGISTRY"] = str(registry)
    env["MUNDARIS_DEV_OUTPUT"] = str(evidence)
    env.pop("MUNDARIS_DEV_BUILD_MANIFEST", None)
    command = [str(binary), argument]
    if developer_interface:
        command.append("--dev-interface")
    with (logs / "stdout.log").open("wb") as stdout, (logs / "stderr.log").open("wb") as stderr:
        child = subprocess.Popen(
            command,
            cwd=str(Path(__file__).resolve().parents[1]),
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
        )
    return child, registry


def inspect_snapshot(
    bridge: Path,
    registry: Path,
    descriptor: dict[str, Any],
    log: list[dict[str, Any]],
    *,
    predicate: Any = None,
    min_command_sequence: int = 0,
    timeout_s: float = OBSERVATION_TIMEOUT_SECONDS,
) -> tuple[dict[str, Any], int]:
    deadline = time.monotonic() + timeout_s
    polls = 0
    latest: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        polls += 1
        response = request(bridge, registry, descriptor["session_id"], {"op": "inspect"}, log)
        data = response_data(response, {"ok"}, "inspect")
        snapshot = data.get("snapshot")
        require(isinstance(snapshot, dict), f"Inspect has no snapshot: {data}")
        latest = snapshot
        development = snapshot.get("development") or {}
        valid = (
            snapshot.get("schema_version") == SNAPSHOT_SCHEMA
            and snapshot.get("general", {}).get("frame_number", 0) >= 3
            and development.get("stale") is False
            and development.get("command_sequence", 0) >= min_command_sequence
        )
        if valid and (predicate is None or predicate(snapshot)):
            return snapshot, polls
        time.sleep(0.1)
    raise CheckFailure(
        f"fresh schema-{SNAPSHOT_SCHEMA} snapshot predicate timed out after {timeout_s:g}s; "
        f"last snapshot={latest}"
    )


def wait_receipt(
    bridge: Path,
    registry: Path,
    descriptor: dict[str, Any],
    command_id: str,
    log: list[dict[str, Any]],
    timeout_s: float = 30.0,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_s
    latest: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        response = request(
            bridge,
            registry,
            descriptor["session_id"],
            {"op": "receipt", "command_id": command_id},
            log,
        )
        data = response_data(response, {"ok"}, "receipt")
        receipt = data
        require(isinstance(receipt, dict), f"receipt response lacks receipt: {data}")
        latest = receipt
        status = receipt.get("status")
        if status != "accepted":
            require(status == "applied", f"command {command_id} ended {status}: {receipt}")
            return receipt
        time.sleep(0.1)
    raise CheckFailure(f"command {command_id} receipt timed out; latest={latest}")


def submit_action(
    bridge: Path,
    registry: Path,
    descriptor: dict[str, Any],
    lease: str,
    command: dict[str, Any],
    log: list[dict[str, Any]],
) -> dict[str, Any]:
    response = request(
        bridge,
        registry,
        descriptor["session_id"],
        {"op": "action", "lease": lease, "command": command},
        log,
    )
    data = response_data(response, {"accepted"}, "action")
    command_id = data.get("command_id")
    require(isinstance(command_id, str), f"action returned no command id: {data}")
    return wait_receipt(bridge, registry, descriptor, command_id, log)


def check_developer_preset(
    *,
    binary: Path,
    bridge: Path,
    root: Path,
    preset: str,
    argument: str,
    target_name: str,
    target_identity: str | None,
    user32: Any,
    close_timeout_s: float,
) -> dict[str, Any]:
    run_data: dict[str, Any] = {
        "preset": preset,
        "argument": argument,
        "status": "running",
        "checks": [],
        "bridge_calls": [],
        "cleanup": {},
    }
    child: subprocess.Popen[bytes] | None = None
    registry = root / "registry"
    descriptor: dict[str, Any] | None = None
    lease: str | None = None
    try:
        child, registry = start_app(binary, root, argument, developer_interface=True)
        descriptor = wait_for_descriptor(registry, child, preset, 30.0)
        run_data["pid"] = child.pid
        run_data["run_directory"] = str(root.resolve())
        run_data["registry"] = str(registry.resolve())
        run_data["evidence_directory"] = str((root / "evidence").resolve())
        run_data["stdout_log"] = str((root / "logs" / "stdout.log").resolve())
        run_data["stderr_log"] = str((root / "logs" / "stderr.log").resolve())
        run_data["session"] = descriptor
        registry_entries = list(registry.glob("*.json"))
        require(len(registry_entries) == 1, f"isolated registry has unexpected descriptors: {registry_entries}")
        require(descriptor.get("pid") == child.pid, "registered PID differs from launched child")
        require(
            Path(descriptor.get("executable", "")).resolve(strict=True) == binary,
            "registered executable differs from --binary",
        )
        require(descriptor.get("build_manifest") in (None, {}), "manual launch unexpectedly has a build manifest")
        require(os.path.samefile(descriptor["output_directory"], root / "evidence"), "app output directory does not match this isolated run")
        run_data["checks"].append("isolated-registry-and-manual-launch-has-no-build-manifest")

        response = request(bridge, registry, descriptor["session_id"], {"op": "capabilities"}, run_data["bridge_calls"])
        capabilities = response_data(response, {"ok"}, "capabilities")
        require(preset in capabilities.get("presets", []), f"capabilities omit preset {preset!r}")
        run_data["capabilities"] = capabilities
        run_data["checks"].append("expected-preset-advertised")

        inspect_response = request(bridge, registry, descriptor["session_id"], {"op": "inspect"}, run_data["bridge_calls"])
        inspect_data = response_data(inspect_response, {"ok"}, "inspect")
        require(inspect_data.get("source_attribution") == "unavailable", "manual launch source attribution was not reported unavailable")
        require(inspect_data.get("session", {}).get("preset") == preset, "inspect session preset mismatch")
        bodies = inspect_data.get("bodies")
        require(isinstance(bodies, list) and bodies, "inspect returned no bodies")
        if target_identity is not None:
            target = next(
                (
                    body
                    for body in bodies
                    if isinstance(body, dict)
                    and body.get("semantic_identity") == target_identity
                    and body.get("name", "").casefold() == target_name.casefold()
                ),
                None,
            )
            require(target is not None, f"inventory omitted {target_name} ({target_identity})")
        else:
            # Fixture inventories currently identify bodies by their displayed name;
            # record the inventory's actual semantic_identity instead of inventing one.
            target = next(
                (
                    body
                    for body in bodies
                    if isinstance(body, dict)
                    and isinstance(body.get("name"), str)
                    and body["name"].casefold() == target_name.casefold()
                ),
                None,
            )
            require(target is not None, f"inventory omitted fixture body {target_name}")
        require(isinstance(target.get("handle"), str) and target["handle"], "target inventory entry has no handle")
        run_data["target_body"] = {
            "name": target.get("name"),
            "semantic_identity": target.get("semantic_identity"),
            "handle": target["handle"],
        }
        run_data["checks"].append("actual-inventory-body-and-identity")

        fresh, polls = inspect_snapshot(bridge, registry, descriptor, run_data["bridge_calls"])
        run_data["initial_snapshot"] = fresh
        run_data["initial_inspect_polls"] = polls
        run_data["checks"].append("fresh-schema-5-snapshot-at-frame-3-or-later")

        acquired = request(
            bridge,
            registry,
            descriptor["session_id"],
            {"op": "acquire_control", "owner": "developer-preset-check"},
            run_data["bridge_calls"],
        )
        acquire_data = response_data(acquired, {"ok"}, "acquire control")
        lease = acquire_data.get("lease")
        require(isinstance(lease, str) and lease, f"acquire returned no lease: {acquire_data}")
        run_data["checks"].append("control-acquired")

        focus_receipt = submit_action(
            bridge,
            registry,
            descriptor,
            lease,
            {"action": "focus", "body": target["handle"], "body_fixed": False},
            run_data["bridge_calls"],
        )
        mode_receipt = submit_action(
            bridge,
            registry,
            descriptor,
            lease,
            {"action": "navigation_mode", "mode": "body_orbit"},
            run_data["bridge_calls"],
        )
        focused, polls = inspect_snapshot(
            bridge,
            registry,
            descriptor,
            run_data["bridge_calls"],
            predicate=lambda snapshot: (
                snapshot.get("general", {}).get("focused_body", {}).get("name", "").casefold()
                == target_name.casefold()
                and snapshot.get("general", {}).get("camera_mode") == "body_orbit"
                and snapshot.get("camera", {}).get("navigation", {}).get("transitioning") is False
            ),
            min_command_sequence=max(
                int(focus_receipt.get("sequence", 0)), int(mode_receipt.get("sequence", 0))
            ),
        )
        run_data["focused_snapshot"] = focused
        run_data["focused_inspect_polls"] = polls
        run_data["checks"].append("focused-target-body-orbit-transition-settled")

        accepted = request(
            bridge,
            registry,
            descriptor["session_id"],
            {"op": "capture", "lease": lease, "name": "preset-smoke"},
            run_data["bridge_calls"],
        )
        accepted_data = response_data(accepted, {"accepted"}, "capture")
        capture_id = accepted_data.get("command_id")
        require(isinstance(capture_id, str), f"capture returned no command id: {accepted_data}")
        receipt = wait_receipt(bridge, registry, descriptor, capture_id, run_data["bridge_calls"])
        capture = receipt.get("data")
        require(isinstance(capture, dict), f"capture receipt has no data: {receipt}")
        manifest_path = Path(capture.get("manifest", "")).resolve()
        full_image = Path(capture.get("full_image", "")).resolve()
        snapshot_path = Path(capture.get("snapshot", "")).resolve()
        require(manifest_path.is_file(), f"capture manifest missing: {manifest_path}")
        require(full_image.is_file(), f"engine client PNG missing: {full_image}")
        require(full_image.name.casefold() == "client.png", f"capture is not the full engine client PNG: {full_image}")
        with full_image.open("rb") as image_file:
            require(image_file.read(8) == b"\x89PNG\r\n\x1a\n", f"capture is not a PNG: {full_image}")
        require(snapshot_path.is_file(), f"capture snapshot missing: {snapshot_path}")
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        snapshot = json.loads(snapshot_path.read_text(encoding="utf-8"))
        for field in ("capture_id", "source_frame", "prepared_frame", "world_revision", "width", "height", "full_image", "snapshot", "manifest"):
            require(manifest.get(field) == capture.get(field), f"manifest field {field} differs from receipt")
        require(manifest.get("source") == "native_surface_after_scene_and_ui", "capture source is not the native scene+UI surface")
        require(snapshot.get("schema_version") == SNAPSHOT_SCHEMA, "captured snapshot schema mismatch")
        require(snapshot.get("general", {}).get("frame_number") == capture.get("prepared_frame"), "capture prepared frame mismatch")
        require(snapshot.get("development", {}).get("submitted_frame") == capture.get("source_frame"), "capture source submission mismatch")
        require(snapshot.get("general", {}).get("focused_body", {}).get("name", "").casefold() == target_name.casefold(), "capture snapshot focused body mismatch")
        run_data["capture"] = {
            "receipt": receipt,
            "manifest": manifest,
            "manifest_path": str(manifest_path),
            "full_image": str(full_image),
            "snapshot": str(snapshot_path),
        }
        run_data["checks"].append("native-capture-receipt-manifest-client-png-and-frame-pair")

        stop_command = ["stop", "--session", descriptor["session_id"], "--registry", str(registry)]
        ownership_marker = Path(descriptor["output_directory"]) / "owned-session.json"
        require(not ownership_marker.exists(), "manual launch unexpectedly has an owned-session marker")
        stop_result, stop_json = cli_call(
            bridge,
            registry,
            stop_command,
            run_data["bridge_calls"],
            allow_failure=True,
        )
        stop_message = str((stop_json or {}).get("error", ""))
        require(
            stop_result.returncode != 0
            and (stop_json or {}).get("status") == "error"
            and "not owned by this launcher" in stop_message,
            f"CLI stop did not refuse the unowned manual child: {stop_json or stop_result.returncode}",
        )
        require(child.poll() is None, "manual app exited after owned-stop refusal")
        run_data["checks"].append("owned-stop-refuses-manual-session-and-preserves-child")

        released = request(
            bridge,
            registry,
            descriptor["session_id"],
            {"op": "release_control", "lease": lease},
            run_data["bridge_calls"],
        )
        response_data(released, {"ok"}, "release control")
        lease = None
        run_data["checks"].append("control-released")
        run_data["status"] = "passed"
    except Exception as exc:
        run_data["status"] = "failed"
        run_data["error"] = f"{type(exc).__name__}: {exc}"
    finally:
        if child is not None and child.poll() is None and lease and descriptor:
            try:
                released = request(
                    bridge,
                    registry,
                    descriptor["session_id"],
                    {"op": "release_control", "lease": lease},
                    run_data["bridge_calls"],
                )
                response_data(released, {"ok"}, "cleanup release control")
                run_data["cleanup"]["control_released"] = True
            except Exception as exc:
                run_data["cleanup"]["release_error"] = f"{type(exc).__name__}: {exc}"
        if child is not None:
            try:
                cleanup = close_owned_child(child, user32, close_timeout_s)
                run_data["cleanup"].update(cleanup)
                if cleanup.get("exit_code") != 0 and run_data["status"] == "passed":
                    run_data["status"] = "failed"
                    run_data["error"] = f"owned child exited unsuccessfully: {cleanup.get('exit_code')}"
            except Exception as exc:
                run_data["cleanup"].update(
                    {"pid": child.pid, "exited": child.poll() is not None, "error": f"{type(exc).__name__}: {exc}"}
                )
                if run_data["status"] == "passed":
                    run_data["status"] = "failed"
                    run_data["error"] = "owned child did not close gracefully"
    return run_data


def check_ordinary_preset(
    binary: Path,
    root: Path,
    user32: Any,
    close_timeout_s: float,
) -> dict[str, Any]:
    run_data: dict[str, Any] = {
        "preset": "solar-system",
        "developer_interface": False,
        "status": "running",
        "checks": [],
        "cleanup": {},
    }
    child: subprocess.Popen[bytes] | None = None
    try:
        child, registry = start_app(binary, root, "--solar-system", developer_interface=False)
        run_data["pid"] = child.pid
        run_data["run_directory"] = str(root.resolve())
        run_data["registry"] = str(registry.resolve())
        run_data["stdout_log"] = str((root / "logs" / "stdout.log").resolve())
        run_data["stderr_log"] = str((root / "logs" / "stderr.log").resolve())
        time.sleep(3.0)
        require(child.poll() is None, f"ordinary solar-system child exited with code {child.returncode}")
        entries = list(registry.iterdir())
        require(not entries, f"ordinary solar-system created files in isolated developer registry: {entries}")
        run_data["checks"].append("ordinary-solar-system-runs-with-empty-dev-registry-after-3s")
        run_data["status"] = "passed"
    except Exception as exc:
        run_data["status"] = "failed"
        run_data["error"] = f"{type(exc).__name__}: {exc}"
    finally:
        if child is not None:
            try:
                cleanup = close_owned_child(child, user32, close_timeout_s)
                run_data["cleanup"].update(cleanup)
                if cleanup.get("exit_code") != 0 and run_data["status"] == "passed":
                    run_data["status"] = "failed"
                    run_data["error"] = f"ordinary child exited unsuccessfully: {cleanup.get('exit_code')}"
            except Exception as exc:
                run_data["cleanup"].update(
                    {"pid": child.pid, "exited": child.poll() is not None, "error": f"{type(exc).__name__}: {exc}"}
                )
                if run_data["status"] == "passed":
                    run_data["status"] = "failed"
                    run_data["error"] = "ordinary child did not close gracefully"
    return run_data


def run(args: argparse.Namespace) -> dict[str, Any]:
    require(os.name == "nt", "this preset check currently requires Windows")
    output = args.output.expanduser().resolve()
    require(not output.exists(), f"output directory must be fresh: {output}")
    binary = args.binary.expanduser().resolve(strict=True)
    require(binary.is_file(), f"application binary is not a file: {binary}")
    bridge = (args.bridge_binary or binary.with_name("mundaris_dev.exe")).expanduser().resolve(strict=True)
    require(bridge.is_file(), f"bridge binary is not a file: {bridge}")
    output.mkdir(parents=True)
    user32 = win_api()

    result: dict[str, Any] = {
        "schema": 1,
        "status": "running",
        "binary": str(binary),
        "bridge_binary": str(bridge),
        "output_directory": str(output),
        "build_manifest": "cleared_from_child_environment",
        "runs": [],
        "errors": [],
    }
    try:
        result["runs"].append(
            check_ordinary_preset(
                binary,
                output / "ordinary-solar-system",
                user32,
                args.close_timeout_seconds,
            )
        )
        if result["runs"][-1]["status"] != "passed":
            raise CheckFailure(result["runs"][-1].get("error", "ordinary preset failed"))

        for preset, argument, target_name, target_identity in [
            ("real-solar-system", "--real-solar-system", "Earth", "solar:earth"),
            ("gravity-orbits", "--gravity-orbits", "Aurelia", None),
        ]:
            run_data = check_developer_preset(
                binary=binary,
                bridge=bridge,
                root=output / preset,
                preset=preset,
                argument=argument,
                target_name=target_name,
                target_identity=target_identity,
                user32=user32,
                close_timeout_s=args.close_timeout_seconds,
            )
            result["runs"].append(run_data)
            if run_data["status"] != "passed":
                raise CheckFailure(run_data.get("error", f"{preset} preset failed"))
        result["status"] = "passed"
    except Exception as exc:
        result["status"] = "failed"
        result["errors"].append(f"{type(exc).__name__}: {exc}")
    write_json(output / "result.json", result)
    return result


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True, help="built mundaris_app.exe")
    parser.add_argument(
        "--bridge-binary",
        type=Path,
        help="mundaris_dev.exe (defaults to the sibling of --binary)",
    )
    parser.add_argument("--output", type=Path, required=True, help="fresh result directory")
    parser.add_argument(
        "--close-timeout-seconds",
        type=float,
        default=30.0,
        help="graceful WM_CLOSE exit timeout (default: 30s)",
    )
    args = parser.parse_args()
    if not (1.0 <= args.close_timeout_seconds <= 300.0):
        parser.error("--close-timeout-seconds must be in 1..=300")
    return args


def main() -> int:
    args = parse_args()
    try:
        result = run(args)
    except Exception as exc:
        print(json.dumps({"status": "error", "error": f"{type(exc).__name__}: {exc}"}))
        return 1
    print(
        json.dumps(
            {
                "status": result["status"],
                "result": str(args.output.expanduser().resolve() / "result.json"),
                "runs": [
                    {"preset": run_data.get("preset"), "status": run_data.get("status")}
                    for run_data in result["runs"]
                ],
            },
            sort_keys=True,
        )
    )
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
