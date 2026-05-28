SHELL     := /bin/bash      # required: 'build' recipe uses the 'source' builtin
KERNEL    := kernel/target/x86_64-unknown-none/debug/kernel
LIMINE    := third_party/limine
ISO_ROOT  := build/iso_root
ISO       := build/os.iso
HELLO     := ruos: ticks=

.PHONY: all build limine iso run run-test clean

all: iso

build:
	source $$HOME/.cargo/env && cd kernel && cargo build

limine:
	@if [ ! -d $(LIMINE) ]; then \
		git clone https://github.com/limine-bootloader/limine.git \
			--branch=v11.4.1-binary --depth=1 $(LIMINE); \
	fi
	$(MAKE) -C $(LIMINE)

iso: build limine
	rm -rf $(ISO_ROOT)
	mkdir -p $(ISO_ROOT)/boot/limine $(ISO_ROOT)/EFI/BOOT
	cp $(KERNEL) $(ISO_ROOT)/boot/kernel
	cp limine.conf $(ISO_ROOT)/boot/limine/
	cp $(LIMINE)/limine-bios.sys $(LIMINE)/limine-bios-cd.bin \
	   $(LIMINE)/limine-uefi-cd.bin $(ISO_ROOT)/boot/limine/
	cp $(LIMINE)/BOOTX64.EFI $(ISO_ROOT)/EFI/BOOT/
	xorriso -as mkisofs -b boot/limine/limine-bios-cd.bin \
		-no-emul-boot -boot-load-size 4 -boot-info-table \
		--efi-boot boot/limine/limine-uefi-cd.bin \
		-efi-boot-part --efi-boot-image --protective-msdos-label \
		$(ISO_ROOT) -o $(ISO)
	$(LIMINE)/limine bios-install $(ISO)

run: iso
	qemu-system-x86_64 -cdrom $(ISO) -serial stdio -m 512

run-test: iso
	@echo "--- serial (timeout 30s) ---"
	@timeout 30 qemu-system-x86_64 -cdrom $(ISO) -serial stdio -display none -no-reboot -m 512 \
		| tee build/serial.log; \
	grep -qF "$(HELLO)" build/serial.log && echo TEST_PASS || { echo TEST_FAIL; exit 1; }

clean:
	rm -rf build kernel/target
