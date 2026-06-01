//! SMP bring-up: start the enumerated Application Processors via Limine's
//! MpRequest and wait until they're online. Fase 1 parks each AP in `hlt`
//! (see `cpu::ap::ap_entry`); no scheduler/IRQs yet (Fase 2).

/// Start all non-BSP CPUs and wait (bounded) for them to register online.
pub fn bringup() {
    // `.response()` returns `Option<&'static MpResponse>`.
    // `MpResponse = Response<MpRespData>`, and `Response<T>: Deref<Target=T>`,
    // so `.bsp_lapic_id` and `.cpus()` auto-deref to `MpRespData` fields.
    let resp = match crate::MP_REQUEST.response() {
        Some(r) => r,
        None => {
            crate::binfo!("smp", "no MP response — single CPU");
            return;
        }
    };

    let bsp_lapic = resp.bsp_lapic_id;
    // BSP is cpu_id 0; map it so cpu_id() resolves on the BSP too.
    crate::cpu::set_cpu_mapping(bsp_lapic, 0);

    let mut next_id: u8 = 1;
    let mut started: u32 = 0;

    // `.cpus()` returns `&[&MpInfo]`; iterating yields `&&MpInfo` but
    // field access and method calls auto-deref — no explicit deref needed.
    for cpu in resp.cpus() {
        if cpu.lapic_id == bsp_lapic {
            continue; // skip the BSP — it's already running
        }
        if (next_id as usize) >= crate::cpu::MAX_CPUS {
            crate::bwarn!(
                "smp",
                "more CPUs than MAX_CPUS={}; rest parked",
                crate::cpu::MAX_CPUS
            );
            break;
        }
        let id = next_id;
        next_id += 1;

        // Fill PER_CPU[id] + LAPIC->cpu mapping BEFORE the AP starts so
        // the AP's cpu_id()/gdt::init(cpu_id) see a valid mapping the
        // instant it runs.
        crate::cpu::set_cpu_mapping(cpu.lapic_id, id);

        // Hand the AP its dense cpu_id via the extra argument.
        cpu.bootstrap(crate::cpu::ap::ap_entry, id as u64);
        started += 1;
    }

    // Bounded wait for all started APs to reach ap_entry and call mark_online.
    let mut spins: u64 = 0;
    while crate::cpu::cpus_online() < started && spins < 200_000_000 {
        core::hint::spin_loop();
        spins += 1;
    }

    let online = crate::cpu::cpus_online();
    if started == 0 {
        crate::binfo!("smp", "no APs to start (1 CPU)");
    } else if online == started {
        crate::binfo!("smp", "{}/{} APs online", online, started);
    } else {
        crate::bwarn!("smp", "{}/{} APs online (timeout)", online, started);
    }
}
