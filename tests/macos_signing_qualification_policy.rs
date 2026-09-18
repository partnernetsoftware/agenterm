//! Static gate assertions for AgenTerm's Apple Developer ID qualification court.
//!
//! These are text contracts over the workflow, exactly like
//! `windows_signing_qualification_policy.rs`. `actionlint` validates expression
//! shape; it cannot tell that a receipt stopped being non-promotable or that the
//! stapling step was dropped. Each assertion below exists because removing the
//! thing it names would produce a green run that proves nothing.

const WORKFLOW: &str = include_str!("../.github/workflows/macos-signing-qualification.yml");
const CANDIDATE: &str = include_str!("../.github/workflows/candidate.yml");
const SIGNING_SCRIPT: &str = include_str!("../scripts/sign-macos-release.sh");
const AUDITOR: &str = include_str!("../scripts/audit-macos-signing-receipt.py");

#[test]
fn qualification_consumes_exact_unsigned_candidate_without_rebuild() {
    for contract in [
        "workflow_dispatch:",
        "Exact current main SHA or immutable vX.Y.Z tag SHA",
        "Successful unsigned Release Candidate run for source_sha",
        "git rev-parse origin/main",
        "source_class=current-main",
        "source_class=immutable-version-tag",
        "refs/tags/$tag^{}",
        "git show \"$SOURCE_SHA:release-policy.json\"",
        "git show \"$SOURCE_SHA:Cargo.toml\"",
        ".github/workflows/candidate.yml",
        "candidate-part-macos-aarch64",
        "candidate-part-macos-x86_64",
        ".signing.macos <<<\"$policy\")\" == unsigned-preview",
        "select(.name == $name and (.expired | not))",
    ] {
        assert!(
            WORKFLOW.contains(contract),
            "missing exact-input contract: {contract}"
        );
    }
    // A qualification transforms sealed bytes; it never produces new ones.
    assert!(!WORKFLOW.contains("cargo build"));
    assert!(!WORKFLOW.contains("client-build"));
    assert!(WORKFLOW.contains("ref: ${{ github.sha }}"));
    assert!(!WORKFLOW.contains("ref: ${{ inputs.source_sha }}"));
}

#[test]
fn qualification_is_real_signing_but_never_release_eligible() {
    for contract in [
        "environment: release-signing",
        "secrets.APPLE_DEVELOPER_ID_P12_BASE64",
        "secrets.APPLE_DEVELOPER_ID_P12_PASSWORD",
        "secrets.APPLE_NOTARY_KEY_P8_BASE64",
        "secrets.APPLE_NOTARY_KEY_ID",
        "secrets.APPLE_NOTARY_ISSUER_ID",
        "vars.AGENTERM_APPLE_TEAM_ID",
        "required release-signing value is missing",
        "./scripts/sign-macos-release.sh",
        "--release-eligible false",
        "\"release_eligible\": False,",
        "\"kind\": \"agenterm-macos-signing-qualification\"",
        "audit-macos-signing-receipt.py audit",
    ] {
        assert!(
            WORKFLOW.contains(contract),
            "missing qualification contract: {contract}"
        );
    }
    // Nothing in this lane may publish, tag, or claim eligibility.
    assert!(!WORKFLOW.contains("gh release"));
    assert!(!WORKFLOW.contains("git tag"));
    assert!(!WORKFLOW.contains("release_eligible\": True"));
}

