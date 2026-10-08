#!/usr/bin/env python3
"""Collect raw Windows process and native-capture measurements for the dev interface.

Both runs use the same explicitly supplied developer-tools executable and the
same 1280x800 physical client-area solar-system window. This script reports observations; it
does not infer or claim that the developer interface is faster or slower.

Example (release executable built with the developer-tools feature)::

    py -3 scripts/measure-developer-interface.py --binary ../target/release/mundaris_app.exe --output ../target/developer-interface-measurement
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import os
import platform
import subprocess
import sys
import time
from ctypes import wintypes
from pathlib import Path
from typing import Any


PRESET = "solar-system"
WINDOW_SIZE = [1280, 800]
BRIDGE_TIMEOUT_SECONDS = 40
PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ = 0x0010
WM_CLOSE = 0x0010
SW_RESTORE = 9
SWP_NOZORDER = 0x0004
SWP_NOACTIVATE = 0x0010
SWP_FRAMECHANGED = 0x0020
MIN_MAIN_WINDOW_AREA = 200 * 200


class RECT(ctypes.Structure):
    _fields_ = [("left", wintypes.LONG), ("top", wintypes.LONG), ("right", wintypes.LONG), ("bottom", wintypes.LONG)]


class FILETIME(ctypes.Structure):
    _fields_ = [("dwLowDateTime", wintypes.DWORD), ("dwHighDateTime", wintypes.DWORD)]


class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("PageFaultCount", wintypes.DWORD),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
    ]


def win_api() -> tuple[Any, Any, Any]:
    if os.name != "nt":
        raise RuntimeError("this measurement script currently requires Windows")
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel32.OpenProcess.restype = wintypes.HANDLE
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel32.CloseHandle.restype = wintypes.BOOL
    kernel32.GetProcessTimes.argtypes = [
        wintypes.HANDLE,
        ctypes.POINTER(FILETIME),
        ctypes.POINTER(FILETIME),
        ctypes.POINTER(FILETIME),
        ctypes.POINTER(FILETIME),
    ]
    kernel32.GetProcessTimes.restype = wintypes.BOOL
    psapi.GetProcessMemoryInfo.argtypes = [
        wintypes.HANDLE,
        ctypes.POINTER(PROCESS_MEMORY_COUNTERS),
        wintypes.DWORD,
    ]
    psapi.GetProcessMemoryInfo.restype = wintypes.BOOL
    user32.EnumWindows.argtypes = [ctypes.c_void_p, wintypes.LPARAM]
    user32.EnumWindows.restype = wintypes.BOOL
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user32.GetWindowThreadProcessId.restype = wintypes.DWORD
    user32.IsWindowVisible.argtypes = [wintypes.HWND]
    user32.IsWindowVisible.restype = wintypes.BOOL
    user32.IsIconic.argtypes = [wintypes.HWND]
    user32.IsIconic.restype = wintypes.BOOL
    user32.IsZoomed.argtypes = [wintypes.HWND]
    user32.IsZoomed.restype = wintypes.BOOL
    user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
    user32.ShowWindow.restype = wintypes.BOOL
    user32.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(RECT)]
    user32.GetClientRect.restype = wintypes.BOOL
    user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(RECT)]
    user32.GetWindowRect.restype = wintypes.BOOL
    user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
    user32.GetWindowTextLengthW.restype = ctypes.c_int
    user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user32.GetWindowTextW.restype = ctypes.c_int
    user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user32.GetClassNameW.restype = ctypes.c_int
    user32.SetWindowPos.argtypes = [
        wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
        ctypes.c_int, ctypes.c_int, wintypes.UINT,
    ]
    user32.SetWindowPos.restype = wintypes.BOOL
    user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    user32.PostMessageW.restype = wintypes.BOOL
    set_dpi_context = getattr(user32, "SetProcessDpiAwarenessContext", None)
    if set_dpi_context is not None:
        set_dpi_context.argtypes = [ctypes.c_void_p]
        set_dpi_context.restype = wintypes.BOOL
        # Make the helper's HWND measurements and placement use physical pixels.
        # ERROR_ACCESS_DENIED means a manifest or earlier initialization already
        # fixed this process's context; the subsequent size check remains decisive.
        if not set_dpi_context(ctypes.c_void_p(-4)):
            error = ctypes.get_last_error()
            if error != 5:
                raise ctypes.WinError(error)
    return kernel32, psapi, user32


class ProcessMetrics:
    def __init__(self, pid: int, kernel32: Any, psapi: Any):
        self.pid = pid
        self.kernel32 = kernel32
        self.psapi = psapi
        self.handle = kernel32.OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, False, pid
        )
        if not self.handle:
            raise ctypes.WinError(ctypes.get_last_error())

    @staticmethod
    def _filetime_seconds(value: FILETIME) -> float:
        ticks = (value.dwHighDateTime << 32) | value.dwLowDateTime
        return ticks / 10_000_000.0

    def cpu_seconds(self) -> float:
        creation, exit_time, kernel, user = FILETIME(), FILETIME(), FILETIME(), FILETIME()
        if not self.kernel32.GetProcessTimes(
            self.handle,
            ctypes.byref(creation),
            ctypes.byref(exit_time),
            ctypes.byref(kernel),
            ctypes.byref(user),
        ):
            raise ctypes.WinError(ctypes.get_last_error())
        return self._filetime_seconds(kernel) + self._filetime_seconds(user)

    def working_set_bytes(self) -> int:
        counters = PROCESS_MEMORY_COUNTERS()
        counters.cb = ctypes.sizeof(counters)
        if not self.psapi.GetProcessMemoryInfo(
            self.handle, ctypes.byref(counters), counters.cb
        ):
            raise ctypes.WinError(ctypes.get_last_error())
        return int(counters.WorkingSetSize)

    def close(self) -> None:
        if self.handle:
            self.kernel32.CloseHandle(self.handle)
            self.handle = None


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def fresh_directory(path: Path) -> None:
    if path.exists():
        if not path.is_dir() or any(path.iterdir()):
            raise RuntimeError(f"output directory must be empty: {path}")
    else:
        path.mkdir(parents=True)


def isolated_environment(run_dir: Path) -> dict[str, str]:
    env = os.environ.copy()
    for key in ("MUNDARIS_DEV_REGISTRY", "MUNDARIS_DEV_OUTPUT", "MUNDARIS_DEV_BUILD_MANIFEST"):
        env.pop(key, None)
    registry = run_dir / "registry"
    evidence = run_dir / "developer-evidence"
    registry.mkdir()
    evidence.mkdir()
    env["MUNDARIS_DEV_REGISTRY"] = str(registry.resolve())
    env["MUNDARIS_DEV_OUTPUT"] = str(evidence.resolve())
    return env


def launch(binary: Path, run_dir: Path, developer: bool) -> subprocess.Popen[bytes]:
    env = isolated_environment(run_dir)
    command = [str(binary), "--solar-system"]
    if developer:
        command.append("--dev-interface")
    stdout_file = (run_dir / "stdout.txt").open("wb")
    stderr_file = (run_dir / "stderr.txt").open("wb")
    try:
        child = subprocess.Popen(
            command,
            cwd=Path(__file__).resolve().parents[1],
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=stdout_file,
            stderr=stderr_file,
            close_fds=True,
        )
    finally:
        stdout_file.close()
        stderr_file.close()
    return child


def request(
    bridge_binary: Path,
    registry: Path,
    session_id: str,
    operation: dict[str, Any],
) -> dict[str, Any]:
    payload = json.dumps(operation, separators=(",", ":"))
    command = [
        str(bridge_binary),
        "--registry",
        str(registry),
        "--session",
        session_id,
        "request",
        payload,
    ]
    try:
        result = subprocess.run(
            command,
            cwd=Path(__file__).resolve().parents[1],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=BRIDGE_TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        raise TimeoutError(
            f"developer bridge request exceeded {BRIDGE_TIMEOUT_SECONDS}s"
        ) from exc
    try:
        response = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        detail = result.stderr.strip() or result.stdout.strip()
        raise RuntimeError(f"developer bridge returned invalid JSON: {detail}") from exc
    if not isinstance(response, dict):
        raise RuntimeError("developer bridge response must be a JSON object")
    if result.returncode and response.get("status") == "error":
        raise RuntimeError(f"developer bridge request failed: {response}; {result.stderr.strip()}")
    return response


def wait_for_session(
    child: subprocess.Popen[bytes],
    registry: Path,
    binary: Path,
    bridge_binary: Path,
    timeout_s: float = 30.0,
) -> dict[str, Any]:
    registry = registry.resolve(strict=True)
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if child.poll() is not None:
            raise RuntimeError(
                f"developer-enabled app exited before session registration ({child.returncode})"
            )
        for path in registry.glob("*.json"):
            try:
                descriptor = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            if descriptor.get("pid") != child.pid:
                continue
            if descriptor.get("preset") != PRESET:
                raise RuntimeError("owned session registered with an unexpected preset")
            if Path(descriptor.get("executable", "")).resolve() != binary.resolve():
                raise RuntimeError("owned session descriptor executable did not match --binary")
            descriptor["_registry_path"] = str(path.resolve())
            response = request(
                bridge_binary,
                registry,
                descriptor["session_id"],
                {"op": "capabilities"},
            )
            if response.get("status") != "ok":
                raise RuntimeError(f"capabilities request failed: {response}")
            descriptor["_capabilities"] = response.get("data")
            return descriptor
        time.sleep(0.1)
    raise TimeoutError("developer-enabled app did not publish a session descriptor")


def rect_size(rect: RECT) -> tuple[int, int]:
    return int(rect.right - rect.left), int(rect.bottom - rect.top)


def window_title(hwnd: int, user32: Any) -> str:
    length = max(1, int(user32.GetWindowTextLengthW(hwnd)))
    buffer = ctypes.create_unicode_buffer(min(length + 1, 1024))
    user32.GetWindowTextW(hwnd, buffer, len(buffer))
    return buffer.value


def window_class(hwnd: int, user32: Any) -> str:
    buffer = ctypes.create_unicode_buffer(512)
    user32.GetClassNameW(hwnd, buffer, len(buffer))
    return buffer.value


def main_window(child: subprocess.Popen[bytes], user32: Any) -> int:
    windows: list[tuple[int, int]] = []
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    @callback_type
    def enum_callback(hwnd: int, _lparam: int) -> bool:
        process_id = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(process_id))
        if process_id.value != child.pid or not user32.IsWindowVisible(hwnd):
            return True
        if window_title(hwnd, user32) != "Mundaris":
            return True
        rect = RECT()
        if not user32.GetWindowRect(hwnd, ctypes.byref(rect)):
            return True
        width, height = rect_size(rect)
        area = width * height
        if width > 0 and height > 0 and area >= MIN_MAIN_WINDOW_AREA:
            windows.append((int(hwnd), area))
        return True

    user32.EnumWindows(enum_callback, 0)
    if not windows:
        raise RuntimeError(
            f"owned PID {child.pid} has no visible top-level window titled exactly 'Mundaris'"
        )
    # Require the app's source-defined main title; area only resolves duplicate
    # owned windows with that title and excludes console/IME/helper windows.
    return max(windows, key=lambda item: item[1])[0]


def wait_for_main_window(
    child: subprocess.Popen[bytes], user32: Any, timeout_s: float = 30.0
) -> int:
    deadline = time.monotonic() + timeout_s
    last_error: RuntimeError | None = None
    while time.monotonic() < deadline:
        if child.poll() is not None:
            raise RuntimeError(
                f"owned app PID {child.pid} exited before its main window appeared "
                f"({child.returncode})"
            )
        try:
            return main_window(child, user32)
        except RuntimeError as exc:
            last_error = exc
        time.sleep(0.1)
    raise TimeoutError(
        f"owned app PID {child.pid} did not show a main-sized window within "
        f"{timeout_s:g}s: {last_error}"
    )


def window_geometry(child: subprocess.Popen[bytes], user32: Any) -> dict[str, Any]:
    hwnd = main_window(child, user32)
    client, outer = RECT(), RECT()
    if not user32.GetClientRect(hwnd, ctypes.byref(client)):
        raise ctypes.WinError(ctypes.get_last_error())
    if not user32.GetWindowRect(hwnd, ctypes.byref(outer)):
        raise ctypes.WinError(ctypes.get_last_error())
    return {
        "hwnd": hwnd,
        "title": window_title(hwnd, user32),
        "class": window_class(hwnd, user32),
        "client_size_px": list(rect_size(client)),
        "window_size_px": list(rect_size(outer)),
        "window_rect_px": [int(outer.left), int(outer.top), int(outer.right), int(outer.bottom)],
        "minimized": bool(user32.IsIconic(hwnd)),
    }


def set_client_size(child: subprocess.Popen[bytes], user32: Any, target: list[int]) -> dict[str, Any]:
    hwnd = wait_for_main_window(child, user32)
    if user32.IsIconic(hwnd) or user32.IsZoomed(hwnd):
        user32.ShowWindow(hwnd, SW_RESTORE)
    client, outer = RECT(), RECT()
    if not user32.GetClientRect(hwnd, ctypes.byref(client)):
        raise ctypes.WinError(ctypes.get_last_error())
    if not user32.GetWindowRect(hwnd, ctypes.byref(outer)):
        raise ctypes.WinError(ctypes.get_last_error())
    client_w, client_h = rect_size(client)
    outer_w, outer_h = rect_size(outer)
    desired_outer_w = target[0] + max(0, outer_w - client_w)
    desired_outer_h = target[1] + max(0, outer_h - client_h)
    if not user32.SetWindowPos(
        hwnd, None, outer.left, outer.top, desired_outer_w, desired_outer_h,
        SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
    ):
        raise ctypes.WinError(ctypes.get_last_error())
    deadline = time.monotonic() + 2.0
    while time.monotonic() < deadline:
        geometry = window_geometry(child, user32)
        if geometry["client_size_px"] == target and not geometry["minimized"]:
            return geometry
        time.sleep(0.05)
    raise RuntimeError(
        f"could not normalize owned HWND {hwnd} client to {target}px; "
        f"actual={window_geometry(child, user32)}"
    )


def require_client_size(
    child: subprocess.Popen[bytes], user32: Any, expected: list[int], phase: str
) -> dict[str, Any]:
    geometry = window_geometry(child, user32)
    if geometry["minimized"]:
        raise RuntimeError(f"owned app window became minimized {phase}")
    if geometry["client_size_px"] != expected:
        raise RuntimeError(
            f"owned app client size changed {phase}: expected={expected}, "
            f"actual={geometry['client_size_px']}"
        )
    return geometry


def windows_sample(
    child: subprocess.Popen[bytes], metrics: ProcessMetrics, user32: Any,
    duration_s: float, expected_client: list[int],
) -> dict[str, Any]:
    geometry_start = require_client_size(child, user32, expected_client, "before sample")
    cpu_start = metrics.cpu_seconds()
    working_start = metrics.working_set_bytes()
    wall_start = time.perf_counter()
    time.sleep(duration_s)
    wall_s = time.perf_counter() - wall_start
    cpu_end = metrics.cpu_seconds()
    working_end = metrics.working_set_bytes()
    geometry_end = require_client_size(child, user32, expected_client, "after sample")
    if child.poll() is not None:
        raise RuntimeError(f"measured app exited unexpectedly ({child.returncode})")
    cpu_s = cpu_end - cpu_start
    return {
        "wall_s": wall_s,
        "cpu_s": cpu_s,
        "cpu_percent_of_one_core": 100.0 * cpu_s / wall_s if wall_s else None,
        "working_set_start_bytes": working_start,
        "working_set_end_bytes": working_end,
        "window_start": geometry_start,
        "window_end": geometry_end,
    }


def owned_visible_windows(child: subprocess.Popen[bytes], user32: Any) -> list[tuple[int, bool]]:
    windows: list[tuple[int, bool]] = []
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    @callback_type
    def enum_callback(hwnd: int, _lparam: int) -> bool:
        process_id = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(process_id))
        if process_id.value == child.pid and user32.IsWindowVisible(hwnd):
            windows.append((int(hwnd), bool(user32.IsIconic(hwnd))))
        return True

    user32.EnumWindows(enum_callback, 0)
    return windows


def capture_sample(
    descriptor: dict[str, Any],
    child: subprocess.Popen[bytes],
    metrics: ProcessMetrics,
    bridge_binary: Path,
    registry: Path,
    output: Path,
    user32: Any,
    expected_client: list[int],
) -> dict[str, Any]:
    acquired = request(
        bridge_binary,
        registry,
        descriptor["session_id"],
        {"op": "acquire_control", "owner": "developer-interface-measurement"},
    )
    if acquired.get("status") != "ok":
        raise RuntimeError(f"could not acquire measurement control lease: {acquired}")
    lease = acquired.get("data", {}).get("lease")
    if not isinstance(lease, str) or not lease:
        raise RuntimeError("acquire-control response did not include a lease")
    try:
        geometry_start = require_client_size(child, user32, expected_client, "before native capture")
        capture_start_wall = time.perf_counter()
        capture_start_cpu = metrics.cpu_seconds()
        accepted = request(
            bridge_binary,
            registry,
            descriptor["session_id"],
            {"op": "capture", "lease": lease, "name": "developer-interface-measurement"},
        )
        if accepted.get("status") != "accepted":
            raise RuntimeError(f"native capture was not accepted: {accepted}")
        command_id = accepted.get("data", {}).get("command_id")
        if not command_id:
            raise RuntimeError("native capture response did not include command_id")
        deadline = time.monotonic() + 60.0
        receipt: dict[str, Any] | None = None
        while time.monotonic() < deadline:
            if child.poll() is not None:
                raise RuntimeError(f"app exited while waiting for native capture ({child.returncode})")
            response = request(
                bridge_binary,
                registry,
                descriptor["session_id"],
                {"op": "receipt", "command_id": command_id},
            )
            if response.get("status") != "ok":
                raise RuntimeError(f"capture receipt request failed: {response}")
            receipt = response.get("data")
            status = receipt.get("status") if isinstance(receipt, dict) else None
            if status in ("applied", "completed"):
                break
            if status in ("failed", "cancelled", "not_found"):
                raise RuntimeError(f"native capture failed: {receipt}")
            time.sleep(0.1)
        else:
            raise TimeoutError("native capture did not complete within 60 seconds")
        capture_wall_s = time.perf_counter() - capture_start_wall
        capture_cpu_s = metrics.cpu_seconds() - capture_start_cpu
        geometry_end = require_client_size(child, user32, expected_client, "after native capture")
        manifest_path = Path(receipt.get("data", {}).get("manifest", ""))
        if not manifest_path.is_file():
            raise RuntimeError(f"native capture manifest is missing: {manifest_path}")
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        (output / "native-capture-manifest.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        return {
            "wall_s": capture_wall_s,
            "cpu_s": capture_cpu_s,
            "cpu_percent_of_one_core": 100.0 * capture_cpu_s / capture_wall_s if capture_wall_s else None,
            "accepted": accepted,
            "receipt": receipt,
            "manifest": manifest,
            "manifest_path": receipt.get("data", {}).get("manifest"),
            "full_image_path": receipt.get("data", {}).get("full_image"),
            "viewport_image_path": receipt.get("data", {}).get("viewport_image"),
            "window_start": geometry_start,
            "window_end": geometry_end,
        }
    finally:
        release = request(
            bridge_binary,
            registry,
            descriptor["session_id"],
            {"op": "release_control", "lease": lease},
        )
        if release.get("status") != "ok":
            raise RuntimeError(f"could not release measurement lease: {release}")


def close_owned_child(child: subprocess.Popen[bytes], user32: Any, timeout_s: float) -> dict[str, Any]:
    sent_to: list[int] = []
    windows_seen: list[int] = []
    deadline = time.monotonic() + timeout_s
    next_close_attempt = 0.0
    while time.monotonic() < deadline:
        code = child.poll()
        if code is not None:
            return {
                "wm_close_sent": sent_to,
                "windows_seen": windows_seen,
                "exit_code": code,
                "exited": True,
            }
        if time.monotonic() >= next_close_attempt:
            for hwnd, _minimized in owned_visible_windows(child, user32):
                if hwnd not in windows_seen:
                    windows_seen.append(hwnd)
                if user32.PostMessageW(hwnd, WM_CLOSE, 0, 0) and hwnd not in sent_to:
                    sent_to.append(hwnd)
            next_close_attempt = time.monotonic() + 0.5
        time.sleep(0.1)
    raise TimeoutError(
        f"WM_CLOSE was retried for owned PID {child.pid} windows {sent_to}; "
        f"windows seen={windows_seen}; process did not exit within {timeout_s:g}s. "
        "No unrelated process was signaled."
    )


def run_measurement(args: argparse.Namespace) -> dict[str, Any]:
    kernel32, psapi, user32 = win_api()
    binary = args.binary.expanduser().resolve(strict=True)
    if not binary.is_file():
        raise RuntimeError(f"binary is not a file: {binary}")
    bridge_binary = (args.bridge_binary or binary.with_name("mundaris_dev.exe")).expanduser().resolve(strict=True)
    if not bridge_binary.is_file():
        raise RuntimeError(f"bridge binary is not a file: {bridge_binary}")
    output = args.output.expanduser().resolve()
    fresh_directory(output)
    baseline_dir = output / "baseline"
    enabled_dir = output / "developer-enabled"
    baseline_dir.mkdir()
    enabled_dir.mkdir()
    digest = sha256_file(binary)
    common = {
        "schema": 1,
        "binary": str(binary),
        "binary_sha256": digest,
        "bridge_binary": str(bridge_binary),
        "bridge_binary_sha256": sha256_file(bridge_binary),
        "bridge_transport": "mundaris_dev request JSON over CLI subprocess",
        "bridge_request_timeout_s": BRIDGE_TIMEOUT_SECONDS,
        "preset": PRESET,
        "target_client_size_px": WINDOW_SIZE,
        "window_size_semantics": "requested physical client pixels; actual client and outer dimensions recorded per sample/capture",
        "minimized": False,
        "sample_count": args.samples,
        "sample_duration_s": args.sample_seconds,
        "warmup_s": args.warmup_seconds,
        "system": {"platform": platform.platform(), "python": sys.version},
        "method": "raw process CPU and working-set observations; no performance conclusion",
    }
    started: dict[str, Any] = {"config": common, "runs": {}}
    children: dict[str, subprocess.Popen[bytes]] = {}
    metrics: dict[str, ProcessMetrics] = {}
    descriptors: dict[str, dict[str, Any]] = {}
    shutdown: dict[str, Any] = {}
    primary_error: str | None = None
    try:
        for name, developer, run_dir in (
            ("baseline", False, baseline_dir),
            ("developer_enabled", True, enabled_dir),
        ):
            child = launch(binary, run_dir, developer=developer)
            children[name] = child
            metrics[name] = ProcessMetrics(child.pid, kernel32, psapi)
            if developer:
                descriptors[name] = wait_for_session(
                    child, run_dir / "registry", binary, bridge_binary
                )
            initial_geometry = set_client_size(child, user32, WINDOW_SIZE)
            run_data: dict[str, Any] = {
                "arguments": ["--solar-system", "--dev-interface"] if developer else ["--solar-system"],
                "pid": child.pid,
                "warmup_s": args.warmup_seconds,
                "window_after_normalization": initial_geometry,
                "samples": [],
            }
            started["runs"][name] = run_data
            before_warmup = window_geometry(child, user32)
            run_data["window_before_warmup"] = before_warmup
            if before_warmup["minimized"] or before_warmup["client_size_px"] != WINDOW_SIZE:
                run_data["window_before_warmup_normalization"] = set_client_size(
                    child, user32, WINDOW_SIZE
                )
                time.sleep(0.25)
                run_data["window_before_warmup_settled"] = require_client_size(
                    child, user32, WINDOW_SIZE, "before warmup normalization and settle"
                )
            time.sleep(args.warmup_seconds)
            if child.poll() is not None:
                raise RuntimeError(f"{name} app exited during warmup ({child.returncode})")
            warmup_geometry = window_geometry(child, user32)
            run_data["window_after_warmup_before_normalization"] = warmup_geometry
            if warmup_geometry["minimized"] or warmup_geometry["client_size_px"] != WINDOW_SIZE:
                run_data["window_after_warmup_normalization"] = set_client_size(
                    child, user32, WINDOW_SIZE
                )
                time.sleep(0.25)
            warmup_end_geometry = require_client_size(
                child, user32, WINDOW_SIZE, "after warmup normalization and settle"
            )
            run_data["window_after_warmup"] = warmup_end_geometry
            samples = run_data["samples"]
            for sample_index in range(args.samples):
                samples.append(
                    {"index": sample_index, **windows_sample(child, metrics[name], user32, args.sample_seconds, WINDOW_SIZE)}
                )
            if developer:
                run_data["session"] = {k: v for k, v in descriptors[name].items() if not k.startswith("_")}
                run_data["capabilities"] = descriptors[name].get("_capabilities")
            if developer:
                run_data["native_capture"] = capture_sample(
                    descriptors[name],
                    child,
                    metrics[name],
                    bridge_binary,
                    run_dir / "registry",
                    output,
                    user32,
                    WINDOW_SIZE,
                )
            try:
                shutdown[name] = close_owned_child(child, user32, args.close_timeout_seconds)
            except Exception as exc:
                shutdown[name] = {"exited": False, "error": str(exc), "pid": child.pid}
                raise
    except Exception as exc:
        primary_error = f"{type(exc).__name__}: {exc}"
        raise
    finally:
        for name, child in reversed(list(children.items())):
            if child.poll() is None and name not in shutdown:
                try:
                    shutdown[name] = close_owned_child(child, user32, args.close_timeout_seconds)
                except Exception as exc:  # preserve partial evidence; never force-kill a child
                    shutdown[name] = {"exited": False, "error": str(exc), "pid": child.pid}
            elif name not in shutdown:
                shutdown[name] = {"exited": True, "exit_code": child.returncode}
            metric = metrics.get(name)
            if metric is not None:
                metric.close()
        started["shutdown"] = shutdown
        cleanup_failed = any(not item.get("exited") for item in shutdown.values())
        started["status"] = "failed" if primary_error or cleanup_failed else "complete"
        if primary_error is not None:
            started["primary_error"] = primary_error
        output_path = output / "measurement.json"
        output_path.write_text(json.dumps(started, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if any(not item.get("exited") for item in started.get("shutdown", {}).values()):
        raise RuntimeError(f"one or more owned apps did not exit gracefully; see {output / 'measurement.json'}")
    return started


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path, help="developer-tools mundaris_app executable")
    parser.add_argument(
        "--bridge-binary",
        type=Path,
        help="mundaris_dev executable (defaults to mundaris_dev.exe beside --binary)",
    )
    parser.add_argument("--output", required=True, type=Path, help="new or empty measurement directory")
    parser.add_argument("--samples", type=int, default=3, help="measurement intervals per mode (default: 3)")
    parser.add_argument("--sample-seconds", type=float, default=2.0, help="duration of each interval (default: 2s)")
    parser.add_argument("--warmup-seconds", type=float, default=3.0, help="warm-up per process (default: 3s)")
    parser.add_argument("--close-timeout-seconds", type=float, default=30.0, help="WM_CLOSE exit wait (default: 30s)")
    args = parser.parse_args()
    if args.samples < 1 or args.samples > 20:
        parser.error("--samples must be in 1..=20")
    if not (0.1 <= args.sample_seconds <= 300.0):
        parser.error("--sample-seconds must be in 0.1..=300")
    if not (0.0 <= args.warmup_seconds <= 300.0):
        parser.error("--warmup-seconds must be in 0..=300")
    if not (1.0 <= args.close_timeout_seconds <= 300.0):
        parser.error("--close-timeout-seconds must be in 1..=300")
    return args


def main() -> int:
    args = parse_args()
    try:
        result = run_measurement(args)
    except Exception as exc:
        print(f"measurement failed: {exc}", file=sys.stderr)
        return 1
    print(json.dumps({"measurement": str(args.output.resolve() / "measurement.json"), "status": "complete"}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
