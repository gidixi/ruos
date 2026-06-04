;; GUI host-fn smoke: query surface info, then blit a 2x2 red square at (0,0).
;; Exercises ruos_gfx gfx_info + gfx_blit through the Wasmtime Linker.
(module
  (import "ruos_gfx" "gfx_info" (func $info (param i32) (result i32)))
  (import "ruos_gfx" "gfx_blit" (func $blit (param i32 i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "_start")
    (drop (call $info (i32.const 0)))
    ;; 4 red RGBA8888 pixels (0xFF0000FF little-endian = FF 00 00 FF) at off 16.
    (i32.store (i32.const 16) (i32.const 0xFF0000FF))
    (i32.store (i32.const 20) (i32.const 0xFF0000FF))
    (i32.store (i32.const 24) (i32.const 0xFF0000FF))
    (i32.store (i32.const 28) (i32.const 0xFF0000FF))
    (drop (call $blit (i32.const 16) (i32.const 16) (i32.const 0) (i32.const 0) (i32.const 2) (i32.const 2)))))
