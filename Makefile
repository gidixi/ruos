SHELL     := /bin/bash      # required: 'build' recipe uses the 'source' builtin
KERNEL    := kernel/target/x86_64-unknown-none/release/kernel
LIMINE    := third_party/limine
ISO_ROOT  := build/iso_root
ISO       := build/os.iso
DISK_IMG  := build/disk.img
DISK_MB   := 64
HELLO     := shell: init.sh complete

# Userspace .wasm tools shipped on the ISO. Root-level tools go to ISO_ROOT/,
# /bin tools go to ISO_ROOT/bin/, /root/ demo blobs go to ISO_ROOT/root/.
# New tools: just append to BIN_TOOLS.
ROOT_WASMS := user-bin/init.wasm
ROOT_DEMOS := user-bin/server.wasm user-bin/client.wasm
BIN_TOOLS  := shell ls cat echo \
              mkdir rmdir rm cp mv \
              head tail grep find diff du \
              whoami id uname uptime free df lscpu dmesg \
              ps kill pkill \
              lspci ip \
              nano \
              touch wc clear which \
              sort uniq cut tr tee \
              ifconfig nc date wget ping
BIN_WASMS  := $(BIN_TOOLS:%=user-bin/%.wasm)
USER_WASMS := $(ROOT_WASMS) $(ROOT_DEMOS) $(BIN_WASMS)

.PHONY: all build limine iso run run-test test-boot clean user-wasm disk

all: iso

# Persistent SATA disk image for AHCI tests. 64 MiB raw, FAT32, with a
# marker file `hello.txt` for the smoke test. Rebuilt only if missing —
# the test mounts read-write, so a stale disk after a run is fine.
$(DISK_IMG):
	mkdir -p build
	dd if=/dev/zero of=$@.tmp bs=1M count=$(DISK_MB) status=none
	mkfs.vfat -F 32 -n RUOS $@.tmp >/dev/null
	echo 'hello from disk' | mcopy -i $@.tmp - ::/hello.txt
	# Seed an empty authorized_keys file — Step 16 SSH server reads
	# /mnt/auth.key. Tests / users can mcopy a real key on top later.
	echo '# ssh-ed25519 pubkeys here, one per line' | mcopy -i $@.tmp - ::/auth.key
	mv $@.tmp $@

disk: $(DISK_IMG)

build:
	source $$HOME/.cargo/env && cd kernel && cargo build --release

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
	         $(ISO_ROOT)/bin $(ISO_ROOT)/etc $(ISO_ROOT)/root
	cp $(KERNEL) $(ISO_ROOT)/boot/kernel
	cp limine.conf $(ISO_ROOT)/boot/limine/
	for f in $(ROOT_WASMS); do cp $$f $(ISO_ROOT)/; done
	for f in $(ROOT_DEMOS); do cp $$f $(ISO_ROOT)/root/; done
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

# NIC = QEMU `-device` model for the Ethernet adapter. Override per invocation:
#   make run NIC=e1000     # Intel e1000 path (covered in net/nic/e1000.rs)
#   make run-test NIC=e1000
# Default keeps virtio-net (Step 14 paravirtual fast path).
NIC ?= virtio-net-pci

run: iso $(DISK_IMG)
	qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom $(ISO) -serial stdio -m 512 \
		-device qemu-xhci -netdev user,id=net0 -device $(NIC),netdev=net0 \
		-drive file=$(DISK_IMG),format=raw,if=none,id=disk0 \
		-device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0

