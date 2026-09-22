//! The shared file-module resolver and its accounting ledger.
//!
//! Every qjswasm door that lets a `.qjs` file `import` another one needs the
//! same five robustness answers: a per-source byte ceiling, an aggregate byte
//! ledger, a resolved-module count ceiling, a wall deadline, and one canonical
//! path cache so a shared module is read and charged once. `check-many` grew
//! them first, in this crate, and the three single-file doors (`script check`,
//! `script run`, `script hash`) went without -- which is why a 51-byte entry
//! could import a 200 KB module under a 1 KiB `--max-source-bytes` on the
//! single-file path. This module is that accounting, written once, so the next
//! door to gain a resolver gains the contract instead of a lookalike.
//!
//! # What the ledger can and cannot decide
//!
//! The compiler upstream owns module **identity**: it keys modules by the
//! specifier string our callback was asked for. This crate's callback is
//! `&dyn Fn(&str) -> Option<String>` -- a specifier in, source text out -- with
//! no identity, budget, deadline or cancellation channel, and that signature
//! is pinned upstream (`tinyvm-qjs`, rev `f476cd2`). So the canonical cache
//! below changes **reading and charging**: the second specifier that lands on
//! the same file is served from the cache and is not charged twice. It does
//! **not** make the compiler evaluate that file once; the compiler still has
//! two specifiers and therefore two modules. A file reached as both `lib/x`
//! and `lib/x.qjs` is still evaluated twice. Changing that needs an identity
//! channel upstream, not a cleverer callback here.
//!
//! # Why the failure survives compilation
//!
//! Returning `None` is the only way to refuse a specifier, and the compiler
//! reports every such refusal with one generic sentence that names the
//! specifier and nothing else. A budget refusal that reached the user as
//! "cannot resolve the module" would send them looking for a missing file. So
//! the reason is recorded here, in our own state, and the caller reads it back
//! **after** compilation returns -- the pattern `check-many` already used, now
//! shared rather than copied.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// The number of recursively resolved modules one compilation may charge.
///
/// A compile-time inclusion graph is not a link graph: each resolved module
/// becomes part of the one `.wasm`, so the ceiling bounds the work one
/// invocation can ask for.
pub const IMPORT_MODULES_MAX: usize = 1_024;

/// A product host's own module set: specifier to source.
///
/// A built-in resolves before the filesystem so a project cannot shadow it,
/// and it is charged like any other module.
pub type BuiltinModuleResolver = fn(&str) -> Option<&'static str>;

/// Which public failure class owns a resolver refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolverFailureCategory {
    /// A declared source, module-count, or wall-time ceiling was exhausted.
    Limit,
    /// The invocation's cancellation identity was set during resolution.
    Cancelled,
    /// The host could not inspect or read an otherwise resolved module.
    Host,
}

impl ResolverFailureCategory {
    /// The existing public wire spelling used by check-many reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Limit => "limit",
            Self::Cancelled => "cancelled",
            Self::Host => "host",
        }
    }
}

/// A typed refusal raised while resolving a module specifier.
///
/// `code` and `category` are fixed sets the callers map onto their own public
/// failure shape (`check-many` calls the category an `exit_class`); `message`
/// is the sentence the operator reads. All three survive compilation, because
/// the compiler's own report cannot carry them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolverFailure {
    pub code: &'static str,
    pub message: String,
    pub category: ResolverFailureCategory,
}

/// How one ledger is bounded, and where it starts.
///
/// `label` names the door in the wall-time sentence ("check-many reached its
/// aggregate wall-time budget while resolving imports"), because that sentence
/// is public text and two doors must not borrow each other's name.
///
/// `entry_bytes` and `imported_modules` are the starting charge. A caller that
/// runs several entries through one ledger passes 0 and calls
/// [`ResolverLedger::charge_entry_bytes`] per entry; a caller that already
/// knows what the entry cost may seed it here instead.
pub struct ResolverLedgerConfig {
    label: &'static str,
    deadline: Instant,
    per_source_max: usize,
    aggregate_source_max: usize,
    entry_bytes: usize,
    imported_modules: usize,
    cancellation: Option<Arc<AtomicBool>>,
    modules_max: usize,
}

