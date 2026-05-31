//! Synchronization primitives for ruos.
//!
//! `IrqMutex<T>` = `spin::Mutex<T>` that also disables interrupts for the
//! duration of the lock and restores the prior IF state on drop. Replaces the
//! ad-hoc `without_interrupts(|| some_mutex.lock())` pattern at the sites that
//! actually NEED interrupt masking (shared state touched from both task and
//! ISR context). The spinlock provides cross-core mutual exclusion (SMP-safe);
//! the IF masking prevents an ISR on THIS core from deadlocking on a lock the
//! interrupted task already holds.

use core::ops::{Deref, DerefMut};
use spin::{Mutex, MutexGuard};

pub struct IrqMutex<T> {
    inner: Mutex<T>,
}

pub struct IrqGuard<'a, T> {
    guard: Option<MutexGuard<'a, T>>,
    saved_if: bool,
}

impl<T> IrqMutex<T> {
    pub const fn new(val: T) -> Self {
        Self { inner: Mutex::new(val) }
    }

    /// Lock: save current IF, disable interrupts, then acquire the spinlock.
    /// On guard drop, the spinlock is released and IF is restored.
    pub fn lock(&self) -> IrqGuard<'_, T> {
        let saved_if = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        let guard = self.inner.lock();
        IrqGuard { guard: Some(guard), saved_if }
    }

    /// Non-blocking try-lock. Restores IF immediately if the lock is contended.
    pub fn try_lock(&self) -> Option<IrqGuard<'_, T>> {
        let saved_if = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        match self.inner.try_lock() {
            Some(guard) => Some(IrqGuard { guard: Some(guard), saved_if }),
            None => {
                if saved_if { x86_64::instructions::interrupts::enable(); }
                None
            }
        }
    }
}

// SAFETY: IrqMutex provides mutual exclusion via the inner spin::Mutex.
unsafe impl<T: Send> Send for IrqMutex<T> {}
unsafe impl<T: Send> Sync for IrqMutex<T> {}

impl<'a, T> Deref for IrqGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T { self.guard.as_ref().unwrap() }
}

impl<'a, T> DerefMut for IrqGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T { self.guard.as_mut().unwrap() }
}

impl<'a, T> Drop for IrqGuard<'a, T> {
    fn drop(&mut self) {
        // Release the spinlock FIRST (drop the inner guard), THEN restore IF.
        self.guard = None;
        if self.saved_if {
            x86_64::instructions::interrupts::enable();
        }
    }
}
