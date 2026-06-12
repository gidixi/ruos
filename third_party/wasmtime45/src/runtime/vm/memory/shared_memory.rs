use crate::Engine;
use crate::prelude::*;
use crate::runtime::vm::memory::{LocalMemory, MmapMemory, validate_atomic_addr};
// ruos: parking_spot (std-only: std::thread::park) sostituito dagli hook futex
// del kernel — vedi il blocco `unsafe extern "C"` in fondo al file.
use crate::runtime::vm::{self, Memory, VMMemoryDefinition, WaitResult};
// ruos: import std → no_std: Arc da alloc, RwLock dal sync layer interno del
// crate (con custom-sync-primitives va sugli hook wasmtime_sync_rwlock_* del
// kernel), Instant rimosso (il timeout passa relativo in ns agli hook futex).
use crate::sync::RwLock;
use alloc::sync::Arc;
use core::ops::Range;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use core::time::Duration;
use wasmtime_environ::Trap;

/// For shared memory (and only for shared memory), this lock-version restricts
/// access when growing the memory or checking its size. This is to conform with
/// the [thread proposal]: "When `IsSharedArrayBuffer(...)` is true, the return
/// value should be the result of an atomic read-modify-write of the new size to
/// the internal `length` slot."
///
/// [thread proposal]:
///     https://github.com/WebAssembly/threads/blob/master/proposals/threads/Overview.md#webassemblymemoryprototypegrow
#[derive(Clone)]
pub struct SharedMemory(Arc<SharedMemoryInner>);

struct SharedMemoryInner {
    memory: RwLock<LocalMemory>,
    // ruos: campo `spot: ParkingSpot` rimosso — il parcheggio dei waiter vive
    // nel kernel (hook wasmtime_futex_*), non nel runtime.
    ty: wasmtime_environ::Memory,
    def: LongTermVMMemoryDefinition,
}

impl SharedMemory {
    /// Construct a new [`SharedMemory`].
    pub fn new(engine: &Engine, ty: &wasmtime_environ::Memory) -> Result<Self> {
        let tunables = engine.tunables();
        let memory_tunables = wasmtime_environ::MemoryTunables::new(
            tunables,
            wasmtime_environ::MemoryKind::LinearMemory,
        );
        // Note that without a limiter being passed to `limit_new` this
        // `assert_ready` should never panic.
        let (minimum_bytes, maximum_bytes) = vm::assert_ready(Memory::limit_new(ty, None))?;
        let mmap_memory = MmapMemory::new(ty, &memory_tunables, minimum_bytes, maximum_bytes)?;
        let boxed: Box<dyn crate::runtime::vm::RuntimeLinearMemory> =
            try_new::<Box<_>>(mmap_memory)?;
        Self::wrap(
            engine,
            ty,
            LocalMemory::new(ty, &memory_tunables, boxed, None)?,
        )
    }

    /// Wrap an existing [Memory] with the locking provided by a [SharedMemory].
    pub fn wrap(
        engine: &Engine,
        ty: &wasmtime_environ::Memory,
        memory: LocalMemory,
    ) -> Result<Self> {
        if !engine.config().shared_memory {
            bail!(
                "shared memory support is disabled for this engine -- see `Config::shared_memory`"
            );
        }
        if !ty.shared {
            bail!("shared memory must have a `shared` memory type");
        }
        Ok(Self(try_new::<Arc<_>>(SharedMemoryInner {
            ty: *ty,
            // ruos: niente `spot` (vedi SharedMemoryInner).
            def: LongTermVMMemoryDefinition(memory.vmmemory()),
            memory: RwLock::new(memory),
        })?))
    }

    /// Return the memory type for this [`SharedMemory`].
    pub fn ty(&self) -> &wasmtime_environ::Memory {
        &self.0.ty
    }

    /// Convert this shared memory into a [`Memory`].
    pub fn as_memory(self) -> Memory {
        Memory::Shared(self)
    }

    /// Return a pointer to the shared memory's [VMMemoryDefinition].
    pub fn vmmemory_ptr(&self) -> NonNull<VMMemoryDefinition> {
        NonNull::from(&self.0.def.0)
    }

    /// Same as `RuntimeLinearMemory::grow`, except with `&self`.
    pub fn grow(&self, delta_pages: u64) -> Result<Option<(usize, usize)>, Error> {
        // ruos: crate::sync::RwLock non è poisonabile — niente .unwrap().
        let mut memory = self.0.memory.write();
        // Without a limiter being passed in this shouldn't have an await point,
        // so it should be safe to assert that it's ready.
        let result = vm::assert_ready(memory.grow(delta_pages, None))?;
        if let Some((_old_size_in_bytes, new_size_in_bytes)) = result {
            // Store the new size to the `VMMemoryDefinition` for JIT-generated
            // code (and runtime functions) to access. No other code can be
            // growing this memory due to the write lock, but code in other
            // threads could have access to this shared memory and we want them
            // to see the most consistent version of the `current_length`; a
            // weaker consistency is possible if we accept them seeing an older,
            // smaller memory size (assumption: memory only grows) but presently
            // we are aiming for accuracy.
            //
            // Note that it could be possible to access a memory address that is
            // now-valid due to changes to the page flags in `grow` above but
            // beyond the `memory.size` that we are about to assign to. In these
            // and similar cases, discussion in the thread proposal concluded
            // that: "multiple accesses in one thread racing with another
            // thread's `memory.grow` that are in-bounds only after the grow
            // commits may independently succeed or trap" (see
            // https://github.com/WebAssembly/threads/issues/26#issuecomment-433930711).
            // In other words, some non-determinism is acceptable when using
            // `memory.size` on work being done by `memory.grow`.
            self.0
                .def
                .0
                .current_length
                .store(new_size_in_bytes, Ordering::SeqCst);
        }
        Ok(result)
    }

