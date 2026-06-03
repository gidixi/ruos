#!/usr/bin/env bash
# M2b-1 end-to-end: prove ruos can AUTHOR a real disk (GPT + FAT32 + /EFI/BOOT)
# AND COPY a SLIM boot tree onto the ESP — bootstrap only (kernel, BOOTX64.EFI,
# the reduced limine.conf, the init chain + shell + network service) — while the
# ~50 command tools go to the DATA partition (where they mount at /mnt/bin and
# load on-demand). Boots M1 with a BLANK disk and INIT_SCRIPT=user-bin/m2b1-init.sh
# (which runs `mkboot 64`). The boot payload (kernel, BOOTX64.EFI, both limine
# configs) ships as Limine modules so it's readable at runtime; `mkboot` splits
# it: bootstrap to the ESP, tools to the data partition. We then extract BOTH
# partitions from the raw image and host-verify them with sgdisk/fsck.fat/mtools:
#   ESP  — structure (BOOTX64.EFI, kernel, limine.conf, shell.wasm, server.wasm)
#          AND that a command tool (ls.wasm) is NOT present (proves slim ESP);
#          byte-identity of the ~20 MB kernel vs the ISO source.
#   DATA — the command tools are present (ls.wasm + readdirtest.wasm, the latter
#          a 14-char LFN), with byte-identity of ls.wasm vs the ISO source.
# No host loop devices — QEMU AHCI is the only disk path.
set -u
cd "$(dirname "$0")/.."
IMG=build/m2b1-disk.img
S=build/serial.log
KERNEL=kernel/target/x86_64-unknown-none/release/kernel   # ISO source for /boot/kernel
LS_SRC=user-bin/ls.wasm                                    # ISO source for /bin/ls.wasm
# Stage the partition images + the mtools/fsck outputs on ext4 /tmp, NOT the
# build/ dir: in the WSL host the repo lives on a 9p /mnt/e mount where mtools'
# many small random reads of the ~20 MB kernel are pathologically slow (can hang
# for minutes). /tmp is on the native ext4 root, so the read-only verification
# runs at disk speed. We still never touch the QEMU-written raw image.
ESP=/tmp/m2b1-esp.img
DATA=/tmp/m2b1-data.img
KERNEL_OUT=/tmp/m2b1-kernel.out
LS_OUT=/tmp/m2b1-ls.out
killq(){ ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 "$p" 2>/dev/null||true; done; }
cleanup(){ rm -f "$ESP" "$DATA" "$KERNEL_OUT" "$LS_OUT" 2>/dev/null||true; }
trap cleanup EXIT

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

# --- extract a partition (raw image, never written) into a /tmp image. ---
# sgdisk -i N fields:  "First sector: <lba> (at ..)" / "Partition size: <sectors> sectors (..)"
extract_part(){ # $1 = partition index, $2 = out image
  local elba esec
  elba=$(sgdisk -i "$1" "$IMG" | awk -F': ' '/First sector/{print $2}' | awk '{print $1}')
  esec=$(sgdisk -i "$1" "$IMG" | awk '/Partition size/{print $3}')
  [ -n "$elba" ] && [ -n "$esec" ] || { echo TEST_FAIL_SGDISK; sgdisk -i "$1" "$IMG"; exit 1; }
  dd if="$IMG" of="$2" bs=512 skip="$elba" count="$esec" status=none
}
extract_part 1 "$ESP"   # ESP  (bootstrap)
extract_part 2 "$DATA"  # DATA (command tools, mounts at /mnt/bin)

