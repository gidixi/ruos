# ruos boot script — M2b-2 (install + boot-from-SSD capstone). Used in BOTH
# phases of tests/m2b2-test.sh, on the SAME init that ships in the ISO and gets
# copied onto the SSD:
#   Phase 1 (ISO boot + BLANK SATA disk): no GPT → M1 leaves /mnt unmounted →
#     `install` guard passes → it authors the first SATA disk + copies the full
#     boot tree onto its ESP, printing `install: ok`.
#   Phase 2 (UEFI/OVMF boot FROM the installed SSD, no cdrom): M1 auto-mounts the
#     SSD's data partition as /mnt BEFORE init → `install` hits the guard
#     (`install: /mnt is mounted`) and boot continues — no re-install loop.
# The host-side test greps serial for "ruos boot OK" + "mnt mounted FAT" (phase 2)
# and "install: ok" (phase 1).
#
# `uname -a` proves ON-DEMAND tool exec on the installed SSD: with the slim ESP,
# /bin/uname.wasm is absent there, so the shell's resolve_path falls through to
# /mnt/bin/uname.wasm on the data partition and loads it from the FAT at first
# use. It prints "ruos ruos 0.1.0 wasm-userland x86_64" — phase 2 asserts the
# uname-only token "wasm-userland" (the banner never emits it). Same init runs
# in both phases: phase 1 (live ISO) resolves uname from the /bin tmpfs instead.
echo ruos boot OK
install 0
uname -a
echo m2b2-installed
