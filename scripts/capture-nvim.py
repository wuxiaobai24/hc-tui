#!/usr/bin/env python3
"""Record clean Neovim redraws on synthetic text for deterministic VT replay."""
import base64
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time


def record(root):
    sample = root / "sample.txt"
    sample.write_text("\n".join(
        f"{number:03d} 中文选区 alpha beta => λ 👋 é" for number in range(1, 151)
    ) + "\n")
    pid, descriptor = pty.fork()
    if pid == 0:
        os.environ.update(TERM="xterm-256color", COLORTERM="truecolor")
        os.execvp("nvim", [
            "nvim", "--clean", "-u", "NONE", "-i", "NONE", "-n",
            "--cmd", "set termguicolors", str(sample), "-c",
            "set number cursorline | hi Normal guifg=#cdd6f4 guibg=#1e1e2e | "
            "hi Visual guibg=#465570 | hi CursorLine guibg=#303044",
        ])
    frames = []

    def resize(columns, rows):
        fcntl.ioctl(descriptor, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))

    def capture(label, columns, rows, keys=b""):
        if keys:
            os.write(descriptor, keys)
        data = bytearray()
        started = last_output = time.monotonic()
        while time.monotonic() - started < 4:
            readable, _, _ = select.select([descriptor], [], [], 0.05)
            if readable:
                try:
                    block = os.read(descriptor, 65536)
                except OSError:
                    break
                if not block:
                    break
                data.extend(block)
                last_output = time.monotonic()
            if data and time.monotonic() - last_output > 0.3 and time.monotonic() - started > 0.5:
                break
        if not data:
            raise RuntimeError(f"nvim produced no output for {label}")
        frames.append(dict(label=label, cols=columns, rows=rows, data=base64.b64encode(data).decode()))

    try:
        resize(80, 24)
        capture("open with truecolor and wide characters", 80, 24)
        capture("visual selection across CJK", 80, 24, b"0llllvllllllll")
        capture("scroll a full screen", 80, 24, b"\x1b\x04\x04")
        capture("vertical split", 80, 24, b":vsplit\r")
        resize(96, 28)
        capture("resize wider", 96, 28)
        capture("replace a line after insert-mode typing", 96, 28, b"ggiREPLAY ascii abc\x1b")
        capture("redraw and move to end", 96, 28, b":redraw!\rG")
        resize(62, 19)
        capture("resize narrower", 62, 19)
        capture("leave alternate screen", 62, 19, b":qa!\r")
    finally:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        os.waitpid(pid, 0)
        os.close(descriptor)
    return frames


def main():
    version = subprocess.check_output(["nvim", "--version"], text=True).splitlines()[0]
    with tempfile.TemporaryDirectory(prefix="hctui-nvim-") as temporary:
        frames = record(Path(temporary))
    destination = Path(__file__).resolve().parents[1] / "tests/fixtures/nvim-redraw.json"
    destination.write_text(json.dumps({
        "source": f"{version}, --clean -u NONE; synthetic multilingual buffer",
        "frames": frames,
    }, ensure_ascii=False, indent=2) + "\n")
    print(destination)
    for frame in frames:
        print(frame["label"], len(base64.b64decode(frame["data"])), "bytes")


if __name__ == "__main__":
    main()
