# ruos boot script — M2b-1 (author + copy boot payload). Boots with a BLANK
# disk (no GPT, so M1 leaves the AHCI port free) and runs `mkboot 64`, which
# authors the first SATA disk (GPT: ESP + data, both FAT32, /EFI/BOOT) AND
# copies the full boot tree onto the ESP: /EFI/BOOT/BOOTX64.EFI, /boot/kernel,
# /boot/limine/limine.conf and every .wasm/init module at its path. The kernel
# logs `mkboot ok ...` and the tool prints `mkboot: ok` — the host-side test
# (tests/m2b1-test.sh) greps serial for `mkboot: ok`, then host-verifies the ESP.
echo ruos boot OK
mkboot 64
echo m2b1-done
