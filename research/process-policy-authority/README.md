# Process-policy authority probe

This directory implements the bounded experiment specified by
`plan/design-process-policy-authority-experiment.md`. It uses one owned child
and one unrelated sibling; it never targets an existing user process.

From the repository root on macOS:

```sh
mkdir -p target/process-policy-authority
clang -std=c11 -O2 -Wall -Wextra -Werror \
  research/process-policy-authority/probe.c \
  -o target/process-policy-authority/probe
target/process-policy-authority/probe
```

The executable is a local ignored experiment artifact. Its structured output
contains only PIDs, Mach return codes and policy flags—no path, environment,
process arguments, credential or user identity.
