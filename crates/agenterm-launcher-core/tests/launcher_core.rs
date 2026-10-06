use agenterm_launcher_core::manifest::{self, ManifestError};
use agenterm_launcher_core::probe::{self, Probe};
use agenterm_launcher_core::select::{self, Pick};
use agenterm_launcher_core::state::{decide, Inputs, LauncherState};
use agenterm_launcher_core::verify::{verify_stream, VerifyError};
use agenterm_launcher_core::{install, version::Version};

const REAL: &[u8] = include_bytes!("fixtures/minicon-0.2.3-manifest.json");
// Captured from MiniCon fb40827 on macOS.
const VERSION_JSON: &str =
    r#"{"version":"0.2.3","hostif":"1.0","os":"macos","arch":"aarch64","asset":"macos-universal"}"#;
const HANDSHAKE: &str = r#"{"hostif":"1.0","version":"0.2.3","capabilities":["exec","mux","pty"]}"#;

fn real() -> manifest::Manifest {
    manifest::parse(REAL).expect("real 0.2.3 manifest parses")
}

fn with(f: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut v: serde_json::Value = serde_json::from_slice(REAL).unwrap();
    f(&mut v);
    serde_json::to_vec(&v).unwrap()
}

// ---- manifest ----

#[test]
fn real_manifest_is_schema_1_with_seven_assets() {
    let m = real();
    assert_eq!((m.schema, m.version.as_str(), m.assets.len()), (1, "0.2.3", 7));
}

#[test]
fn unknown_schema_is_refused() {
    let b = with(|v| v["schema"] = 2.into());
    assert_eq!(manifest::parse(&b), Err(ManifestError::UnknownSchema(2)));
}

#[test]
fn wrong_kind_and_tag_mismatch_are_refused() {
    assert!(matches!(manifest::parse(&with(|v| v["kind"] = "x".into())), Err(ManifestError::WrongKind(_))));
    assert!(matches!(
        manifest::parse(&with(|v| v["expected_tag"] = "v9.9.9".into())),
        Err(ManifestError::TagMismatch { .. })
    ));
}

#[test]
fn oversize_manifest_is_refused_before_parsing() {
    let big = vec![b' '; 256 * 1024 + 1];
    assert!(matches!(manifest::parse(&big), Err(ManifestError::TooLarge(_))));
}

// ---- probe ----

#[test]
fn probe_json_is_hostif() {
    assert!(matches!(probe::parse_version_json(0, VERSION_JSON), Probe::Hostif(_)));
}

#[test]
fn legacy_plain_text_version_exit_0_is_legacy() {
    assert_eq!(probe::parse_version_json(0, "minicon 0.2.3\n"), Probe::Legacy);
}

#[test]
fn legacy_unknown_flag_exit_2_is_legacy() {
    assert_eq!(probe::parse_version_json(2, VERSION_JSON), Probe::Legacy);
}

// ---- select ----

#[test]
fn every_host_picks_its_native_asset_and_macos_prefers_dmg() {
    let m = real();
    for (os, arch, want) in [
        ("windows", "x86_64", "minicon-0.2.3-windows-x86_64.zip"),
        ("windows", "aarch64", "minicon-0.2.3-windows-arm64.zip"),
        ("linux", "x86_64", "minicon-0.2.3-linux-x86_64.tar.gz"),
        ("linux", "aarch64", "minicon-0.2.3-linux-arm64.tar.gz"),
        ("macos", "aarch64", "minicon-0.2.3-macos-universal.dmg"),
        ("macos", "x86_64", "minicon-0.2.3-macos-universal.dmg"),
    ] {
        let key = select::host_asset_key(os, arch);
        match select::pick(&m, key.as_deref()) {
            Pick::Native(a) => assert_eq!(a.name, want, "{os}/{arch}"),
            other => panic!("{os}/{arch}: {other:?}"),
        }
    }
}

#[test]
fn unknown_host_falls_back_to_ape() {
    let m = real();
    assert!(matches!(select::pick(&m, select::host_asset_key("freebsd", "x86_64").as_deref()), Pick::ApeFallback(a) if a.name == "minicon.com"));
}

#[test]
fn macos_falls_back_to_tar_when_dmg_absent() {
    let mut m = real();
    m.assets.retain(|a| !a.name.ends_with(".dmg"));
    assert!(matches!(select::pick(&m, Some("macos-universal")), Pick::Native(a) if a.name.ends_with(".tar.gz")));
}

