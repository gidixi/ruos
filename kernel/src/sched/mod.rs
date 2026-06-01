//! Scheduler-adjacent bookkeeping. Today this is only `cpustat` (per-core
//! busy/idle TSC accounting for `rtop`); the cooperative executor itself
//! lives in `crate::executor`.

pub mod cpustat;
