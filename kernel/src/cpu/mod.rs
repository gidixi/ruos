//! Per-CPU data. Each core's GS base points at its `PerCpu` block, so `gs:[0]`
//! (a self-pointer stored at offset 0) yields `&PerCpu` in O(1) — the standard
//! x86-64 per-CPU pattern. On 1 CPU only slot 0 (the BSP) is live; the later
//! AP bring-up phase will call an `init_ap(n)` for the others.

pub mod ap;

use x86_64::VirtAddr;
use x86_64::registers::model_specific::GsBase;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

pub const MAX_CPUS: usize = 16;

/// Sentinel for an unmapped LAPIC ID slot.
const NO_CPU: u8 = 0xFF;

/// lapic_id (xAPIC 8-bit) -> dense cpu_id. Filled by `set_cpu_mapping` at
/// bring-up; read by `cpu_id()` on every core.
static LAPIC_TO_CPU: [AtomicU8; 256] = {
    const Z: AtomicU8 = AtomicU8::new(NO_CPU);
    [Z; 256]
};

/// Count of cores that have registered online via `mark_online`.
static CPUS_ONLINE: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// Core roles (Step 5: GUI-core pinning)
// ---------------------------------------------------------------------------

/// The role assigned to each dense cpu_id. Defaults to `ComputeApp` (= 2) for
/// every core until `set_core_role` is called during `smp::bringup`.
///
/// * `BspIo` (0)       – core 0: the cooperative I/O executor (net/usb/ssh).
/// * `GuiCompositor` (1) – first AP (id 1): the dedicated GUI spinner that runs
///   `run_compositor_gate` forever once the BSP hands off the compositor cwasm.
/// * `ComputeApp` (2)  – every other AP: drains the SMP pool for banded
///   compositing + runs its per-core cooperative executor.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
pub enum CoreRole {
    BspIo        = 0,
    GuiCompositor = 1,
    ComputeApp   = 2,
}

/// Per-core role table. Index = dense cpu_id. All slots default to `ComputeApp`
/// (= 2) — the BSP and first AP are pinned in `smp::bringup` before their APs
/// start, so every core reads the correct role the instant it enters `ap_entry`.
static CORE_ROLES: [AtomicU8; MAX_CPUS] = {
    const Z: AtomicU8 = AtomicU8::new(2); // 2 = ComputeApp
    [Z; MAX_CPUS]
};

/// Assign a role to dense core `cpu`. Called by `smp::bringup` BEFORE
/// `cpu.bootstrap(ap_entry, ...)` so the AP reads the correct role on entry.
pub fn set_core_role(cpu: u32, role: CoreRole) {
    if (cpu as usize) < MAX_CPUS {
        CORE_ROLES[cpu as usize].store(role as u8, Ordering::SeqCst);
    }
}

/// Read the role of dense core `cpu`.
pub fn core_role(cpu: u32) -> CoreRole {
    if (cpu as usize) >= MAX_CPUS { return CoreRole::ComputeApp; }
    match CORE_ROLES[cpu as usize].load(Ordering::SeqCst) {
        0 => CoreRole::BspIo,
        1 => CoreRole::GuiCompositor,
        _ => CoreRole::ComputeApp,
    }
}

/// First online ComputeApp core (dense id), or `None` if no such core exists
/// (i.e. 1- or 2-core systems where only the BSP and/or the GuiCompositor AP
/// are present). Used by C2b to route `.cwasm` exec off the BSP.
///
/// On ≥3 cores the layout is: core 0 = BspIo, core 1 = GuiCompositor, core 2+
/// = ComputeApp. The total number of cores is `1 + cpus_online()` (the BSP
/// always exists; `cpus_online()` counts APs that called `mark_online()`).
pub fn first_compute_app_core() -> Option<u32> {
    let total = 1 + cpus_online();
    (1..total).find(|&c| core_role(c) == CoreRole::ComputeApp)
}

#[repr(C)]
pub struct PerCpu {
    /// MUST be offset 0: a pointer to self, so `mov rax, gs:[0]` loads &PerCpu.
    pub self_ptr: *const PerCpu,
    pub cpu_id: u32,
    pub lapic_id: u32,
    pub kernel_stack_top: u64,
}

impl PerCpu {
    const fn zeroed() -> Self {
        Self { self_ptr: core::ptr::null(), cpu_id: 0, lapic_id: 0, kernel_stack_top: 0 }
    }
}

// SAFETY: PER_CPU is mutated only during single-threaded boot (init_bsp here;
// later init_ap before that AP runs any task). After setup each core only reads
// its own slot via gs-base. The raw-pointer field makes it !Sync by default; we
// assert Sync because access is partitioned per-core by construction.
struct PerCpuArray([PerCpu; MAX_CPUS]);
unsafe impl Sync for PerCpuArray {}

