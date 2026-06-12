//! MT Fase 2 — scheduler dei wasm-thread come fiber cooperativi M:N.
//! Spec: docs/superpowers/specs/2026-06-12-wasm-mt-fase2-threads-design.md
//!
//! I fiber girano sui core ComputeApp dentro `run_core()` (fallback BSP sui
//! sistemi 1-2 core); cedono SOLO a `atomic.wait` (hook futex, Task 4),
//! host-call bloccante o return. Il TLS wasmtime per-core va swappato a ogni
//! suspend/resume: l'activation chain (CallThreadState) di una call sospesa
//! vive nello stack del fiber, quindi il puntatore TLS viaggia con lui — è
//! anche ciò che permette la migrazione di un fiber su un core diverso.
//!
//! Protocollo park/wake (anti lost-wakeup): il fiber che si parcheggia setta
//! `park_key` e si sospende; è `run_one` — che POSSIEDE la Box — a spostarlo
//! in WAITQ al ritorno del resume. Un wake che arriva in quella finestra non
//! trova il waiter nello shard e lascia un "credito" (`credits`): `run_one`
//! consuma il credito PRIMA di inserire in WAITQ e in tal caso il fiber resta
//! runnable. Un credito stantio al più produce un wake spurio (il chiamante
//! futex ricontrolla il valore e si ri-parcheggia).

use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use crate::sync::IrqMutex;

/// Stack nativo di ogni fiber: frame cranelift + host call (max_wasm_stack=512K).
pub const FIBER_STACK_SIZE: usize = 2 * 1024 * 1024;
const WAITQ_SHARDS: usize = 16;

/// Tipo Suspend concreto dei nostri fiber: Resume=(), Yield=(), Return=exit code.
type FiberSuspend = wasmtime_internal_fiber::Suspend<(), (), i32>;

/// Un fiber-thread registrato. Lo stato di esecuzione (Store, Instance,
/// activation wasmtime) vive nello stack del fiber stesso.
struct ThreadFiber {
    fiber: wasmtime_internal_fiber::Fiber<'static, (), (), i32>,
    /// TLS wasmtime salvato mentre il fiber è sospeso (vedi `run_one`).
    saved_tls: *mut u8,
    /// `&mut Suspend` pubblicato dal corpo del fiber al primo run
    /// (`publish_suspend`) — serve a `park_current` per sospendere.
    suspend_ptr: AtomicUsize,
    /// Gruppo dell'app threaded; `None` per i fiber host-only (self-test).
    group: Option<Arc<ThreadGroup>>,
    tid: u32,
    /// Chiave futex su cui il fiber vuole parcheggiarsi (0 = nessuna).
    /// Scritta dal fiber in `park_current`, letta da `run_one` dopo il suspend.
    park_key: usize,
    /// Deadline del park in tick (u64::MAX = infinito). Riscatto nel Task 4.
    park_deadline: u64,
}

// SAFETY: `Fiber` è !Send (contiene Cell) e i raw pointer non sono Send, ma un
// ThreadFiber è posseduto ESCLUSIVAMENTE da un contesto alla volta: o dentro
// RUNQ/WAITQ (fermo), o dentro run_one su un solo core (in esecuzione). Il
// passaggio tra code avviene sotto IrqMutex, che sincronizza la memoria.
unsafe impl Send for ThreadFiber {}

/// Stato condiviso di UNA app threaded (1 Module + 1 SharedMemory + N thread).
/// Creato da `exec_threaded`; usato da `spawn_fiber` e dal `thread-spawn`
/// host fn (spawn reale nel Task 5).
pub struct ThreadGroup {
    pub module: wasmtime::Module,
    pub linker: Arc<wasmtime::Linker<crate::wasm::wt::state::WtState>>,
    /// La linear memory condivisa del gruppo, già definita come `env::memory`
    /// nel linker (engine-scoped: vale per ogni Store/instantiate del gruppo).
    /// Tenuta qui anche solo per ancorarne la vita a quella del gruppo.
    #[allow(dead_code)]
    pub shared: wasmtime::SharedMemory,
    #[allow(dead_code)] // allocatore tid per thread-spawn (Task 5)
    pub next_tid: AtomicU32,
    pub live: AtomicU32,
    /// Trap in un thread → muore tutto il gruppo (kill-group, Task 7).
    pub poisoned: AtomicBool,
    /// Exit code del main (tid 0).
    pub exit: IrqMutex<Option<i32>>,
    /// Core (dense id) che attende la fine del gruppo dentro `exec_threaded`,
    /// da svegliare quando `live` arriva a 0.
    pub waiter_core: AtomicU32,
    pub base_args: Vec<Vec<u8>>,
    /// Environ "K=V" del gruppo (RAYON_NUM_THREADS iniettato da exec_threaded).
    pub env: Vec<Vec<u8>>,
}

/// Run-queue globale dei fiber runnable. Per-core sharding = ottimizzazione
/// futura: la contesa è bassa (enqueue solo a spawn/notify, non per-poll).
static RUNQ: IrqMutex<VecDeque<Box<ThreadFiber>>> = IrqMutex::new(VecDeque::new());

