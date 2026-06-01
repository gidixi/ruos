#!/usr/bin/env bash
# Interactive test for rtop's timer-driven refresh + clean quit.
#
# Boots the ISO under QEMU with an SSH port-forward, opens an interactive
# `ssh -tt` session, launches `rtop`, then waits ~3.5 s WITHOUT pressing a key.
# If poll_stdin's read-vs-timer race works, rtop must auto-refresh (~1 Hz),
# emitting several frames. Then we send 'q' and expect an instant clean exit:
# the alt-screen is left and the shell prompt returns.
#
# PASS =
#   - alt-screen ENTER  (\x1b[?1049h) seen  -> rtop started in raw mode
#   - >= 3 frame redraws while idle          -> timer refresh works
#   - alt-screen LEAVE  (\x1b[?1049l) seen   -> 'q' quit + terminal restored
#   - prompt `ruos:/$` returns               -> shell alive after rtop
#
# Run from repo root: bash tests/rtop-ssh-test.sh
set -u
cd "$(dirname "$0")/.."

ISO=build/os.iso
DISK=build/disk.img
KEY=build/id_ed25519
PORT=2222
SERIAL=build/serial.log
CLIENT=build/rtop-ssh-client.log

for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
for _ in $(seq 1 30); do ss -ltn 2>/dev/null | grep -q ":$PORT " && sleep 1 || break; done

cp "$KEY" /tmp/ruos_id && chmod 600 /tmp/ruos_id
rm -f "$SERIAL" "$CLIENT"

timeout 70 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 512 -device qemu-xhci \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:$PORT-:22 \
  -device virtio-net-pci,netdev=net0 \
  -drive file="$DISK",format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
  > "$SERIAL" 2>&1 &
QEMUPID=$!
sleep 15

echo "--- ssh -tt: run rtop, idle 3.5s (expect auto-refresh), then 'q' ---"
# No newline after 'q': rtop reads it as a raw keystroke and exits immediately.
( printf 'rtop\n'; sleep 4; printf 'q'; sleep 2; printf 'exit\n'; sleep 1 ) | \
  timeout 30 ssh -tt -p "$PORT" -i /tmp/ruos_id \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o ConnectTimeout=5 root@127.0.0.1 > "$CLIENT" 2>/dev/null || true

kill -9 "$QEMUPID" 2>/dev/null || true

echo "client bytes: $(wc -c < "$CLIENT")"

fail() { echo "TEST_FAIL_RTOP_$1"; exit 1; }

grep -qF $'\x1b[?1049h' "$CLIENT" || fail ALTSCREEN_ENTER
grep -qF $'\x1b[?1049l' "$CLIENT" || fail ALTSCREEN_LEAVE
# Count frame redraws. ratatui writes each cell with its own cursor-move, so
# the on-screen text is never contiguous in the byte stream — we instead count
# the hide-cursor (\x1b[?25l) that ratatui emits at the END of every draw().
# One belongs to the alt-screen enter; idle 3.5 s at ~1 Hz adds >=3 more.
frames=$(grep -oF $'\x1b[?25l' "$CLIENT" | wc -l)
echo "draw frames (hide-cursor count): $frames"
[ "$frames" -ge 3 ] || fail NO_AUTOREFRESH
grep -qF 'ruos:/$' "$CLIENT" || fail NO_PROMPT_AFTER

echo TEST_PASS_RTOP
