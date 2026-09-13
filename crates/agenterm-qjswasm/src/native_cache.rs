//! The engine-owned set of loaded native libraries.
//!
//! `agenterm-dyn` loads one library per call: [`invoke_abi`] opens it, resolves
//! the symbol and closes it on the way out, which is the right shape for a
//! one-shot caller and the wrong one for a script that calls the same library in
//! a loop. `dyn` already offers the reuse entry
//! (`invoke_abi_with_handle`), and this module is the caller it exists for: one
//! [`Engine`](crate::Engine) keeps at most [`MAX_CACHED_LIBRARIES`] handles,
//! keyed by the library string the guest declared, and hands each of them back
//! as a shared [`Rc<LibraryHandle>`].
//!
//! # The cache is an optimization, so it cannot narrow the door
//!
//! Every decision here is "reuse, or do what this crate did before the cache
//! existed":
//!
//! * an adopted string returns the handle it was adopted with;
//! * a full table returns [`LibrarySource::AtCapacity`], whose handler is the
//!   one-shot [`invoke_abi`] entry — the same call, the same typed error
//!   vocabulary. A full cache is not a refusal and gets no new error code;
//! * a load that fails is attempted **once**. Its own [`AbiError`] is handed
//!   back for the caller's existing mapping instead of being retried by the
//!   one-shot entry: opening a library runs its initialisers, and a second
//!   attempt would run them a second time for one call. A failed load also owns
//!   no slot, so a bad name can never crowd out a good one;
//! * nothing is evicted. A handle stays loaded until its engine drops, so an
//!   adopted string can never stop working at capacity.
//!
//! # What it deliberately is not
//!
//! No symbols are cached — dyn resolves the symbol per call, so a symbol that
//! disappears is still reported. The cache belongs to one engine; it is not a
//! process-global and is not shared across guests or threads, which is why it is
//! plain `Rc<RefCell<..>>` and not a lock. The key is the declared string
//! **verbatim** — an empty string means the current process, exactly as it does
//! for `NativeCall::library`, and two strings that differ by one byte are two
//! keys. It owns no schema, allow-list or budget: the door decides what may be
//! called, this decides only what may stay loaded.

use std::cell::RefCell;
use std::rc::Rc;

use agenterm_dyn::{AbiError, LibraryHandle};

/// Distinct libraries one engine keeps loaded.
///
/// A bound, not a policy: past it the engine stops optimizing and starts
/// behaving exactly as it did before this module existed. The number is the one
/// the retired `Dyn` cache used for the same purpose (32 distinct libraries, no
/// eviction), so a script that ran there has the same working set here.
pub(crate) const MAX_CACHED_LIBRARIES: usize = 32;

/// What the engine could do for one declared library string.
#[derive(Debug)]
pub(crate) enum LibrarySource {
    /// A handle this engine holds: every later call naming this exact string
    /// reuses this one load.
    Cached(Rc<LibraryHandle>),
    /// The engine's table is full. The caller takes the one-shot entry, which
    /// loads and closes its own handle exactly as it always did.
    AtCapacity,
}

/// One adopted library: the exact string it was declared with, and the handle
/// opened for it. The two travel together because
/// `invoke_abi_with_handle` refuses a call whose library string differs from the
/// handle's, so a handle must never be reachable under a name it was not opened
/// for.
struct Adopted {
    name: String,
    handle: Rc<LibraryHandle>,
}

/// The bounded, non-evicting table one engine owns.
pub(crate) struct NativeLibraryCache {
    adopted: RefCell<Vec<Adopted>>,
}

impl NativeLibraryCache {
    pub(crate) fn new() -> Self {
        Self {
            adopted: RefCell::new(Vec::new()),
        }
    }

    /// How many libraries this engine is holding open.
    pub(crate) fn len(&self) -> usize {
        self.adopted.borrow().len()
    }

    /// Resolve one declared library string for a call that is about to run.
    ///
    /// `Ok` is the whole contract for a string this engine can serve; see
    /// [`LibrarySource`]. `Err` is the load's own [`AbiError`], returned
    /// untouched so the caller's existing mapping reports it — the load is
    /// attempted exactly once per call, because opening a library runs its
    /// initialisers and a retry would run them again for one call.
    ///
    /// No borrow of the table is alive while those initialisers run: the load
    /// happens between the lookups, so a door call that somehow re-enters this
    /// one cannot meet a `BorrowMutError`.
    pub(crate) fn resolve(&self, library: &str) -> Result<LibrarySource, AbiError> {
        if let Some(handle) = self.adopted_handle(library) {
            return Ok(LibrarySource::Cached(handle));
        }
        if self.len() >= MAX_CACHED_LIBRARIES {
            return Ok(LibrarySource::AtCapacity);
        }
        // One attempt. A failure leaves the table untouched and keeps dyn's own
        // error, library name and message included.
        let handle = Rc::new(LibraryHandle::open(library)?);
        self.adopted.borrow_mut().push(Adopted {
            name: library.to_owned(),
            handle: Rc::clone(&handle),
        });
        // One engine is one thread with a synchronous door, so the capacity
        // checked above still holds here; the assertion keeps that invariant
        // visible if either half ever stops being true.
        debug_assert!(self.len() <= MAX_CACHED_LIBRARIES);
        Ok(LibrarySource::Cached(handle))
    }

