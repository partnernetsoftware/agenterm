(module
  (import "agenterm" "native_invoke"
    (func $native_invoke (param i32 i32 i32 i32) (result i32)))
  (import "agenterm" "native_result_len" (func $native_result_len (result i32)))
  (import "agenterm" "native_result" (func $native_result (param i32 i32) (result i32)))
  (import "agenterm" "print" (func $print (param i32 i32)))
  (memory 1)
  (data (i32.const 0) "|access|i32(ptr,i32)")
  ;; A 64-byte region is exactly the slot bound this court runs with, and its
  ;; `raw` byte answer is larger than that bound: the door must refuse the
  ;; answer whole rather than hand back the first 64 bytes of JSON.
  (data (i32.const 64) "[{\"region\":{\"capacity\":64,\"termination\":\"raw\",\"output\":\"bytes\"}},0]")
  (func (export "main") (result i64)
    (local $status i32) (local $need i32)
    (local.set $status
      (call $native_invoke (i32.const 0) (i32.const 20) (i32.const 64) (i32.const 67)))
    (local.set $need (call $native_result_len))
    ;; Whatever the door answered -- the JSON answer or its own refusal -- is
    ;; what this court reads back byte for byte.
    (call $print
      (i32.const 8192)
      (call $native_result (i32.const 8192) (local.get $need)))
    (i64.extend_i32_s (local.get $status))))
