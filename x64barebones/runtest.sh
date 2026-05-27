#!/bin/bash
# Builds nothing; run `make all` first. Boots the image, runs memTest,
# prints serial output to this terminal, and exits via isa-debug-exit.
qemu-system-x86_64 -hda Image/x64BareBonesImage.qcow2 -m 512 \
	-serial stdio -display none -no-reboot \
	-device isa-debug-exit,iobase=0xf4,iosize=0x04 \
	-device rtl8139,netdev=n0,mac=DE:00:40:AA:21:2E -netdev user,id=n0
