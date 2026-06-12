;; Gate 2 MT Fase 2: thread-spawn reale. Il main (export "run") chiama
;; l'import wasi.thread-spawn; il kernel crea un nuovo fiber con una fresh
;; Instance sulla STESSA memoria shared e ne chiama wasi_thread_start, che
;; scrive 99 in mem[64] e fa notify; il main attende e rilegge il valore.
;; Prova: spawn -> nuova Instance -> stessa memoria -> il main vede la scrittura.
(module
  (import "env" "memory" (memory 1 1 shared))
  (import "wasi" "thread-spawn" (func $spawn (param i32) (result i32)))
  (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
    (i32.atomic.store (i32.const 64) (i32.const 99))
    (drop (memory.atomic.notify (i32.const 64) (i32.const 1))))
  (func (export "run") (result i32)
    (drop (call $spawn (i32.const 0)))
    ;; aspetta che il thread scriva 99 (not-equal se ha gia' scritto: ok)
    (drop (memory.atomic.wait32 (i32.const 64) (i32.const 0) (i64.const -1)))
    (i32.atomic.load (i32.const 64))))
