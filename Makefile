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
              service \
              rtop \
              mkdisk mkboot install umount disks
BIN_WASMS  := $(BIN_TOOLS:%=user-bin/%.wasm)
USER_WASMS := $(ROOT_WASMS) $(ROOT_DEMOS) $(BIN_WASMS)

.PHONY: all build limine iso run run-test test-boot clean user-wasm disk run-smp-test run-comp-smp-test run-ssh-gui-test run-exec-ap-test

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

# Optional Cargo feature flags forwarded to the kernel build.
# Pass on the command line to enable extra features, e.g.:
#   make iso CARGO_FEATURES=boot-checks
CARGO_FEATURES ?=

build:
	source $$HOME/.cargo/env && cd kernel && \
		RUOS_DEFAULT_PASSWORD='$(RUOS_PASSWORD)' cargo build --release \
		$(if $(CARGO_FEATURES),--features $(CARGO_FEATURES),)

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

# Wasmtime AOT precompiler (host tool) + a demo `.cwasm` command staged at
# /bin/wtecho.cwasm so the shell's `.cwasm` router (Wasmtime) can be exercised.
WT_PRECOMPILE := tools/wt-precompile/target/release/wt-precompile

$(WT_PRECOMPILE): tools/wt-precompile/src/main.rs tools/wt-precompile/Cargo.toml
	source $$HOME/.cargo/env && cd tools/wt-precompile && cargo build --release

build/wtecho.cwasm: user-bin/echo.wasm $(WT_PRECOMPILE)
	@mkdir -p build
	$(WT_PRECOMPILE) user-bin/echo.wasm build/wtecho.cwasm

# Boot-check AOT demos embedded in the kernel via include_bytes! (compiled ONLY
# under the `boot-checks` feature). Regenerated into the source tree from their
# .wat/.wasm inputs so the (large) .cwasm need not be committed.
WT_KDIR    := kernel/src/wasm/wt
WT_KCWASMS := $(WT_KDIR)/hello.cwasm $(WT_KDIR)/gfxtest.cwasm \
              $(WT_KDIR)/echo.cwasm $(WT_KDIR)/cat.cwasm $(WT_KDIR)/spin.cwasm

$(WT_KDIR)/hello.cwasm: tools/wt-hello/hello.wat $(WT_PRECOMPILE)
	$(WT_PRECOMPILE) $< $@
$(WT_KDIR)/spin.cwasm: tools/wt-spin/spin.wat $(WT_PRECOMPILE)
	$(WT_PRECOMPILE) $< $@
$(WT_KDIR)/gfxtest.cwasm: tools/wt-gfxtest/gfx.wat $(WT_PRECOMPILE)
	$(WT_PRECOMPILE) $< $@
$(WT_KDIR)/echo.cwasm: user-bin/echo.wasm $(WT_PRECOMPILE)
	$(WT_PRECOMPILE) $< $@
$(WT_KDIR)/cat.cwasm: user-bin/cat.wasm $(WT_PRECOMPILE)
	$(WT_PRECOMPILE) $< $@

.PHONY: wt-cwasm
wt-cwasm: $(WT_KCWASMS)

# ruos-desktop submodule: the egui desktop UI, grouped as crates/ (portable libs
# gui-core + ruos-window) + apps/ (per-window cdylib wasm) + backends/ (PC dev).
# Built wasm32-wasip1 and AOT-precompiled to .cwasm by the rules below. NOTE: the
# old monolithic desktop (gui.cwasm from `ruos-backend`) was RETIRED with the
# Model A pivot — each window is now its own app .cwasm (shell + about/files/
# terminal/system + egui-demo). `ruos-backend` and its rule no longer exist.
RUOS_DESKTOP ?= ruos-desktop

# egui CSD demo window (SP-B): compositor-app built wasm32-wasip1 (std, gui-core),
# then AOT-precompiled to a CORE .cwasm embedded in the kernel (NOT a component).
# The workspace puts the output under ruos-desktop/target/ (a shared target dir),
# not under apps/compositor-app/target/.
EGUI_DEMO_SRCS := $(shell find $(RUOS_DESKTOP)/apps/compositor-app/src $(RUOS_DESKTOP)/crates/gui-core/src -name '*.rs' 2>/dev/null) \
                  $(wildcard $(RUOS_DESKTOP)/apps/compositor-app/Cargo.toml \
                             $(RUOS_DESKTOP)/Cargo.toml $(RUOS_DESKTOP)/Cargo.lock)
