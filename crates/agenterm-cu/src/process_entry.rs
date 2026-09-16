//! Authoritative classification and direct execution for CU process entries.
//!
//! The monolith binary and fixed-sibling process-main provider share this
//! table. The thin launcher keeps only a boundary prediction mirror so it can
//! validate the provider result before presenting buffered output.

/// Closed process entry family published by the process-main ABI.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessEntryMode {
    OrdinaryArgv = 0,
    NativeMessagingHost = 1,
    NetworkProbeWorker = 2,
    BrowserSessionOwner = 3,
    ManagedJobOwner = 4,
    DeviceLeaseOwner = 5,
    PrivilegeBroker = 6,
    DeviceIoFixture = 7,
    NetworkProbeFixture = 8,
    HotkeyHost = 9,
    VerbsText = 10,
    X11ClipboardOwner = 11,
    VersionText = 12,
}

/// Classifies raw process argv before ordinary command parsing.
pub fn classify(args: &[String]) -> ProcessEntryMode {
    let Some(first) = args.first().map(String::as_str) else {
        return ProcessEntryMode::OrdinaryArgv;
    };
    if first.starts_with("chrome-extension://") {
        ProcessEntryMode::NativeMessagingHost
    } else if matches!(args, [arg] if arg == crate::network_probe::WORKER_ARG) {
        ProcessEntryMode::NetworkProbeWorker
    } else if first == crate::browser_session_owner::OWNER_ARG {
        ProcessEntryMode::BrowserSessionOwner
    } else if matches!(args, [arg] if arg == crate::MANAGED_JOB_OWNER_ARG) {
        ProcessEntryMode::ManagedJobOwner
    } else if matches!(args, [arg] if arg == crate::DEVICE_LEASE_OWNER_ARG) {
        ProcessEntryMode::DeviceLeaseOwner
    } else if matches!(args, [arg] if arg == crate::PRIVILEGE_BROKER_ARG) {
        ProcessEntryMode::PrivilegeBroker
    } else if first == crate::DEVICE_IO_FIXTURE_ARG {
        ProcessEntryMode::DeviceIoFixture
    } else if first == crate::network_probe::FIXTURE_ARG {
        ProcessEntryMode::NetworkProbeFixture
    } else if crate::cli::verbs::lookup(first).map(|spec| spec.name) == Some("host") {
        ProcessEntryMode::HotkeyHost
    } else if crate::cli::verbs::lookup(first).map(|spec| spec.name) == Some("verbs") {
        ProcessEntryMode::VerbsText
    } else if first == crate::mechanism::clipboard::X11_CLIPBOARD_OWNER_ARG {
        ProcessEntryMode::X11ClipboardOwner
    } else if matches!(args, [arg] if matches!(arg.as_str(), "--version" | "-V")) {
        ProcessEntryMode::VersionText
    } else {
        ProcessEntryMode::OrdinaryArgv
    }
}

/// Runs an entry whose process owns its real stdio and lifetime directly.
/// Buffered ordinary/version/verbs modes return `None` for their caller to
/// present through either the monolith wrapper or the process-main ABI.
pub fn run_direct(mode: ProcessEntryMode, args: &[String]) -> Option<i32> {
    match mode {
        ProcessEntryMode::NativeMessagingHost => {
            Some(crate::browser_bridge::run_native_host_entry(args))
        }
        ProcessEntryMode::NetworkProbeWorker => Some(crate::network_probe::run_worker_stdio()),
        ProcessEntryMode::BrowserSessionOwner => {
            Some(crate::browser_session_owner::run_owner(&args[1..]))
        }
        ProcessEntryMode::ManagedJobOwner => Some(crate::run_managed_job_owner()),
        ProcessEntryMode::DeviceLeaseOwner => Some(crate::run_device_lease_owner()),
        ProcessEntryMode::PrivilegeBroker => Some(crate::run_privilege_broker()),
        ProcessEntryMode::DeviceIoFixture => Some(crate::run_device_io_test_fixture(&args[1..])),
        ProcessEntryMode::NetworkProbeFixture => {
            Some(crate::network_probe::run_loopback_fixture(&args[1..]))
        }
        ProcessEntryMode::HotkeyHost => Some(crate::hotkeys::run()),
        ProcessEntryMode::X11ClipboardOwner => Some(crate::run_x11_clipboard_owner()),
        ProcessEntryMode::OrdinaryArgv
        | ProcessEntryMode::VerbsText
        | ProcessEntryMode::VersionText => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn classifies_every_binary_entry_and_preserves_exact_sentinels() {
        let cases = [
            (
                &["chrome-extension://foreign/"][..],
                ProcessEntryMode::NativeMessagingHost,
            ),
            (
                &[crate::network_probe::WORKER_ARG][..],
                ProcessEntryMode::NetworkProbeWorker,
            ),
            (
                &[crate::browser_session_owner::OWNER_ARG, "tail"][..],
                ProcessEntryMode::BrowserSessionOwner,
            ),
            (
                &[crate::MANAGED_JOB_OWNER_ARG][..],
                ProcessEntryMode::ManagedJobOwner,
            ),
            (
                &[crate::DEVICE_LEASE_OWNER_ARG][..],
                ProcessEntryMode::DeviceLeaseOwner,
            ),
            (
                &[crate::PRIVILEGE_BROKER_ARG][..],
                ProcessEntryMode::PrivilegeBroker,
            ),
            (
                &[crate::DEVICE_IO_FIXTURE_ARG, "tail"][..],
                ProcessEntryMode::DeviceIoFixture,
            ),
            (
                &[crate::network_probe::FIXTURE_ARG, "tail"][..],
                ProcessEntryMode::NetworkProbeFixture,
            ),
            (&["host"][..], ProcessEntryMode::HotkeyHost),
            (
                &["hotkeys", "--self-test"][..],
                ProcessEntryMode::HotkeyHost,
            ),
            (&["verbs", "--json"][..], ProcessEntryMode::VerbsText),
            (
                &[crate::mechanism::clipboard::X11_CLIPBOARD_OWNER_ARG][..],
                ProcessEntryMode::X11ClipboardOwner,
            ),
            (&["--version"][..], ProcessEntryMode::VersionText),
        ];
        for (argv, expected) in cases {
            assert_eq!(classify(&strings(argv)), expected, "{argv:?}");
        }
        for sentinel in [
            crate::network_probe::WORKER_ARG,
            crate::MANAGED_JOB_OWNER_ARG,
            crate::DEVICE_LEASE_OWNER_ARG,
            crate::PRIVILEGE_BROKER_ARG,
            "--version",
            "-V",
        ] {
            assert_eq!(
                classify(&strings(&[sentinel, "extra"])),
                ProcessEntryMode::OrdinaryArgv,
                "{sentinel} with a tail must retain ordinary typed refusal"
            );
        }
    }

    #[test]
    fn buffered_modes_return_no_direct_process_exit() {
        let args = strings(&["placeholder"]);
        for mode in [
            ProcessEntryMode::OrdinaryArgv,
            ProcessEntryMode::VerbsText,
            ProcessEntryMode::VersionText,
        ] {
            assert_eq!(run_direct(mode, &args), None, "{mode:?}");
        }
    }
}