impl ResolverLedgerConfig {
    pub fn new(
        label: &'static str,
        deadline: Instant,
        per_source_max: usize,
        aggregate_source_max: usize,
    ) -> Self {
        Self {
            label,
            deadline,
            per_source_max,
            aggregate_source_max,
            entry_bytes: 0,
            imported_modules: 0,
            cancellation: None,
            modules_max: IMPORT_MODULES_MAX,
        }
    }

    /// Bytes already charged before the first specifier is resolved.
    pub fn entry_bytes(mut self, bytes: usize) -> Self {
        self.entry_bytes = bytes;
        self
    }

    /// Modules already charged before the first specifier is resolved.
    pub fn imported_modules(mut self, count: usize) -> Self {
        self.imported_modules = count;
        self
    }

    /// A flag the host sets to end this call at its next host wait. The
    /// resolver has no other way to hear about a cancellation: the compiler
    /// upstream does not pass one down.
    pub fn cancellation(mut self, cancellation: Option<Arc<AtomicBool>>) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// Lower the module ceiling for a test that cannot realistically build
    /// 1025 modules. The product ceiling is [`IMPORT_MODULES_MAX`] and is not
    /// reachable from a caller: this setter exists only under `cfg(test)`.
    #[cfg(test)]
    pub fn modules_max_for_test(mut self, modules_max: usize) -> Self {
        self.modules_max = modules_max;
        self
    }
}

struct ResolverState {
    label: &'static str,
    deadline: Instant,
    per_source_max: usize,
    aggregate_source_max: usize,
    modules_max: usize,
    compile_source_bytes: usize,
    imported_modules: usize,
    cancellation: Option<Arc<AtomicBool>>,
    failure: Option<ResolverFailure>,
}

impl ResolverState {
    fn cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    fn refuse(
        &mut self,
        code: &'static str,
        message: String,
        category: ResolverFailureCategory,
    ) -> Option<String> {
        self.failure = Some(ResolverFailure {
            code,
            message,
            category,
        });
        None
    }
}

/// One accounting ledger, plus the canonical cache that keeps a shared module
/// from being read and charged more than once.
///
/// The ledger is `Rc`-shared and not `Sync`: the resolver callback it hands out
/// is called on the thread that owns the compilation, and the compiler is not
/// asked to move it across threads. Callers that need one ledger per entry
/// share a single ledger across entries instead, which is what makes the
/// aggregate charge and the canonical cache genuinely aggregate.
#[derive(Clone)]
pub struct ResolverLedger {
    state: Rc<RefCell<ResolverState>>,
    resolved_modules: Rc<RefCell<HashMap<PathBuf, String>>>,
    resolved_builtins: Rc<RefCell<HashSet<String>>>,
}

impl ResolverLedger {
    pub fn new(config: ResolverLedgerConfig) -> Self {
        Self {
            state: Rc::new(RefCell::new(ResolverState {
                label: config.label,
                deadline: config.deadline,
                per_source_max: config.per_source_max,
                aggregate_source_max: config.aggregate_source_max,
                modules_max: config.modules_max,
                compile_source_bytes: config.entry_bytes,
                imported_modules: config.imported_modules,
                cancellation: config.cancellation,
                failure: None,
            })),
            resolved_modules: Rc::new(RefCell::new(HashMap::new())),
            resolved_builtins: Rc::new(RefCell::new(HashSet::new())),
        }
    }

    /// Charge one entry's own bytes against the aggregate ledger.
    ///
    /// An entry is not resolved, so it is charged here rather than through
    /// [`Self::resolver`]. The refusal is returned and **not** recorded in the
    /// ledger: the caller is already holding it, and recording it would make
    /// the next entry of the same manifest inherit this entry's failure.
    ///
    /// `entry_bytes` and this method are the same accumulator, so a caller
    /// must use one or the other for a given entry, not both.
    pub fn charge_entry_bytes(&self, bytes: usize) -> Result<(), ResolverFailure> {
        let mut state = self.state.borrow_mut();
        state.compile_source_bytes = state.compile_source_bytes.saturating_add(bytes);
        if state.compile_source_bytes > state.aggregate_source_max {
            return Err(ResolverFailure {
                code: "limit_import_source_bytes",
                message: format!(
                    "entry and imported source exceeds aggregate limit of {} bytes",
                    state.aggregate_source_max
                ),
                category: ResolverFailureCategory::Limit,
            });
        }
        Ok(())
    }

