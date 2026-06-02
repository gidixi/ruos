#!/usr/bin/env bash
# USB HID keyboard keystroke test.
#
# Boots QEMU with `-device qemu-xhci -device usb-kbd` and a QMP control socket.
# After the boot shell is up (raw-mode line editor on PTY 0, which echoes typed
# chars to the serial console via console_drain), it sends a unique key sequence
# "usbkey\n" through QMP `send-key` (which QEMU delivers as USB HID reports).
# If the full USB stack works (xHCI interrupt endpoint -> on_report -> usage map
# -> master_input_push(0)), the shell echoes the string and it appears on serial.
#
# PASS = the typed token "usbkey" appears in the serial log (echoed by the shell).
#
# Run from repo root: bash tests/usb-key-test.sh
set -u
cd "$(dirname "$0")/.."
ISO=build/os.iso; SERIAL=build/serial.log; QMP=/tmp/ruos-qmp.sock

for p in $(pgrep -f qemu-system-x86_64); do kill -9 "$p" 2>/dev/null || true; done
sleep 1
rm -f "$SERIAL" "$QMP"

timeout 60 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 512 \
  -device qemu-xhci -device usb-kbd \
  -qmp unix:$QMP,server,nowait > "$SERIAL" 2>&1 &
QP=$!
sleep 16   # let boot finish + the shell prompt come up

# Drive QMP: capabilities handshake, then send-key for each char of "usbkey"
# plus Return. QEMU qcode names: letters are their letter; Return = "ret".
python3 - "$QMP" <<'PY'
import socket, json, sys, time
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); f = s.makefile('rw')
f.readline()                                  # greeting
f.write(json.dumps({"execute":"qmp_capabilities"})+"\n"); f.flush(); f.readline()
def key(q):
    f.write(json.dumps({"execute":"send-key","arguments":
        {"keys":[{"type":"qcode","data":q}]}})+"\n"); f.flush(); f.readline()
    time.sleep(0.15)
for q in ["u","s","b","k","e","y","ret"]:
    key(q)
time.sleep(0.5)
PY

sleep 2
kill -9 "$QP" 2>/dev/null || true

if grep -qF "usbkey" "$SERIAL"; then
    echo TEST_PASS_USBKEY
else
    echo "TEST_FAIL_USBKEY (token not echoed)"
    echo "--- tail of serial ---"; tail -5 "$SERIAL" | cat -v
fi
