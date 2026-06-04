(module
  (import "ruos" "print" (func $print (param i32)))
  (func (export "run")
    i32.const 42
    call $print))
