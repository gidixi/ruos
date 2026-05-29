# 98 — PTY shell raw-mode line editor

**Data:** 2026-05-29

## Cosa
Rewrote `user/shell/src/main.rs` interactive loop to use raw-mode termios and a
full line editor. After init.sh runs and prints the sentinel, the shell sleeps 1s,
clears the screen, switches stdin to raw mode (ICANON|ECHO|ISIG off via
`tcsetattr`), and enters `read_line_raw`.

Line editor features:
- `\n`/`\r`: commit line, append to history.
- `0x7F`/`0x08` (Backspace/DEL): delete char before cursor.
- `Ctrl-A` (`0x01`): jump to line start.
- `Ctrl-E` (`0x05`): jump to line end.
- `Ctrl-L` (`0x0C`): clear screen, redraw prompt.
- `Ctrl-C` (`0x03`): discard current line, print `^C`.
- `\t` (Tab): tab completion against builtins + `/bin/*.wasm` via `ruos_readdir`.
- `ESC[A`/`ESC[B`: up/down history navigation.
- `ESC[C`/`ESC[D`: cursor right/left (intra-line navigation).
- Printable chars (`>= 0x20`): insert at cursor.

Termios saved at entry; restored on `exit` or EOF.

Added new `extern "C"` imports under `ruos` module: `readdir`, `tcgetattr`,
`tcsetattr`. The old `read_line` and `print_prompt` helpers were removed (unused).

Smoke test (`make run-test`) still exits `TEST_PASS` with sentinel
`shell: init.sh complete` hit before interactive mode — init.sh still runs in
cooked mode (kernel default before tcsetattr).

Manual VBox interactive smoke (arrows, history, tab) required as human verification
— subagent cannot drive interactive QEMU.

## Perché
Step 12 final task: shell must provide a proper line editor for interactive PTY
use, matching the terminal experience expected once SSH + GUI are in place.

## File toccati
- `user/shell/src/main.rs`
- `user-bin/shell.wasm`
