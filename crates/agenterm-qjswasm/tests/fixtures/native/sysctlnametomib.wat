(module
  (import "agenterm" "native_call"
    (func $native_call (param i32 i32 i32 i32) (result i32)))
  (memory 1)
  (data (i32.const 0) "libSystem.B.dylib|sysctlnametomib|i32(ptr,ptr,ptr)")
  (data (i32.const 512) "hw.ncpu\00")
  (func (export "main") (param $which i64) (result i64)
    (i64.store (i32.const 1056) (i64.const 8))
    (i32.store (i32.const 128) (i32.const 1))
    (i32.store (i32.const 132) (i32.const 3))
    (i64.store (i32.const 136) (i64.const 0))
    (i32.store (i32.const 144) (i32.const 1))
    (i32.store (i32.const 148) (i32.const 0))
    (i64.store (i32.const 152) (i64.const 34359738880))
    (i32.store (i32.const 160) (i32.const 1))
    (i32.store (i32.const 164) (i32.const 0))
    (i64.store (i32.const 168) (i64.const 137438954496))
    (i32.store (i32.const 176) (i32.const 1))
    (i32.store (i32.const 180) (i32.const 0))
    (i64.store (i32.const 184) (i64.const 34359739424))
    (drop (call $native_call (i32.const 0) (i32.const 50) (i32.const 128) (i32.const 64)))
    (if (i32.ne (i32.load (i32.const 136)) (i32.const 0)) (then unreachable))
    (if (result i64) (i64.eqz (local.get $which))
      (then (i64.load (i32.const 1056)))
      (else
        (if (i64.gt_u (local.get $which) (i64.load (i32.const 1056))) (then unreachable))
        (i64.extend_i32_s
          (i32.load
            (i32.add
              (i32.const 1024)
              (i32.shl
                (i32.sub (i32.wrap_i64 (local.get $which)) (i32.const 1))
                (i32.const 2)))))))))
