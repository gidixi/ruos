SHELL     := /bin/bash      # required: 'build' recipe uses the 'source' builtin
KERNEL    := kernel/target/x86_64-unknown-none/debug/kernel
LIMINE    := third_party/limine
ISO_ROOT  := build/iso_root
ISO       := build/os.iso
HELLO     := shell: init.sh complete

# Userspace .wasm tools shipped on the ISO. Root-level tools go to ISO_ROOT/,
# /bin tools go to ISO_ROOT/bin/. New tools: just append to BIN_TOOLS.
ROOT_WASMS := user-bin/init.wasm user-bin/server.wasm user-bin/client.wasm
BIN_TOOLS  := shell ls cat echo \
              mkdir rmdir rm cp mv \
              head tail grep find diff du \
              whoami id uname uptime free df lscpu dmesg \
              ps kill pkill
BIN_WASMS  := $(BIN_TOOLS:%=user-bin/%.wasm)
USER_WASMS := $(ROOT_WASMS) $(BIN_WASMS)

.PHONY: all build limine iso run run-test test-boot clean user-wasm

all: iso

build:
	source $$HOME/.cargo/env && cd kernel && cargo build

limine:
	@if [ ! -d $(LIMINE) ]; then \
		git clone https://github.com/limine-bootloader/limine.git \
			--branch=v11.4.1-binary --depth=1 $(LIMINE); \
	fi
	$(MAKE) -C $(LIMINE)

# Generic pattern rule for every user-bin/*.wasm. Cargo handles
# incremental rebuilds; the per-crate manifest changes will retrigger
# the cargo build (workspace shares target/), so we don't need to list
# every .rs file as a prereq.
user-bin/%.wasm: user/%/src/main.rs user/%/Cargo.toml user/Cargo.toml
	source $$HOME/.cargo/env && cd user && cargo build --target wasm32-wasip1 --release -p $*
	cp user/target/wasm32-wasip1/release/$*.wasm user-bin/$*.wasm

.PHONY: user-wasm
user-wasm: $(USER_WASMS)

iso: build limine $(USER_WASMS) user-bin/init.sh
	rm -rf $(ISO_ROOT)
	mkdir -p $(ISO_ROOT)/boot/limine $(ISO_ROOT)/EFI/BOOT \
	         $(ISO_ROOT)/bin $(ISO_ROOT)/etc
	cp $(KERNEL) $(ISO_ROOT)/boot/kernel
	cp limine.conf $(ISO_ROOT)/boot/limine/
	for f in $(ROOT_WASMS); do cp $$f $(ISO_ROOT)/; done
	for n in $(BIN_TOOLS); do cp user-bin/$$n.wasm $(ISO_ROOT)/bin/; done
	cp user-bin/init.sh $(ISO_ROOT)/etc/
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
	@echo "--- serial (timeout 120s) ---"
	@timeout 120 qemu-system-x86_64 -cdrom $(ISO) -serial stdio -display none -no-reboot -m 512 \
		| tee build/serial.log; \
	grep -qF "$(HELLO)" build/serial.log && echo TEST_PASS || { echo TEST_FAIL; exit 1; }

test-boot: limine $(USER_WASMS) user-bin/init.sh
	@echo "--- build with boot-checks feature ---"
	source $$HOME/.cargo/env && cd kernel && cargo build \
		-Zbuild-std=core,compiler_builtins,alloc \
		-Zbuild-std-features=compiler-builtins-mem \
		--target x86_64-unknown-none \
		--features boot-checks
	rm -rf $(ISO_ROOT) $(ISO)
	mkdir -p $(ISO_ROOT)/boot/limine $(ISO_ROOT)/EFI/BOOT \
	         $(ISO_ROOT)/bin $(ISO_ROOT)/etc
	cp $(KERNEL) $(ISO_ROOT)/boot/kernel
	cp limine.conf $(ISO_ROOT)/boot/limine/
	for f in $(ROOT_WASMS); do cp $$f $(ISO_ROOT)/; done
	for n in $(BIN_TOOLS); do cp user-bin/$$n.wasm $(ISO_ROOT)/bin/; done
	cp user-bin/init.sh $(ISO_ROOT)/etc/
	cp $(LIMINE)/limine-bios.sys $(LIMINE)/limine-bios-cd.bin \
	   $(LIMINE)/limine-uefi-cd.bin $(ISO_ROOT)/boot/limine/
	cp $(LIMINE)/BOOTX64.EFI $(ISO_ROOT)/EFI/BOOT/
	xorriso -as mkisofs -b boot/limine/limine-bios-cd.bin \
		-no-emul-boot -boot-load-size 4 -boot-info-table \
		--efi-boot boot/limine/limine-uefi-cd.bin \
		-efi-boot-part --efi-boot-image --protective-msdos-label \
		$(ISO_ROOT) -o $(ISO)
	$(LIMINE)/limine bios-install $(ISO)
	@echo "--- test-boot (boot-checks feature) ---"
	@timeout 60 qemu-system-x86_64 -m 512 -no-reboot -display none -serial stdio \
		-cdrom $(ISO) > build/test-boot.log 2>&1 || true
	@grep -qF "smoke" build/test-boot.log || \
		{ echo "FAIL: no smoke lines in boot log"; cat build/test-boot.log | head -60; exit 1; }
	@grep -qF "$(HELLO)" build/test-boot.log || \
		{ echo "FAIL: no shell sentinel in boot log"; cat build/test-boot.log | tail -30; exit 1; }
	@echo "TEST_BOOT_PASS"

clean:
	rm -rf build kernel/target
