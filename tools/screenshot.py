# /// script
# requires-python = ">=3.10"
# dependencies = ["pillow>=10"]
# ///
"""Screenshot the MISO Terminal window (Windows only).

    uv run tools/screenshot.py docs/screenshots/home.png
    uv run tools/screenshot.py docs/screenshots/gp.png --run "GP MINN.HUB" --wait 30
    uv run tools/screenshot.py docs/screenshots/light.png --run "THEME everforge-light" --offline

Starts the app in portable mode under a throwaway home directory, so your real
config, layout and cache are never touched. Captures only the app's own window
with PrintWindow (never the whole screen) and then closes the app.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wt
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
TITLE = "MISO Terminal"

user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32
user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))  # per-monitor v2


def find_window(pid: int) -> int | None:
    found: list[int] = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        owner = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            buf = ctypes.create_unicode_buffer(256)
            user32.GetWindowTextW(hwnd, buf, 256)
            if buf.value == TITLE:
                found.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    return found[0] if found else None


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", wt.DWORD), ("biWidth", wt.LONG), ("biHeight", wt.LONG), ("biPlanes", wt.WORD),
        ("biBitCount", wt.WORD), ("biCompression", wt.DWORD), ("biSizeImage", wt.DWORD),
        ("biXPelsPerMeter", wt.LONG), ("biYPelsPerMeter", wt.LONG), ("biClrUsed", wt.DWORD),
        ("biClrImportant", wt.DWORD),
    ]


def capture(hwnd: int) -> Image.Image:
    """PrintWindow with PW_RENDERFULLCONTENT: works for GPU-rendered windows,
    even partly covered ones, and never captures anything else on screen."""
    r = wt.RECT()
    user32.GetClientRect(hwnd, ctypes.byref(r))
    w, h = r.right - r.left, r.bottom - r.top
    wnd_dc = user32.GetWindowDC(hwnd)
    mem_dc = gdi32.CreateCompatibleDC(wnd_dc)
    bmp = gdi32.CreateCompatibleBitmap(wnd_dc, w, h)
    gdi32.SelectObject(mem_dc, bmp)
    PW_CLIENTONLY, PW_RENDERFULLCONTENT = 1, 2
    user32.PrintWindow(hwnd, mem_dc, PW_CLIENTONLY | PW_RENDERFULLCONTENT)
    header = BITMAPINFOHEADER(ctypes.sizeof(BITMAPINFOHEADER), w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
    buf = ctypes.create_string_buffer(w * h * 4)
    gdi32.GetDIBits(mem_dc, bmp, 0, h, buf, ctypes.byref(header), 0)
    gdi32.DeleteObject(bmp)
    gdi32.DeleteDC(mem_dc)
    user32.ReleaseDC(hwnd, wnd_dc)
    return Image.frombuffer("RGBA", (w, h), buf, "raw", "BGRA", 0, 1).convert("RGB")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("out", type=Path)
    ap.add_argument("--exe", type=Path, default=ROOT / "target" / "release" / "miso-terminal.exe")
    ap.add_argument("--run", action="append", default=[], help="command to run at startup (repeatable)")
    ap.add_argument("--wait", type=float, default=20.0, help="seconds to let data load")
    ap.add_argument("--offline", action="store_true", help="replay fixtures instead of calling MISO")
    args = ap.parse_args()

    exe = args.exe if args.exe.exists() else ROOT / "target" / "debug" / "miso-terminal.exe"
    if not exe.exists():
        print("build the app first: cargo build --release", file=sys.stderr)
        return 1
    with tempfile.TemporaryDirectory(prefix="mt-shot-") as home:
        cmd = [str(exe), "--home", home]
        if args.offline:
            cmd.append("--offline")
        for c in args.run:
            cmd += ["--run", c]
        proc = subprocess.Popen(cmd, cwd=ROOT, creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            hwnd = None
            for _ in range(120):
                time.sleep(0.25)
                hwnd = find_window(proc.pid)
                if hwnd:
                    break
            if not hwnd:
                print("the app window never appeared", file=sys.stderr)
                return 1
            time.sleep(args.wait)
            args.out.parent.mkdir(parents=True, exist_ok=True)
            capture(hwnd).save(args.out)
            print(f"wrote {args.out}")
        finally:
            proc.terminate()
            proc.wait(timeout=10)
    return 0


if __name__ == "__main__":
    sys.exit(main())
