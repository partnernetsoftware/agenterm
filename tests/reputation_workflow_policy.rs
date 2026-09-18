//! Static gate assertions for AgenTerm's final-byte reputation chain.
//!
//! `release-policy.json` has declared `reputation.windows_final_candidate_bytes:
//! "required"` while nothing produced or consumed that evidence. `candidate.yml`
//! asserted only that the field said "required" -- a gate that checks its own
//! configuration and calls that a pass. These tests pin the chain that now makes
//! the field mean something: Defender court -> qualification -> reputation.yml
//! -> release.yml.

const REPUTATION: &str = include_str!("../.github/workflows/reputation.yml");
const RELEASE: &str = include_str!("../.github/workflows/release.yml");
const CANDIDATE: &str = include_str!("../.github/workflows/candidate.yml");
const COURT: &str = include_str!("../scripts/utm-win-defender-court.sh");
const QUALIFIER: &str = include_str!("../scripts/agenterm-reputation-court.py");
const POLICY: &str = include_str!("../release-policy.json");

#[test]
fn policy_still_declares_the_reputation_requirement() {
    assert!(POLICY.contains("\"windows_final_candidate_bytes\": \"required\""));
    // candidate.yml keeps asserting the checked-in value; the point of the new
    // chain is that something downstream now consumes it.
    assert!(CANDIDATE.contains(
        "test \"$(jq -r .reputation.windows_final_candidate_bytes release-policy.json)\" = required"
    ));
    assert!(REPUTATION.contains(
        "test \"$(jq -r .reputation.windows_final_candidate_bytes release-policy.json)\" = required"
    ));
}

#[test]
fn reputation_workflow_binds_one_exact_successful_candidate() {
    for contract in [
        "workflow_dispatch:",
        "candidate_run_id:",
        "source_sha:",
        "qualification_base64:",
        ".github/workflows/candidate.yml",
        "[[ \"$GITHUB_SHA\" == \"$SOURCE_SHA\" ]]",
        "[[ \"$(jq -r .conclusion <<<\"$run\")\" == success ]]",
        "[[ \"$(jq -r .head_sha <<<\"$run\")\" == \"$SOURCE_SHA\" ]]",
        "release-candidate-$CANDIDATE_RUN_ID",
        "scripts/agenterm-reputation-court.py verify",
        "reputation-qualification-${{ github.run_id }}",
    ] {
        assert!(
            REPUTATION.contains(contract),
            "reputation workflow is missing: {contract}"
        );
    }
    // The qualification is verified, never trusted because it parsed.
    assert!(!REPUTATION.contains("continue-on-error"));
    assert!(!REPUTATION.contains("|| true"));
}

#[test]
fn release_fails_closed_without_a_reputation_qualification() {
    for contract in [
        "reputation_run_id:",
        "Require the final-byte reputation qualification",
        "mode=\"$(jq -r .reputation.windows_final_candidate_bytes release-policy.json)\"",
        "Unsupported reputation policy",
        "release-policy.json requires a final-byte reputation qualification.",
        "[[ \"$(jq -r .path <<<\"$reputation\")\" == \".github/workflows/reputation.yml\" ]]",
        "[[ \"$(jq -r .conclusion <<<\"$reputation\")\" == success ]]",
        "[[ \"$(jq -r .head_sha <<<\"$reputation\")\" == \"$SOURCE_SHA\" ]]",
        "reputation-qualification-$REPUTATION_RUN_ID",
        // Re-derived against the sealed manifest, not merely "a run existed".
        "scripts/agenterm-reputation-court.py verify",
        "--qualification reputation/reputation-qualification.json",
    ] {
        assert!(
            RELEASE.contains(contract),
            "release workflow is missing reputation gate: {contract}"
        );
    }
}

#[test]
fn defender_court_is_agenterm_owned_and_never_routes_through_another_product() {
    for contract in [
        "agenterm-defender-court",
        "AGENTERM_DEFENDER_COURT",
        "win-aarch64-desktop",
        "win-x86_64-desktop",
        // The same product-neutral CLI discovery and spoof guard the other
        // AgenTerm court callers use.
        "Uniform, product-neutral lifecycle",
        "UTM_COURT_CLI",
        "UTM_COURT_HOME",
        "$COURT_CLI\" lease \"$COURT\" --disposable",
        "$COURT_CLI\" release \"$COURT\"",
        "trap cleanup EXIT",
        "-DisableRemediation",
        "MpCmdRun.exe",
        // Windows guests are driven through the login-session job agent.
        // utm-court's interactive-exec is Linux-only and answers a Windows
        // court with "interactive-exec is currently Linux-only", so a court
        // built on it would fail on every run.
        "windows-agent-root",
        "job.pending.ps1",
        "job.ready",
    ] {
        assert!(
            COURT.contains(contract),
            "Defender court is missing: {contract}"
        );
    }
    assert!(
        !COURT.contains("interactive-exec"),
        "Windows Defender court must not use the Linux-only interactive-exec"
    );
    // Never borrow another product's runner scripts or receipt kinds.
    assert!(!COURT.contains("minicon"));
    assert!(!COURT.contains("utm-runner"));
    assert!(!QUALIFIER.contains("minicon"));
    // The verdict must be derived, never injected by the operator.
    assert!(COURT.contains("the Defender verdict is machine-derived"));
}

#[test]
fn qualifier_binds_exactly_the_sealed_windows_bytes() {
    for contract in [
        "agenterm-defender-court",
        "agenterm-reputation-qualification",
        "{\"windows-x86_64\", \"windows-aarch64\"}",
        "Defender changed the bytes of",
        "Defender reported a detection on",
        "Defender scanned bytes are not the sealed Candidate Windows archives",
        "does not cover exactly the sealed Windows Candidate bytes",
        "Defender court was not run against the Candidate source SHA",
        "Defender court is bound to a different Candidate run",
        "qualification is bound to a different Candidate run",
        "a foreign product's court receipt is not AgenTerm evidence",
    ] {
        assert!(
            QUALIFIER.contains(contract),
            "reputation qualifier is missing: {contract}"
        );
    }
}
