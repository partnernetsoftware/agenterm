//! Lua bytecode cache: source hash → cached bytecode file.

use std::path::PathBuf;

use sha2::Digest;

use crate::compile::{compile_lua, hash_source};

/// Cache directory for compiled Lua bytecode: `<cache base>/AgenTerm/lua-cache`.
pub fn cache_dir() -> PathBuf {
    cache_base().join("AgenTerm").join("lua-cache")
}

/// Where a cache belongs on this platform, and never a relative path.
///
/// The first version read `LOCALAPPDATA` and fell back to
/// `$USERPROFILE/AppData/Local` -- both Windows names -- with `"."` when
/// neither was set. On macOS and Linux that is every run, so `cargo test -p
/// agenterm-lua` wrote `crates/agenterm-lua/AppData/Local/AgenTerm/lua-cache`
/// into the repository (found 2026-08-30). Order now: an explicit
/// `AGENTERM_LUA_CACHE_DIR`, the platform's cache directory, else the
/// temporary directory. Every answer is absolute.
fn cache_base() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGENTERM_LUA_CACHE_DIR") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return dir;
        }
    }
    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local);
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library").join("Caches");
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
            return PathBuf::from(xdg);
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".cache");
        }
    }
    std::env::temp_dir()
}

/// Result of cached compilation.
#[derive(Debug)]
pub struct CachedCompileResult {
    /// The bytecode (either freshly compiled or from cache).
    pub bytecode: Vec<u8>,
    /// SHA256 hex hash of the bytecode.
    pub bytecode_hash: String,
    /// True if the bytecode was served from cache.
    pub cache_hit: bool,
}

/// Compile Lua source with caching: if source hash matches an existing cache entry,
/// return cached bytecode. Otherwise compile fresh and store in cache.
pub fn cached_compile(source: &str) -> Result<CachedCompileResult, String> {
    let source_hash = hash_source(source);
    let cache_dir = cache_dir();
    let cache_path = cache_dir.join(format!("{source_hash}.luac"));

    // Check cache
    if cache_path.exists() {
        let bytecode = std::fs::read(&cache_path).map_err(|e| format!("cache_read: {e}"))?;
        let hash = sha2::Sha256::digest(&bytecode);
        let bytecode_hash = hex_encode(&hash);
        return Ok(CachedCompileResult {
            bytecode,
            bytecode_hash,
            cache_hit: true,
        });
    }

    // Compile fresh
    let (bytecode, bytecode_hash) = compile_lua(source)?;

    // Store in cache
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("cache_create_dir: {e}"))?;
    std::fs::write(&cache_path, &bytecode).map_err(|e| format!("cache_write: {e}"))?;

    Ok(CachedCompileResult {
        bytecode,
        bytecode_hash,
        cache_hit: false,
    })
}

/// Clear the cache for a specific source hash.
pub fn clear_cache_for_source(source: &str) {
    let source_hash = hash_source(source);
    let cache_path = cache_dir().join(format!("{source_hash}.luac"));
    let _ = std::fs::remove_file(&cache_path);
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_compile_is_miss() {
        clear_cache_for_source("return 99");
        let result = cached_compile("return 99").expect("compile");
        assert!(!result.cache_hit, "first compile must be a miss");
        assert!(!result.bytecode.is_empty());
        assert_eq!(result.bytecode_hash.len(), 64);
    }

    #[test]
    fn second_compile_is_hit() {
        clear_cache_for_source("return 88");
        let r1 = cached_compile("return 88").expect("compile1");
        assert!(!r1.cache_hit);
        let r2 = cached_compile("return 88").expect("compile2");
        assert!(r2.cache_hit, "second compile must be a hit");
        assert_eq!(r1.bytecode_hash, r2.bytecode_hash);
    }

    #[test]
    fn different_sources_different_cache() {
        clear_cache_for_source("return 1");
        clear_cache_for_source("return 2");
        let r1 = cached_compile("return 1").expect("compile1");
        let r2 = cached_compile("return 2").expect("compile2");
        assert!(!r1.cache_hit, "fresh source 1");
        assert!(!r2.cache_hit, "fresh source 2");
        assert_ne!(r1.bytecode_hash, r2.bytecode_hash);
    }
}

#[cfg(test)]
mod cache_dir_tests {
    use super::cache_dir;

    /// The one property that put a directory into the repository: the
    /// answer must be absolute, and never under the working directory.
    #[test]
    fn the_cache_directory_is_absolute_and_not_under_the_working_directory() {
        let dir = cache_dir();
        assert!(dir.is_absolute(), "{}", dir.display());
        let cwd = std::env::current_dir().expect("cwd");
        assert!(!dir.starts_with(&cwd), "{} is under {}", dir.display(), cwd.display());
        assert!(dir.ends_with("AgenTerm/lua-cache"), "{}", dir.display());
    }
}
