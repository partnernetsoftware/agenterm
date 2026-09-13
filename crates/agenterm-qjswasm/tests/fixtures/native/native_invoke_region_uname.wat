(module
  (import "agenterm" "native_invoke"
    (func $native_invoke (param i32 i32 i32 i32) (result i32)))
  (import "agenterm" "native_result_len" (func $native_result_len (result i32)))
  (import "agenterm" "native_result" (func $native_result (param i32 i32) (result i32)))
  (import "agenterm" "print" (func $print (param i32 i32)))
  (memory 1)
  ;; The same declaration the `native_call` fixture uses, reached through the
  ;; language adapter instead: one pointer position, no guest span.
  (data (i32.const 0) "|uname|i32(ptr)")
  ;; The region the host allocates for this one call. 4096 bytes because
  ;; `uname` takes no length argument and writes a whole `struct utsname`, so
  ;; under-sizing it would be the guest's own C overflow.
  (data (i32.const 64) "[{\"region\":{\"capacity\":4096,\"termination\":\"nul\",\"output\":\"text\"}}]")
  (func (export "main") (result i64)
    (local $status i32) (local $need i32)
    (local.set $status
      (call $native_invoke (i32.const 0) (i32.const 15) (i32.const 64) (i32.const 66)))
    (local.set $need (call $native_result_len))
    ;; The two-pass retrieval every byte answer goes through, then stdout so
    ;; the host side reads the exact JSON the door produced.
    (call $print
      (i32.const 8192)
      (call $native_result (i32.const 8192) (local.get $need)))
    (i64.extend_i32_s (local.get $status))))
