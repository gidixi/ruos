//! Unified boot error type.

#[derive(Debug)]
pub enum BootError {
    LimineUnsupported,
    HeapInit(&'static str),
    AcpiInit(&'static str),
    FramesInit(&'static str),
    PagingInit(&'static str),
    TimerInit(&'static str),
    VfsInit(&'static str),
    ModulesMount(&'static str),
    NetInit(&'static str),
    Other(&'static str),
}
