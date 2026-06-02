#!/usr/bin/env bash
# USB hot-plug (add/remove at runtime) test.
#
# Boots QEMU with an xHCI controller but NO keyboard attached at boot, plus a QMP
# control socket. After the boot shell is up (raw-mode line editor on PTY 0, which
# echoes typed chars to the serial console via console_drain), it hot-plugs a USB
# keyboard at runtime via QMP `device_add`, types a unique token "usbhp\n" through
# `send-key`, then hot-unplugs it via QMP `device_del` and types "zzz".
#
# If hot-plug enumeration works, the kernel attaches the keyboard after device_add
# and the shell echoes "usbhp" to serial. If hot-unplug (device removal) works,
# the keyboard is gone afterwards and "zzz" is NOT delivered/echoed.
#
# PASS = token "usbhp" appears in serial (plugged kbd worked) AND token "zzz"
#        does NOT appear (kbd removed cleanly, no further input).
#
# Run from repo root: bash tests/usb-hotplug-test.sh
set -u
cd "$(dirname "$0")/.."
ISO=build/os.iso; SERIAL=build/serial.log; QMP=/tmp/ruos-qmp.sock

for p in $(pgrep -f qemu-system-x86_64); do kill -9 "$p" 2>/dev/null || true; done
sleep 1
rm -f "$SERIAL" "$QMP"

# xHCI controller with an id (xhci.0 bus) but no usb-kbd at boot — we add it later.
timeout 60 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 512 \
  -device qemu-xhci,id=xhci \
  -qmp unix:$QMP,server,nowait > "$SERIAL" 2>&1 &
QP=$!
sleep 16   # let boot finish + the shell prompt come up

# Drive QMP: capabilities handshake; hot-plug a usb-kbd on xhci.0 port 2; type the
# token "usbhp" + Return; hot-unplug it; type "zzz" (should go nowhere). QEMU qcode
# names: letters are their letter; Return = "ret".
python3 - "$QMP" <<'PY'
import socket, json, sys, time
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); f = s.makefile('rw')
f.readline()                                  # greeting
f.write(json.dumps({"execute":"qmp_capabilities"})+"\n"); f.flush(); f.readline()
def cmd(obj):
    f.write(json.dumps(obj)+"\n"); f.flush(); return f.readline()
def key(q):
    cmd({"execute":"send-key","arguments":{"keys":[{"type":"qcode","data":q}]}})
    time.sleep(0.15)
# 1) hot-plug the keyboard on controller port 2
cmd({"execute":"device_add","arguments":
     {"driver":"usb-kbd","id":"k1","bus":"xhci.0","port":"2"}})
time.sleep(1.0)                               # let the kernel enumerate the new dev
# 2) type the token while plugged
for q in ["u","s","b","h","p","ret"]:
    key(q)
time.sleep(0.5)
# 3) hot-unplug the keyboard
cmd({"execute":"device_del","arguments":{"id":"k1"}})
time.sleep(1.0)                               # let the kernel tear it down
# 4) type after removal — must NOT be delivered/echoed
for q in ["z","z","z","ret"]:
    key(q)
time.sleep(0.5)
PY

sleep 2
kill -9 "$QP" 2>/dev/null || true

if grep -qF "usbhp" "$SERIAL" && ! grep -qF "zzz" "$SERIAL"; then
    echo TEST_PASS_HOTPLUG
else
    echo "TEST_FAIL_HOTPLUG (usbhp missing or zzz leaked after device_del)"
    echo "--- tail of serial ---"; tail -8 "$SERIAL" | cat -v
fi