    /// A resolver callback rooted at `roots`, in order of preference.
    ///
    /// Each root is canonicalized once and confined to itself; the first root
    /// holding the file answers. A specifier with no extension is read as
    /// `.qjs`. A candidate that leaves its root once canonicalized is refused,
    /// which makes `../`, an absolute path and a symlink pointing out of the
    /// root one case instead of three.
    ///
    /// The returned callback owns everything it needs, so it outlives this
    /// borrow and reflects the ledger it was cloned from -- including a
    /// refusal recorded by an earlier call in the same compilation.
    pub fn resolver(
        &self,
        roots: &[PathBuf],
        builtin: BuiltinModuleResolver,
    ) -> impl Fn(&str) -> Option<String> + use<> {
        // Roots in order of preference, each confined to itself; duplicates
        // (the usual case: the entry's directory *is* a root) collapse.
        let mut canonical = Vec::new();
        for root in roots {
            if let Ok(root) = root.canonicalize()
                && !canonical.contains(&root)
            {
                canonical.push(root);
            }
        }
        let state = Rc::clone(&self.state);
        let resolved_modules = Rc::clone(&self.resolved_modules);
        let resolved_builtins = Rc::clone(&self.resolved_builtins);
        move |specifier: &str| {
            // A refusal already recorded ends resolution rather than
            // reporting a second, unrelated reason for the same compilation.
            if state.borrow().failure.is_some() {
                return None;
            }
            // A cancellation is its own public class, not a host failure: the
            // catalog's exit classes are `configuration, limit, script, child,
            // cancelled, fleet, protocol, host`, and an operator who cancelled
            // the call should not be told the host misbehaved.
            if state.borrow().cancelled() {
                return state.borrow_mut().refuse(
                    "host_cancelled",
                    "the invocation was cancelled while resolving imports".to_owned(),
                    ResolverFailureCategory::Cancelled,
                );
            }
            if Instant::now() >= state.borrow().deadline {
                let label = state.borrow().label;
                return state.borrow_mut().refuse(
                    "limit_wall_time",
                    format!(
                        "{label} reached its aggregate wall-time budget while resolving imports"
                    ),
                    ResolverFailureCategory::Limit,
                );
            }
            if let Some(source) = builtin(specifier) {
                if !resolved_builtins.borrow().contains(specifier) {
                    let mut budget = state.borrow_mut();
                    if source.len() > budget.per_source_max
                        || budget.compile_source_bytes.saturating_add(source.len())
                            > budget.aggregate_source_max
                    {
                        return budget.refuse(
                            "limit_import_source_bytes",
                            format!("built-in module {specifier:?} exceeds the source budget"),
                            ResolverFailureCategory::Limit,
                        );
                    }
                    if budget.imported_modules >= budget.modules_max {
                        let modules_max = budget.modules_max;
                        return budget.refuse(
                            "limit_import_modules",
                            format!("recursive imports exceed {modules_max} resolved modules"),
                            ResolverFailureCategory::Limit,
                        );
                    }
                    budget.compile_source_bytes =
                        budget.compile_source_bytes.saturating_add(source.len());
                    budget.imported_modules += 1;
                    drop(budget);
                    resolved_builtins.borrow_mut().insert(specifier.to_owned());
                }
                return Some(source.to_owned());
            }
            for root in &canonical {
                let mut candidate = root.join(specifier);
                if candidate.extension().is_none() {
                    candidate.set_extension("qjs");
                }
                let Ok(resolved) = candidate.canonicalize() else {
                    continue;
                };
                if !resolved.starts_with(root) {
                    continue;
                }
                // The canonical cache answers the second specifier that lands
                // on this file. It is the only thing that keeps a shared
                // module from being read and charged twice.
                if let Some(source) = resolved_modules.borrow().get(&resolved).cloned() {
                    return Some(source);
                }
                let metadata = match std::fs::metadata(&resolved) {
                    Ok(metadata) if metadata.is_file() => metadata,
                    Ok(_) => continue,
                    Err(error) => {
                        return state.borrow_mut().refuse(
                            "host_import_read",
                            format!("cannot inspect imported module {specifier:?}: {error}"),
                            ResolverFailureCategory::Host,
                        );
                    }
                };
                let source_len = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
                let mut budget = state.borrow_mut();
                if source_len > budget.per_source_max {
                    let per_source_max = budget.per_source_max;
                    return budget.refuse(
                        "limit_import_source_bytes",
                        format!(
                            "imported module {specifier:?} exceeds per-source limit of {per_source_max} bytes"
                        ),
                        ResolverFailureCategory::Limit,
                    );
                }
                if budget.imported_modules >= budget.modules_max {
                    let modules_max = budget.modules_max;
                    return budget.refuse(
                        "limit_import_modules",
                        format!("recursive imports exceed {modules_max} resolved modules"),
                        ResolverFailureCategory::Limit,
                    );
                }
                let next_total = budget.compile_source_bytes.saturating_add(source_len);
                if next_total > budget.aggregate_source_max {
                    let aggregate_source_max = budget.aggregate_source_max;
                    return budget.refuse(
                        "limit_import_source_bytes",
                        format!(
                            "entry and imported source exceeds aggregate limit of {aggregate_source_max} bytes"
                        ),
                        ResolverFailureCategory::Limit,
                    );
                }
                drop(budget);
                // Bounded read. The metadata check above priced the file, but a
                // file can grow between that check and the read, and an
                // unbounded read allocates in proportion to whatever arrived --
                // so an over-limit file would be paid for before it was
                // refused. Reading at most one byte past the per-source
                // ceiling lets the size decide, not the allocation.
                let per_source_max = state.borrow().per_source_max;
                let file = match std::fs::File::open(&resolved) {
                    Ok(file) => file,
                    Err(error) => {
                        return state.borrow_mut().refuse(
                            "host_import_read",
                            format!("cannot read imported module {specifier:?}: {error}"),
                            ResolverFailureCategory::Host,
                        );
                    }
                };
                let mut source = String::new();
                let read_limit = u64::try_from(per_source_max)
                    .unwrap_or(u64::MAX)
                    .saturating_add(1);
                if let Err(error) = file.take(read_limit).read_to_string(&mut source) {
                    return state.borrow_mut().refuse(
                        "host_import_read",
                        format!("cannot read imported module {specifier:?}: {error}"),
                        ResolverFailureCategory::Host,
                    );
                }
                // The read is the step that costs time, so the two conditions
                // that can arrive *during* it are re-checked before the bytes
                // are accepted and charged. Deferring that to the next
                // specifier would let the last import of a call outlive both.
                if state.borrow().cancelled() {
                    return state.borrow_mut().refuse(
                        "host_cancelled",
                        "the invocation was cancelled while resolving imports".to_owned(),
                        ResolverFailureCategory::Cancelled,
                    );
                }
                if Instant::now() >= state.borrow().deadline {
                    let label = state.borrow().label;
                    return state.borrow_mut().refuse(
                        "limit_wall_time",
                        format!(
                            "{label} reached its aggregate wall-time budget while resolving imports"
                        ),
                        ResolverFailureCategory::Limit,
                    );
                }
                let mut budget = state.borrow_mut();
                let actual_len = source.len();
                if actual_len > budget.per_source_max
                    || budget.compile_source_bytes.saturating_add(actual_len)
                        > budget.aggregate_source_max
                {
                    // The file grew between the metadata check and the read, so
                    // the size that was priced is not the size that arrived.
                    return budget.refuse(
                        "limit_import_source_bytes",
                        format!(
                            "imported module {specifier:?} changed while reading and exceeds the source budget"
                        ),
                        ResolverFailureCategory::Limit,
                    );
                }
                budget.compile_source_bytes =
                    budget.compile_source_bytes.saturating_add(actual_len);
                budget.imported_modules += 1;
                drop(budget);
                resolved_modules
                    .borrow_mut()
                    .insert(resolved, source.clone());
                return Some(source);
            }
            None
        }
    }

