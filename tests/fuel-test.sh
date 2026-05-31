#!/usr/bin/env bash
# Integration test: tight-loop .wasm is fuel-killed; kernel survives.
#
# Boots ruos headless under QEMU, runs /bin/spinloop.wasm over SSH.
# spinloop makes NO host calls after WASI startup — it burns through the
# 2_000_000_000 fuel budget in a tight compute loop and must be killed by
# the wasmi fuel meter.  After the kill the kernel must still answer a
# second SSH command (proving it kept serving).
#
# PASS criteria (both must hold):
#   1. serial.log contains: wasm: task killed (fuel exhausted)
#   2. A second SSH command (`ls /bin | wc -l`) returns a numeric count.
#
# Print TEST_PASS_FUEL on success, TEST_FAIL_FUEL otherwise.
# Run from the repo root: bash tests/fuel-test.sh
set -u
cd "$(dirname "$0")/.."

ISO=build/os.iso
DISK=build/disk.img
KEY=build/id_ed25519
PORT=2222
SERIAL=build/fuel-serial.log
SPINLOG=build/fuel-spin.log
SURVIVELOG=build/fuel-survive.log

# Kill any stray QEMU from prior runs and wait for the port to free up.
for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
for _ in $(seq 1 30); do ss -ltn 2>/dev/null | grep -q ":$PORT " && sleep 1 || break; done

# OpenSSH refuses world-readable private keys; stage a 0600 copy.
cp "$KEY" /tmp/ruos_id && chmod 600 /tmp/ruos_id

rm -f "$SERIAL" "$SPINLOG" "$SURVIVELOG"
timeout 120 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 512 -device qemu-xhci \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:$PORT-:22 \
  -device virtio-net-pci,netdev=net0 \
  -drive file="$DISK",format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
  > "$SERIAL" 2>&1 &
QEMUPID=$!
sleep 15

SSH_OPTS="-T -p $PORT -i /tmp/ruos_id -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=10"

# Step 1: run spinloop — expect it to be killed by fuel (exit non-zero).
# We give it up to 60 s; the fuel kill should happen within a few seconds on
# a release-profile build (2_000_000_000 instructions at ~500M wasmi ops/s).
echo "--- SSH: running /bin/spinloop.wasm (expect fuel-kill) ---"
timeout 60 ssh $SSH_OPTS root@127.0.0.1 '/bin/spinloop.wasm' </dev/null \
  > "$SPINLOG" 2>/dev/null || true
echo "spinloop exited ($(wc -c < "$SPINLOG") bytes of output)"

# Step 2: prove kernel is still alive after the kill.
echo "--- SSH: survival check (ls /bin | wc -l) ---"
timeout 20 ssh $SSH_OPTS root@127.0.0.1 'ls /bin | wc -l' </dev/null \
  > "$SURVIVELOG" 2>/dev/null || true
echo "survival output ($(wc -c < "$SURVIVELOG") bytes):"
cat -v "$SURVIVELOG"

sleep 2
kill "$QEMUPID" 2>/dev/null || true
wait "$QEMUPID" 2>/dev/null || true

echo "--- verdict ---"
ok=1

# Criterion 1: kernel logged the fuel-kill message.
grep -qF "wasm: task killed (fuel exhausted)" "$SERIAL" \
  || { echo "FAIL: no fuel-kill marker in serial.log"; ok=0; tail -20 "$SERIAL"; }

# Criterion 2: kernel answered the survival check with a numeric count.
if ! tr -d '\r' < "$SURVIVELOG" | grep -qE '^[[:space:]]*[0-9]+[[:space:]]*$'; then
  echo "FAIL: kernel did not survive (no numeric count in survival response)"
  ok=0
fi

if [ "$ok" -eq 1 ]; then
  echo TEST_PASS_FUEL
  exit 0
else
  echo TEST_FAIL_FUEL
  exit 1
fi
