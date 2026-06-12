#!/usr/bin/env bash
# SP4 parallel-vs-serial compositing equivalence test.
#
# Proves that the SMP-banded parallel composite is byte-identical to a serial
# (n_bands=1) reference composite, AND that >=2 distinct cores actually ran band
# jobs in the parallel build. The reactor apps are static at boot (no input), so
# a screendump after warm-up is deterministic and band-split-independent — a
# correct band split therefore renders the EXACT same framebuffer as the serial
# reference. Any horizontal seam at a screen_h/n_bands boundary (a row composited
# twice, skipped, or a footprint not clipped to [band_y0,band_y1)) would make the
# two PNGs differ.
set -u
cd "$(dirname "$0")/.."

QMP=/tmp/qmp.sock
ISO_PAR=build/comptest.iso
ISO_SER=build/comptest-serial.iso
SHOT_PAR=build/shot-parallel.png
SHOT_SER=build/shot-serial.png
SERIAL=build/comp-smp-serial.log
INIT=user-bin/compositor-init.sh
WAIT=16

kill_qemu() {
  ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do
    kill -9 "$p" 2>/dev/null || true
  done
  rm -f "$QMP" 2>/dev/null || true
}

boot_and_shot() {
  # $1 = iso path, $2 = output png, $3 = optional serial log (else /dev/null)
  local iso="$1" out="$2" slog="${3:-/dev/null}"
  rm -f "$out" "$QMP"
  # -m 2048: HEAP_SIZE bumped to 768 MiB (memory/heap.rs) needs a contiguous
  # USABLE region ≥ 768 MiB; -m 512 fails HeapInit("no usable region"). Match the
  # GUI run target (the JS-viewer heap bump that left this script stale).
  timeout 180 qemu-system-x86_64 -machine q35,accel=kvm:tcg -cpu max -smp 4 \
    -cdrom "$iso" -serial "file:$slog" -display none -no-reboot -m 2048 \
    -device qemu-xhci \
    -qmp "unix:$QMP,server,nowait" &
  local qp=$!
  python3 build/comp_shot.py "$QMP" "$out" "$WAIT"
  # comp_shot.py quits the VM; give QEMU a moment, then make sure it's gone.
  wait "$qp" 2>/dev/null || true
  kill_qemu
}

kill_qemu
sleep 1

# --- Parallel build + screendump ------------------------------------------
echo "[comp-smp] building PARALLEL iso ($ISO_PAR)..."
make iso INIT_SCRIPT="$INIT" ISO="$ISO_PAR" > build/comptest-par-iso.log 2>&1 || {
  echo TEST_FAIL_COMP_SMP; echo "(parallel iso build failed)"; tail -20 build/comptest-par-iso.log; exit 1;
}
echo "[comp-smp] booting PARALLEL iso (-smp 4) + screendump..."
boot_and_shot "$ISO_PAR" "$SHOT_PAR" "$SERIAL"

# Distinct cores that ran band jobs (the wm boot marker: "composite cores=N [..]").
CORES_LINE=$(grep -aE "composite cores=" "$SERIAL" | tail -1)
NCORES=$(printf '%s' "$CORES_LINE" | sed -nE 's/.*composite cores=([0-9]+).*/\1/p')
NCORES=${NCORES:-0}
echo "[comp-smp] $CORES_LINE"
echo "[comp-smp] distinct_cores=$NCORES"

# --- Serial reference build + screendump ----------------------------------
echo "[comp-smp] building SERIAL-reference iso ($ISO_SER, CARGO_FEATURES=serial-composite)..."
make iso INIT_SCRIPT="$INIT" ISO="$ISO_SER" CARGO_FEATURES=serial-composite \
  > build/comptest-ser-iso.log 2>&1 || {
  echo TEST_FAIL_COMP_SMP; echo "(serial iso build failed)"; tail -20 build/comptest-ser-iso.log; exit 1;
}
echo "[comp-smp] booting SERIAL iso (-smp 4) + screendump..."
boot_and_shot "$ISO_SER" "$SHOT_SER"

# --- Assertions ------------------------------------------------------------
if cmp -s "$SHOT_PAR" "$SHOT_SER"; then
  IDENT=yes
else
  IDENT=no
fi
echo "[comp-smp] screendump_identical=$IDENT"
echo "[comp-smp] parallel=$SHOT_PAR serial=$SHOT_SER"

FAIL=0
[ "$NCORES" -ge 2 ] 2>/dev/null || { echo "(distinct_cores=$NCORES < 2)"; FAIL=1; }
[ "$IDENT" = yes ] || { echo "(screendumps differ — band-boundary seam suspected)"; FAIL=1; }

if [ "$FAIL" -eq 0 ]; then
  echo "TEST_PASS_COMP_SMP"
  exit 0
else
  echo "TEST_FAIL_COMP_SMP"
  echo "--- serial log tail ---"; tail -20 "$SERIAL"
  exit 1
fi