struct ShardState {
    /// Fiber parcheggiati: (chiave futex, deadline tick, fiber).
    waiters: Vec<(usize, u64, Box<ThreadFiber>)>,
    /// Crediti wake per chiave: notify arrivato mentre il parker era "in volo"
    /// (tra park_current e l'inserimento in WAITQ da parte di run_one).
    credits: Vec<(usize, u32)>,
}

/// Shard della wait-queue futex. align(64) anti false-sharing tra shard.
#[repr(align(64))]
struct WaitShard(IrqMutex<ShardState>);

static WAITQ: [WaitShard; WAITQ_SHARDS] = [const {
    WaitShard(IrqMutex::new(ShardState { waiters: Vec::new(), credits: Vec::new() }))
}; WAITQ_SHARDS];

#[inline]
fn shard(key: usize) -> &'static WaitShard {
    // >>3: le chiavi sono indirizzi di parole atomiche — scarta i bit bassi.
    &WAITQ[(key >> 3) % WAITQ_SHARDS]
}

/// Fiber correntemente in esecuzione su ogni core (0 = nessuno). Letto da
/// `publish_suspend`/`park_current` per ritrovare il proprio ThreadFiber.
static CURRENT: [AtomicUsize; crate::cpu::MAX_CPUS] =
    [const { AtomicUsize::new(0) }; crate::cpu::MAX_CPUS];

/// Predicato wake-source per `run_core`: c'è lavoro fiber in coda?
pub fn runnable_empty() -> bool {
    RUNQ.lock().is_empty()
}

/// I fiber girano sui core ComputeApp; fallback BSP quando non ne esistono
/// (sistemi a 1-2 core, dove core 1 — se c'è — è il GuiCompositor).
pub fn core_allowed(cpu: u32) -> bool {
    match crate::cpu::core_role(cpu) {
        crate::cpu::CoreRole::ComputeApp => true,
        crate::cpu::CoreRole::BspIo => crate::cpu::first_compute_app_core().is_none(),
        _ => false,
    }
}

/// Accoda un fiber runnable e sveglia i core dormienti (stesso pattern di
/// `pool::submit`: broadcast VEC_WAKE, i core non-allowed lo ignorano).
fn enqueue_runnable(f: Box<ThreadFiber>) {
    RUNQ.lock().push_back(f);
    crate::apic::lapic::send_ipi_all_but_self(crate::idt::VEC_WAKE);
}

