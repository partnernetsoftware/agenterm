(module
  (import "agenterm" "native_call"
    (func $native_call (param i32 i32 i32 i32) (result i32)))
  (memory 1)
  (data (i32.const 0) "|getcwd|ptr(ptr,usize)")
  (func (export "main") (result i64)
    (local $index i32)
    (local $hash i64)
    (i32.store (i32.const 128) (i32.const 1))
    (i32.store (i32.const 132) (i32.const 2))
    (i64.store (i32.const 136) (i64.const 0))
    (i32.store (i32.const 144) (i32.const 1))
    (i32.store (i32.const 148) (i32.const 0))
    (i64.store (i32.const 152) (i64.const 17592186045440))
    (i32.store (i32.const 160) (i32.const 0))
    (i32.store (i32.const 164) (i32.const 0))
    (i64.store (i32.const 168) (i64.const 4096))
    (drop (call $native_call
      (i32.const 0) (i32.const 22) (i32.const 128) (i32.const 48)))
    (if (i64.ne (i64.load (i32.const 136)) (i64.const 1024))
      (then unreachable))
    (block $done
      (loop $next
        (br_if $done
          (i32.eqz (i32.load8_u (i32.add (i32.const 1024) (local.get $index)))))
        (local.set $hash
          (i64.xor
            (i64.mul (local.get $hash) (i64.const 257))
            (i64.extend_i32_u
              (i32.load8_u (i32.add (i32.const 1024) (local.get $index))))))
        (local.set $index (i32.add (local.get $index) (i32.const 1)))
        (br_if $next (i32.lt_u (local.get $index) (i32.const 4096)))
        unreachable))
    (local.get $hash)))