    /// Borrow the table only long enough to clone the shared owner out of it,
    /// so no borrow is alive while a foreign call runs.
    fn adopted_handle(&self, library: &str) -> Option<Rc<LibraryHandle>> {
        self.adopted
            .borrow()
            .iter()
            .find(|adopted| adopted.name == library)
            .map(|adopted| Rc::clone(&adopted.handle))
    }

    /// Insert a handle under `name`, or report that the table is full.
    ///
    /// Test-only: the production caller can only reach this through [`resolve`],
    /// which opens the handle for the very string it then inserts. A test cannot
    /// conjure 32 distinct loadable libraries, and the property under test here —
    /// the bound and the exact-string key — does not depend on which library a
    /// handle names.
    #[cfg(test)]
    pub(crate) fn admit_for_test(&self, name: &str, handle: Rc<LibraryHandle>) -> bool {
        if self.len() >= MAX_CACHED_LIBRARIES {
            return false;
        }
        self.adopted.borrow_mut().push(Adopted {
            name: name.to_owned(),
            handle,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A handle any cell can open: this process. The unit court therefore needs
    /// no system library name and no compiled fixture.
    fn process_handle() -> Rc<LibraryHandle> {
        Rc::new(LibraryHandle::open("").expect("this process is always loadable"))
    }

    /// The handle this engine holds for `library`, or a panic naming the string.
    fn cached(cache: &NativeLibraryCache, library: &str) -> Rc<LibraryHandle> {
        match cache.resolve(library) {
            Ok(LibrarySource::Cached(handle)) => handle,
            Ok(LibrarySource::AtCapacity) => panic!("{library:?} must be adopted, not declined"),
            Err(error) => panic!("{library:?} must be loadable: {error}"),
        }
    }

    /// The reuse the whole module exists for: one string, one load, one
    /// allocation handed back twice.
    #[test]
    fn one_declared_string_hands_back_one_load() {
        let cache = NativeLibraryCache::new();
        let first = cached(&cache, "");
        let second = cached(&cache, "");
        assert!(
            Rc::ptr_eq(&first, &second),
            "two calls naming one library must share one handle"
        );
        assert_eq!(cache.len(), 1);
        assert_eq!(first.library_name(), "");
    }

    /// A failed load is reported once, by the mechanism's own error, and owns no
    /// slot: a bad name can neither crowd out a good one nor be opened twice for
    /// one call.
    #[test]
    fn a_failed_load_is_reported_once_and_occupies_no_slot() {
        let cache = NativeLibraryCache::new();
        let error = cache
            .resolve("agenterm-native-library-that-is-not-here")
            .expect_err("a library that does not exist cannot load");
        assert!(
            matches!(error, AbiError::LibraryLoad { .. }),
            "the load's own error must be handed back for the caller's mapping"
        );
        assert_eq!(cache.len(), 0, "a load that failed owns no slot");
        // And a second call is a second call: the name was never adopted, so it
        // fails the same way and still holds nothing.
        assert!(
            cache
                .resolve("agenterm-native-library-that-is-not-here")
                .is_err()
        );
        assert_eq!(cache.len(), 0);
    }

    /// The 33rd distinct name is declined, nothing is evicted, and the adopted
    /// names keep the handles they were admitted with.
    #[test]
    fn the_thirty_third_distinct_name_is_declined_without_eviction() {
        let cache = NativeLibraryCache::new();
        let first = process_handle();
        for index in 0..MAX_CACHED_LIBRARIES {
            let key = format!("declared-{index}");
            let handle = if index == 0 {
                Rc::clone(&first)
            } else {
                process_handle()
            };
            assert!(
                cache.admit_for_test(&key, handle),
                "slot {index} is inside the bound"
            );
        }
        assert_eq!(cache.len(), MAX_CACHED_LIBRARIES);
        assert!(
            matches!(cache.resolve("declared-32"), Ok(LibrarySource::AtCapacity)),
            "the next distinct name is the case that falls back to one-shot"
        );
        assert_eq!(
            cache.len(),
            MAX_CACHED_LIBRARIES,
            "a declined name must not evict an adopted one"
        );
        let still_there = cached(&cache, "declared-0");
        assert!(Rc::ptr_eq(&still_there, &first));
    }

    /// Keys are the declared bytes: no trimming, no case folding, no path
    /// normalization. A near-miss is a different library, and a difference in a
    /// string that was never loadable is still not a hit.
    #[test]
    fn keys_are_the_declared_bytes_verbatim() {
        let cache = NativeLibraryCache::new();
        let handle = process_handle();
        assert!(cache.admit_for_test("declared", handle));
        let _ = cached(&cache, "declared");
        for near_miss in ["declared ", "DECLARED"] {
            assert!(
                matches!(cache.resolve(near_miss), Err(AbiError::LibraryLoad { .. })),
                "{near_miss:?} names a different library and must not hit"
            );
        }
        assert_eq!(cache.len(), 1);
    }
}