    /// The recorded refusal, if one is pending.
    pub fn failure(&self) -> Option<ResolverFailure> {
        self.state.borrow().failure.clone()
    }

    /// Take the recorded refusal.
    ///
    /// A caller running several entries through one ledger takes it after each
    /// entry, so a refusal recorded for one entry is not read as the next
    /// entry's -- the compiler's own generic refusal is the same sentence for
    /// both, and only this record tells them apart.
    pub fn take_failure(&self) -> Option<ResolverFailure> {
        self.state.borrow_mut().failure.take()
    }

    /// Bytes charged so far, entry and imported alike.
    pub fn charged_source_bytes(&self) -> usize {
        self.state.borrow().compile_source_bytes
    }

    /// Modules charged so far, entry excluded.
    pub fn imported_modules(&self) -> usize {
        self.state.borrow().imported_modules
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::Duration;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("fixture is writable");
    }

    fn ledger(
        per_source_max: usize,
        aggregate_source_max: usize,
        deadline: Instant,
    ) -> ResolverLedger {
        ResolverLedger::new(ResolverLedgerConfig::new(
            "test door",
            deadline,
            per_source_max,
            aggregate_source_max,
        ))
    }

    #[test]
    fn a_specifier_without_an_extension_reads_the_qjs_file() {
        let dir = tempdir();
        write(dir.path(), "value.qjs", "export const value = 42;");
        let root = dir.path().canonicalize().unwrap();
        let ledger = ledger(128, 1_024, Instant::now() + Duration::from_secs(1));
        let resolve = ledger.resolver(&[root], |_| None);
        assert_eq!(
            resolve("value"),
            Some("export const value = 42;".to_owned())
        );
        assert_eq!(ledger.imported_modules(), 1);
        assert_eq!(ledger.charged_source_bytes(), 24);
        assert_eq!(ledger.failure(), None);
    }

