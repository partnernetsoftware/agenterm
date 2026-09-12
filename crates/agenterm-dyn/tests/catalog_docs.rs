//! Cross-host documentation consistency for Darwin system-probe catalog rows.
//!
//! This suite reads only compile-time host data and repository Markdown. It is
//! intentionally portable and does not claim Darwin native-call evidence.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use agenterm_dyn::{LINUX_AARCH64, LINUX_X86_64, MACOS_AARCH64, MACOS_X86_64, SystemProbeStatus};

const DARWIN_ONLY_LIVE_EXAMPLES: &[(&str, &str)] = &[
    ("mach_absolute_time", "mach-absolute-time.md"),
    ("getprogname", "getprogname.md"),
    ("issetugid", "issetugid.md"),
    ("nsget_executable_path", "nsget-executable-path.md"),
    ("proc_pidpath", "proc-pidpath.md"),
    ("arc4random", "arc4random.md"),
    ("clock_gettime_nsec_np", "clock-gettime-nsec-np.md"),
    ("sysctl", "sysctl.md"),
    ("pthread_main_np", "pthread-main-np.md"),
    ("pthread_threadid_np", "pthread-threadid-np.md"),
    ("pthread_getname_np", "pthread-getname-np.md"),
    ("proc_pidinfo", "proc-pidinfo.md"),
    ("nsget_argc", "nsget-argc.md"),
    ("nsget_argv", "nsget-argv.md"),
    ("nsget_environ", "nsget-environ.md"),
    ("proc_pid_rusage", "proc-pid-rusage.md"),
    ("dyld_image_count", "dyld-image-count.md"),
    ("getentropy", "getentropy.md"),
    ("proc_name", "proc-name.md"),
    ("pthread_get_stackaddr_np", "pthread-get-stackaddr-np.md"),
    ("pthread_get_stacksize_np", "pthread-get-stacksize-np.md"),
    ("pthread_self", "pthread-self.md"),
    ("pthread_cpu_number_np", "pthread-cpu-number-np.md"),
    ("malloc_good_size", "malloc-good-size.md"),
    ("nsget_progname", "nsget-progname.md"),
    ("proc_libversion", "proc-libversion.md"),
    (
        "pthread_jit_write_protect_supported_np",
        "pthread-jit-write-protect-supported-np.md",
    ),
    ("sysctlnametomib", "sysctlnametomib.md"),
    ("pthread_equal", "pthread-equal.md"),
    ("confstr", "confstr.md"),
    ("clock_getres", "clock-getres.md"),
    ("pthread_is_threaded_np", "pthread-is-threaded-np.md"),
    ("nsget_mach_execute_header", "nsget-mach-execute-header.md"),
    ("dyld_get_image_name", "dyld-get-image-name.md"),
    (
        "dyld_get_image_vmaddr_slide",
        "dyld-get-image-vmaddr-slide.md",
    ),
    ("gethostuuid", "gethostuuid.md"),
    ("dyld_get_image_header", "dyld-get-image-header.md"),
    ("arc4random_uniform", "arc4random-uniform.md"),
    ("gettimeofday", "gettimeofday.md"),
    ("realpath", "realpath.md"),
];

#[test]
fn darwin_arch_cells_have_identical_ordered_probe_contracts() {
    assert_eq!(
        MACOS_X86_64.system_probes, MACOS_AARCH64.system_probes,
        "Darwin x86_64 and aarch64 must expose the same ordered names and statuses"
    );
}

#[test]
fn every_darwin_only_live_probe_has_a_linked_example() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = fs::read_to_string(root.join("README.md")).expect("crate README is readable");
    let manifest = DARWIN_ONLY_LIVE_EXAMPLES
        .iter()
        .copied()
        .collect::<HashMap<_, _>>();
    assert_eq!(
        manifest.len(),
        DARWIN_ONLY_LIVE_EXAMPLES.len(),
        "Darwin example manifest names must be unique"
    );

    let darwin_only_live = MACOS_X86_64
        .system_probes
        .iter()
        .zip(LINUX_X86_64.system_probes.iter())
        .filter_map(|(darwin, linux)| {
            let darwin_is_live = matches!(darwin.status, SystemProbeStatus::LiveDlcall { .. });
            let linux_is_placeholder = matches!(linux.status, SystemProbeStatus::Placeholder);
            (darwin_is_live && linux_is_placeholder).then_some(darwin.name)
        })
        .collect::<HashSet<_>>();
    let documented = manifest.keys().copied().collect::<HashSet<_>>();
    assert_eq!(
        documented, darwin_only_live,
        "the explicit example manifest must cover every Darwin-only live row exactly"
    );

    for (probe, file) in DARWIN_ONLY_LIVE_EXAMPLES {
        assert!(
            root.join("examples").join(file).is_file(),
            "Darwin live probe `{probe}` is missing examples/{file}"
        );
        assert!(
            readme.contains(&format!("](examples/{file})")),
            "Darwin live probe `{probe}` is missing its README link to examples/{file}"
        );
    }
}

