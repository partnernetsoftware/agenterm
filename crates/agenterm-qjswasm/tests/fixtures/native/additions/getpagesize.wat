(module
  (import "agenterm" "native_call"
    (func $native_call (param i32 i32 i32 i32) (result i32)))
  (memory 1)
  (data (i32.const 0) "|getpagesize|i32()")
  (func (export "main") (result i64)
    (local $page i64)
    (i32.store (i32.const 128) (i32.const 1))
    (i32.store (i32.const 132) (i32.const 0))
    (i64.store (i32.const 136) (i64.const 0))
    (drop (call $native_call
      (i32.const 0) (i32.const 18) (i32.const 128) (i32.const 16)))
    (local.set $page (i64.load (i32.const 136)))
    (i64.extend_i32_u
      (i32.and
        (i64.ge_u (local.get $page) (i64.const 4096))
        (i64.eqz
          (i64.and
            (local.get $page)
            (i64.sub (local.get $page) (i64.const 1))))))))