    #[test]
    fn one_canonical_file_is_charged_once_across_two_specifiers() {
        let dir = tempdir();
        write(dir.path(), "shared.qjs", "export const shared = 1;");
        let root = dir.path().canonicalize().unwrap();
        let ledger = ledger(128, 1_024, Instant::now() + Duration::from_secs(1));
        let resolve = ledger.resolver(&[root], |_| None);
        assert_eq!(
            resolve("shared"),
            Some("export const shared = 1;".to_owned())
        );
        let charged = ledger.charged_source_bytes();
        assert_eq!(ledger.imported_modules(), 1);
        // The second specifier names the same file. Reading and charging are
        // the two things this cache owns; the compiler still sees two
        // specifiers, and the module graph it builds is therefore unchanged.
        assert_eq!(
            resolve("shared.qjs"),
            Some("export const shared = 1;".to_owned())
        );
        assert_eq!(ledger.charged_source_bytes(), charged);
        assert_eq!(ledger.imported_modules(), 1);
    }

    #[test]
    fn the_module_ceiling_refuses_the_next_distinct_module() {
        let dir = tempdir();
        write(dir.path(), "one.qjs", "export const one = 1;");
        write(dir.path(), "two.qjs", "export const two = 2;");
        let root = dir.path().canonicalize().unwrap();
        let ledger = ResolverLedger::new(
            ResolverLedgerConfig::new(
                "test door",
                Instant::now() + Duration::from_secs(1),
                128,
                1_024,
            )
            .entry_bytes(100)
            .imported_modules(IMPORT_MODULES_MAX - 1)
            .modules_max_for_test(IMPORT_MODULES_MAX),
        );
        let resolve = ledger.resolver(&[root], |_| None);
        assert_eq!(resolve("one"), Some("export const one = 1;".to_owned()));
        assert_eq!(ledger.imported_modules(), IMPORT_MODULES_MAX);
        assert_eq!(resolve("one"), Some("export const one = 1;".to_owned()));
        assert_eq!(ledger.imported_modules(), IMPORT_MODULES_MAX);
        assert_eq!(resolve("two"), None);
        let failure = ledger.failure().expect("typed limit");
        assert_eq!(failure.code, "limit_import_modules");
        assert_eq!(failure.category, ResolverFailureCategory::Limit);
        assert!(failure.message.contains("1024"), "{failure:?}");
    }

