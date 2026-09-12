# agenterm-dyn

`agenterm-dyn` is AgenTerm's policy-free, in-process dynamic ABI mechanism.
It opens a caller-selected library, resolves a caller-selected symbol, and
executes a C ABI shape that the current mechanism can represent. It is roughly
`libdl`-like loading plus a deliberately incomplete `libffi`-like call surface,
implemented with monomorphic Rust `extern "C"` trampolines.

## Boundary

dyn owns only mechanism:

- the single shared-library loader and symbol lookup path;
- `AbiSignature`, `AbiValue`, validation against the real trampoline matrix,
  and `unsafe invoke_abi`;
- raw scalar and pointer transport;
- the Unix variadic `ioctl` ABI exception;
- the separate W^X executable-buffer mechanism and its errors.

The caller owns the asserted native signature, pointer validity, alignment,
aliasing, lifetimes, library/thread requirements, cleanup, and side effects.
An ABI pointer has no pointee or nullability policy inside dyn.

dyn does not own product exposure policy, allowlists, guest-memory decoding,
Wasm span checks, budgets, cancellation, or supervision. Those remain in
`agenterm-qjswasm` and the Script Runtime. The typed OS snapshots and six-cell
host catalog still present in this crate are migration debt; they are not a
direction for new dyn APIs.

## Primary API

```rust
use agenterm_dyn::{AbiSignature, AbiType, AbiValue, NativeCall, invoke_abi};

let call = NativeCall {
    library: "",
    symbol: "getpid",
    signature: AbiSignature {
        result: AbiType::I32,
        params: &[],
    },
    arguments: &[],
};

// SAFETY: the caller asserts that `getpid` really has C ABI `i32()`.
let pid = unsafe { invoke_abi(&call)? };
assert!(matches!(pid, AbiValue::I32(value) if value > 0));
# Ok::<(), agenterm_dyn::AbiError>(())
```

`validate_abi` checks argument count/types and whether a real trampoline exists
without loading or calling the symbol. An unsupported shape returns
`AbiError::SignatureUnsupported`; dyn never approximates one ABI as another.

## Relationship to qjswasm

`agenterm-qjswasm` owns the native-call grammar and catalog, `ptr` versus
`ptr?`, scalar canonicalization, guest-span bounds, result-pointer rebasing,
and public typed errors. Once that upper layer admits and decodes a call, its
five scalar/pointer execution paths delegate to `invoke_abi`. Unix `ioctl`
uses the same native door but its dedicated variadic mechanism.

There is no second loader and no second native door.

## Historical material

The former S-expression evaluator (`Dyn`, `Value`, `Symbol`, and textual
`dlcall`) was removed after its executable claims moved to `invoke_abi` tests
or qjswasm WAT courts. Files under `examples/` that still show `(dlcall ...)`
are historical migration records, not current API examples; they will leave
with the remaining facts/catalog documentation migration.

The temporary migration index is: [mach_absolute_time](examples/mach-absolute-time.md),
[getprogname](examples/getprogname.md), [issetugid](examples/issetugid.md),
[_NSGetExecutablePath](examples/nsget-executable-path.md), [proc_pidpath](examples/proc-pidpath.md),
[arc4random](examples/arc4random.md), [clock_gettime_nsec_np](examples/clock-gettime-nsec-np.md),
[sysctl](examples/sysctl.md), [pthread_main_np](examples/pthread-main-np.md),
[pthread_threadid_np](examples/pthread-threadid-np.md),
[pthread_getname_np](examples/pthread-getname-np.md), [proc_pidinfo](examples/proc-pidinfo.md),
[_NSGetArgc](examples/nsget-argc.md), [_NSGetArgv](examples/nsget-argv.md),
[_NSGetEnviron](examples/nsget-environ.md), [proc_pid_rusage](examples/proc-pid-rusage.md),
[_dyld_image_count](examples/dyld-image-count.md), [getentropy](examples/getentropy.md),
[proc_name](examples/proc-name.md), [pthread_get_stackaddr_np](examples/pthread-get-stackaddr-np.md),
[pthread_get_stacksize_np](examples/pthread-get-stacksize-np.md), [pthread_self](examples/pthread-self.md),
[pthread_cpu_number_np](examples/pthread-cpu-number-np.md),
[malloc_good_size](examples/malloc-good-size.md), [_NSGetProgname](examples/nsget-progname.md),
[proc_libversion](examples/proc-libversion.md),
[pthread_jit_write_protect_supported_np](examples/pthread-jit-write-protect-supported-np.md),
[sysctlnametomib](examples/sysctlnametomib.md), [pthread_equal](examples/pthread-equal.md),
[confstr](examples/confstr.md), [clock_getres](examples/clock-getres.md),
[pthread_is_threaded_np](examples/pthread-is-threaded-np.md),
[_NSGetMachExecuteHeader](examples/nsget-mach-execute-header.md),
[_dyld_get_image_name](examples/dyld-get-image-name.md),
[_dyld_get_image_vmaddr_slide](examples/dyld-get-image-vmaddr-slide.md),
[gethostuuid](examples/gethostuuid.md), [_dyld_get_image_header](examples/dyld-get-image-header.md),
[arc4random_uniform](examples/arc4random-uniform.md), [gettimeofday](examples/gettimeofday.md),
[mach_host_self](examples/mach-host-self.md), [sysctlbyname](examples/sysctlbyname.md),
[getifaddrs](examples/getifaddrs.md), [getgroups](examples/getgroups.md),
[statvfs](examples/statvfs.md), [realpath](examples/realpath.md),
[gethostname](examples/gethostname.md), [dladdr](examples/dladdr.md),
[getdomainname](examples/getdomainname.md), [getlogin_r](examples/getlogin-r.md), and
[mach_timebase_info](examples/mach-timebase-info.md).

The owning product contract, including the required Markdown tree-DAG and
Mermaid memory-palace view, is
[`prd/PRD_02_34_agenterm_dyn.md`](../../prd/PRD_02_34_agenterm_dyn.md).
