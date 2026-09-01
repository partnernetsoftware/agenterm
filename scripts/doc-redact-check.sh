#!/usr/bin/env bash
# Scan public text files for host identity, home paths, and common credential leaks.
# Docs must use repo-relative paths or ~/... only (see Agents.md Document redaction).
set -euo pipefail

# With no arguments, scan every tracked public-text format. This is the mode
# used by Candidate preflight: checking only files touched in the last commit
# allowed old disclosures to survive indefinitely.
paths=("$@")
if [[ ${#paths[@]} -eq 0 ]]; then
  while IFS= read -r -d '' path; do
    paths+=("$path")
  done < <(git ls-files -z -- \
    '*.md' '*.html' '*.yml' '*.yaml' '*.sh' '*.bat' '*.cmd' '*.js' '*.json')
fi

if [[ ${#paths[@]} -eq 0 ]]; then
  echo "doc-redact-check: no tracked public text"
  exit 0
fi

scan_paths=()
for path in "${paths[@]}"; do
  [[ "$path" == "scripts/doc-redact-check.sh" ]] && continue
  scan_paths+=("$path")
done

if [[ ${#scan_paths[@]} -eq 0 ]]; then
  echo "doc-redact-check: clean"
  exit 0
fi

# Match host home forms that must become ~/... (see Agents.md conversion table):
# Darwin /Users/, Linux /home/, Windows C:\Users\ and %USERPROFILE% / $env:USERPROFILE
pattern='(^|[^~])(/Users/|/home/|[A-Za-z]:\\Users\\)|%USERPROFILE%|%UserProfile%|\$env:USERPROFILE|[[:alnum:]._%+-]+@[[:alnum:].-]+\.[[:alpha:]]{2,}|-----BEGIN (OPENSSH |RSA |EC |DSA )?PRIVATE KEY-----|gh[opusr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[A-Z0-9]{16}'

if command -v rg >/dev/null 2>&1; then
  # The checker contains its own patterns and is excluded. RFC 2606 example
  # domains and GitHub noreply identities are intentional public placeholders.
  hits="$(rg -n -e "$pattern" \
    --glob '!scripts/doc-redact-check.sh' \
    --glob '!target/**' --glob '!**/node_modules/**' \
    "${scan_paths[@]}" 2>/dev/null \
    | rg -v '@(example\.(com|org|net)|example\.invalid|users\.noreply\.github\.com)([^[:alnum:].-]|$)' \
    || true)"
  if [[ -n "$hits" ]]; then
    printf '%s\n' "$hits"
    echo "doc-redact-check: hits above must be redacted (repo-relative, ~/, or placeholders)" >&2
    exit 1
  fi
else
  # grep -E fallback (no globs); callers should prefer rg when available
  if grep -nE "$pattern" "${scan_paths[@]}" 2>/dev/null; then
    echo "doc-redact-check: hits above must be redacted (repo-relative, ~/, or placeholders)" >&2
    exit 1
  fi
fi

echo "doc-redact-check: clean"
exit 0
