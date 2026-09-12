# Read Unix supplementary groups with `getgroups`

On Linux and macOS, `SupplementaryGroups::acquire()` is the typed, bounded,
pointer-free API for reading the current process's supplementary group IDs.
It retries a changing two-call `getgroups` snapshot only within a fixed bound
and reports typed OS, size-limit, and instability failures.

The public contract test compares the owned group set with the independent
`id -G` process fact. POSIX permits `getgroups` to omit the effective group, so
the court accepts exactly that documented difference. Windows reports typed
`Unsupported`; no raw array or borrowed pointer crosses this API.