# --- fsck BOTH partitions (read-only). `fsck.fat -n` prints a benign
#     "Free cluster summary uninitialized" (we don't write FSInfo free counts)
#     and a "<n> files, <a>/<b> clusters" summary. Corruption prints hard errors.
#     Gate: summary present AND no hard error on EACH partition. ---
HARD='Dirty bit|orphan|Checksum|cross-link|free cluster chain|Reclaim|corrupt|invalid|bad cluster|Got [0-9]'
fsck_part(){ # $1 = image, $2 = log
  fsck.fat -n "$1" > "$2" 2>&1
  grep -qiE 'files, .*clusters' "$2" || { echo TEST_FAIL_FSCK; cat "$2"; exit 1; }
  grep -qiE "$HARD"             "$2" && { echo TEST_FAIL_FSCK; cat "$2"; exit 1; }
}
fsck_part "$ESP"  build/m2b1-fsck-esp.log
fsck_part "$DATA" build/m2b1-fsck-data.log

# --- ESP structure: bootstrap files at their UEFI/Limine ESP locations. ---
mdir -i "$ESP" ::/EFI/BOOT 2>&1 | grep -qi "BOOTX64" \
  || { echo TEST_FAIL_BOOTX64; mdir -i "$ESP" ::/EFI/BOOT; exit 1; }
mdir -i "$ESP" ::/boot 2>&1 | grep -qi "kernel" \
  || { echo TEST_FAIL_KERNEL; mdir -i "$ESP" ::/boot; exit 1; }
mdir -i "$ESP" ::/boot/limine 2>&1 | grep -qi "limine" \
  || { echo TEST_FAIL_CONF; mdir -i "$ESP" ::/boot/limine; exit 1; }
mdir -i "$ESP" ::/bin 2>&1 | grep -qi "shell.wasm" \
  || { echo TEST_FAIL_SHELL_ON_ESP; mdir -i "$ESP" ::/bin; exit 1; }
mdir -i "$ESP" ::/root 2>&1 | grep -qi "server.wasm" \
  || { echo TEST_FAIL_SERVER_ON_ESP; mdir -i "$ESP" ::/root; exit 1; }

# --- SLIM ESP proof: a command tool (ls.wasm) must NOT be on the ESP. The
#     bootstrap keeps only shell/init/network; the ~50 tools live on the data
#     partition and load on-demand from /mnt/bin. (`mdir ::/bin` may error if
#     /bin is absent; merging stderr makes the grep robust either way.) ---
mdir -i "$ESP" ::/bin 2>&1 | grep -qi "ls.wasm" \
  && { echo TEST_FAIL_TOOL_ON_ESP; mdir -i "$ESP" ::/bin; exit 1; }

# --- DATA structure: the command tools live here (mount at /mnt/bin). The LFN
#     proof now lives on the data partition: readdirtest.wasm is a 14-char name
#     (not 8.3), so its presence proves long-name dir entries were written. ---
mdir -i "$DATA" ::/bin 2>&1 | grep -qi "ls.wasm" \
  || { echo TEST_FAIL_TOOL_ON_DATA; mdir -i "$DATA" ::/bin; exit 1; }
mdir -i "$DATA" ::/bin 2>&1 | grep -qi "readdirtest.wasm" \
  || { echo TEST_FAIL_LFN; mdir -i "$DATA" ::/bin; exit 1; }

# --- byte-identity proof: the copied bytes must equal the ISO sources. The
#     kernel (~20 MB, on the ESP) — the copy (FAT cluster-chain write) must be
#     exact. ls.wasm now lives on the DATA partition, so read it from there. ---
mcopy -i "$ESP" ::/boot/kernel "$KERNEL_OUT" 2>/dev/null \
  || { echo TEST_FAIL_KERNEL_READ; exit 1; }
cmp "$KERNEL" "$KERNEL_OUT" \
  || { echo TEST_FAIL_KERNEL_BYTES; ls -l "$KERNEL" "$KERNEL_OUT"; exit 1; }
mcopy -i "$DATA" ::/bin/ls.wasm "$LS_OUT" 2>/dev/null \
  || { echo TEST_FAIL_LS_READ; exit 1; }
cmp "$LS_SRC" "$LS_OUT" \
  || { echo TEST_FAIL_LS_BYTES; ls -l "$LS_SRC" "$LS_OUT"; exit 1; }

echo TEST_PASS_M2B1
