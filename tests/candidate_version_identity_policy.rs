const CANDIDATE: &str = include_str!("../.github/workflows/candidate.yml");

#[test]
fn candidate_requires_the_exact_current_main_sha() {
    assert!(CANDIDATE.contains("[[ \"$(git rev-parse origin/main)\" == \"$SOURCE_SHA\" ]]"));
    assert!(!CANDIDATE.contains("git merge-base --is-ancestor \"$SOURCE_SHA\" origin/main"));
}

#[test]
fn candidate_rejects_an_already_published_version_before_building() {
    assert!(CANDIDATE.contains(
        "git ls-remote --exit-code --tags origin \"refs/tags/v$version\""
    ));
    assert!(CANDIDATE.contains(
        "already exists; bump and commit the next version before Candidate"
    ));
}
