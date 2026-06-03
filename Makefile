SHELL     := /bin/bash      # required: 'build' recipe uses the 'source' builtin
KERNEL    := kernel/target/x86_64-unknown-none/release/kernel
LIMINE    := third_party/limine
ISO_ROOT  := build/iso_root
ISO       := build/os.iso
DISK_IMG  := build/disk.img
DISK_MB   := 64
HELLO     := shell: init.sh complete

# Script copied as /etc/init.sh into the ISO. Defaults to the minimal
# greeter; `make run-test` overrides with user-bin/smoke.sh to run the
# full assertion battery (slower boot, ~80s before shell prompt).
INIT_SCRIPT ?= user-bin/init.sh

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
              ifconfig nc date wget ping \
              service readdirtest \
              spinloop smptest \
              rtop \
              mkdisk
BIN_WASMS  := $(BIN_TOOLS:%=user-bin/%.wasm)
USER_WASMS := $(ROOT_WASMS) $(ROOT_DEMOS) $(BIN_WASMS)

.PHONY: all build limine iso run run-test test-boot clean user-wasm disk run-fuel-test run-smp-test run-smp2-test

all: iso

# Persistent SATA disk image for AHCI tests. 64 MiB raw, FAT32, with a
# marker file `hello.txt` for the smoke test. Rebuilt only if missing —
# the test mounts read-write, so a stale disk after a run is fine.
#
# Default-seeded files for SSH:
#  - /auth.key : empty (placeholder). Inject your client pubkey via
#                `make ssh-key-on-disk` (test key) or `mcopy` manually.
#  - /passwd   : PBKDF2-HMAC-SHA256 of $(RUOS_PASSWORD) — default 'ruos'.
#                Override per build: `make disk RUOS_PASSWORD=hunter2`.
#                The SSH server offers password auth iff this file parses.
$(DISK_IMG):
	mkdir -p build
	dd if=/dev/zero of=$@.tmp bs=1M count=$(DISK_MB) status=none
	mkfs.vfat -F 32 -n RUOS $@.tmp >/dev/null
	echo 'hello from disk' | mcopy -i $@.tmp - ::/hello.txt
	echo '# ssh-ed25519 pubkeys here, one per line' | mcopy -i $@.tmp - ::/auth.key
	RUOS_PASSWORD='$(RUOS_PASSWORD)' PASSWD_ITER=$(PASSWD_ITER) python3 -c "$$PASSWD_GEN" | mcopy -i $@.tmp - ::/passwd
	mv $@.tmp $@
	@echo "disk.img: SSH password seeded — login as any user with password='$(RUOS_PASSWORD)'"

disk: $(DISK_IMG)

build:
	source $$HOME/.cargo/env && cd kernel && \
		RUOS_DEFAULT_PASSWORD='$(RUOS_PASSWORD)' cargo build --release

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

iso: build limine $(USER_WASMS) $(INIT_SCRIPT)
	rm -rf $(ISO_ROOT)
	mkdir -p $(ISO_ROOT)/boot/limine $(ISO_ROOT)/EFI/BOOT \
	         $(ISO_ROOT)/bin $(ISO_ROOT)/etc $(ISO_ROOT)/root
	cp $(KERNEL) $(ISO_ROOT)/boot/kernel
	cp limine.conf $(ISO_ROOT)/boot/limine/
	for f in $(ROOT_WASMS); do cp $$f $(ISO_ROOT)/; done
	for f in $(ROOT_DEMOS); do cp $$f $(ISO_ROOT)/root/; done
	for n in $(BIN_TOOLS); do cp user-bin/$$n.wasm $(ISO_ROOT)/bin/; done
	cp $(INIT_SCRIPT) $(ISO_ROOT)/etc/init.sh
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
		-device qemu-xhci -device usb-kbd -netdev user,id=net0 -device $(NIC),netdev=net0 \
		-drive file=$(DISK_IMG),format=raw,if=none,id=disk0 \
		-device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0

run-test: $(DISK_IMG)
	@$(MAKE) iso INIT_SCRIPT=user-bin/smoke.sh
	@echo "--- serial (timeout 240s, NIC=$(NIC)) ---"
	@timeout 240 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom $(ISO) -serial stdio -display none -no-reboot -m 512 \
		-device qemu-xhci -device usb-kbd -netdev user,id=net0 -device $(NIC),netdev=net0 \
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
	grep -qE "readdir-std: [1-9][0-9]* entries" build/serial.log || { echo TEST_FAIL_READDIR; exit 1; }; \
		grep -qE "rtop: uptime=" build/serial.log || { echo TEST_FAIL_RTOP; exit 1; }; \
		grep -qE "^cpu0:[0-9]+%" build/serial.log || { echo TEST_FAIL_RTOP_CORE; exit 1; }; \
		grep -qE "usb  xhci up" build/serial.log || { echo TEST_FAIL_USB_UP; exit 1; }; \
		grep -qE "usb  keyboard ready" build/serial.log || { echo TEST_FAIL_USB_KBD; exit 1; }; \
	echo TEST_PASS

