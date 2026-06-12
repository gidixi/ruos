;; Gate 1 MT Fase 2 (spec 2026-06-12-wasm-mt-fase2-threads-design): CORE module
;; con memoria SHARED importata che fa due RMW atomici (lock-prefixed su x86
;; via AOT) e rilegge il valore. Niente atomic.wait/notify: il gate prova solo
;; SharedMemory + atomics nativi nell'engine no_std. Atteso: run() == 42.
(module
  (import "env" "memory" (memory 1 1 shared))
  (func (export "run") (result i32)
    (drop (i32.atomic.rmw.add (i32.const 16) (i32.const 41)))
    (drop (i32.atomic.rmw.add (i32.const 16) (i32.const 1)))
    (i32.atomic.load (i32.const 16))))