/// Drena ed esegue UN fiber runnable. Chiamato da `run_core` sui core
/// abilitati (`core_allowed`). Ritorna true se ha eseguito qualcosa.
pub fn run_one(cpu: u32) -> bool {
    let mut f = match RUNQ.lock().pop_front() {
        Some(f) => f,
        None => return false,
    };
    // TLS swap: dentro va il TLS del fiber (activation chain sospesa nel suo
    // stack), fuori si ripristina quello del core. Vedi doc-comment in testa.
    let prev = crate::wasm::wt::platform::tls_raw_get();
    crate::wasm::wt::platform::tls_raw_set(f.saved_tls);
    CURRENT[cpu as usize].store(&mut *f as *mut ThreadFiber as usize, Ordering::SeqCst);
    let done = f.fiber.resume(());
    CURRENT[cpu as usize].store(0, Ordering::SeqCst);
    f.saved_tls = crate::wasm::wt::platform::tls_raw_get();
    crate::wasm::wt::platform::tls_raw_set(prev);
    match done {
        // Return: il fiber è finito.
        Ok(code) => finish_fiber(f, code),
        // Suspend: park richiesto da park_current (o spurio) — è QUI che la
        // Box si sposta in WAITQ, mai dentro il fiber (vedi doc-comment).
        Err(()) => {
            let key = f.park_key;
            if key == 0 {
                // Suspend senza park registrato: resta runnable.
                RUNQ.lock().push_back(f);
            } else {
                f.park_key = 0;
                let deadline = f.park_deadline;
                let mut s = shard(key).0.lock();
                let credited = if let Some(pos) = s.credits.iter().position(|e| e.0 == key) {
                    s.credits[pos].1 -= 1;
                    if s.credits[pos].1 == 0 {
                        s.credits.swap_remove(pos);
                    }
                    true
                } else {
                    false
                };
                if credited {
                    // Un notify ha incrociato il park in volo: resta runnable.
                    drop(s);
                    RUNQ.lock().push_back(f);
                } else {
                    s.waiters.push((key, deadline, f));
                    if deadline != u64::MAX {
                        TIMED_WAITERS.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        }
    }
    true
}

/// Chiusura di un fiber terminato (return o trap già tradotta in exit code).
fn finish_fiber(f: Box<ThreadFiber>, code: i32) {
    if let Some(g) = f.group.as_ref() {
        if f.tid == 0 {
            *g.exit.lock() = Some(code);
        }
        if g.live.fetch_sub(1, Ordering::SeqCst) == 1 {
            // Ultimo thread del gruppo: sveglia il core che attende in
            // exec_threaded (se ne accorge dal live==0).
            crate::executor::wake_core(g.waiter_core.load(Ordering::SeqCst));
        }
    }
    // (Task 6: unregister da proc/ps.)
}

/// Pubblica l'handle Suspend del fiber corrente (chiamata come PRIMA cosa dal
/// corpo di ogni fiber). Il fiber non ha accesso diretto alla propria Box:
/// passa da CURRENT, che run_one ha appena settato su questo core.
fn publish_suspend(sus: &mut FiberSuspend) {
    let me = CURRENT[crate::cpu::cpu_id() as usize].load(Ordering::SeqCst) as *mut ThreadFiber;
    debug_assert!(!me.is_null(), "publish_suspend outside run_one");
    if !me.is_null() {
        // SAFETY: `me` punta alla Box viva posseduta da run_one su questo core
        // per tutta la durata del resume corrente.
        unsafe { (*me).suspend_ptr.store(sus as *mut FiberSuspend as usize, Ordering::SeqCst) };
    }
}

/// Parcheggia il fiber corrente sulla chiave `key` e sospende. Ritorna dopo un
/// wake (`wake_key`). Riusato dall'hook futex (Task 4) con le sue deadline;
/// `deadline` in tick (u64::MAX = infinito), riscatto timeout nel Task 4.
fn park_current(key: usize, deadline: u64) -> bool {
    let me = CURRENT[crate::cpu::cpu_id() as usize].load(Ordering::SeqCst) as *mut ThreadFiber;
    if me.is_null() {
        // Contesto non-fiber: niente da parcheggiare (il chiamante degrada a spin).
        return false;
    }
    // SAFETY: come publish_suspend — Box viva, posseduta da run_one, un solo
    // core la tocca. Il suspend rientra in run_one che la sposta in WAITQ.
    unsafe {
        (*me).park_key = key;
        (*me).park_deadline = deadline;
        let sus = (*me).suspend_ptr.load(Ordering::SeqCst) as *mut FiberSuspend;
        debug_assert!(!sus.is_null(), "park_current before publish_suspend");
        (*sus).suspend(());
    }
    true
}

/// Sveglia fino a `count` fiber parcheggiati su `key`; ritorna quanti ne ha
/// effettivamente rimessi in RUNQ. Il resto del budget diventa credito per i
/// parker in volo (vedi doc-comment in testa). Base dell'hook notify (Task 4).
fn wake_key(key: usize, count: u32) -> u32 {
    let mut woken = 0u32;
    let mut to_run: Vec<Box<ThreadFiber>> = Vec::new();
    {
        let mut s = shard(key).0.lock();
        let mut i = 0;
        while i < s.waiters.len() && woken < count {
            if s.waiters[i].0 == key {
                let (_, d, f) = s.waiters.swap_remove(i);
                if d != u64::MAX {
                    TIMED_WAITERS.fetch_sub(1, Ordering::SeqCst);
                }
                to_run.push(f);
                woken += 1;
            } else {
                i += 1;
            }
        }
        if woken < count && count > 0 {
            // Nessun (altro) waiter parcheggiato: lascia UN credito per il
            // parker eventualmente in volo. Non accumulare l'intero budget
            // residuo (notify con count alto e zero waiter è il caso comune):
            // un credito basta a coprire la finestra, e al più costa un wake
            // spurio al prossimo park su questa chiave.
            match s.credits.iter().position(|e| e.0 == key) {
                Some(_) => {} // credito già presente: non gonfiarlo
                None => s.credits.push((key, 1)),
            }
        }
    } // lock shard rilasciato PRIMA di toccare RUNQ (niente nesting tra lock)
    if !to_run.is_empty() {
        let mut q = RUNQ.lock();
        for f in to_run {
            q.push_back(f);
        }
        drop(q);
        crate::apic::lapic::send_ipi_all_but_self(crate::idt::VEC_WAKE);
    }
    woken
}

// ---------------------------------------------------------------------------
// Hook futex (Task 4) — chiamati dal fork wasmtime (third_party/wasmtime45,
// shared_memory.rs) come back-end di memory.atomic.{wait32,wait64,notify}.
// `addr` è un puntatore HOST già validato dal runtime (in-bounds, allineato).
// Contratto wait: 0 = woken, 1 = not-equal, 2 = timed-out; timeout_ns < 0 =
// infinito. notify ritorna il numero di waiter risvegliati.
// ---------------------------------------------------------------------------

/// Spin adattivo prima del park: le critical section medie (mutex guest) sono
/// più corte di un ciclo suspend → IPI → resume.
const SPIN_ITERS: u32 = 200;

/// Waiter parcheggiati CON timeout. Pre-filtro O(1) di `expire_timeouts`:
/// senza waiter a tempo (caso comune: wait infiniti) il riscatto non scansiona
/// nulla. Mantenuto SOLO ai siti insert/remove sotto il lock dello shard.
static TIMED_WAITERS: AtomicU32 = AtomicU32::new(0);

#[no_mangle]
pub extern "C" fn wasmtime_futex_wait32(addr: *const u32, expected: u32, timeout_ns: i64) -> u32 {
    // SAFETY: addr validato dal chiamante (vedi header); load 4-allineato.
    futex_wait(addr as usize, timeout_ns,
        move || unsafe { core::ptr::read_volatile(addr) } != expected)
}

#[no_mangle]
pub extern "C" fn wasmtime_futex_wait64(addr: *const u64, expected: u64, timeout_ns: i64) -> u32 {
    // SAFETY: come wait32; load 8-allineato.
    futex_wait(addr as usize, timeout_ns,
        move || unsafe { core::ptr::read_volatile(addr) } != expected)
}

#[no_mangle]
pub extern "C" fn wasmtime_futex_notify(addr: *const u8, count: u32) -> u32 {
    wake_key(addr as usize, count)
}

fn futex_wait(key: usize, timeout_ns: i64, not_equal: impl Fn() -> bool) -> u32 {
    // Fast path: spin con PAUSE ricontrollando il valore.
    for _ in 0..SPIN_ITERS {
        if not_equal() {
            return 1;
        }
        core::hint::spin_loop();
    }
    // ns → tick 100 Hz, arrotondando in SU: un timeout > 0 attende ≥ 1 tick.
    let deadline = if timeout_ns < 0 {
        u64::MAX
    } else {
        crate::timer::ticks() + ((timeout_ns as u64) / 10_000_000).max(1)
    };
    {
        // Ricontrollo sotto il lock dello shard: serializza col percorso
        // notify (wake_key prende lo stesso lock). La finestra residua —
        // notify tra questo unlock e l'inserimento in WAITQ da parte di
        // run_one — è coperta dai crediti (vedi doc-comment in testa al file).
        let _s = shard(key).0.lock();
        if not_equal() {
            return 1;
        }
    }
    if !park_current(key, deadline) {
        // Contesto non-fiber: NON deve succedere (i moduli threaded girano
        // sempre su fiber), ma non deve nemmeno bloccare il kernel.
        crate::bwarn!("wt", "futex wait outside a fiber: degraded to spin");
        while !not_equal() {
            core::hint::spin_loop();
        }
        return 1;
    }
    // Risvegliato: da notify (0) o da expire_timeouts a deadline scaduta (2).
    if deadline != u64::MAX && crate::timer::ticks() >= deadline { 2 } else { 0 }
}

/// Riscatta i waiter futex col timeout scaduto: re-enqueue in RUNQ (il loro
/// `futex_wait` ritorna 2 = timed-out). Chiamato da `run_core` e dal wait-loop
/// di `exec_threaded` a ogni giro — costo ~0 senza waiter a tempo.
pub fn expire_timeouts() {
    if TIMED_WAITERS.load(Ordering::SeqCst) == 0 {
        return;
    }
    let now = crate::timer::ticks();
    let mut expired: Vec<Box<ThreadFiber>> = Vec::new();
    for sh in WAITQ.iter() {
        let mut s = sh.0.lock();
        let mut i = 0;
        while i < s.waiters.len() {
            if s.waiters[i].1 <= now {
                let (_, _, f) = s.waiters.swap_remove(i);
                TIMED_WAITERS.fetch_sub(1, Ordering::SeqCst);
                expired.push(f);
            } else {
                i += 1;
            }
        }
    }
    if !expired.is_empty() {
        let mut q = RUNQ.lock();
        for f in expired {
            q.push_back(f);
        }
        drop(q);
        crate::apic::lapic::send_ipi_all_but_self(crate::idt::VEC_WAKE);
    }
}

// ---------------------------------------------------------------------------
// exec_threaded — esecuzione di un modulo wasm32-wasip1-threads (Task 3).
// ---------------------------------------------------------------------------

/// Esegue un modulo threaded: gruppo + SharedMemory + linker condiviso, main
/// (`_start`) su fiber tid=0, attesa cooperativa della fine del gruppo.
/// Chiamato SINCRONO da `run_cwasm` (su un AP via run_app_on_core, o inline
/// sul BSP nei sistemi 1-2 core), come ogni altro exec `.cwasm`.
pub fn exec_threaded(
    module: &wasmtime::Module,
    args: Vec<Vec<u8>>,
    pts: Option<usize>,
) -> i32 {
    let engine = crate::wasm::wt::engine();
    let mem_ty = match module.imports().find_map(|i| {
        if i.module() == "env" && i.name() == "memory" { i.ty().memory().cloned() } else { None }
    }) {
        Some(t) => t,
        None => return 126, // il chiamante ha già verificato l'import shared
    };
    let shared = match wasmtime::SharedMemory::new(engine, mem_ty) {
        Ok(s) => s,
        Err(e) => {
            crate::kprintln!("ruos: wt exec_threaded SharedMemory: {:?}", e);
            return 126;
        }
    };
    // Linker condiviso del gruppo: stessa superficie di run_cwasm + thread-spawn.
    let mut linker: wasmtime::Linker<crate::wasm::wt::state::WtState> =
        wasmtime::Linker::new(engine);
    if let Err(e) = crate::wasm::wt::wasi::add_to_linker(&mut linker) {
        crate::kprintln!("ruos: wt exec_threaded wasi link: {}", e);
        return 126;
    }
    if let Err(e) = crate::wasm::wt::gfx::add_to_linker(&mut linker) {
        crate::kprintln!("ruos: wt exec_threaded gfx link: {}", e);
        return 126;
    }
    if let Err(e) = crate::wasm::wt::gui::add_to_linker(&mut linker) {
        crate::kprintln!("ruos: wt exec_threaded gui link: {}", e);
        return 126;
    }
    if let Err(e) = add_thread_spawn_to_linker(&mut linker) {
        crate::kprintln!("ruos: wt exec_threaded thread-spawn link: {}", e);
        return 126;
    }
    // env::memory definita UNA volta: SharedMemory è engine-scoped, quindi il
    // define vale per ogni Store/instantiate del gruppo (la store qui serve
    // solo da contesto per la firma di Linker::define).
    {
        let throwaway = wasmtime::Store::new(engine, crate::wasm::wt::state::WtState::new(Vec::new()));
        if let Err(e) = linker.define(&throwaway, "env", "memory", shared.clone()) {
            crate::kprintln!("ruos: wt exec_threaded define memory: {:?}", e);
            return 126;
        }
    }
    // rayon non può scoprire i core (available_parallelism = Unsupported su
    // wasi): inietta il parallelismo reale = numero di core ComputeApp.
    let total = 1 + crate::cpu::cpus_online();
    let ncomp = total.saturating_sub(2).max(1);
    let mut env: Vec<Vec<u8>> = Vec::new();
    env.push(alloc::format!("RAYON_NUM_THREADS={}", ncomp).into_bytes());
    let cpu = crate::cpu::cpu_id();
    let group = Arc::new(ThreadGroup {
        module: module.clone(),
        linker: Arc::new(linker),
        shared,
        next_tid: AtomicU32::new(1),
        live: AtomicU32::new(0),
        poisoned: AtomicBool::new(false),
        exit: IrqMutex::new(None),
        waiter_core: AtomicU32::new(cpu),
        base_args: args,
        env,
    });
    // main = tid 0
    spawn_fiber(group.clone(), 0, 0, pts);
    // Attesa cooperativa della fine del gruppo, DRENANDO i fiber da questo
    // stesso core: exec_threaded occupa sync il suo core ComputeApp (o il BSP
    // inline su 1-2 core) — se non drenasse, su un sistema con UN solo core
    // abilitato il main fiber non girerebbe mai (deadlock). Gli altri core
    // ComputeApp rubano comunque dalla RUNQ globale dentro run_core.
    loop {
        if group.live.load(Ordering::SeqCst) == 0 {
            let code = *group.exit.lock();
            return code.unwrap_or(if group.poisoned.load(Ordering::SeqCst) { 134 } else { 0 });
        }
        if core_allowed(cpu) {
            // Anche il riscatto timeout: su 1-2 core (BSP inline) nessun
            // run_core gira mentre questo loop blocca il core.
            expire_timeouts();
            while run_one(cpu) {}
            if group.live.load(Ordering::SeqCst) == 0 {
                continue;
            }
        }
        // Tutti i fiber parcheggiati/altrove: dormi fino a timer (100 Hz) o
        // IPI wake (enqueue_runnable / wake_key / finish_fiber → waiter_core).
        x86_64::instructions::hlt();
    }
}

/// Crea il fiber di UN thread (main tid=0 o spawned tid>0) e lo accoda
/// runnable. Il corpo del fiber costruisce Store+WtState, istanzia il modulo
/// del gruppo contro la SharedMemory condivisa e chiama `_start` (main) o
/// `wasi_thread_start(tid, start_arg)` (thread, Task 5).
fn spawn_fiber(group: Arc<ThreadGroup>, tid: u32, start_arg: i32, pts: Option<usize>) {
    group.live.fetch_add(1, Ordering::SeqCst);
    let stack = match wasmtime_internal_fiber::FiberStack::new(FIBER_STACK_SIZE, false) {
        Ok(s) => s,
        Err(e) => {
            crate::kprintln!("ruos: wt spawn_fiber stack: {:?}", e);
            group.poisoned.store(true, Ordering::SeqCst);
            group.live.fetch_sub(1, Ordering::SeqCst);
            return;
        }
    };
    let g = group.clone();
    let fiber = match wasmtime_internal_fiber::Fiber::new(stack, move |_: (), sus| -> i32 {
        publish_suspend(sus);
        let engine = crate::wasm::wt::engine();
        let mut state = crate::wasm::wt::state::WtState::new(g.base_args.clone());
        state.env = g.env.clone();
        state.threads = Some(g.clone());
        // Solo il main eredita il PTY del chiamante (stesso wiring di run_cwasm).
        if tid == 0 {
            if let Some(n) = pts {
                let path = alloc::format!("/dev/pts/{}", n);
                if let Ok(fd) = crate::vfs::block_on(
                    crate::vfs::open(&path, crate::vfs::OpenFlags::WRITE)) {
                    state.stdout_pty = Some(fd);
                }
            }
        }
        let mut store = wasmtime::Store::new(engine, state);
        // Niente deadline assoluta: un thread può restare parcheggiato a lungo
        // e una deadline trapperebbe al resume (deviazione documentata dalla
        // spec §7 — la protezione del desktop resta strutturale: un runaway
        // occupa UN core ComputeApp, la GUI sul core 1 non è toccata).
        store.set_epoch_deadline(crate::wasm::wt::NO_DEADLINE_TICKS);
        let inst = match g.linker.instantiate(&mut store, &g.module) {
            Ok(i) => i,
            Err(e) => {
                crate::kprintln!("ruos: wt thread tid={} instantiate: {:?}", tid, e);
                g.poisoned.store(true, Ordering::SeqCst);
                return 126;
            }
        };
        // DF=0 prima di entrare nel guest (convenzione di ogni call site wt).
        #[cfg(target_arch = "x86_64")]
        unsafe { core::arch::asm!("cld", options(nostack)); }
        let r = if tid == 0 {
            inst.get_typed_func::<(), ()>(&mut store, "_start")
                .and_then(|f| f.call(&mut store, ()))
        } else {
            inst.get_typed_func::<(i32, i32), ()>(&mut store, "wasi_thread_start")
                .and_then(|f| f.call(&mut store, (tid as i32, start_arg)))
        };
        let code = match r {
            Ok(()) => store.data().exit.unwrap_or(0),
            Err(e) => match store.data().exit {
                Some(c) => c, // proc_exit trappa per srotolare: non è un errore
                None => {
                    crate::bwarn!("wt", "thread tid={} trap: {:?} — group poisoned", tid, e);
                    g.poisoned.store(true, Ordering::SeqCst);
                    134
                }
            },
        };
        if let Some(fd) = store.data().stdout_pty {
            let _ = crate::vfs::block_on(crate::vfs::close(fd));
        }
        code
    }) {
        Ok(f) => f,
        Err(e) => {
            crate::kprintln!("ruos: wt spawn_fiber Fiber::new: {:?}", e);
            group.poisoned.store(true, Ordering::SeqCst);
            group.live.fetch_sub(1, Ordering::SeqCst);
            return;
        }
    };
    enqueue_runnable(Box::new(ThreadFiber {
        fiber,
        saved_tls: core::ptr::null_mut(),
        suspend_ptr: AtomicUsize::new(0),
        group: Some(group),
        tid,
        park_key: 0,
        park_deadline: 0,
    }));
}

/// Registra l'import wasi-threads: modulo `"wasi"`, field `"thread-spawn"`
/// (TRATTINO), `(param i32) (result i32)`. Spawn = nuovo fiber accodato
/// runnable (lo prende il primo core libero, NON esegue inline): fresh
/// Instance dello stesso Module sulla STESSA SharedMemory, entry
/// `wasi_thread_start(tid, start_arg)` (stack pointer + TLS del thread sono
/// affare del guest, preparati da pthread_create nel blocco `start_arg`).
/// Ritorna il tid > 0, o -1 su errore (pthread_create → EAGAIN guest-side).
pub fn add_thread_spawn_to_linker(
    linker: &mut wasmtime::Linker<crate::wasm::wt::state::WtState>,
) -> wasmtime::Result<()> {
    linker.func_wrap("wasi", "thread-spawn",
        |caller: wasmtime::Caller<'_, crate::wasm::wt::state::WtState>, start_arg: i32| -> i32 {
            let g = match caller.data().threads.clone() {
                Some(g) => g,
                None => return -1, // modulo non threaded: niente gruppo
            };
            if g.poisoned.load(Ordering::SeqCst) {
                return -1; // gruppo morente: non rianimarlo
            }
            let tid = g.next_tid.fetch_add(1, Ordering::SeqCst);
            if tid >= (1 << 29) {
                return -1; // range tid valido wasi-threads: [1, 2^29)
            }
            crate::binfo!("wt", "thread-spawn tid={} live={}", tid,
                          g.live.load(Ordering::SeqCst) + 1);
            spawn_fiber(g, tid, start_arg, None);
            tid as i32
        })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Gate 3 (boot-checks): atomic.wait sospende il FIBER, notify risveglia (IPI).
// ---------------------------------------------------------------------------

/// Gate 3 MT Fase 2: due fiber sullo stesso modulo + SharedMemory — `waiter`
/// fa `memory.atomic.wait32` (sospende il SUO fiber: il core resta libero e
/// può eseguire il waker), `waker` scrive il payload e fa notify (IPI).
/// Ritorna true sse il waiter esce con 7 (il payload letto DOPO il wake).
#[cfg(feature = "boot-checks")]
pub fn gate3_run(cwasm: &[u8]) -> bool {
    let engine = crate::wasm::wt::engine();
    // SAFETY: cwasm prodotto da tools/wt-precompile per questa exact config.
    let module = match unsafe { wasmtime::Module::deserialize(engine, cwasm) } {
        Ok(m) => m,
        Err(e) => { crate::kprintln!("ruos: gate3 deserialize: {:?}", e); return false; }
    };
    let mem_ty = match module.imports().find_map(|i| i.ty().memory().cloned()) {
        Some(t) => t,
        None => { crate::kprintln!("ruos: gate3: no memory import"); return false; }
    };
    let shared = match wasmtime::SharedMemory::new(engine, mem_ty) {
        Ok(s) => s,
        Err(e) => { crate::kprintln!("ruos: gate3 SharedMemory: {:?}", e); return false; }
    };
    let mut linker: wasmtime::Linker<crate::wasm::wt::state::WtState> =
        wasmtime::Linker::new(engine);
    {
        let throwaway =
            wasmtime::Store::new(engine, crate::wasm::wt::state::WtState::new(Vec::new()));
        if let Err(e) = linker.define(&throwaway, "env", "memory", shared.clone()) {
            crate::kprintln!("ruos: gate3 define: {:?}", e);
            return false;
        }
    }
    let cpu = crate::cpu::cpu_id();
    let group = Arc::new(ThreadGroup {
        module,
        linker: Arc::new(linker),
        shared,
        next_tid: AtomicU32::new(1),
        live: AtomicU32::new(0),
        poisoned: AtomicBool::new(false),
        exit: IrqMutex::new(None),
        waiter_core: AtomicU32::new(cpu),
        base_args: Vec::new(),
        env: Vec::new(),
    });
    // waiter = tid 0: il suo valore di ritorno finisce in group.exit.
    spawn_fiber_export(group.clone(), 0, "waiter");
    spawn_fiber_export(group.clone(), 1, "waker");
    // Come fiber_self_test: con ≥3 core drenano gli AP; senza ComputeApp è il
    // BSP (questa fase di boot) a dover drenare da sé.
    let deadline = crate::timer::ticks() + 500; // 5 s
    while group.live.load(Ordering::SeqCst) != 0 {
        if core_allowed(cpu) {
            expire_timeouts();
            while run_one(cpu) {}
        }
        if crate::timer::ticks() > deadline {
            crate::kprintln!(
                "ruos: gate3 timeout (live={})", group.live.load(Ordering::SeqCst));
            return false;
        }
        core::hint::spin_loop();
    }
    let code = *group.exit.lock();
    if code != Some(7) {
        crate::kprintln!("ruos: gate3 waiter exit = {:?} (want Some(7))", code);
    }
    code == Some(7)
}

/// Gate 2 MT Fase 2: il main (export `run`, su fiber) chiama l'import
/// `wasi.thread-spawn`; il kernel crea un SECONDO fiber con una fresh
/// Instance sulla STESSA SharedMemory che esegue `wasi_thread_start` (scrive
/// 99 + notify); il main attende in atomic.wait e rilegge il valore.
/// Ritorna true sse il main esce con 99.
#[cfg(feature = "boot-checks")]
pub fn gate2_run(cwasm: &[u8]) -> bool {
    let engine = crate::wasm::wt::engine();
    // SAFETY: cwasm prodotto da tools/wt-precompile per questa exact config.
    let module = match unsafe { wasmtime::Module::deserialize(engine, cwasm) } {
        Ok(m) => m,
        Err(e) => { crate::kprintln!("ruos: gate2 deserialize: {:?}", e); return false; }
    };
    let mem_ty = match module.imports().find_map(|i| i.ty().memory().cloned()) {
        Some(t) => t,
        None => { crate::kprintln!("ruos: gate2: no memory import"); return false; }
    };
    let shared = match wasmtime::SharedMemory::new(engine, mem_ty) {
        Ok(s) => s,
        Err(e) => { crate::kprintln!("ruos: gate2 SharedMemory: {:?}", e); return false; }
    };
    let mut linker: wasmtime::Linker<crate::wasm::wt::state::WtState> =
        wasmtime::Linker::new(engine);
    if let Err(e) = add_thread_spawn_to_linker(&mut linker) {
        crate::kprintln!("ruos: gate2 thread-spawn link: {}", e);
        return false;
    }
    {
        let throwaway =
            wasmtime::Store::new(engine, crate::wasm::wt::state::WtState::new(Vec::new()));
        if let Err(e) = linker.define(&throwaway, "env", "memory", shared.clone()) {
            crate::kprintln!("ruos: gate2 define: {:?}", e);
            return false;
        }
    }
    let cpu = crate::cpu::cpu_id();
    let group = Arc::new(ThreadGroup {
        module,
        linker: Arc::new(linker),
        shared,
        next_tid: AtomicU32::new(1),
        live: AtomicU32::new(0),
        poisoned: AtomicBool::new(false),
        exit: IrqMutex::new(None),
        waiter_core: AtomicU32::new(cpu),
        base_args: Vec::new(),
        env: Vec::new(),
    });
    // main = tid 0 sull'export `run`; il thread tid 1 lo crea thread-spawn.
    spawn_fiber_export(group.clone(), 0, "run");
    let deadline = crate::timer::ticks() + 500; // 5 s
    while group.live.load(Ordering::SeqCst) != 0 {
        if core_allowed(cpu) {
            expire_timeouts();
            while run_one(cpu) {}
        }
        if crate::timer::ticks() > deadline {
            crate::kprintln!(
                "ruos: gate2 timeout (live={})", group.live.load(Ordering::SeqCst));
            return false;
        }
        core::hint::spin_loop();
    }
    let code = *group.exit.lock();
    if code != Some(99) {
        crate::kprintln!("ruos: gate2 main exit = {:?} (want Some(99))", code);
    }
    code == Some(99)
}

/// Variante test di `spawn_fiber`: chiama un export custom `() -> i32` invece
/// di `_start`/`wasi_thread_start` (i guest dei gate non sono binari WASI).
#[cfg(feature = "boot-checks")]
fn spawn_fiber_export(group: Arc<ThreadGroup>, tid: u32, export: &'static str) {
    group.live.fetch_add(1, Ordering::SeqCst);
    let stack = match wasmtime_internal_fiber::FiberStack::new(FIBER_STACK_SIZE, false) {
        Ok(s) => s,
        Err(e) => {
            crate::kprintln!("ruos: gate3 stack: {:?}", e);
            group.live.fetch_sub(1, Ordering::SeqCst);
            return;
        }
    };
    let g = group.clone();
    let fiber = match wasmtime_internal_fiber::Fiber::new(stack, move |_: (), sus| -> i32 {
        publish_suspend(sus);
        let engine = crate::wasm::wt::engine();
        let mut state = crate::wasm::wt::state::WtState::new(Vec::new());
        // Il gruppo serve anche qui: il main del gate 2 chiama thread-spawn.
        state.threads = Some(g.clone());
        let mut store = wasmtime::Store::new(engine, state);
        store.set_epoch_deadline(crate::wasm::wt::NO_DEADLINE_TICKS);
        let inst = match g.linker.instantiate(&mut store, &g.module) {
            Ok(i) => i,
            Err(e) => {
                crate::kprintln!("ruos: gate3 instantiate {}: {:?}", export, e);
                return -1;
            }
        };
        // DF=0 prima di entrare nel guest (convenzione di ogni call site wt).
        #[cfg(target_arch = "x86_64")]
        unsafe { core::arch::asm!("cld", options(nostack)); }
        match inst.get_typed_func::<(), i32>(&mut store, export)
            .and_then(|f| f.call(&mut store, ()))
        {
            Ok(v) => v,
            Err(e) => {
                crate::kprintln!("ruos: gate3 {} trap: {:?}", export, e);
                -1
            }
        }
    }) {
        Ok(f) => f,
        Err(e) => {
            crate::kprintln!("ruos: gate3 Fiber::new: {:?}", e);
            group.live.fetch_sub(1, Ordering::SeqCst);
            return;
        }
    };
    enqueue_runnable(Box::new(ThreadFiber {
        fiber,
        saved_tls: core::ptr::null_mut(),
        suspend_ptr: AtomicUsize::new(0),
        group: Some(group),
        tid,
        park_key: 0,
        park_deadline: 0,
    }));
}

/// Boot-check MT Fase 2: fiber host-only (niente wasm) con suspend/resume
/// cross-core. Il fiber pubblica il Suspend, avanza un contatore, si
/// parcheggia su una chiave di test; il BSP lo sveglia con `wake_key` e il
/// fiber finisce. Ritorna (ok, core del primo run, core del resume) —
/// con ≥3 core entrambi devono essere core ComputeApp (≠0, ≠1).
#[cfg(feature = "boot-checks")]
pub fn fiber_self_test() -> (bool, u32, u32) {
    /// 0 = mai girato, 1 = primo tratto eseguito (park imminente), 2 = finito.
    static STAGE: AtomicU32 = AtomicU32::new(0);
    static FIRST_CORE: AtomicU32 = AtomicU32::new(u32::MAX);
    static RESUME_CORE: AtomicU32 = AtomicU32::new(u32::MAX);
    /// Solo come indirizzo-chiave di park (mai letta).
    static TEST_FUTEX: AtomicU32 = AtomicU32::new(0);

    let key = &TEST_FUTEX as *const AtomicU32 as usize;
    let fail = || (false, FIRST_CORE.load(Ordering::SeqCst), RESUME_CORE.load(Ordering::SeqCst));

    let stack = match wasmtime_internal_fiber::FiberStack::new(64 * 1024, false) {
        Ok(s) => s,
        Err(e) => {
            crate::bwarn!("wt", "fiber self-test: stack alloc failed: {:?}", e);
            return fail();
        }
    };
    let fiber = match wasmtime_internal_fiber::Fiber::new(stack, move |_: (), sus| -> i32 {
        publish_suspend(sus);
        FIRST_CORE.store(crate::cpu::cpu_id(), Ordering::SeqCst);
        STAGE.store(1, Ordering::SeqCst);
        park_current(key, u64::MAX);
        RESUME_CORE.store(crate::cpu::cpu_id(), Ordering::SeqCst);
        STAGE.store(2, Ordering::SeqCst);
        0
    }) {
        Ok(f) => f,
        Err(e) => {
            crate::bwarn!("wt", "fiber self-test: Fiber::new failed: {:?}", e);
            return fail();
        }
    };
    enqueue_runnable(Box::new(ThreadFiber {
        fiber,
        saved_tls: core::ptr::null_mut(),
        suspend_ptr: AtomicUsize::new(0),
        group: None,
        tid: 0,
        park_key: 0,
        park_deadline: 0,
    }));

    // Il BSP qui è nella fase di boot, NON in run_core: sui sistemi senza core
    // ComputeApp deve drenare i fiber da sé; con ≥3 core li lascia agli AP.
    let deadline = crate::timer::ticks() + 300; // 3 s
    while STAGE.load(Ordering::SeqCst) < 1 {
        if core_allowed(0) {
            run_one(0);
        }
        if crate::timer::ticks() > deadline {
            crate::bwarn!("wt", "fiber self-test: timeout waiting first run");
            return fail();
        }
        core::hint::spin_loop();
    }
    // Il fiber è parcheggiato o in volo verso WAITQ (coperto dai crediti).
    wake_key(key, 1);
    while STAGE.load(Ordering::SeqCst) < 2 {
        if core_allowed(0) {
            run_one(0);
        }
        if crate::timer::ticks() > deadline {
            crate::bwarn!("wt", "fiber self-test: timeout waiting resume");
            return fail();
        }
        core::hint::spin_loop();
    }
    (true, FIRST_CORE.load(Ordering::SeqCst), RESUME_CORE.load(Ordering::SeqCst))
}
