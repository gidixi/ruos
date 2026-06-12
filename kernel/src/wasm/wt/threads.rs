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
#[allow(dead_code)]
pub const FIBER_STACK_SIZE: usize = 2 * 1024 * 1024;
const WAITQ_SHARDS: usize = 16;

/// Tipo Suspend concreto dei nostri fiber: Resume=(), Yield=(), Return=exit code.
/// (dead_code: fuori da boot-checks il protocollo park è usato solo dal
/// self-test finché il Task 4 non aggancia gli hook futex.)
#[allow(dead_code)]
type FiberSuspend = wasmtime_internal_fiber::Suspend<(), (), i32>;

/// Un fiber-thread registrato. Lo stato di esecuzione (Store, Instance,
/// activation wasmtime) vive nello stack del fiber stesso.
struct ThreadFiber {
    fiber: wasmtime_internal_fiber::Fiber<'static, (), (), i32>,
    /// TLS wasmtime salvato mentre il fiber è sospeso (vedi `run_one`).
    saved_tls: *mut u8,
    /// `&mut Suspend` pubblicato dal corpo del fiber al primo run
    /// (`publish_suspend`) — serve a `park_current` per sospendere.
    #[allow(dead_code)] // letto solo dal protocollo park (self-test → Task 4)
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
/// Creato da `exec_threaded` (Task 3); i campi servono a `spawn_fiber` e al
/// futuro `thread-spawn` (Task 5).
#[allow(dead_code)]
pub struct ThreadGroup {
    pub module: wasmtime::Module,
    pub linker: Arc<wasmtime::Linker<crate::wasm::wt::state::WtState>>,
    pub shared: wasmtime::SharedMemory,
    pub next_tid: AtomicU32,
    pub live: AtomicU32,
    /// Trap in un thread → muore tutto il gruppo (kill-group, Task 7).
    pub poisoned: AtomicBool,
    /// Exit code del main (tid 0).
    pub exit: IrqMutex<Option<i32>>,
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
#[allow(dead_code)] // producer: self-test ora, spawn_fiber dal Task 3
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
            // Ultimo thread del gruppo: exec_threaded (BSP/AP, Task 3) se ne
            // accorge dal live==0 — sveglialo.
            crate::executor::wake_core(0);
        }
    }
    // (Task 6: unregister da proc/ps.)
}

/// Pubblica l'handle Suspend del fiber corrente (chiamata come PRIMA cosa dal
/// corpo di ogni fiber). Il fiber non ha accesso diretto alla propria Box:
/// passa da CURRENT, che run_one ha appena settato su questo core.
#[allow(dead_code)] // chiamato dai corpi fiber: self-test ora, spawn_fiber dal Task 3
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
#[allow(dead_code)] // usato dal self-test; gli hook futex lo agganciano nel Task 4
fn park_current(key: usize, deadline: u64) {
    let me = CURRENT[crate::cpu::cpu_id() as usize].load(Ordering::SeqCst) as *mut ThreadFiber;
    if me.is_null() {
        // Contesto non-fiber: niente da parcheggiare (il chiamante degrada a spin).
        return;
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
}

/// Sveglia fino a `count` fiber parcheggiati su `key`; ritorna quanti ne ha
/// effettivamente rimessi in RUNQ. Il resto del budget diventa credito per i
/// parker in volo (vedi doc-comment in testa). Base dell'hook notify (Task 4).
#[allow(dead_code)] // chiamato dal self-test; hook notify nel Task 4
pub fn wake_key(key: usize, count: u32) -> u32 {
    let mut woken = 0u32;
    let mut to_run: Vec<Box<ThreadFiber>> = Vec::new();
    {
        let mut s = shard(key).0.lock();
        let mut i = 0;
        while i < s.waiters.len() && woken < count {
            if s.waiters[i].0 == key {
                let (_, _, f) = s.waiters.swap_remove(i);
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
