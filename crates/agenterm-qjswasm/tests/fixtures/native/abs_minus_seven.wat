(module
  (import "agenterm" "native_call"
    (func $native_call (param i32 i32 i32 i32) (result i32)))
  (memory 1)
  (data (i32.const 0) "|abs|i32(i32)")
  (func (export "main") (result i64)
    (i32.store (i32.const 128) (i32.const 1))
    (i32.store (i32.const 132) (i32.const 1))
    (i64.store (i32.const 136) (i64.const 0))
    (i32.store (i32.const 144) (i32.const 0))
    (i32.store (i32.const 148) (i32.const 0))
    (i64.store (i32.const 152) (i64.const -7))
    (drop (call $native_call
      (i32.const 0) (i32.const 13) (i32.const 128) (i32.const 32)))
    (i64.load (i32.const 136))))
