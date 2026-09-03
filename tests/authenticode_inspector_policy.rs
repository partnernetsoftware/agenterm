const POWERSHELL: &str = include_str!("../scripts/inspect-authenticode.ps1");
const PORTABLE: &str = include_str!("../scripts/inspect-authenticode.sh");
const README: &str = include_str!("../README.md");

#[test]
fn windows_inspector_reports_trust_timestamp_and_versioninfo() {
    for contract in [
        "Get-AuthenticodeSignature",
        "PARTNERNET SOFTWARE PTY LTD",
        "TimeStamperCertificate",
        "product_name",
        "product_version",
        "file_description",
        "original_filename",
        "exit 2",
        "exit 69",
    ] {
        assert!(POWERSHELL.contains(contract), "missing Windows inspector contract: {contract}");
    }
}

#[test]
fn portable_inspector_is_explicitly_diagnostic() {
    let readme_words = README.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(PORTABLE.contains("osslsigncode verify"));
    assert!(PORTABLE.contains("Windows Get-AuthenticodeSignature is authoritative"));
    assert!(readme_words.contains("The public v0.1.16 files are unsigned"));
    assert!(readme_words.contains("a qualification artifact is not a signed Release"));
}