// ---- state ----

fn state(probe: Option<&Probe>, hs: Option<&str>, latest: Option<&str>) -> LauncherState {
    let h = hs.and_then(|s| probe::parse_handshake(0, s));
    decide(&Inputs { probe, handshake: h.as_ref(), latest_version: latest })
}

#[test]
fn state_table() {
    let p = probe::parse_version_json(0, VERSION_JSON);
    assert_eq!(state(None, None, Some("0.2.4")), LauncherState::Missing);
    assert_eq!(state(None, None, None), LauncherState::OfflineMissing);
    assert_eq!(state(Some(&Probe::Legacy), None, Some("0.2.4")), LauncherState::Legacy);
    assert_eq!(
        state(Some(&p), Some(HANDSHAKE), Some("0.2.4")),
        LauncherState::UpdateAvailable { local: "0.2.3".into(), latest: "0.2.4".into() }
    );
    assert_eq!(
        state(Some(&p), Some(HANDSHAKE), Some("0.2.3")),
        LauncherState::Current { local: "0.2.3".into(), remote_older: false }
    );
    assert_eq!(state(Some(&p), Some(HANDSHAKE), None), LauncherState::Offline { local: "0.2.3".into() });
}

#[test]
fn remote_older_never_offers_downgrade() {
    let p = probe::parse_version_json(0, VERSION_JSON);
    let s = state(Some(&p), Some(HANDSHAKE), Some("0.2.2"));
    assert_eq!(s, LauncherState::Current { local: "0.2.3".into(), remote_older: true });
    assert!(!s.needs_consent_to_download());
}

#[test]
fn hostif_major_2_and_missing_capability_are_incompatible() {
    let p2 = probe::parse_version_json(0, &VERSION_JSON.replace("\"1.0\"", "\"2.0\""));
    assert!(matches!(state(Some(&p2), None, Some("0.2.3")), LauncherState::Incompatible { .. }));
    let p = probe::parse_version_json(0, VERSION_JSON);
    let hs = HANDSHAKE.replace(",\"pty\"", "");
    assert!(matches!(state(Some(&p), Some(&hs), Some("0.2.3")), LauncherState::Incompatible { .. }));
}

#[test]
fn version_ordering_is_numeric() {
    assert!(Version::parse("0.2.10") > Version::parse("0.2.9"));
    assert_eq!(Version::parse("1.0"), Version::parse("1.0").clone());
    assert!(Version::parse("1.0-rc1").is_none());
}

// ---- verify ----

fn asset_for(data: &[u8]) -> manifest::Asset {
    use sha2::{Digest, Sha256};
    let h: String = Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect();
    manifest::Asset { name: "x".into(), bytes: data.len() as u64, sha256: h, sidecar: None }
}

#[test]
fn verify_accepts_exact_bytes_and_rejects_tampering() {
    let data = b"minicon bytes";
    let a = asset_for(data);
    assert_eq!(verify_stream(&data[..], &a), Ok(()));
    assert_eq!(verify_stream(&b"minicon bytez"[..], &a), Err(VerifyError::HashMismatch));
    assert!(matches!(verify_stream(&data[..5], &a), Err(VerifyError::SizeMismatch { .. })));
    assert!(matches!(verify_stream(&b"minicon bytes!!"[..], &a), Err(VerifyError::Oversize { .. })));
}

// ---- install ----

#[test]
fn install_activate_and_rollback_switch_only_the_pointer() {
    let root = tempfile::tempdir().unwrap();
    for v in ["0.2.3", "0.2.4"] {
        let staged = tempfile::tempdir_in(root.path()).unwrap().keep();
        std::fs::write(staged.join("minicon"), v).unwrap();
        install::commit_version(root.path(), &staged, v).unwrap();
    }
    install::activate(root.path(), "0.2.3").unwrap();
    let p = install::activate(root.path(), "0.2.4").unwrap();
    assert_eq!(p.previous.as_deref(), Some("0.2.3"));
    let p = install::rollback(root.path()).unwrap();
    assert_eq!((p.current.as_str(), p.previous.as_deref()), ("0.2.3", Some("0.2.4")));
    // Both version dirs remain intact.
    assert_eq!(std::fs::read_to_string(root.path().join("0.2.4/minicon")).unwrap(), "0.2.4");
    assert!(install::activate(root.path(), "9.9.9").is_err());
}
