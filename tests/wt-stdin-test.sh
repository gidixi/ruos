#!/usr/bin/env bash
# Stdin on the Wasmtime runtime (.cwasm), deterministic boot-marker check.
#
# Boots an ISO whose init launches `wtreadline` on the boot PTY. With no live
# input the tool must still:
#   1. instantiate + run        -> prints WT_READLINE_READY
#   2. reach the blocking stdin read on a ComputeApp core (fd_read fd 0 ->
#      pty::slave_read_blocking), i.e. it BLOCKS instead of exiting — so the
#      init's trailing marker WTSTDIN_INIT_DONE must NOT appear (proof the read
#      path is live, not the old EOF stub).
# -smp 4 so the .cwasm runs on an AP (stdin needs SMP; the BSP pumps the
# line discipline). The full echo round-trip (type a line, get STDIN_ECHO:)
# is verified manually (`make run`, then type) — interactive input over a
# headless harness is environment-fragile.
set -u
cd "$(dirname "$0")/.."

ISO=build/wtstdin.iso
LOG=build/wtstdin-marker.log

for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1

echo "[wt-stdin] building iso (init = wtreadline)..."
make iso ISO="$ISO" INIT_SCRIPT=user-bin/wtstdin-init.sh > build/wtstdin-iso.log 2>&1 || {
  echo TEST_FAIL_WT_STDIN; echo "(iso build failed)"; tail -20 build/wtstdin-iso.log; exit 1; }

echo "[wt-stdin] booting -smp 4 (wtreadline blocks on stdin)..."
rm -f "$LOG"
timeout 40 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 1024 -cdrom "$ISO" \
  -serial file:"$LOG" -display none -no-reboot -device qemu-xhci >/dev/null 2>&1
for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done

grep -aE "WT_READLINE_READY|WTSTDIN_INIT_DONE" "$LOG" | sed 's/^/[wt-stdin] /'

FAIL=0
grep -aq "WT_READLINE_READY" "$LOG" || { echo "(no WT_READLINE_READY — tool did not run)"; FAIL=1; }
# Must NOT have finished: a live blocking read keeps wtreadline alive, so the
# init never reaches the next line. If DONE appears, fd 0 returned EOF (the
# old stub) instead of blocking.
grep -aq "WTSTDIN_INIT_DONE" "$LOG" && { echo "(WTSTDIN_INIT_DONE present — stdin returned EOF, read path not active)"; FAIL=1; }

if [ "$FAIL" -eq 0 ]; then echo TEST_PASS_WT_STDIN; exit 0; else echo TEST_FAIL_WT_STDIN; tail -15 "$LOG"; exit 1; fi
