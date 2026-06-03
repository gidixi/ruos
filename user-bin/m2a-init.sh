# ruos boot script — M2a phase 1 (author). Boots with a BLANK disk (no GPT,
# so M1 leaves the AHCI port free) and runs `mkdisk 64`, which authors the
# first SATA disk: GPT (ESP + Microsoft-Basic-Data) + FAT32 + /EFI/BOOT.
# The host-side test (tests/m2a-test.sh) greps serial for `mkdisk: ok`.
echo ruos boot OK
mkdisk 64
echo m2a-phase1-done
