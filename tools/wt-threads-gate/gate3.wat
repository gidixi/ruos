;; Gate 3 MT Fase 2: atomic.wait sospende il fiber, notify risveglia.
;; Due export eseguiti su DUE fiber dello stesso gruppo (stessa memoria shared):
;;  - waiter: wait32 su mem[32] finche' vale 0, poi ritorna il payload mem[36];
;;  - waker:  scrive il payload (7), sblocca mem[32] e fa notify.
;; Se il wait NON sospendesse il fiber (bloccando il core), su un sistema a un
;; solo core abilitato il waker non girerebbe mai -> timeout del gate.
(module
  (import "env" "memory" (memory 1 1 shared))
  (func (export "waiter") (result i32)
    ;; wait infinito; ritorno 0=woken 1=not-equal (waker gia' passato): ok entrambi
    (drop (memory.atomic.wait32 (i32.const 32) (i32.const 0) (i64.const -1)))
    (i32.atomic.load (i32.const 36)))
  (func (export "waker") (result i32)
    (i32.atomic.store (i32.const 36) (i32.const 7))
    (i32.atomic.store (i32.const 32) (i32.const 1))
    (drop (memory.atomic.notify (i32.const 32) (i32.const 1)))
    (i32.const 0)))