#[test]
fn mach_host_self_is_owned_live_with_callable_documentation() {
    for cell in [MACOS_X86_64, MACOS_AARCH64] {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "mach_host_self")
            .expect("Darwin catalog contains mach_host_self");
        assert_eq!(
            probe.status,
            SystemProbeStatus::LiveOwned {
                api: "MachHostPort::acquire"
            }
        );
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example = fs::read_to_string(root.join("examples/mach-host-self.md"))
        .expect("mach_host_self honesty document is readable");
    assert!(example.contains("send right"));
    assert!(example.contains("MachHostPort::acquire"));

    let readme = fs::read_to_string(root.join("README.md")).expect("crate README is readable");
    assert!(readme.contains("](examples/mach-host-self.md)"));
}

#[test]
fn darwin_cpu_count_is_a_typed_hw_ncpu_snapshot() {
    for cell in [MACOS_X86_64, MACOS_AARCH64] {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "sysctlbyname")
            .expect("Darwin catalog contains sysctlbyname");
        assert_eq!(
            probe.status,
            SystemProbeStatus::LiveOwned {
                api: "CpuCountSnapshot::acquire"
            }
        );
    }

    for cell in [LINUX_X86_64, LINUX_AARCH64] {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "sysctlbyname")
            .expect("Linux catalog contains the unavailable Darwin API");
        assert_eq!(probe.status, SystemProbeStatus::Placeholder);
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example = fs::read_to_string(root.join("examples/sysctlbyname.md"))
        .expect("sysctlbyname typed snapshot documentation is readable");
    assert!(example.contains("CpuCountSnapshot::acquire"));
    assert!(example.contains("hw.ncpu"));
    assert!(!example.contains("(dlcall"));

    let readme = fs::read_to_string(root.join("README.md")).expect("crate README is readable");
    assert!(readme.contains("](examples/sysctlbyname.md)"));
}

#[test]
fn unix_getifaddrs_owner_has_callable_documentation() {
    for cell in [LINUX_X86_64, LINUX_AARCH64, MACOS_X86_64, MACOS_AARCH64] {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "getifaddrs")
            .expect("Unix catalog contains getifaddrs");
        assert_eq!(
            probe.status,
            SystemProbeStatus::LiveOwned {
                api: "InterfaceAddresses::acquire"
            }
        );
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example = fs::read_to_string(root.join("examples/getifaddrs.md"))
        .expect("getifaddrs ownership example is readable");
    assert!(example.contains("freeifaddrs"));
    assert!(example.contains("InterfaceAddresses::acquire"));

    let readme = fs::read_to_string(root.join("README.md")).expect("crate README is readable");
    assert!(readme.contains("](examples/getifaddrs.md)"));
}

#[test]
fn unix_getgroups_has_typed_owner_documentation() {
    for cell in [LINUX_X86_64, LINUX_AARCH64, MACOS_X86_64, MACOS_AARCH64] {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "getgroups")
            .expect("Unix catalog contains getgroups");
        assert_eq!(
            probe.status,
            SystemProbeStatus::LiveOwned {
                api: "SupplementaryGroups::acquire",
            }
        );
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example = fs::read_to_string(root.join("examples/getgroups.md"))
        .expect("getgroups documentation is readable");
    assert!(example.contains("SupplementaryGroups::acquire"));
    assert!(!example.contains("(dlcall"));

    let readme = fs::read_to_string(root.join("README.md")).expect("crate README is readable");
    assert!(readme.contains("](examples/getgroups.md)"));
}

#[test]
fn unix_statvfs_has_typed_snapshot_documentation() {
    for cell in [LINUX_X86_64, LINUX_AARCH64, MACOS_X86_64, MACOS_AARCH64] {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "statvfs")
            .expect("Unix catalog contains statvfs");
        assert_eq!(
            probe.status,
            SystemProbeStatus::LiveOwned {
                api: "StatVfsSnapshot::acquire",
            }
        );
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example = fs::read_to_string(root.join("examples/statvfs.md"))
        .expect("statvfs documentation is readable");
    assert!(example.contains("StatVfsSnapshot::acquire"));
    assert!(!example.contains("(dlcall"));

    let readme = fs::read_to_string(root.join("README.md")).expect("crate README is readable");
    assert!(readme.contains("](examples/statvfs.md)"));
}

