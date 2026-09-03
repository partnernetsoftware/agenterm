const SIGNING_SCRIPT: &str = include_str!("../scripts/windows-signing-candidate.ps1");
const RECEIPT_VALIDATOR: &str = include_str!("../scripts/qjs/lib/release_candidate.qjs");

#[test]
fn public_signing_receipt_has_explicit_trust_and_release_facts() {
    assert!(SIGNING_SCRIPT.contains("authenticode_status = \"$($signature.Status)\""));
    assert!(SIGNING_SCRIPT.contains("release_eligible = -not $QualificationOnly"));
    assert!(SIGNING_SCRIPT.contains("signing state eligibility mismatch"));
    assert!(RECEIPT_VALIDATOR.contains("receipt.release_eligible === true"));
    assert!(RECEIPT_VALIDATOR.contains("receipt_asset.authenticode_status === \"Valid\""));
}

#[test]
fn public_signing_receipt_omits_protected_provider_coordinates() {
    for forbidden in [
        "provider_resource",
        "ARTIFACT_SIGNING_ENDPOINT",
        "ARTIFACT_SIGNING_ACCOUNT",
        "ARTIFACT_SIGNING_PROFILE",
        "AZURE_CLIENT_ID",
        "AZURE_TENANT_ID",
        "AZURE_SUBSCRIPTION_ID",
    ] {
        assert!(
            !SIGNING_SCRIPT.contains(forbidden),
            "public signing receipt script contains protected coordinate key: {forbidden}"
        );
    }
}
