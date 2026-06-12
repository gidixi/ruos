#!/usr/bin/env bash
# MT Fase 1: parallel window frame() dispatch test.
#
# Proves that ≥2 windows' frame() ran on ≥2 DISTINCT cores in the same frame
# when the compositor dispatches frame() across the SMP compute pool (the
# default, parallel build). The compositor-init script launches two reactor
# windows that stay awake, so by frame 30 there are ≥2 awake windows dispatched
# in parallel; the wm boot marker "frame cores=N [..]" reports the distinct
# cores that ran a frame() job that frame. The serial build (wm-serial-frames)
# is checked as a regression baseline: it must report exactly 1 core.
set -u
cd "$(dirname "$0")/.."

ISO_PAR=build/frametest.iso
ISO_SER=build/frametest-serial.iso
LOG_PAR=build/frame-smp-par.log
LOG_SER=build/frame-smp-ser.log
INIT=user-bin/compositor-init.sh

kill_qemu() {
  ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do
    kill -9 "$p" 2>/dev/null || true
  done
}

boot() {
  # $1 = iso, $2 = serial log, $3 = extra cargo features (passed at build time)
  local iso="$1" slog="$2"
  rm -f "$slog"
  timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 2048 \
    -cdrom "$iso" -serial "file:$slog" -display none -no-reboot \
    -device qemu-xhci >/dev/null 2>&1
  kill_qemu
}

kill_qemu
sleep 1

# --- Parallel build (default) -------------------------------------------------
echo "[frame-smp] building PARALLEL iso ($ISO_PAR)..."
make iso INIT_SCRIPT="$INIT" ISO="$ISO_PAR" > build/frametest-par-iso.log 2>&1 || {
  echo TEST_FAIL_FRAME_SMP; echo "(parallel iso build failed)"; tail -20 build/frametest-par-iso.log; exit 1;
}
echo "[frame-smp] booting PARALLEL iso (-smp 4)..."
boot "$ISO_PAR" "$LOG_PAR"

PAR_LINE=$(grep -aE "frame cores=" "$LOG_PAR" | tail -1)
PAR_N=$(printf '%s' "$PAR_LINE" | sed -nE 's/.*frame cores=([0-9]+).*/\1/p')
PAR_N=${PAR_N:-0}
echo "[frame-smp] parallel: $PAR_LINE"
echo "[frame-smp] parallel distinct_cores=$PAR_N"

# --- Serial-frames build (regression baseline) -------------------------------
echo "[frame-smp] building SERIAL iso ($ISO_SER, CARGO_FEATURES=wm-serial-frames)..."
make iso INIT_SCRIPT="$INIT" ISO="$ISO_SER" CARGO_FEATURES=wm-serial-frames \
  > build/frametest-ser-iso.log 2>&1 || {
  echo TEST_FAIL_FRAME_SMP; echo "(serial iso build failed)"; tail -20 build/frametest-ser-iso.log; exit 1;
}
echo "[frame-smp] booting SERIAL iso (-smp 4)..."
boot "$ISO_SER" "$LOG_SER"

SER_LINE=$(grep -aE "frame cores=" "$LOG_SER" | tail -1)
SER_N=$(printf '%s' "$SER_LINE" | sed -nE 's/.*frame cores=([0-9]+).*/\1/p')
SER_N=${SER_N:-0}
echo "[frame-smp] serial: $SER_LINE"
echo "[frame-smp] serial distinct_cores=$SER_N"

# --- Assertions ---------------------------------------------------------------
FAIL=0
[ "$PAR_N" -ge 2 ] 2>/dev/null || { echo "(parallel distinct_cores=$PAR_N < 2)"; FAIL=1; }
[ "$SER_N" -eq 1 ] 2>/dev/null || { echo "(serial distinct_cores=$SER_N != 1)"; FAIL=1; }

if [ "$FAIL" -eq 0 ]; then
  echo "TEST_PASS_FRAME_SMP"
  exit 0
else
  echo "TEST_FAIL_FRAME_SMP"
  echo "--- parallel log tail ---"; tail -15 "$LOG_PAR"
  exit 1
fi
