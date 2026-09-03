const WORKFLOW: &str = include_str!("../.github/workflows/windows-signing-qualification.yml");
const SIGNING_SCRIPT: &str = include_str!("../scripts/windows-signing-candidate.ps1");
const CANDIDATE_VALIDATOR: &str = include_str!("../scripts/qjs/lib/release_candidate.qjs");

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
        "candidate-part-windows-x86_64",
        "candidate-part-windows-aarch64",
        ".signing.windows <<<\"$policy\")\" == off",
        "select(.name == $name and (.expired | not))",
    ] {
        assert!(
            WORKFLOW.contains(contract),
            "missing exact-input contract: {contract}"
        );
    }
    assert!(!WORKFLOW.contains("cargo build"));
    assert!(!WORKFLOW.contains("cargo xwin"));
    assert!(WORKFLOW.contains("ref: ${{ github.sha }}"));
    assert!(!WORKFLOW.contains("ref: ${{ inputs.source_sha }}"));
}

#[test]
fn qualification_is_real_signing_but_never_release_eligible() {
    for contract in [
        "environment: release-signing",
        "id-token: write",
        "azure/login@0949e32778441b2c442592b7a0e6313466dc8f29",
        "Azure/artifact-signing-action@208f8af4bf26cf2af8597424e3cb5582801523ba",
        "windows-signing-candidate.ps1 -Mode Prepare",
        "windows-signing-candidate.ps1 -Mode Finalize",
        "-QualificationOnly",
        "-UpstreamRunId $env:CANDIDATE_RUN_ID -UpstreamRunAttempt $env:CANDIDATE_RUN_ATTEMPT",
        "--release-eligible false",
        "--archive-root final",
        "\"release_eligible\": False",
        "\"source_class\": os.environ[\"SOURCE_CLASS\"]",
    ] {
        assert!(
            WORKFLOW.contains(contract),
            "missing qualification contract: {contract}"
        );
    }
    assert!(SIGNING_SCRIPT.contains("qualification requires signing.windows=off"));
    assert!(SIGNING_SCRIPT.contains("Candidate signing requires signing.windows=required"));
    assert!(SIGNING_SCRIPT.contains("release_eligible = -not $QualificationOnly"));
    assert!(SIGNING_SCRIPT.contains("qualification requires upstream Candidate identity"));
    assert!(SIGNING_SCRIPT.contains("Candidate signing must not claim an upstream run"));
    assert!(CANDIDATE_VALIDATOR.contains("receipt.release_eligible === true"));
    assert!(!WORKFLOW.contains("gh release"));
    assert!(!WORKFLOW.contains("git tag"));
}

#[test]
fn qualification_executes_and_scans_both_windows_isas() {
    for contract in [
        "windows-2025",
        "windows-11-arm",
        "Get-AuthenticodeSignature",
        "PARTNERNET SOFTWARE PTY LTD",
        "agenterm.com cli --version",
        "MpCmdRun.exe",
        "DEFENDER PASS",
        "require both signed Windows courts",
        "runtime archive SHA does not match signing receipt",
        "actual_hash=\"$(sha256sum \"$archive\" | awk '{print $1}')\"",
        "[[ \"$actual_hash\" =~ ^[0-9a-f]{64}$ ]]",
        "os.environ.get(\"ARCHIVE_SHA256\")",
    ] {
        assert!(
            WORKFLOW.contains(contract),
            "missing final-byte court: {contract}"
        );
    }
    assert!(WORKFLOW.contains("if: always() && needs.sign.result == 'success'"));
}

#[test]
fn public_qualification_artifacts_do_not_receive_provider_coordinates() {
    let receipt_step = WORKFLOW
        .split("- name: Verify signatures, rebuild archives, and audit public receipt")
        .nth(1)
        .expect("missing qualification receipt step")
        .split("- name: Upload signed non-promotable qualification bundle")
        .next()
        .expect("missing end of qualification receipt step");
    for forbidden in [
        "ARTIFACT_SIGNING_ENDPOINT",
        "ARTIFACT_SIGNING_ACCOUNT",
        "ARTIFACT_SIGNING_PROFILE",
        "AZURE_CLIENT_ID",
        "AZURE_TENANT_ID",
        "AZURE_SUBSCRIPTION_ID",
    ] {
        assert!(
            !receipt_step.contains(forbidden),
            "protected coordinate entered receipt step: {forbidden}"
        );
    }
}
