#!/bin/bash
qemu-system-x86_64 -hda Image/x64BareBonesImage.qcow2 -m 512 -no-kvm-irqchip -device rtl8139,netdev=mynet0,mac=DE:AD:C0:FF:EE:11 -netdev socket,id=mynet0,connect=:1234