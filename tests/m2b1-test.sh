#!/usr/bin/env bash
# M2b-1 end-to-end: prove ruos can AUTHOR a real disk (GPT + FAT32 + /EFI/BOOT)
# AND COPY its full boot tree onto the ESP, so the SSD boots standalone. Boots
# M1 with a BLANK disk and INIT_SCRIPT=user-bin/m2b1-init.sh (which runs
# `mkboot 64`). The boot payload (kernel, BOOTX64.EFI, limine.conf) ships as
# Limine modules so it's readable at runtime; `mkboot` writes it + every
# .wasm/init module to the ESP. We then extract the ESP from the raw image and
# host-verify it with sgdisk/fsck.fat/mtools: structure (BOOTX64.EFI, kernel,
# limine.conf), LFN listing (readdirtest.wasm is not 8.3), and — the real proof
# — byte-identity of the ~20 MB kernel and a wasm against the ISO sources.
# No host loop devices — QEMU AHCI is the only disk path.
set -u
cd "$(dirname "$0")/.."
IMG=build/m2b1-disk.img
S=build/serial.log
KERNEL=kernel/target/x86_64-unknown-none/release/kernel   # ISO source for /boot/kernel
LS_SRC=user-bin/ls.wasm                                    # ISO source for /bin/ls.wasm
killq(){ ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 "$p" 2>/dev/null||true; done; }

killq; sleep 1
dd if=/dev/zero of="$IMG" bs=1M count=512 status=none      # blank target (≥ kernel + payload)

# --- build the ISO with the M2b-1 init (author + copy) ---
make iso INIT_SCRIPT=user-bin/m2b1-init.sh > build/m2b1-iso.log 2>&1 \
  || { echo TEST_FAIL_ISO; tail -20 build/m2b1-iso.log; exit 1; }

# --- boot: blank disk on AHCI; mkboot authors + copies; 20 MB over emulated
#     AHCI is slow, so allow a long timeout (180 s) and a long poll (85×2 s). ---
timeout 180 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom build/os.iso \
  -serial stdio -display none -no-reboot -m 1024 -device qemu-xhci \
  -drive file="$IMG",format=raw,if=none,id=d0 -device ahci,id=ahci \
  -device ide-hd,drive=d0,bus=ahci.0 > "$S" 2>&1 & QP=$!
for _ in $(seq 1 85); do
  grep -qF "mkboot: ok" "$S" 2>/dev/null && break
  kill -0 "$QP" 2>/dev/null || break
  sleep 2
done
killq
cp "$S" build/serial.m2b1.log
grep -qF "mkboot: ok" "$S" || { echo TEST_FAIL_MKBOOT; tail -40 "$S"; exit 1; }

# --- extract the ESP (partition 1) from the raw image (never write the img) ---
# sgdisk -i N fields:  "First sector: <lba> (at ..)" / "Partition size: <sectors> sectors (..)"
ELBA=$(sgdisk -i 1 "$IMG" | awk -F': ' '/First sector/{print $2}' | awk '{print $1}')
ESEC=$(sgdisk -i 1 "$IMG" | awk '/Partition size/{print $3}')
[ -n "$ELBA" ] && [ -n "$ESEC" ] || { echo TEST_FAIL_SGDISK; sgdisk -i 1 "$IMG"; exit 1; }
dd if="$IMG" of=build/m2b1-esp.img bs=512 skip="$ELBA" count="$ESEC" status=none

# --- fsck the extracted ESP (read-only). `fsck.fat -n` prints a benign
#     "Free cluster summary uninitialized" (we don't write FSInfo free counts)
#     and a "<n> files, <a>/<b> clusters" summary. Corruption prints hard errors.
#     Gate: summary present AND no hard error. ---
fsck.fat -n build/m2b1-esp.img > build/m2b1-fsck.log 2>&1
HARD='Dirty bit|orphan|Checksum|cross-link|free cluster chain|Reclaim|corrupt|invalid|bad cluster|Got [0-9]'
grep -qiE 'files, .*clusters' build/m2b1-fsck.log || { echo TEST_FAIL_FSCK; cat build/m2b1-fsck.log; exit 1; }
grep -qiE "$HARD"             build/m2b1-fsck.log && { echo TEST_FAIL_FSCK; cat build/m2b1-fsck.log; exit 1; }

# --- structure: the three boot files at their UEFI/Limine ESP locations ---
mdir -i build/m2b1-esp.img ::/EFI/BOOT 2>&1 | grep -qi "BOOTX64" \
  || { echo TEST_FAIL_BOOTX64; mdir -i build/m2b1-esp.img ::/EFI/BOOT; exit 1; }
mdir -i build/m2b1-esp.img ::/boot 2>&1 | grep -qi "kernel" \
  || { echo TEST_FAIL_KERNEL; mdir -i build/m2b1-esp.img ::/boot; exit 1; }
mdir -i build/m2b1-esp.img ::/boot/limine 2>&1 | grep -qi "limine" \
  || { echo TEST_FAIL_CONF; mdir -i build/m2b1-esp.img ::/boot/limine; exit 1; }

# --- LFN proof: readdirtest.wasm is a 14-char name (not 8.3), so its presence
#     in the mdir listing proves the long-name directory entries were written. ---
mdir -i build/m2b1-esp.img ::/bin 2>&1 | grep -qi "readdirtest.wasm" \
  || { echo TEST_FAIL_LFN; mdir -i build/m2b1-esp.img ::/bin; exit 1; }

# --- byte-identity proof: the copied bytes must equal the ISO sources. The
#     kernel is ~20 MB — the copy (FAT cluster-chain write) must be exact. ---
mcopy -i build/m2b1-esp.img ::/boot/kernel build/m2b1-kernel.out 2>/dev/null \
  || { echo TEST_FAIL_KERNEL_READ; exit 1; }
cmp "$KERNEL" build/m2b1-kernel.out \
  || { echo TEST_FAIL_KERNEL_BYTES; ls -l "$KERNEL" build/m2b1-kernel.out; exit 1; }
mcopy -i build/m2b1-esp.img ::/bin/ls.wasm build/m2b1-ls.out 2>/dev/null \
  || { echo TEST_FAIL_LS_READ; exit 1; }
cmp "$LS_SRC" build/m2b1-ls.out \
  || { echo TEST_FAIL_LS_BYTES; ls -l "$LS_SRC" build/m2b1-ls.out; exit 1; }

echo TEST_PASS_M2B1
