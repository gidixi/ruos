#!/usr/bin/env bash
# MT Fase 2: wasm-threads gate test.
#
# Builds the boot-checks ISO and asserts the four thread-runtime markers:
#   THREADS-OK 1        — SharedMemory + native atomics (no_std wasmtime fork)
#   THREADS-FIBER-OK    — fiber suspend/resume cross-core (host-only)
#   THREADS-OK 3        — atomic.wait suspends the FIBER, notify wakes via IPI
#   THREADS-OK 2        — thread-spawn: fresh Instance on the same SharedMemory
# Boots -smp 4 (ComputeApp cores) AND -smp 1 (BSP fallback): the gates must
# pass on both — the single-core boot is the deadlock regression.
set -u
cd "$(dirname "$0")/.."

ISO_T=build/threadstest.iso
LOG4=build/threads-smp4.log
LOG1=build/threads-smp1.log

kill_qemu() {
  ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do
    kill -9 "$p" 2>/dev/null || true
  done
}

boot() {
  # $1 = -smp count, $2 = serial log
  local smp="$1" slog="$2"
  rm -f "$slog"
  timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp "$smp" -m 2048 \
    -cdrom "$ISO_T" -serial "file:$slog" -display none -no-reboot \
    -device qemu-xhci >/dev/null 2>&1
  kill_qemu
}

check_log() {
  # $1 = log, $2 = label. Echoes failures; returns count.
  local log="$1" label="$2" fail=0
  for m in "THREADS-OK 1 = ok" "THREADS-FIBER-OK = ok" \
           "THREADS-OK 3 = ok" "THREADS-OK 2 = ok"; do
    grep -aq "$m" "$log" || { echo "($label: missing '$m')"; fail=1; }
  done
  return $fail
}

kill_qemu
sleep 1

echo "[threads] building boot-checks iso ($ISO_T)..."
make wt-cwasm > build/threadstest-iso.log 2>&1 || true
make iso ISO="$ISO_T" CARGO_FEATURES=boot-checks >> build/threadstest-iso.log 2>&1 || {
  echo TEST_FAIL_THREADS; echo "(iso build failed)"; tail -20 build/threadstest-iso.log; exit 1;
}

echo "[threads] booting -smp 4..."
boot 4 "$LOG4"
echo "[threads] booting -smp 1 (BSP fallback)..."
boot 1 "$LOG1"

FAIL=0
check_log "$LOG4" "smp4" || FAIL=1
check_log "$LOG1" "smp1" || FAIL=1
grep -a "THREADS" "$LOG4" | sed 's/^/[threads] smp4: /'
grep -a "THREADS" "$LOG1" | sed 's/^/[threads] smp1: /'

if [ "$FAIL" -eq 0 ]; then
  echo "TEST_PASS_THREADS"
  exit 0
else
  echo "TEST_FAIL_THREADS"
  echo "--- smp4 log tail ---"; tail -15 "$LOG4"
  exit 1
fi
