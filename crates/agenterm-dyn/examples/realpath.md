# Resolve a Unix path with `ResolvedPath`

Linux and macOS expose `ResolvedPath::acquire(path)` as the typed owner for
`realpath(path, NULL)`. It copies the native result into an owned `PathBuf` and
releases the C allocation exactly once before returning.

```rust
use std::path::Path;

use agenterm_dyn::ResolvedPath;

let resolved = ResolvedPath::acquire(Path::new("."))?;
assert!(resolved.as_path().is_absolute());
# Ok::<(), agenterm_dyn::RealPathError>(())
```

Interior NUL input and native failures remain typed errors. Windows returns
`RealPathError::Unsupported`; it is not reported as a live Unix fact.
