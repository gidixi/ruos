//! Schedule dei timer unit: parse della stringa schedule e calcolo del
//! prossimo scatto. Vedi spec init-units §1/§4.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Schedule {
    /// Intervallo monotono in tick (OnUnitActiveSec-like).
    EveryTicks(u64),
    /// One-shot a boot+N tick (si disabilita dopo lo scatto).
    BootPlus(u64),
    Hourly { minute: u8 },
    Daily  { hour: u8, minute: u8 },
    Weekly { dow: u8, hour: u8, minute: u8 },   // 0=Sun..6=Sat
}