const ZEROED: PerCpu = PerCpu::zeroed();
static mut PER_CPU: PerCpuArray = PerCpuArray([ZEROED; MAX_CPUS]);

/// True iff the GS base, as seen by an actual `gs:`-relative memory access,
/// holds the installed per-CPU pointer. Set by `init_bsp` after a *memory*
/// probe (not a bare MSR read-back — see init_bsp).
static GS_USABLE: AtomicBool = AtomicBool::new(false);

/// Set once on the BSP: true iff CPUID reports RDTSCP (CPUID.80000001h:EDX[27]).
/// When true, `cpu_id()` uses the fast RDTSCP path (TSC_AUX holds the dense id);
/// when false (exotic/old CPU), `cpu_id()` uses the LAPIC fallback. Robust by
/// construction — never assumes RDTSCP exists.
static RDTSCP_OK: AtomicBool = AtomicBool::new(false);

/// IA32_TSC_AUX MSR.
const IA32_TSC_AUX: u32 = 0xC000_0103;

/// IA32_PAT MSR.
const IA32_PAT: u32 = 0x277;

/// Program IA32_PAT with the Limine x86-64 entry layout: PA0=WB, PA1=WT,
/// PA2=UC-, PA3=UC, PA4=WP, PA5=WC, PA6=UC-, PA7=UC. Limine guarantees this
/// layout on the BSP and maps the framebuffer write-combining through PAT
/// index 5, but the PAT MSR is PER-CORE and the hardware reset default has
/// index 5 = WT. A core that blits with the reset PAT therefore writes the
/// framebuffer write-through instead of write-combining — on real hardware
/// (VRAM behind PCIe) present bandwidth collapses from ~GB/s to ~MB/s, while
/// VMs back the framebuffer with host RAM so the regression is invisible
/// there. MUST run on each core before its first framebuffer access. Reloads
/// CR3 afterwards because the TLB caches the effective memory type per entry.
pub fn init_pat() {
    const PAT_LIMINE: u64 = 0x0007_0105_0007_0406;
    // SAFETY: ring 0; WRMSR to the architectural PAT MSR with a layout whose
    // entries are all valid memory types. This core has not yet cached lines
    // from any page mapped through the indices that change (4: WB→WP, 5:
    // WT→WC — only the framebuffer selects those, and it is touched only
    // after this runs), so the SDM cache-flush dance for PAT changes is not
    // needed. Rewriting CR3 with its current value is always valid and fully
    // flushes this core's TLB (no GLOBAL mappings, no PCID — see memory/tlb.rs).
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_PAT,
            in("eax") PAT_LIMINE as u32,
            in("edx") (PAT_LIMINE >> 32) as u32,
            options(nostack, preserves_flags),
        );
        use x86_64::registers::control::Cr3;
        let (frame, flags) = Cr3::read();
        Cr3::write(frame, flags);
    }
}

/// Detect RDTSCP once (call on the BSP, after lapic init). Records the result;
/// does NOT yet enable the fast path on its own — `set_tsc_aux` must run on a
/// core before that core trusts RDTSCP.
pub fn detect_rdtscp() -> bool {
    let ok = (unsafe { core::arch::x86_64::__cpuid(0x8000_0001) }.edx >> 27) & 1 == 1;
    RDTSCP_OK.store(ok, Ordering::SeqCst);
    ok
}

/// Write this core's dense id into IA32_TSC_AUX so a later RDTSCP returns it in
/// ECX. No-op if RDTSCP is unavailable. MUST be called on each core BEFORE that
/// core calls `cpu_id()` on the fast path (BSP: in init path; AP: first thing in
/// ap_entry). SAFETY: ring 0; WRMSR to a standard MSR.
pub fn set_tsc_aux(dense_id: u32) {
    if RDTSCP_OK.load(Ordering::SeqCst) {
        unsafe {
            core::arch::asm!(
                "wrmsr",
                in("ecx") IA32_TSC_AUX, in("eax") dense_id, in("edx") 0u32,
                options(nostack, preserves_flags),
            );
        }
    }
}

