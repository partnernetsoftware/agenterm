(module
  (import "agenterm" "native_call"
    (func $native_call (param i32 i32 i32 i32) (result i32)))
  (memory 1)
  (data (i32.const 0) "libSystem.B.dylib|proc_name|i32(i32,ptr,u32)")
  (func (export "main") (param $pid i64) (result i64)
    (local $written i32)
    (local $index i32)
    (local $hash i64)
    (i32.store (i32.const 128) (i32.const 1))
    (i32.store (i32.const 132) (i32.const 3))
    (i64.store (i32.const 136) (i64.const 0))
    (i32.store (i32.const 144) (i32.const 0))
    (i32.store (i32.const 148) (i32.const 0))
    (i64.store (i32.const 152) (local.get $pid))
    (i32.store (i32.const 160) (i32.const 1))
    (i32.store (i32.const 164) (i32.const 0))
    (i64.store (i32.const 168) (i64.const 17592186045440))
    (i32.store (i32.const 176) (i32.const 0))
    (i32.store (i32.const 180) (i32.const 0))
    (i64.store (i32.const 184) (i64.const 4096))
    (drop (call $native_call
      (i32.const 0) (i32.const 44)
      (i32.const 128) (i32.const 64)))
    (local.set $written (i32.load (i32.const 136)))
    (if (i32.le_s (local.get $written) (i32.const 0)) (then unreachable))
    (if (i32.ge_u (local.get $written) (i32.const 4096)) (then unreachable))
    (if (i32.ne
          (i32.load8_u (i32.add (i32.const 1024) (local.get $written)))
          (i32.const 0))
      (then unreachable))
    (local.set $hash (i64.const 0))
    (block $done
      (loop $next
        (br_if $done (i32.ge_u (local.get $index) (local.get $written)))
        (local.set $hash
          (i64.xor
            (i64.mul (local.get $hash) (i64.const 257))
            (i64.extend_i32_u
              (i32.load8_u (i32.add (i32.const 1024) (local.get $index))))))
        (local.set $index (i32.add (local.get $index) (i32.const 1)))
        (br $next)))
    (local.get $hash)))
