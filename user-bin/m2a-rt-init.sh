# ruos boot script — M2a phase 2 (round-trip). Boots with the disk authored in
# phase 1. M1 (storage.rs) sees the GPT and auto-mounts the Microsoft-Basic-Data
# partition as /mnt; this init writes + reads back a marker on it. Proves M1
# mounts what M2a authored AND that it persists across a reboot.
echo ruos boot OK
echo m2a-roundtrip-ok > /mnt/MK.TXT
cat /mnt/MK.TXT
echo m2a-phase2-done