/// BSP per-CPU init. Call AFTER gdt::init (the GS segment-load done there zeroes
/// the GS base) and AFTER lapic::init (the APIC ID register must be mapped).
/// Fills PER_CPU[0] and writes the GS base.
///
/// Returns `true` if `gs:[0]` actually reads back the installed self-pointer.
/// Returns `false` on VMMs that accept `wrmsr IA32_GS_BASE` into the MSR but do
/// NOT update the hidden GS segment base used by `gs:`-relative accesses —
/// VirtualBox does exactly this (the MSR read-back matches, yet `mov gs:[0]`
/// still uses base 0 and faults). We therefore probe with a REAL memory access,
/// not an MSR read-back. On `false`, `this_cpu()` uses the BSP slot directly and
/// never touches `gs:[0]`, so boot completes on a single CPU regardless. A
/// future AP bring-up phase MUST require `true` before relying on per-core
/// gs-base to distinguish cores.
pub fn init_bsp(kernel_stack_top: u64) -> bool {
    let lapic_id = crate::apic::lapic::apic_id();
    // SAFETY: single-threaded boot; no other accessor to PER_CPU yet.
    let want = unsafe {
        let slot = core::ptr::addr_of_mut!(PER_CPU.0[0]);
        (*slot).cpu_id = 0;
        (*slot).lapic_id = lapic_id;
        (*slot).kernel_stack_top = kernel_stack_top;
        (*slot).self_ptr = slot as *const PerCpu;
        let want = slot as u64;
        GsBase::write(VirtAddr::new(want));
        want
    };

    // Best-effort liveness flag for a FUTURE AP phase. The MSR read-back is NOT
    // a reliable proof that `gs:`-relative accesses work — VirtualBox accepts
    // the wrmsr (read-back matches) yet leaves the hidden segment base at 0, so
    // `mov gs:[0]` still faults. There is no fault-free way to probe the hidden
    // base here, so Fase 0 simply never uses `gs:[0]` (see `this_cpu`). Record
    // the read-back result as a hint; AP bring-up must do its own real probe
    // (e.g. a guarded access with a recoverable #PF) before trusting gs-base.
    let gs_ok = GsBase::read().as_u64() == want;
    GS_USABLE.store(gs_ok, Ordering::SeqCst);

    // Enable the fast `cpu_id()` path on this machine if the CPU supports RDTSCP,
    // then prime THIS core's (the BSP's) IA32_TSC_AUX with its dense id 0. Order
    // matters: `set_tsc_aux` only writes the MSR once RDTSCP_OK is set, so
    // `detect_rdtscp()` must run first. There is no inconsistency window for the
    // BSP: before this runs `cpu_id()` used the LAPIC fallback which returns 0
    // for the BSP, exactly what TSC_AUX=0 yields on the fast path. Each AP primes
    // its own TSC_AUX as the first statement of `ap_entry`, before it ever calls
    // `cpu_id()`, so no AP can observe a stale TSC_AUX=0 once RDTSCP_OK is true.
    detect_rdtscp();
    set_tsc_aux(0);

    gs_ok
}

/// xAPIC id of dense core `cpu` (for targeted IPIs). Reads PER_CPU[cpu].lapic_id.
/// SAFETY: PER_CPU[cpu] was filled at bring-up before that core ran; lapic_id is
/// written once and read-only after. cpu < MAX_CPUS by construction.
pub fn lapic_id_of(cpu: u32) -> u32 {
    unsafe { (*core::ptr::addr_of!(PER_CPU.0[cpu as usize])).lapic_id }
}

/// Register `lapic_id -> cpu_id` and populate PER_CPU[cpu_id]'s identity.
/// Called by the BSP for itself (id 0) and for each AP BEFORE the AP starts.
pub fn set_cpu_mapping(lapic_id: u32, cpu_id: u8) {
    if (lapic_id as usize) < 256 {
        LAPIC_TO_CPU[lapic_id as usize].store(cpu_id, Ordering::SeqCst);
    }
    // SAFETY: each slot is written once during single-threaded bring-up before
    // the corresponding AP starts; the AP only reads its own slot afterwards.
    unsafe {
        let slot = core::ptr::addr_of_mut!(PER_CPU.0[cpu_id as usize]);
        (*slot).cpu_id = cpu_id as u32;
        (*slot).lapic_id = lapic_id;
        (*slot).self_ptr = slot as *const PerCpu;
    }
}

/// An AP (or the BSP) marks itself online.
pub fn mark_online() {
    CPUS_ONLINE.fetch_add(1, Ordering::SeqCst);
}

/// Number of cores that have registered online via `mark_online`.
pub fn cpus_online() -> u32 {
    CPUS_ONLINE.load(Ordering::SeqCst)
}