#[test]
fn qualification_proves_the_gatekeeper_verdict_a_bare_binary_cannot() {
    // A bare Mach-O cannot be stapled and `spctl -t exec` rejects it as "not an
    // app" even when Apple holds the ticket. Only the bundle can carry the
    // ticket offline, so the bundle is what must be stapled and assessed.
    for contract in [
        "ditto -c -k --keepParent work/AgenTerm.app work/AgenTerm.app.zip",
        "xcrun notarytool submit work/AgenTerm.app.zip",
        "[[ \"$(jq -r .status <<<\"$result\")\" == Accepted ]]",
        "xcrun stapler staple work/AgenTerm.app",
        "xcrun stapler validate work/AgenTerm.app",
        "spctl -a -t exec -vv work/AgenTerm.app",
        "grep -q 'source=Notarized Developer ID' spctl.txt",
        "TeamIdentifier=$AGENTERM_APPLE_TEAM_ID",
        "grep -q 'flags=.*runtime'",
    ] {
        assert!(
            WORKFLOW.contains(contract),
            "missing Gatekeeper contract: {contract}"
        );
    }
    // The aggregate must refuse an unstapled or unassessed bundle.
    assert!(WORKFLOW.contains("a macOS bundle was not stapled"));
    assert!(WORKFLOW.contains("Gatekeeper did not accept a macOS bundle"));
    assert!(WORKFLOW.contains("a macOS bundle is not a notarized Developer ID"));
}

#[test]
fn qualification_requires_both_macos_cells_and_exact_byte_change() {
    for contract in [
        "platform_id: macos-aarch64",
        "platform_id: macos-x86_64",
        "macOS court set mismatch",
        "one or more macOS receipts became promotable",
        "\"kind\": \"agenterm-macos-signing-qualification-aggregate\"",
        "unsigned Candidate input already carries a signature",
        "provider did not change the bytes of",
    ] {
        assert!(
            WORKFLOW.contains(contract),
            "missing two-cell contract: {contract}"
        );
    }
}

#[test]
fn receipt_auditor_rejects_protected_apple_coordinates() {
    // The Team ID and publisher name are public provenance. The Key ID, Issuer
    // ID, key material and passwords are not, and a receipt carrying one must
    // fail before it is published rather than after.
    for key in [
        "\"key_id\"",
        "\"issuer_id\"",
        "\"notary_key_id\"",
        "\"notary_issuer_id\"",
        "\"p12_password\"",
        "\"keychain_password\"",
        "\"private_key\"",
    ] {
        assert!(
            AUDITOR.contains(key),
            "receipt auditor does not reject protected key: {key}"
        );
    }
    for contract in [
        "protected configuration key at",
        "key or certificate material at",
        "was not changed by the provider",
        "receipt bundle is not stapled",
        "Gatekeeper did not accept the receipt bundle",
        "receipt bundle is not a notarized Developer ID",
        "signed asset set drift",
        "was not signed with the hardened runtime",
    ] {
        assert!(
            AUDITOR.contains(contract),
            "receipt auditor is missing assertion: {contract}"
        );
    }
    // The signed set is derived from the artifact manifest, never a glob.
    assert!(AUDITOR.contains("scripts/artifacts.json"));
}

#[test]
fn candidate_supplies_every_value_the_macos_signer_requires() {
    // sign-macos-release.sh hard-requires AGENTERM_APPLE_TEAM_ID. candidate.yml
    // previously imported the certificate and the notary key but never set the
    // Team ID, so the first signed Candidate would have died at its first
    // codesign call, after the owner had already flipped the policy.
    assert!(SIGNING_SCRIPT.contains("AGENTERM_APPLE_TEAM_ID:?"));
    assert!(CANDIDATE.contains("AGENTERM_APPLE_TEAM_ID: ${{ vars.AGENTERM_APPLE_TEAM_ID }}"));
    assert!(CANDIDATE.contains("echo \"AGENTERM_APPLE_TEAM_ID=$AGENTERM_APPLE_TEAM_ID\""));
    assert!(
        CANDIDATE.contains("AGENTERM_APPLE_TEAM_ID must be a 10-character Apple Team identifier.")
    );
    // The macOS switch stays independent of the Windows switch.
    assert!(CANDIDATE.contains("Unsupported macOS signing policy"));
    assert!(WORKFLOW.contains("independent switches with"));
}
