# Read the Darwin domain name with a typed snapshot

Use `DomainNameSnapshot::acquire()` on macOS. It immediately copies the
bounded native result into owned bytes, does not assume UTF-8, and accepts an
empty domain name. A successful native call without a real NUL terminator is
rejected rather than publishing a truncated value.

The typed court compares those owned bytes with an independent direct
`getdomainname` buffer. Other hosts return typed `Unsupported`; this API does
not expose caller-owned pointers or arbitrary native invocation.