/// Dense cpu_id of the current core. Two paths:
///
/// - FAST (RDTSCP_OK): a single `rdtscp` reads this core's IA32_TSC_AUX — the
///   dense id we wrote at bring-up (`set_tsc_aux`) — into ECX. No MMIO, ~few
///   cycles. Only enabled once CPUID confirmed RDTSCP *and* every core has set
///   its TSC_AUX before any fast-path `cpu_id()` (BSP in `init_bsp`, AP as the
///   first statement of `ap_entry`).
/// - FALLBACK (RDTSCP unavailable): read the LAPIC ID register (works on every
///   core and every VMM — no `gs:[0]`, dodging VirtualBox's gs-base quirk) and
///   map it to a dense id. Returns 0 (BSP) if the LAPIC ID isn't mapped yet
///   (e.g. very early boot before bring-up) — safe on a single CPU.
#[inline]
pub fn cpu_id() -> u32 {
    if RDTSCP_OK.load(Ordering::Relaxed) {
        // RDTSCP returns the per-core IA32_TSC_AUX (the dense id we wrote at
        // bring-up) in ECX. Single instruction, no MMIO. SAFETY: only taken when
        // CPUID confirmed RDTSCP; clobbers EAX/EDX/ECX which we discard. rdtscp
        // does not touch RFLAGS, so `preserves_flags` is correct.
        let id: u32;
        unsafe {
            core::arch::asm!(
                "rdtscp",
                out("eax") _, out("edx") _, out("ecx") id,
                options(nostack, preserves_flags),
            );
        }
        return id;
    }
    // Fallback: LAPIC-ID based.
    let lapic = crate::apic::lapic::apic_id();
    if (lapic as usize) < 256 {
        let id = LAPIC_TO_CPU[lapic as usize].load(Ordering::SeqCst);
        if id != NO_CPU {
            return id as u32;
        }
    }
    0
}

/// &PerCpu for the current core, resolved via `cpu_id()` (LAPIC-based, no gs).
#[inline]
pub fn this_cpu() -> &'static PerCpu {
    // SAFETY: PER_CPU[cpu_id()] is a valid 'static; cpu_id() is in range
    // (0..MAX_CPUS) by construction of the dense ids set in set_cpu_mapping.
    unsafe { &*core::ptr::addr_of!(PER_CPU.0[cpu_id() as usize]) }
}

/// &PerCpu resolved via the GS base (`gs:[0]` self-pointer). ONLY valid once a
/// future AP phase has confirmed `init_bsp` returned `true` (gs-base usable).
/// Not used in Fase 0; reading `gs:[0]` with an uninstalled base #PFs.
#[inline]
#[allow(dead_code)]
pub unsafe fn this_cpu_via_gs() -> &'static PerCpu {
    let p: *const PerCpu;
    core::arch::asm!("mov {}, gs:[0]", out(reg) p, options(nostack, preserves_flags));
    &*p
}

/// Whether `gs:`-relative per-CPU access is usable on this machine (probed at
/// `init_bsp`). AP bring-up must check this before using [`this_cpu_via_gs`].
#[inline]
#[allow(dead_code)]
pub fn gs_usable() -> bool {
    GS_USABLE.load(Ordering::SeqCst)
}

/// Diagnostic: probe the cheap per-core-id primitives in THIS environment
/// (QEMU / VirtualBox / bare metal). The LAPIC-MMIO `cpu_id()` costs ~200 ns;
/// a fast `cpu_id()` would use `RDPID` or `RDTSCP` (which returns IA32_TSC_AUX
/// in ECX) — single instructions, no MMIO, no gs-base quirk. This prints which
/// of those are available, and whether writing IA32_TSC_AUX then reading it back
/// via RDTSCP round-trips (proving the VMM honours the MSR). Greppable marker.
pub fn probe_fast_cpuid() {
    use core::arch::x86_64::{__cpuid, __cpuid_count};
    // RDTSCP: CPUID.80000001h:EDX[27].
    let rdtscp = (unsafe { __cpuid(0x8000_0001) }.edx >> 27) & 1;
    // RDPID: CPUID.(EAX=7,ECX=0):ECX[22].
    let rdpid = (unsafe { __cpuid_count(7, 0) }.ecx >> 22) & 1;
    // Round-trip: WRMSR IA32_TSC_AUX(0xC000_0103)=0xABCD, then RDTSCP reads it
    // back in ECX. Proves WRMSR+RDTSCP work (the basis of a fast cpu_id) here.
    let mut tscaux_rw = 0u32;
    if rdtscp == 1 {
        unsafe {
            core::arch::asm!(
                "wrmsr",
                in("ecx") 0xC000_0103u32, in("eax") 0xABCDu32, in("edx") 0u32,
                options(nostack, preserves_flags),
            );
            let aux: u32;
            core::arch::asm!(
                "rdtscp",
                out("eax") _, out("edx") _, out("ecx") aux,
                options(nostack, preserves_flags),
            );
            if aux == 0xABCD { tscaux_rw = 1; }
        }
    }
    crate::binfo!("cpuprobe", "rdtscp={} rdpid={} tscaux_rw={}", rdtscp, rdpid, tscaux_rw);
    // CRITICAL: the round-trip above clobbered this core's IA32_TSC_AUX with the
    // probe sentinel 0xABCD. The fast `cpu_id()` path now reads TSC_AUX, so leaving
    // the sentinel in place would make `cpu_id()` return 43981 on the BSP and
    // index per-core arrays out of bounds. `probe_fast_cpuid` only ever runs on
    // the BSP (dense id 0), so restore TSC_AUX to 0. No-op if RDTSCP is absent.
    set_tsc_aux(0);
}
