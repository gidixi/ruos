#!/bin/bash
qemu-system-x86_64 -hda Image/x64BareBonesImage.qcow2 -m 512 -no-kvm-irqchip -device rtl8139,netdev=mynet0,mac=DE:00:40:AA:21:2E -netdev socket,id=mynet0,listen=:1234