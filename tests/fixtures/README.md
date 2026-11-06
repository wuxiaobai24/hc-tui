# Terminal replay fixtures

`core-first-paint.bin` is the original core attach paint used by the style/column tests.

`nvim-redraw.json` records Neovim 0.12.5 with `--clean -u NONE -i NONE` on a synthetic multilingual buffer. The capture covers truecolor, CJK, emoji/combining marks, visual selection, scrolling, a vertical split, two terminal resizes, insert-mode edits, redraw and alternate-screen exit. No user configuration or source files are included.

Regenerate with `python3 scripts/capture-nvim.py` (requires nvim). Replays do not require nvim installed: the test streams these bytes in 31-byte chunks through TerminalView, draws with the real CrosstermBackend, reparses that output in a host VT and compares visible symbols, nonblank foregrounds and every background cell.

The synchronized-output test separately divides a clear/redraw across chunks to ensure no intermediate frame is exposed; its timeout prevents a stalled writer freezing the pane indefinitely. Replays are deterministic renderer evidence, not verification of every physical terminal's glyph metrics or every SSH/IME combination.