run-test: iso $(DISK_IMG)
	@echo "--- serial (timeout 120s, NIC=$(NIC)) ---"
	@timeout 120 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom $(ISO) -serial stdio -display none -no-reboot -m 512 \
		-device qemu-xhci -netdev user,id=net0 -device $(NIC),netdev=net0 \
		-drive file=$(DISK_IMG),format=raw,if=none,id=disk0 \
		-device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
		| tee build/serial.log; \
	grep -qF "$(HELLO)" build/serial.log || { echo TEST_FAIL_SHELL; exit 1; }; \
	grep -qE "pci .* init ok devices=[1-9]" build/serial.log || { echo TEST_FAIL_PCI; exit 1; }; \
	grep -qE "pci .* xhci @" build/serial.log || { echo TEST_FAIL_XHCI; exit 1; }; \
	grep -qE "net .* dhcp bound ip=10\.0\.2\.15" build/serial.log || { echo TEST_FAIL_DHCP; exit 1; }; \
	grep -qF "ahci HBA up" build/serial.log || { echo TEST_FAIL_AHCI; exit 1; }; \
	grep -qE "ahci port [0-9]+ sata sectors=" build/serial.log || { echo TEST_FAIL_AHCI_IDENTIFY; exit 1; }; \
	grep -qF "disk read OK sector 0" build/serial.log || { echo TEST_FAIL_AHCI_READ; exit 1; }; \
	grep -qF "mnt mounted FAT" build/serial.log || { echo TEST_FAIL_FAT_MOUNT; exit 1; }; \
	grep -qF "hello from disk" build/serial.log || { echo TEST_FAIL_FAT_CAT; exit 1; }; \
	echo TEST_PASS

# Per-NIC gates: each runs run-test with a specific QEMU adapter model and
# asserts that adapter's family-specific 'net: <chip> mac=..' boot line.
.PHONY: run-test-e1000
run-test-e1000: iso
	@$(MAKE) run-test NIC=e1000
	@grep -qE "net .* e1000 mac=" build/serial.log || { echo TEST_FAIL_E1000_MAC; exit 1; }
	@echo TEST_PASS_E1000

# SSH client smoke: forwards host 127.0.0.1:2222 -> guest :22, stages a
# fresh ed25519 pubkey on disk as auth.key, boots, runs OpenSSH locally.
.PHONY: run-ssh-test
SSH_KEY := build/id_ed25519
$(SSH_KEY):
	mkdir -p build
	ssh-keygen -t ed25519 -N '' -f $@ -q -C ruos-test
ssh-key-on-disk: $(SSH_KEY) $(DISK_IMG)
	mcopy -o -i $(DISK_IMG) $(SSH_KEY).pub ::/auth.key
run-ssh-test: iso ssh-key-on-disk
	@echo "--- SSH client test (timeout 60s) ---"
	@rm -f build/serial.log
	@(timeout 60 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom $(ISO) \
		-serial stdio -display none -no-reboot -m 512 \
		-device qemu-xhci \
		-netdev user,id=net0,hostfwd=tcp:127.0.0.1:2222-:22 \
		-device virtio-net-pci,netdev=net0 \
		-drive file=$(DISK_IMG),format=raw,if=none,id=disk0 \
		-device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
		> build/serial.log) & QEMUPID=$$! ; \
	sleep 15 ; \
	echo "--- launching ssh client ---" ; \
	cp $(SSH_KEY) /tmp/ruos_id && chmod 600 /tmp/ruos_id ; \
	ssh -p 2222 -i /tmp/ruos_id \
		-o StrictHostKeyChecking=no \
		-o UserKnownHostsFile=/dev/null \
		-o ConnectTimeout=5 \
		root@127.0.0.1 'echo hello-from-host' 2>&1 | tee build/ssh-client.log ; \
	sleep 3 ; \
	kill $$QEMUPID 2>/dev/null ; \
	wait $$QEMUPID 2>/dev/null ; \
	grep -F "auth ok" build/serial.log && echo TEST_PASS_SSH || { echo TEST_FAIL_AUTH; tail -30 build/serial.log; exit 1; }

test-boot: limine $(USER_WASMS) user-bin/init.sh
	@echo "--- build with boot-checks feature ---"
	source $$HOME/.cargo/env && cd kernel && cargo build \
		-Zbuild-std=core,compiler_builtins,alloc \
		-Zbuild-std-features=compiler-builtins-mem \
		--target x86_64-unknown-none \
		--features boot-checks
	rm -rf $(ISO_ROOT) $(ISO)
	mkdir -p $(ISO_ROOT)/boot/limine $(ISO_ROOT)/EFI/BOOT \
	         $(ISO_ROOT)/bin $(ISO_ROOT)/etc $(ISO_ROOT)/root
	cp $(KERNEL) $(ISO_ROOT)/boot/kernel
	cp limine.conf $(ISO_ROOT)/boot/limine/
	for f in $(ROOT_WASMS); do cp $$f $(ISO_ROOT)/; done
	for f in $(ROOT_DEMOS); do cp $$f $(ISO_ROOT)/root/; done
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