#[test]
fn unix_gethostname_has_typed_snapshot_documentation() {
    for cell in [LINUX_X86_64, LINUX_AARCH64, MACOS_X86_64, MACOS_AARCH64] {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "gethostname")
            .expect("Unix catalog contains gethostname");
        assert_eq!(
            probe.status,
            SystemProbeStatus::LiveOwned {
                api: "HostnameSnapshot::acquire",
            }
        );
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example = fs::read_to_string(root.join("examples/gethostname.md"))
        .expect("gethostname documentation is readable");
    assert!(example.contains("HostnameSnapshot::acquire"));
    assert!(!example.contains("(dlcall"));

    let readme = fs::read_to_string(root.join("README.md")).expect("crate README is readable");
    assert!(readme.contains("](examples/gethostname.md)"));
}

#[test]
fn darwin_dladdr_keeps_dlcall_and_typed_snapshot_documentation() {
    for cell in [MACOS_X86_64, MACOS_AARCH64] {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "dladdr")
            .expect("Darwin catalog contains dladdr");
        assert_eq!(
            probe.status,
            SystemProbeStatus::LiveDlcallOwned {
                lib: "libSystem.B.dylib",
                symbol: "dladdr",
                api: "DlAddressSnapshot::current_image",
            }
        );
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example =
        fs::read_to_string(root.join("examples/dladdr.md")).expect("dladdr docs are readable");
    assert!(example.contains("DlAddressSnapshot::current_image"));
    assert!(example.contains("dlcall"));

    let readme = fs::read_to_string(root.join("README.md")).expect("crate README is readable");
    assert!(readme.contains("](examples/dladdr.md)"));
}

#[test]
fn darwin_domain_name_has_typed_snapshot_documentation() {
    for cell in [MACOS_X86_64, MACOS_AARCH64] {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "getdomainname")
            .expect("Darwin catalog contains getdomainname");
        assert_eq!(
            probe.status,
            SystemProbeStatus::LiveOwned {
                api: "DomainNameSnapshot::acquire",
            }
        );
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example = fs::read_to_string(root.join("examples/getdomainname.md"))
        .expect("getdomainname documentation is readable");
    assert!(example.contains("DomainNameSnapshot::acquire"));
    assert!(!example.contains("(dlcall"));

    let readme = fs::read_to_string(root.join("README.md")).expect("crate README is readable");
    assert!(readme.contains("](examples/getdomainname.md)"));
}

#[test]
fn darwin_login_name_has_typed_snapshot_documentation() {
    for cell in [MACOS_X86_64, MACOS_AARCH64] {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "getlogin_r")
            .expect("Darwin catalog contains getlogin_r");
        assert_eq!(
            probe.status,
            SystemProbeStatus::LiveOwned {
                api: "LoginNameSnapshot::acquire",
            }
        );
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example = fs::read_to_string(root.join("examples/getlogin-r.md"))
        .expect("getlogin_r documentation is readable");
    assert!(example.contains("LoginNameSnapshot::acquire"));
    assert!(!example.contains("(dlcall"));

    let readme = fs::read_to_string(root.join("README.md")).expect("crate README is readable");
    assert!(readme.contains("](examples/getlogin-r.md)"));
}

#[test]
fn darwin_mach_timebase_keeps_dlcall_and_typed_snapshot_documentation() {
    for cell in [MACOS_X86_64, MACOS_AARCH64] {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "mach_timebase_info")
            .expect("Darwin catalog contains Mach timebase");
        assert_eq!(
            probe.status,
            SystemProbeStatus::LiveDlcallOwned {
                lib: "libSystem.B.dylib",
                symbol: "mach_timebase_info",
                api: "MachTimebaseSnapshot::acquire",
            }
        );
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example = fs::read_to_string(root.join("examples/mach-timebase-info.md"))
        .expect("Mach timebase docs are readable");
    assert!(example.contains("MachTimebaseSnapshot::acquire"));
    assert!(example.contains("dlcall"));
    let readme = fs::read_to_string(root.join("README.md")).expect("crate README is readable");
    assert!(readme.contains("](examples/mach-timebase-info.md)"));
}
