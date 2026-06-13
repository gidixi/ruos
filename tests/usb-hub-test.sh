#!/usr/bin/env bash
# USB hub enumeration + keyboard-behind-a-hub test.
#
# Boots QEMU with an xHCI controller, a USB hub on controller port 1, and a USB
# keyboard behind the hub (hub port 1 -> "1.1"), present at boot. Also opens a QMP
# control socket. After the boot shell is up (raw-mode line editor on PTY 0, which
# echoes typed chars to the serial console via console_drain), it types a unique
# token "hubkbd\n" through QMP `send-key`.
#
# If the USB stack walks the hub (sets hub config, powers ports, reads port status,
# resets the child, assigns it a slot) AND the HID path works, the kernel logs a
# hub-enumeration marker ("usb hub slot=") and the shell echoes "hubkbd" to serial.
#
# PASS = serial contains the hub marker "usb hub slot=" (hub was enumerated) AND
#        the token "hubkbd" (keyboard behind the hub delivered input).
#
# Run from repo root: bash tests/usb-hub-test.sh
set -u
cd "$(dirname "$0")/.."
ISO=build/os.iso; SERIAL=build/serial.log; QMP=/tmp/ruos-qmp.sock

for p in $(pgrep -f qemu-system-x86_64); do kill -9 "$p" 2>/dev/null || true; done
sleep 1
rm -f "$SERIAL" "$QMP"

# Nested USB topology: xHCI (id=xhci -> bus xhci.0), a hub on controller port 1,
# and a keyboard behind that hub on hub port 1 (port "1.1").
timeout 60 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 2048 \
  -device qemu-xhci,id=xhci \
  -device usb-hub,id=h1,bus=xhci.0,port=1 \
  -device usb-kbd,id=k1,bus=xhci.0,port=1.1 \
  -qmp unix:$QMP,server,nowait > "$SERIAL" 2>&1 &
QP=$!
sleep 16   # let boot finish + the shell prompt come up

# Drive QMP: capabilities handshake, then send-key for each char of "hubkbd" plus
# Return. QEMU qcode names: letters are their letter; Return = "ret".
python3 - "$QMP" <<'PY'
import socket, json, sys, time
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); f = s.makefile('rw')
f.readline()                                  # greeting
f.write(json.dumps({"execute":"qmp_capabilities"})+"\n"); f.flush(); f.readline()
def key(q):
    f.write(json.dumps({"execute":"send-key","arguments":
        {"keys":[{"type":"qcode","data":q}]}})+"\n"); f.flush(); f.readline()
    time.sleep(0.15)
for q in ["h","u","b","k","b","d","ret"]:
    key(q)
time.sleep(0.5)
PY

sleep 2
kill -9 "$QP" 2>/dev/null || true

if grep -qF "usb hub slot=" "$SERIAL" && grep -qF "hubkbd" "$SERIAL"; then
    echo TEST_PASS_HUB
else
    echo "TEST_FAIL_HUB (hub marker missing or token not echoed)"
    echo "--- tail of serial ---"; tail -8 "$SERIAL" | cat -v
fi
