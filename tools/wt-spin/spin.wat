(module
  ;; WASI command entry. Busy-loops to consume CPU, then returns (exit 0).
  (func (export "_start")
    (local $i i64)
    (local.set $i (i64.const 0))
    (block $done
      (loop $spin
        (local.set $i (i64.add (local.get $i) (i64.const 1)))
        ;; LIMIT: tune for ~300-800 ms/run on QEMU -cpu max. Start at 2e9.
        (br_if $done (i64.ge_u (local.get $i) (i64.const 2000000000)))
        (br $spin)))))
