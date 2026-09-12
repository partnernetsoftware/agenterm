# Read the Darwin domain name with `getdomainname`

macOS retains two distinct facts for `getdomainname`. The legacy native-call
court uses `getdomainname(char *name, int namelen) -> int`; its length is an
`i32`, not the `size_t` used by some other Unix hosts. Before evaluation, the
embedding Rust host binds a writable byte buffer as `domain` and keeps it alive
for the call.

```lisp
(dlcall "libSystem.B.dylib" "getdomainname" "i32"
  "ptr" domain
  "i32" 256)
```

A zero status means the caller-owned bounded buffer contains the domain name;
an empty domain is valid. That low-level call does not retain the buffer.

New Rust consumers should use `DomainNameSnapshot::acquire()`. It owns a fixed,
bounded, pointer-free copy of the native bytes without assuming UTF-8, accepts
an empty domain, rejects successful output without a real NUL terminator, and
returns typed unsupported or OS failures.
