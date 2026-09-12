(module
  (import "agenterm" "native_call"
    (func $native_call (param i32 i32 i32 i32) (result i32)))
  (memory 1)
  (data (i32.const 0) "libSystem.B.dylib|pthread_getname_np|i32(u64,ptr,u64)")
  (func (export "main") (param $thread i64) (result i64)
    (local $status i32) (local $index i32) (local $hash i64)
    (i32.store (i32.const 128) (i32.const 1))
    (i32.store (i32.const 132) (i32.const 3))
    (i64.store (i32.const 136) (i64.const 0))
    (i32.store (i32.const 144) (i32.const 0))
    (i32.store (i32.const 148) (i32.const 0))
    (i64.store (i32.const 152) (local.get $thread))
    (i32.store (i32.const 160) (i32.const 1))
    (i32.store (i32.const 164) (i32.const 0))
    (i64.store (i32.const 168) (i64.const 274877907968))
    (i32.store (i32.const 176) (i32.const 0))
    (i32.store (i32.const 180) (i32.const 0))
    (i64.store (i32.const 184) (i64.const 64))
    (drop (call $native_call (i32.const 0) (i32.const 53) (i32.const 128) (i32.const 64)))
    (local.set $status (i32.load (i32.const 136)))
    (if (i32.eqz (local.get $status))
      (then
        (block $done
          (loop $next
            (br_if $done (i32.ge_u (local.get $index) (i32.const 64)))
            (br_if $done (i32.eqz (i32.load8_u (i32.add (i32.const 1024) (local.get $index)))))
            (local.set $hash
              (i64.and
                (i64.xor (i64.mul (local.get $hash) (i64.const 257))
                  (i64.extend_i32_u (i32.load8_u (i32.add (i32.const 1024) (local.get $index)))))
                (i64.const 4294967295)))
            (local.set $index (i32.add (local.get $index) (i32.const 1)))
            (br $next)))
        (if (i32.eq (local.get $index) (i32.const 64)) (then unreachable))))
    (i64.or (i64.shl (i64.extend_i32_u (local.get $status)) (i64.const 32)) (local.get $hash))))