# Per-NIC gates: each runs run-test with a specific QEMU adapter model and
# asserts that adapter's family-specific 'net: <chip> mac=..' boot line.
.PHONY: run-test-e1000
run-test-e1000: iso
	@$(MAKE) run-test NIC=e1000
	@grep -qE "net .* e1000 mac=" build/serial.log || { echo TEST_FAIL_E1000_MAC; exit 1; }
	@echo TEST_PASS_E1000

# GPT mount test: builds a GPT-partitioned SATA disk (ESP + Microsoft-Basic-Data
# holding a marker), boots ruos with it as the only AHCI disk, asserts the GPT
# data partition is parsed + mounted as /mnt and the marker file is read.
# Builds the iso with the smoke battery as init (like run-test) so the boot
# shell `cat`s /mnt/GPTHELLO.TXT to serial — the minimal default init.sh does not.
.PHONY: run-gpt-test
run-gpt-test:
	@$(MAKE) iso INIT_SCRIPT=user-bin/smoke.sh
	bash tests/gpt-test.sh

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
	bash tests/ssh-shell-test.sh

# Bake a PBKDF2-HMAC-SHA256 password hash into disk.img as /passwd. The
# kernel SSH server (sunset_io.rs) consumes /mnt/passwd at boot and
# accepts the configured password as an alternative to pubkey auth.
# Override per invocation: `make passwd-on-disk RUOS_PASSWORD=hunter2`.
# Iteration count matches the verify-side check in password.rs (>=1000).
RUOS_PASSWORD ?= ruos
PASSWD_ITER   ?= 100000
.PHONY: passwd-on-disk
passwd-on-disk: $(DISK_IMG)
	@RUOS_PASSWORD='$(RUOS_PASSWORD)' PASSWD_ITER=$(PASSWD_ITER) python3 -c "$$PASSWD_GEN" > build/passwd
	mcopy -o -i $(DISK_IMG) build/passwd ::/passwd
	@echo "passwd: hash written for password='$(RUOS_PASSWORD)' ($(PASSWD_ITER) iterations)"

define PASSWD_GEN
import os, hashlib, secrets
pw    = os.environ['RUOS_PASSWORD']
iters = int(os.environ['PASSWD_ITER'])
salt  = secrets.token_bytes(16)
h     = hashlib.pbkdf2_hmac('sha256', pw.encode(), salt, iters)
print(f'pbkdf2-sha256:{iters}:{salt.hex()}:{h.hex()}')
endef
export PASSWD_GEN

.PHONY: run-pipe-test
run-pipe-test: iso ssh-key-on-disk
	bash tests/pipe-test.sh

.PHONY: run-fuel-test
run-fuel-test: iso ssh-key-on-disk
	bash tests/fuel-test.sh

.PHONY: run-smp-test
run-smp-test: iso $(DISK_IMG)
	bash tests/smp-test.sh

.PHONY: run-smp2-test
run-smp2-test: iso ssh-key-on-disk
	bash tests/smp2-test.sh

# rtop interactive test: runs rtop over SSH, asserts timer-driven auto-refresh
# (multiple frames while idle) + clean 'q' quit (alt-screen restored).
.PHONY: run-rtop-test
run-rtop-test: iso ssh-key-on-disk
	bash tests/rtop-ssh-test.sh

# Ctrl-C test: runs a long app over SSH, sends ^C, asserts the foreground app
# is killed and the shell prompt returns (line-discipline VINTR + cooked exec).
.PHONY: run-ctrlc-test
run-ctrlc-test: iso ssh-key-on-disk
	bash tests/ctrlc-ssh-test.sh

# SSH idle-survival test: a connected session left idle must NOT be reaped by
# the pty watchdog (bridge heartbeat keeps it alive; only leaked pairs reap).
.PHONY: run-ssh-idle-test
run-ssh-idle-test: iso ssh-key-on-disk
	bash tests/ssh-idle-test.sh

# USB keyboard test: QMP send-key drives QEMU's usb-kbd; the typed token must be
# echoed by the boot shell on serial (proves xHCI HID -> master_input_push(0)).
.PHONY: run-usb-key-test
run-usb-key-test: iso
	bash tests/usb-key-test.sh

# USB hub test: a keyboard behind a usb-hub at boot must enumerate (route string
# + recursive enumeration) and type — proves the hub class driver.
.PHONY: run-usb-hub-test
run-usb-hub-test: iso
	bash tests/usb-hub-test.sh

# USB hot-plug test: QMP device_add/device_del a usb-kbd at runtime — it must
# enumerate + type after add (Port Status Change Event) and tear down after del.
.PHONY: run-usb-hotplug-test
run-usb-hotplug-test: iso
	bash tests/usb-hotplug-test.sh

.PHONY: run-passwd-test
run-passwd-test: iso passwd-on-disk
	RUOS_PASSWORD='$(RUOS_PASSWORD)' bash tests/ssh-passwd-test.sh

# Diskless boot test: no -drive on QEMU. Verifies SSH password works
# against the compile-time fallback (no /mnt/passwd available).
.PHONY: run-passwd-diskless-test
run-passwd-diskless-test: iso
	bash tests/ssh-passwd-diskless-test.sh

test-boot: limine $(USER_WASMS) $(INIT_SCRIPT)
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
	cp $(INIT_SCRIPT) $(ISO_ROOT)/etc/init.sh
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
