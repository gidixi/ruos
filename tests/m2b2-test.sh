#!/usr/bin/env bash
# M2b-2 boot-from-SSD end-to-end (the SSD-installer capstone). Two phases on ONE
# disk image, boot-marker-only (NO mtools — mtools on the WSL 9p /mnt/e mount is
# pathologically slow):
#   Phase 1 (install): boot the ISO (cdrom, BIOS/SeaBIOS) + a BLANK SATA disk +
#     an init that runs `install`. Blank disk ⇒ no /mnt ⇒ guard passes ⇒ install
#     authors the disk (GPT + FAT ESP + data) and copies the full boot tree onto
#     the ESP. Wait for `install: ok`.
#   Phase 2 (boot FROM the SSD under UEFI/OVMF, NO cdrom): OVMF auto-runs
#     /EFI/BOOT/BOOTX64.EFI (Limine) from the SSD's ESP → limine.conf → /boot/kernel
#     → ruos boots → M1 mounts /mnt from the SSD's data partition. The init copied
#     onto the SSD is the same one that ran `install`, but now /mnt is mounted, so
#     that `install` hits the guard (prevents a re-install loop) and boot continues.
#     Assert ruos booted from the SSD ("ruos boot OK" + "mnt mounted FAT") AND that
#     a tool runs ON-DEMAND off /mnt/bin: the init's `uname -a` is NOT on the slim
#     ESP, so resolve_path loads /mnt/bin/uname.wasm from the data partition. It
#     prints "ruos ruos 0.1.0 wasm-userland x86_64"; assert the uname-only token
#     "wasm-userland" (the boot banner never emits it).
set -u
cd "$(dirname "$0")/.."
IMG=build/m2b2-disk.img; S=build/serial.log
killq(){ ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 "$p" 2>/dev/null||true; done; }
killq; sleep 1
dd if=/dev/zero of="$IMG" bs=1M count=512 status=none
make iso INIT_SCRIPT=user-bin/m2b2-init.sh > build/m2b2-iso.log 2>&1 || { echo TEST_FAIL_ISO; tail -20 build/m2b2-iso.log; exit 1; }
# --- Phase 1: install (ISO boot + blank SATA disk) ---
timeout 220 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom build/os.iso -serial stdio -display none -no-reboot -m 2048 -device qemu-xhci \
  -drive file="$IMG",format=raw,if=none,id=d0 -device ahci,id=ahci -device ide-hd,drive=d0,bus=ahci.0 > "$S" 2>&1 & QP=$!
for _ in $(seq 1 100); do grep -qF "install: ok" "$S" && break; kill -0 $QP 2>/dev/null||break; sleep 2; done
killq; cp "$S" build/serial.m2b2p1.log
grep -qF "install: ok" build/serial.m2b2p1.log || { echo TEST_FAIL_INSTALL; tail -30 build/serial.m2b2p1.log; exit 1; }
sgdisk -p "$IMG" | grep -qi "EF00\|EFI System" || { echo TEST_FAIL_NO_ESP; sgdisk -p "$IMG"; exit 1; }
# --- Phase 2: boot FROM the SSD under UEFI (OVMF), NO cdrom ---
cp /usr/share/OVMF/OVMF_VARS_4M.fd build/ovmf_vars.fd
timeout 150 qemu-system-x86_64 -machine q35 -cpu max \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
  -drive if=pflash,format=raw,file=build/ovmf_vars.fd \
  -drive file="$IMG",format=raw,if=none,id=d0 -device ahci,id=ahci -device ide-hd,drive=d0,bus=ahci.0 \
  -serial stdio -display none -no-reboot -m 2048 -device qemu-xhci > "$S" 2>&1 & QP=$!
# Wait for the init script's LAST marker (`m2b2-installed`), not the kernel-level
# `mnt mounted FAT`: the kernel mounts /mnt (~T+1.0s) well before the executor
# spawns the shell that runs init (~T+1.5s), so keying on `mnt mounted FAT` would
# kill QEMU before init emits `ruos boot OK`. `m2b2-installed` is the final init
# line, so by the time it appears both asserted markers are guaranteed present.
for _ in $(seq 1 70); do grep -qF "m2b2-installed" "$S" && break; kill -0 $QP 2>/dev/null||break; sleep 2; done
killq; cp "$S" build/serial.m2b2p2.log
grep -qF "ruos boot OK" build/serial.m2b2p2.log || { echo TEST_FAIL_SSD_BOOT; tail -50 build/serial.m2b2p2.log; exit 1; }
grep -qF "mnt mounted FAT" build/serial.m2b2p2.log || { echo TEST_FAIL_SSD_MNT; tail -50 build/serial.m2b2p2.log; exit 1; }
# On-demand exec: the init's `uname -a` is absent from the slim ESP, so the shell
# resolved /mnt/bin/uname.wasm from the data partition and ran it. "wasm-userland"
# is the uname version string — emitted ONLY by uname, never by the boot banner —
# so finding it after the SSD booted proves the tool loaded on-demand off /mnt/bin.
grep -qF "wasm-userland" build/serial.m2b2p2.log || { echo TEST_FAIL_ONDEMAND; tail -50 build/serial.m2b2p2.log; exit 1; }
echo TEST_PASS_M2B2
