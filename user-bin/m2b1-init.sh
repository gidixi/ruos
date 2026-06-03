# ruos boot script — M2b-1 (author + copy boot payload). Boots with a BLANK
# disk (no GPT, so M1 leaves the AHCI port free) and runs `mkboot 64`, which
# authors the first SATA disk (GPT: ESP + data, both FAT32, /EFI/BOOT) AND
# copies the boot tree, SPLIT: the bootstrap (BOOTX64.EFI + kernel + slim
# limine.conf + init/shell/server/client) onto the ESP, and the /bin/*.wasm
# command tools onto the data partition (so they mount at /mnt/bin, loaded
# on-demand). The kernel logs `mkboot ok ...` and the tool prints `mkboot: ok` —
# the host-side test (tests/m2b1-test.sh) greps serial for `mkboot: ok`, then
# host-verifies both partitions (bootstrap on the ESP, tools on the data part).
echo ruos boot OK
mkboot 64
echo m2b1-done