    /// Implementation of `memory.atomic.notify` for this shared memory.
    pub fn atomic_notify(&self, addr_index: u64, count: u32) -> Result<u32, Trap> {
        let ptr = validate_atomic_addr(&self.0.def.0, addr_index, 4, 4)?;
        log::trace!("memory.atomic.notify(addr={addr_index:#x}, count={count})");
        // ruos: parking_spot.notify → hook futex del kernel (n. waiter svegliati).
        // SAFETY: `ptr` è stato validato (in-bounds, allineato) qui sopra.
        Ok(unsafe { wasmtime_futex_notify(ptr, count) })
    }

    /// Implementation of `memory.atomic.wait32` for this shared memory.
    pub fn atomic_wait32(
        &self,
        addr_index: u64,
        expected: u32,
        timeout: Option<Duration>,
    ) -> Result<WaitResult, Trap> {
        let addr = validate_atomic_addr(&self.0.def.0, addr_index, 4, 4)?;
        log::trace!(
            "memory.atomic.wait32(addr={addr_index:#x}, expected={expected}, timeout={timeout:?})"
        );

        // ruos: parking_spot.wait32 (deadline std::time::Instant + Waiter TLS) →
        // hook futex del kernel; il timeout passa relativo in ns (-1 = infinito).
        // Il confronto con `expected` è atomico dentro l'hook.
        // SAFETY: `addr` è stato validato (in-bounds, allineato a 4) qui sopra.
        let res = unsafe { wasmtime_futex_wait32(addr.cast(), expected, timeout_ns(timeout)) };
        Ok(wait_result(res))
    }

    /// Implementation of `memory.atomic.wait64` for this shared memory.
    pub fn atomic_wait64(
        &self,
        addr_index: u64,
        expected: u64,
        timeout: Option<Duration>,
    ) -> Result<WaitResult, Trap> {
        let addr = validate_atomic_addr(&self.0.def.0, addr_index, 8, 8)?;
        log::trace!(
            "memory.atomic.wait64(addr={addr_index:#x}, expected={expected}, timeout={timeout:?})"
        );

        // ruos: come atomic_wait32, su wasmtime_futex_wait64.
        // SAFETY: `addr` è stato validato (in-bounds, allineato a 8) qui sopra.
        let res = unsafe { wasmtime_futex_wait64(addr.cast(), expected, timeout_ns(timeout)) };
        Ok(wait_result(res))
    }

    // ruos: crate::sync::RwLock non è poisonabile — niente .unwrap() (×3 sotto).
    pub(crate) fn byte_size(&self) -> usize {
        self.0.memory.read().byte_size()
    }

    pub(crate) fn needs_init(&self) -> bool {
        self.0.memory.read().needs_init()
    }

    pub(crate) fn wasm_accessible(&self) -> Range<usize> {
        self.0.memory.read().wasm_accessible()
    }
}

// ruos: il thread_local! WAITER (RefCell<Waiter> per parking_spot) è rimosso —
// lo stato dei waiter vive interamente nel kernel dietro gli hook futex.

// ruos: hook futex implementati dal kernel (kernel/src/wasm/wt/threads.rs; stub
// temporanei in wt/platform.rs finché Task 4 non li sostituisce). Contratto
// wait (semantica wasm threads): ritorno 0 = woken, 1 = not-equal, 2 =
// timed-out; `timeout_ns < 0` = attesa infinita. Il confronto `*addr ==
// expected` e l'accodamento del waiter sono atomici dentro l'hook. notify
// ritorna il numero di waiter svegliati.
unsafe extern "C" {
    fn wasmtime_futex_wait32(addr: *const u32, expected: u32, timeout_ns: i64) -> u32;
    fn wasmtime_futex_wait64(addr: *const u64, expected: u64, timeout_ns: i64) -> u32;
    fn wasmtime_futex_notify(addr: *const u8, count: u32) -> u32;
}

// ruos: niente std::time::Instant in no_std — il timeout resta una durata
// relativa in ns, troncata a i64::MAX (≈292 anni, di fatto infinito).
fn timeout_ns(timeout: Option<Duration>) -> i64 {
    match timeout {
        Some(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        None => -1,
    }
}

// ruos: mappa il codice di ritorno degli hook futex sul WaitResult wasm.
fn wait_result(code: u32) -> WaitResult {
    match code {
        0 => WaitResult::Ok,
        1 => WaitResult::Mismatch,
        _ => WaitResult::TimedOut,
    }
}

/// Shared memory needs some representation of a `VMMemoryDefinition` for
/// JIT-generated code to access. This structure owns the base pointer and
/// length to the actual memory and we share this definition across threads by:
/// - never changing the base pointer; according to the specification, shared
///   memory must be created with a known maximum size so it can be allocated
///   once and never moved
/// - carefully changing the length, using atomic accesses in both the runtime
///   and JIT-generated code.
struct LongTermVMMemoryDefinition(VMMemoryDefinition);
unsafe impl Send for LongTermVMMemoryDefinition {}
unsafe impl Sync for LongTermVMMemoryDefinition {}