    #[test]
    fn a_lowered_ceiling_is_reachable_only_from_a_test() {
        let dir = tempdir();
        write(dir.path(), "one.qjs", "export const one = 1;");
        write(dir.path(), "two.qjs", "export const two = 2;");
        let root = dir.path().canonicalize().unwrap();
        let ledger = ResolverLedger::new(
            ResolverLedgerConfig::new(
                "test door",
                Instant::now() + Duration::from_secs(1),
                128,
                1_024,
            )
            .modules_max_for_test(1),
        );
        let resolve = ledger.resolver(&[root], |_| None);
        assert_eq!(resolve("one"), Some("export const one = 1;".to_owned()));
        assert_eq!(resolve("two"), None);
        let failure = ledger.failure().expect("typed limit");
        assert_eq!(failure.code, "limit_import_modules");
        assert!(failure.message.contains("exceed 1 resolved"), "{failure:?}");
    }

    #[test]
    fn the_per_source_ceiling_prices_the_imported_module() {
        let dir = tempdir();
        write(
            dir.path(),
            "large.qjs",
            &format!("export const v = \"{}\";", "x".repeat(64)),
        );
        let root = dir.path().canonicalize().unwrap();
        let ledger = ledger(16, 1_024, Instant::now() + Duration::from_secs(1));
        let resolve = ledger.resolver(&[root], |_| None);
        assert_eq!(resolve("large"), None);
        let failure = ledger.failure().expect("typed per-source limit");
        assert_eq!(failure.code, "limit_import_source_bytes");
        assert_eq!(failure.category, ResolverFailureCategory::Limit);
        assert!(
            failure.message.contains("per-source limit of 16"),
            "{failure:?}"
        );
    }

    /// A module that exactly fills the ceiling is accepted, not refused.
    ///
    /// The read is bounded at one byte past the ceiling so that the byte which
    /// decides is the first byte over it. A file that never reaches that byte
    /// must therefore still be read whole and charged whole.
    #[test]
    fn a_module_exactly_at_the_ceiling_is_accepted() {
        let dir = tempdir();
        let body = "export const exact = 1;";
        write(dir.path(), "exact.qjs", body);
        let root = dir.path().canonicalize().unwrap();
        let ledger = ledger(body.len(), 1_024, Instant::now() + Duration::from_secs(1));
        let resolve = ledger.resolver(&[root], |_| None);
        assert_eq!(resolve("exact"), Some(body.to_owned()));
        assert_eq!(ledger.charged_source_bytes(), body.len());
        assert_eq!(ledger.imported_modules(), 1);
        assert_eq!(ledger.failure(), None);
    }

    #[test]
    fn the_aggregate_ledger_includes_the_entry_charge() {
        let dir = tempdir();
        write(dir.path(), "value.qjs", "export const value = 42;");
        let root = dir.path().canonicalize().unwrap();
        let ledger = ledger(128, 32, Instant::now() + Duration::from_secs(1));
        // The entry alone is over the aggregate ceiling, so nothing is even
        // resolved: this is the charge `check-many` makes before compiling.
        let failure = ledger.charge_entry_bytes(40).expect_err("aggregate limit");
        assert_eq!(failure.code, "limit_import_source_bytes");
        assert_eq!(failure.category, ResolverFailureCategory::Limit);
        // A refusal returned by the charge is not recorded, so it cannot be
        // misread as the next entry's reason.
        assert_eq!(ledger.failure(), None);
        assert_eq!(ledger.charged_source_bytes(), 40);
        let resolve = ledger.resolver(&[root], |_| None);
        assert_eq!(resolve("value"), None);
        let recorded = ledger.failure().expect("aggregate limit via resolver");
        assert_eq!(recorded.code, "limit_import_source_bytes");
    }