kernel/src/wasm/wt/egui_demo.cwasm: $(WT_PRECOMPILE) $(EGUI_DEMO_SRCS)
	@mkdir -p build
	source $$HOME/.cargo/env && cd $(RUOS_DESKTOP) && \
		cargo build -p compositor-app --target wasm32-wasip1 --release
	$(WT_PRECOMPILE) $(RUOS_DESKTOP)/target/wasm32-wasip1/release/compositor_app.wasm kernel/src/wasm/wt/egui_demo.cwasm

# Userspace desktop SHELL (SP-D): the `shell` crate built wasm32-wasip1 (std,
# gui-core's shell_chrome + ruos-window's frame_once_bare), then AOT-precompiled
# to a CORE .cwasm shipped at /bin/shell.cwasm. The compositor boots this as the
# full-screen background window. Like egui-demo the output lands under the shared
# ruos-desktop/target/ dir; the crate also pulls ruos-window, so its src is a prereq.
SHELL_SRCS := $(shell find $(RUOS_DESKTOP)/apps/shell/src $(RUOS_DESKTOP)/crates/gui-core/src $(RUOS_DESKTOP)/crates/ruos-window/src -name '*.rs' 2>/dev/null) \
              $(wildcard $(RUOS_DESKTOP)/apps/shell/Cargo.toml $(RUOS_DESKTOP)/crates/ruos-window/Cargo.toml \
                         $(RUOS_DESKTOP)/Cargo.toml $(RUOS_DESKTOP)/Cargo.lock)
kernel/src/wasm/wt/shell.cwasm: $(WT_PRECOMPILE) $(SHELL_SRCS)
	@mkdir -p build
	source $$HOME/.cargo/env && cd $(RUOS_DESKTOP) && \
		cargo build -p shell --target wasm32-wasip1 --release
	$(WT_PRECOMPILE) $(RUOS_DESKTOP)/target/wasm32-wasip1/release/shell.wasm kernel/src/wasm/wt/shell.cwasm

# Desktop app windows (SP-E): each gui-core DeskApp wrapped as a thin wasip1 window
# crate (on ruos-window), built wasm32-wasip1 then AOT-precompiled to build/<id>.cwasm,
# shipped to /bin/<id>.cwasm and spawned by the shell launcher (wm.spawn(id)). Shared
# prereqs = gui-core + ruos-window src + the workspace manifests; each app rule adds its
# own crate's src/manifest. Note the underscore in the wasm output (about-app→about_app.wasm).
APP_SRCS := $(shell find $(RUOS_DESKTOP)/crates/gui-core/src $(RUOS_DESKTOP)/crates/ruos-window/src -name '*.rs' 2>/dev/null) \
            $(wildcard $(RUOS_DESKTOP)/Cargo.toml $(RUOS_DESKTOP)/Cargo.lock)
