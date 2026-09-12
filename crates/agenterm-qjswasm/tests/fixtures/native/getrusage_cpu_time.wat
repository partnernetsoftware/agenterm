(module
  (import "agenterm" "native_call"
    (func $native_call (param i32 i32 i32 i32) (result i32)))
  (memory 1)
  (data (i32.const 0) "|getrusage|i32(i32,ptr)")
  (func (export "main") (param $system i64) (result i64)
    (local $offset i32)
    (i32.store (i32.const 128) (i32.const 1))
    (i32.store (i32.const 132) (i32.const 2))
    (i64.store (i32.const 136) (i64.const 0))
    (i32.store (i32.const 144) (i32.const 0))
    (i32.store (i32.const 148) (i32.const 0))
    (i64.store (i32.const 152) (i64.const 0))
    (i32.store (i32.const 160) (i32.const 1))
    (i32.store (i32.const 164) (i32.const 0))
    (i64.store (i32.const 168) (i64.const 1099511628800))
    (drop (call $native_call
      (i32.const 0) (i32.const 23) (i32.const 128) (i32.const 48)))
    (if (i64.ne (i64.load (i32.const 136)) (i64.const 0))
      (then unreachable))
    (local.set $offset
      (if (result i32) (i64.eq (local.get $system) (i64.const 0))
        (then (i32.const 1024))
        (else (i32.const 1040))))
    (i64.add
      (i64.mul (i64.load (local.get $offset)) (i64.const 1000000))
      (i64.load offset=8 (local.get $offset)))))
