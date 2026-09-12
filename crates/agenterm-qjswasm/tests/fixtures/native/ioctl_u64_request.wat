(module
  (import "agenterm" "native_call"
    (func $native_call (param i32 i32 i32 i32) (result i32)))
  (memory 1)
  (data (i32.const 0) "|ioctl|i32(i32,u64,ptr)")
  (func (export "main") (result i64)
    (i32.store (i32.const 128) (i32.const 1))
    (i32.store (i32.const 132) (i32.const 3))
    (i64.store (i32.const 136) (i64.const 0))
    (i32.store (i32.const 144) (i32.const 0))
    (i32.store (i32.const 148) (i32.const 0))
    (i64.store (i32.const 152) (i64.const 2147483000))
    (i32.store (i32.const 160) (i32.const 0))
    (i32.store (i32.const 164) (i32.const 0))
    (i64.store (i32.const 168) (i64.const 2147483001))
    (i32.store (i32.const 176) (i32.const 1))
    (i32.store (i32.const 180) (i32.const 0))
    (i64.store (i32.const 184) (i64.const 34359738624))
    (drop (call $native_call
      (i32.const 0) (i32.const 23)
      (i32.const 128) (i32.const 64)))
    (if (result i64) (i64.ne (i64.load (i32.const 136)) (i64.const 0))
      (then (i64.load (i32.const 136)))
      (else
        (i64.or
          (i64.shl (i64.extend_i32_u (i32.load16_u (i32.const 256))) (i64.const 32))
          (i64.extend_i32_u (i32.load16_u (i32.const 258))))))))