build/about.cwasm: $(WT_PRECOMPILE) $(APP_SRCS) $(wildcard $(RUOS_DESKTOP)/apps/about-app/src/*.rs $(RUOS_DESKTOP)/apps/about-app/Cargo.toml)
	@mkdir -p build
	source $$HOME/.cargo/env && cd $(RUOS_DESKTOP) && cargo build -p about-app --target wasm32-wasip1 --release
	$(WT_PRECOMPILE) $(RUOS_DESKTOP)/target/wasm32-wasip1/release/about_app.wasm build/about.cwasm
build/files.cwasm: $(WT_PRECOMPILE) $(APP_SRCS) $(wildcard $(RUOS_DESKTOP)/apps/files-app/src/*.rs $(RUOS_DESKTOP)/apps/files-app/Cargo.toml)
	@mkdir -p build
	source $$HOME/.cargo/env && cd $(RUOS_DESKTOP) && cargo build -p files-app --target wasm32-wasip1 --release
	$(WT_PRECOMPILE) $(RUOS_DESKTOP)/target/wasm32-wasip1/release/files_app.wasm build/files.cwasm
build/terminal.cwasm: $(WT_PRECOMPILE) $(APP_SRCS) $(wildcard $(RUOS_DESKTOP)/apps/terminal-app/src/*.rs $(RUOS_DESKTOP)/apps/terminal-app/Cargo.toml)
	@mkdir -p build
	source $$HOME/.cargo/env && cd $(RUOS_DESKTOP) && cargo build -p terminal-app --target wasm32-wasip1 --release
	$(WT_PRECOMPILE) $(RUOS_DESKTOP)/target/wasm32-wasip1/release/terminal_app.wasm build/terminal.cwasm
build/system.cwasm: $(WT_PRECOMPILE) $(APP_SRCS) $(wildcard $(RUOS_DESKTOP)/apps/system-app/src/*.rs $(RUOS_DESKTOP)/apps/system-app/Cargo.toml)
	@mkdir -p build
	source $$HOME/.cargo/env && cd $(RUOS_DESKTOP) && cargo build -p system-app --target wasm32-wasip1 --release
	$(WT_PRECOMPILE) $(RUOS_DESKTOP)/target/wasm32-wasip1/release/system_app.wasm build/system.cwasm
build/notepad.cwasm: $(WT_PRECOMPILE) $(APP_SRCS) $(wildcard $(RUOS_DESKTOP)/apps/notepad-app/src/*.rs $(RUOS_DESKTOP)/apps/notepad-app/Cargo.toml)
	@mkdir -p build
	source $$HOME/.cargo/env && cd $(RUOS_DESKTOP) && cargo build -p notepad-app --target wasm32-wasip1 --release
	$(WT_PRECOMPILE) $(RUOS_DESKTOP)/target/wasm32-wasip1/release/notepad_app.wasm build/notepad.cwasm

# Bring-up component (Step-0 gate): guest -> component -> AOT cwasm embedded in kernel.
kernel/src/wasm/wt/bringup.cwasm: wit/ruos-bringup.wit tools/wt-bringup/src/lib.rs tools/wt-bringup/Cargo.toml $(WT_PRECOMPILE)
	@mkdir -p build
	source $$HOME/.cargo/env && cd tools/wt-bringup && \
		cargo build --release --target wasm32-wasip1
	source $$HOME/.cargo/env && wasm-tools component new \
		tools/wt-bringup/target/wasm32-wasip1/release/wt_bringup.wasm \
		-o build/wt-bringup.component.wasm
	$(WT_PRECOMPILE) --component build/wt-bringup.component.wasm kernel/src/wasm/wt/bringup.cwasm

# Compositor GATE reactor guest (Task 1): no_std wasm32-unknown-unknown core
# module (exports `frame`, imports the raw `wm` module) -> AOT cwasm embedded in
# the kernel. Unlike bringup it is a CORE module, not a component (no `--component`).
kernel/src/wasm/wt/reactor.cwasm: tools/wt-reactor/src/lib.rs tools/wt-reactor/Cargo.toml $(WT_PRECOMPILE)
	@mkdir -p build
	source $$HOME/.cargo/env && cd tools/wt-reactor && \
		cargo build --release --target wasm32-unknown-unknown
	$(WT_PRECOMPILE) tools/wt-reactor/target/wasm32-unknown-unknown/release/wt_reactor.wasm kernel/src/wasm/wt/reactor.cwasm

# Self-closing reactor guest (SP5 lifecycle demo): wasm32-unknown-unknown, no_std,
# precompiled to a CORE .cwasm (not a component). Imports wm.close; calls it on frame 3.
kernel/src/wasm/wt/reactor_close.cwasm: tools/wt-reactor-close/src/lib.rs tools/wt-reactor-close/Cargo.toml $(WT_PRECOMPILE)
	@mkdir -p build
	source $$HOME/.cargo/env && cd tools/wt-reactor-close && \
		cargo build --release --target wasm32-unknown-unknown
	$(WT_PRECOMPILE) tools/wt-reactor-close/target/wasm32-unknown-unknown/release/wt_reactor_close.wasm kernel/src/wasm/wt/reactor_close.cwasm

# wasip1 STD reactor probe (egui SP-A): proves a std/wasip1 guest runs as a
# compositor window. Built wasm32-wasip1 (std), precompiled to a CORE .cwasm.
kernel/src/wasm/wt/probe.cwasm: tools/wt-wasip1-probe/src/lib.rs tools/wt-wasip1-probe/Cargo.toml $(WT_PRECOMPILE)
	@mkdir -p build
	source $$HOME/.cargo/env && cd tools/wt-wasip1-probe && \
		cargo build --release --target wasm32-wasip1
	$(WT_PRECOMPILE) tools/wt-wasip1-probe/target/wasm32-wasip1/release/wt_wasip1_probe.wasm kernel/src/wasm/wt/probe.cwasm

iso: build limine $(USER_WASMS) $(INIT_SCRIPT) build/wtecho.cwasm build/about.cwasm build/files.cwasm build/terminal.cwasm build/system.cwasm build/notepad.cwasm kernel/src/wasm/wt/reactor.cwasm kernel/src/wasm/wt/reactor_close.cwasm kernel/src/wasm/wt/probe.cwasm kernel/src/wasm/wt/egui_demo.cwasm kernel/src/wasm/wt/shell.cwasm
	rm -rf $(ISO_ROOT)
	mkdir -p $(ISO_ROOT)/boot/limine $(ISO_ROOT)/EFI/BOOT \
	         $(ISO_ROOT)/bin $(ISO_ROOT)/etc $(ISO_ROOT)/root
	cp $(KERNEL) $(ISO_ROOT)/boot/kernel
	cp limine.conf $(ISO_ROOT)/boot/limine/
	cp limine-ssd.conf $(ISO_ROOT)/boot/limine/
	for f in $(ROOT_WASMS); do cp $$f $(ISO_ROOT)/; done
	for f in $(ROOT_DEMOS); do cp $$f $(ISO_ROOT)/root/; done
	for n in $(BIN_TOOLS); do cp user-bin/$$n.wasm $(ISO_ROOT)/bin/; done
	cp build/wtecho.cwasm $(ISO_ROOT)/bin/wtecho.cwasm
	# Desktop app .cwasm: baked into the ISO /bin as boot modules (available diskless).
	# The compositor's launcher still discovers them dynamically via manifest() scan.
	cp build/about.cwasm $(ISO_ROOT)/bin/about.cwasm
	cp build/files.cwasm $(ISO_ROOT)/bin/files.cwasm
	cp build/terminal.cwasm $(ISO_ROOT)/bin/terminal.cwasm
	cp build/system.cwasm $(ISO_ROOT)/bin/system.cwasm
	cp build/notepad.cwasm $(ISO_ROOT)/bin/notepad.cwasm
	cp kernel/src/wasm/wt/reactor.cwasm $(ISO_ROOT)/bin/compositor.cwasm
	cp kernel/src/wasm/wt/egui_demo.cwasm $(ISO_ROOT)/bin/egui-demo.cwasm
	cp kernel/src/wasm/wt/shell.cwasm $(ISO_ROOT)/bin/shell.cwasm
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
		grep -qE "rtop: uptime=" build/serial.log || { echo TEST_FAIL_RTOP; exit 1; }; \
		grep -qE "^cpu0:[0-9]+%" build/serial.log || { echo TEST_FAIL_RTOP_CORE; exit 1; }; \
		grep -qE "usb  xhci up" build/serial.log || { echo TEST_FAIL_USB_UP; exit 1; }; \
		grep -qE "usb.*keyboard ready" build/serial.log || { echo TEST_FAIL_USB_KBD; exit 1; }; \
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

# M2a disk-authoring end-to-end: proves ruos AUTHORS a real disk (GPT + FAT32 +
# /EFI/BOOT via `mkdisk`) AND that M1 boots + auto-mounts it. The script runs two
# phases on the same image with its own `make iso INIT_SCRIPT=...` builds (author,
# then round-trip), host-verifying the authored image with sgdisk/fsck.fat/mtools.
.PHONY: run-m2a-test
run-m2a-test:
	bash tests/m2a-test.sh

# M2b-2 boot-from-SSD end-to-end (the SSD-installer CAPSTONE). Two phases on ONE
# disk image, boot-marker-only (no mtools): phase 1 boots the ISO + a BLANK SATA
# disk + an init running `install` (guard passes ⇒ authors disk + copies boot
# tree to the ESP, asserts `install: ok`); phase 2 boots FROM that SSD under
# UEFI/OVMF with NO cdrom (OVMF → /EFI/BOOT/BOOTX64.EFI → Limine → /boot/kernel →
# ruos → M1 mounts /mnt), asserting "ruos boot OK" + "mnt mounted FAT".
.PHONY: run-m2b2-test
run-m2b2-test:
	bash tests/m2b2-test.sh

# Disk-management end-to-end: prove `disks` lists the SATA disks AND that
# `umount /mnt` unblocks `install`. Builds a GPT disk with a FAT32 data partition
# (so M1 auto-mounts /mnt at boot, like run-gpt-test), boots with
# INIT_SCRIPT=user-bin/dm-init.sh (disks → umount /mnt → install 0), and asserts
# the disk was listed, `/mnt` unmounted, and install PROCEEDED (did not refuse).
# The script does its own `make iso INIT_SCRIPT=...`.
.PHONY: run-dm-test
run-dm-test:
	bash tests/disk-mgmt-test.sh

# SP4 SMP-compositing equivalence: prove the parallel (SMP-banded) composite is
# pixel-identical to a serial (n_bands=1) reference, AND that >=2 cores ran band
# jobs. Builds two ISOs (default parallel + CARGO_FEATURES=serial-composite),
# boots each headless under QMP, screendumps the steady two-window composite, and
# asserts the PNGs are byte-identical. The script does its own `make iso`.
.PHONY: run-comp-smp-test
run-comp-smp-test:
	@bash tests/comp-smp-test.sh

# Step 5 GOAL GATE — SSH alive while the compositor runs on a dedicated GUI core.
# Builds the ISO with the compositor init script, boots with -smp 4 (so cpu 1
# becomes the GuiCompositor core), waits for the hand-off marker, then SSHes.
# PASS requires: "compositor handed off to gui core" in serial + "auth ok" in
# serial + interactive "ruos:/$" prompt from the SSH client.
# This directly proves the BSP executor (ssh_serve_task) stayed alive while the
# compositor ran — i.e. THE GOAL of Step 5.
.PHONY: run-ssh-gui-test
run-ssh-gui-test: ssh-key-on-disk
	@$(MAKE) iso INIT_SCRIPT=user-bin/compositor-init.sh
	bash tests/ssh-during-gui-test.sh

# C2b gate: a .cwasm exec'd from the shell runs on a ComputeApp core (off the BSP).
# Boots with -smp 4 so core 2 = ComputeApp; wtecho.cwasm is the .cwasm tool.
# PASS requires: "exec-ap ran_on=core[1-9]" (routed off BSP) AND "EXEC_AP_OK"
# (wtecho's stdout reached serial via the PTY, proving cross-core app I/O).
.PHONY: run-exec-ap-test
run-exec-ap-test:
	@$(MAKE) iso INIT_SCRIPT=user-bin/exec-ap-init.sh
	@echo "--- exec-on-AP (-smp 4) ---"
	@timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio \
	  -device qemu-xhci -cdrom $(ISO) 2>&1 | tee build/exec-ap.log; \
	grep -qE "exec-ap ran_on=core[1-9]" build/exec-ap.log || { echo TEST_FAIL_EXEC_AP_CORE; exit 1; }; \
	grep -qF "EXEC_AP_OK" build/exec-ap.log || { echo TEST_FAIL_EXEC_AP_OUTPUT; exit 1; }; \
	echo TEST_PASS_EXEC_AP

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

.PHONY: run-smp-test
run-smp-test: iso $(DISK_IMG)
	bash tests/smp-test.sh

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

# Console engine self-test: engine_test::run() emits CONSOLE_TEST: OK on serial
# in the devices boot phase, then init powers off.
.PHONY: run-console-test
run-console-test: iso
	@$(MAKE) iso INIT_SCRIPT=user-bin/console-test-init.sh CARGO_FEATURES=boot-checks > build/console-iso.log 2>&1 || { echo TEST_FAIL_ISO; tail -20 build/console-iso.log; exit 1; }
	@timeout 60 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom $(ISO) -serial stdio -display none -no-reboot -m 512 \
		2>&1 | tee build/console-test.log | grep -q 'CONSOLE_TEST: OK' && echo CONSOLE_TEST_PASS || { echo CONSOLE_TEST_FAIL; tail -40 build/console-test.log; exit 1; }

test-boot: limine $(USER_WASMS) $(WT_KCWASMS) kernel/src/wasm/wt/bringup.cwasm kernel/src/wasm/wt/reactor.cwasm kernel/src/wasm/wt/reactor_close.cwasm kernel/src/wasm/wt/probe.cwasm kernel/src/wasm/wt/egui_demo.cwasm kernel/src/wasm/wt/shell.cwasm $(INIT_SCRIPT) build/wtecho.cwasm build/about.cwasm build/files.cwasm build/terminal.cwasm build/system.cwasm build/notepad.cwasm
	@echo "--- build with boot-checks feature ---"
	source $$HOME/.cargo/env && cd kernel && cargo build --release \
		-Zbuild-std=core,compiler_builtins,alloc \
		-Zbuild-std-features=compiler-builtins-mem \
		--target x86_64-unknown-none \
		--features boot-checks
	rm -rf $(ISO_ROOT) $(ISO)
	mkdir -p $(ISO_ROOT)/boot/limine $(ISO_ROOT)/EFI/BOOT \
	         $(ISO_ROOT)/bin $(ISO_ROOT)/etc $(ISO_ROOT)/root
	cp $(KERNEL) $(ISO_ROOT)/boot/kernel
	cp limine.conf $(ISO_ROOT)/boot/limine/
	cp limine-ssd.conf $(ISO_ROOT)/boot/limine/
	for f in $(ROOT_WASMS); do cp $$f $(ISO_ROOT)/; done
	for f in $(ROOT_DEMOS); do cp $$f $(ISO_ROOT)/root/; done
	for n in $(BIN_TOOLS); do cp user-bin/$$n.wasm $(ISO_ROOT)/bin/; done
	cp build/wtecho.cwasm $(ISO_ROOT)/bin/wtecho.cwasm
	# Desktop app .cwasm: baked into the ISO /bin as boot modules (available diskless).
	# The compositor's launcher still discovers them dynamically via manifest() scan.
	cp build/about.cwasm $(ISO_ROOT)/bin/about.cwasm
	cp build/files.cwasm $(ISO_ROOT)/bin/files.cwasm
	cp build/terminal.cwasm $(ISO_ROOT)/bin/terminal.cwasm
	cp build/system.cwasm $(ISO_ROOT)/bin/system.cwasm
	cp build/notepad.cwasm $(ISO_ROOT)/bin/notepad.cwasm
	cp kernel/src/wasm/wt/reactor.cwasm $(ISO_ROOT)/bin/compositor.cwasm
	cp kernel/src/wasm/wt/egui_demo.cwasm $(ISO_ROOT)/bin/egui-demo.cwasm
	cp kernel/src/wasm/wt/shell.cwasm $(ISO_ROOT)/bin/shell.cwasm
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
	@timeout 60 qemu-system-x86_64 -machine q35 -cpu max -m 512 -no-reboot -display none -serial stdio \
		-device qemu-xhci -cdrom $(ISO) > build/test-boot.log 2>&1 || true
	@grep -qF "smoke" build/test-boot.log || \
		{ echo "FAIL: no smoke lines in boot log"; cat build/test-boot.log | head -60; exit 1; }
	@grep -qF "$(HELLO)" build/test-boot.log || \
		{ echo "FAIL: no shell sentinel in boot log"; cat build/test-boot.log | tail -30; exit 1; }
	@grep -qF "WT-COMPONENT-OK" build/test-boot.log || \
		{ echo "FAIL: component bring-up did not run (no WT-COMPONENT-OK)"; grep -E "component (bringup|deserialize|instantiate|run)" build/test-boot.log | tail -10; exit 1; }
	@echo "TEST_BOOT_PASS"

clean:
	rm -rf build kernel/target