    #[test]
    fn an_elapsed_deadline_refuses_resolution_and_names_the_door() {
        let dir = tempdir();
        write(dir.path(), "late.qjs", "export const late = 1;");
        let root = dir.path().canonicalize().unwrap();
        let ledger = ledger(128, 1_024, Instant::now());
        let resolve = ledger.resolver(&[root], |_| None);
        assert_eq!(resolve("late"), None);
        let failure = ledger.failure().expect("typed deadline");
        assert_eq!(failure.code, "limit_wall_time");
        assert_eq!(failure.category, ResolverFailureCategory::Limit);
        assert!(
            failure.message.starts_with(
                "test door reached its aggregate wall-time budget while resolving imports"
            ),
            "{failure:?}"
        );
    }

    #[test]
    fn a_set_cancellation_flag_ends_resolution() {
        let dir = tempdir();
        write(dir.path(), "value.qjs", "export const value = 42;");
        let root = dir.path().canonicalize().unwrap();
        let flag = Arc::new(AtomicBool::new(true));
        let ledger = ResolverLedger::new(
            ResolverLedgerConfig::new(
                "test door",
                Instant::now() + Duration::from_secs(1),
                128,
                1_024,
            )
            .cancellation(Some(Arc::clone(&flag))),
        );
        let resolve = ledger.resolver(&[root], |_| None);
        assert_eq!(resolve("value"), None);
        let failure = ledger.failure().expect("typed cancellation");
        assert_eq!(failure.code, "host_cancelled");
        assert_eq!(failure.category, ResolverFailureCategory::Cancelled);
    }

    #[test]
    fn a_specifier_that_leaves_the_root_is_refused_without_a_failure_record() {
        let dir = tempdir();
        std::fs::create_dir(dir.path().join("inner")).unwrap();
        write(dir.path(), "outside.qjs", "export const secret = 1;");
        let root = dir.path().join("inner").canonicalize().unwrap();
        let ledger = ledger(128, 1_024, Instant::now() + Duration::from_secs(1));
        let resolve = ledger.resolver(&[root], |_| None);
        // `../outside` resolves outside the root, so no root answers. That is
        // the compiler's own "cannot resolve" case, not a budget refusal, and
        // it must not be dressed up as one.
        assert_eq!(resolve("../outside"), None);
        assert_eq!(ledger.failure(), None);
    }

    #[test]
    fn a_builtin_resolves_before_the_filesystem_and_is_charged_once() {
        fn builtin(specifier: &str) -> Option<&'static str> {
            (specifier == "product-typed").then_some("export const value = 42;")
        }

        let dir = tempdir();
        write(
            dir.path(),
            "product-typed.qjs",
            "this shadow must not compile",
        );
        let root = dir.path().canonicalize().unwrap();
        let ledger = ledger(1_024, 1_024, Instant::now() + Duration::from_secs(1));
        let resolve = ledger.resolver(&[root], builtin);
        assert_eq!(
            resolve("product-typed"),
            Some("export const value = 42;".to_owned())
        );
        let charged = ledger.charged_source_bytes();
        assert_eq!(ledger.imported_modules(), 1);
        // The built-in set is keyed by specifier, so a second ask is free.
        assert_eq!(
            resolve("product-typed"),
            Some("export const value = 42;".to_owned())
        );
        assert_eq!(ledger.charged_source_bytes(), charged);
        assert_eq!(ledger.imported_modules(), 1);
    }

    #[test]
    fn taking_a_failure_clears_it_for_the_next_entry() {
        let dir = tempdir();
        write(dir.path(), "late.qjs", "export const late = 1;");
        let root = dir.path().canonicalize().unwrap();
        let ledger = ledger(128, 1_024, Instant::now());
        let resolve = ledger.resolver(&[root], |_| None);
        assert_eq!(resolve("late"), None);
        assert!(ledger.failure().is_some());
        let taken = ledger.take_failure().expect("typed deadline");
        assert_eq!(taken.code, "limit_wall_time");
        assert_eq!(ledger.failure(), None);
    }
}
