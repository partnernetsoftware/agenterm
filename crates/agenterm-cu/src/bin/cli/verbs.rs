//! The single static verb table behind the `agenterm-cu` CLI surface.
//!
//! Every spelling the shell accepts — canonical names, MCU aliases and the
//! two-token forms such as `menu inspect` — is declared here exactly once.
//! Dispatch, `--help`, `help <verb>` and `verbs --json` all read this table;
//! none of them keeps its own string matches.

use serde::{Deserialize, Serialize};

/// The authorization scope a verb needs before its mechanism runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    Observe,
    Actuate,
    /// Entry modes (`exec`, `grant`, `host`, `help`, `verbs`) carry no fixed
    /// scope of their own; `exec` takes the JSON command's scope.
    #[serde(rename = "none")]
    Unscoped,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Actuate => "actuate",
            Self::Unscoped => "none",
        }
    }
}

/// Verb family: one help group, one parse module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    System,
    Windows,
    Process,
    A11yObserve,
    A11yActuate,
    Browser,
    Clipboard,
    Placement,
    Transports,
    Host,
}

impl Family {
    pub const ALL: [Family; 10] = [
        Family::System,
        Family::Windows,
        Family::Process,
        Family::A11yObserve,
        Family::A11yActuate,
        Family::Browser,
        Family::Clipboard,
        Family::Placement,
        Family::Transports,
        Family::Host,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Windows => "windows",
            Self::Process => "process",
            Self::A11yObserve => "a11y-observe",
            Self::A11yActuate => "a11y-actuate",
            Self::Browser => "browser",
            Self::Clipboard => "clipboard",
            Self::Placement => "placement",
            Self::Transports => "transports",
            Self::Host => "host",
        }
    }

    /// Group header printed in `--help`.
    pub fn header(self) -> &'static str {
        match self {
            Self::System => "System & permissions",
            Self::Windows => "Windows & apps",
            Self::Process => "Processes",
            Self::A11yObserve => "Accessibility: observe",
            Self::A11yActuate => "Accessibility: actuate",
            Self::Browser => "Browser page & tabs",
            Self::Clipboard => "Clipboard",
            Self::Placement => "Window placement",
            Self::Transports => "Transports",
            Self::Host => "Grants, host & help",
        }
    }
}

/// One argument line for `help <verb>`.
pub struct ArgSpec {
    pub flag: &'static str,
    /// Value placeholder; empty for a bare switch.
    pub value: &'static str,
    pub help: &'static str,
}

/// One row of the verb table.
pub struct VerbSpec {
    /// Canonical CLI name (what `help <verb>` and the table key on).
    pub name: &'static str,
    /// `reply.command` this verb produces; differs from `name` for the
    /// placement shorthands (`frame` answers as `window-place`).
    pub command: &'static str,
    /// Other accepted spellings. A two-token alias (`menu inspect`) is the
    /// sub-command form of a group word.
    pub aliases: &'static [&'static str],
    pub scope: Scope,
    pub family: Family,
    /// One line for the grouped `--help` list.
    pub summary: &'static str,
    /// Usage line(s) without the `agenterm-cu --target …` prefix.
    pub usage: &'static str,
    pub args: &'static [ArgSpec],
    /// The long reference prose printed by `help <verb>`.
    pub details: &'static str,
}

impl VerbSpec {
    /// Every spelling that resolves to this verb, canonical name first.
    pub fn spellings(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.name).chain(self.aliases.iter().copied())
    }

    pub fn to_json(&self) -> VerbJson {
        VerbJson {
            name: self.name.to_owned(),
            command: self.command.to_owned(),
            aliases: self.aliases.iter().map(|s| (*s).to_owned()).collect(),
            grant: self.scope,
            family: self.family,
            summary: self.summary.to_owned(),
            usage: self.usage.to_owned(),
            args: self
                .args
                .iter()
                .map(|arg| ArgJson {
                    flag: arg.flag.to_owned(),
                    value: arg.value.to_owned(),
                    help: arg.help.to_owned(),
                })
                .collect(),
        }
    }
}

/// The machine-readable row `verbs --json` emits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerbJson {
    pub name: String,
    pub command: String,
    pub aliases: Vec<String>,
    pub grant: Scope,
    pub family: Family,
    pub summary: String,
    pub usage: String,
    pub args: Vec<ArgJson>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgJson {
    pub flag: String,
    pub value: String,
    pub help: String,
}

/// Exact single-token match on a name or alias; otherwise the first verb
/// whose two-token alias starts with `token` (so the bare group word `menu`
/// still lands in the right family and its parser can report the missing
/// sub-command).
pub fn lookup(token: &str) -> Option<&'static VerbSpec> {
    VERBS
        .iter()
        .find(|spec| spec.spellings().any(|spelling| spelling == token))
        .or_else(|| {
            VERBS.iter().find(|spec| {
                spec.aliases
                    .iter()
                    .any(|alias| alias.split_once(' ').is_some_and(|(head, _)| head == token))
            })
        })
}

/// Resolve `first [second]` from argv: the two-token alias wins when it
/// exists (`tab select`), otherwise the first token alone.
pub fn resolve(first: &str, second: Option<&str>) -> Option<&'static VerbSpec> {
    if let Some(second) = second {
        let joined = format!("{first} {second}");
        if let Some(spec) = VERBS
            .iter()
            .find(|spec| spec.aliases.contains(&joined.as_str()))
        {
            return Some(spec);
        }
    }
    lookup(first)
}

/// Spellings close to `token`: shared prefix, substring, or edit distance
/// two or less. Bounded and deterministic so a usage error can list them.
pub fn near_matches(token: &str) -> Vec<&'static str> {
    let token = token.to_ascii_lowercase();
    let mut scored: Vec<(u8, &'static str)> = VERBS
        .iter()
        .flat_map(VerbSpec::spellings)
        .filter_map(|spelling| {
            let score = if spelling.starts_with(&token) || token.starts_with(spelling) {
                0
            } else if spelling.contains(&token) || token.contains(spelling) {
                1
            } else if levenshtein(spelling, &token) <= 2 {
                2
            } else {
                return None;
            };
            Some((score, spelling))
        })
        .collect();
    scored.sort();
    scored.dedup();
    scored.into_iter().take(6).map(|(_, s)| s).collect()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(ca != cb);
            current.push(substitute.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

pub fn by_family(family: Family) -> impl Iterator<Item = &'static VerbSpec> {
    VERBS.iter().filter(move |spec| spec.family == family)
}

pub fn table_json() -> Vec<VerbJson> {
    VERBS.iter().map(VerbSpec::to_json).collect()
}

const WINDOW: ArgSpec = ArgSpec {
    flag: "--window",
    value: "HANDLE",
    help: "window handle from `windows` (numeric or App#N)",
};
const NAME_PAT: ArgSpec = ArgSpec {
    flag: "--name",
    value: "PAT",
    help: "unique showing node whose name contains PAT (wait matching)",
};
const ROLE: ArgSpec = ArgSpec {
    flag: "--role",
    value: "ROLE",
    help: "narrow --name to one role",
};
const DEPTH: ArgSpec = ArgSpec {
    flag: "--depth",
    value: "N",
    help: "walk depth (root = 0, at most 64)",
};
const MAX_NODES: ArgSpec = ArgSpec {
    flag: "--max-nodes",
    value: "N",
    help: "node budget while the platform walks (1..20000)",
};
const OFFSET: ArgSpec = ArgSpec {
    flag: "--offset",
    value: "N",
    help: "page start",
};
const MAX: ArgSpec = ArgSpec {
    flag: "--max",
    value: "N",
    help: "page size",
};
const PORT: ArgSpec = ArgSpec {
    flag: "--port",
    value: "N",
    help: "CDP listener port on 127.0.0.1 (default 9222)",
};
const TARGET_ID: ArgSpec = ArgSpec {
    flag: "--target-id",
    value: "ID",
    help: "exact CDP target id (from page-targets)",
};
const TARGET_URL: ArgSpec = ArgSpec {
    flag: "--target-url",
    value: "SUB",
    help: "case-insensitive url substring of the page target",
};
const TARGET_TITLE: ArgSpec = ArgSpec {
    flag: "--target-title",
    value: "SUB",
    help: "case-insensitive title substring of the page target",
};
const CDP_SELECTOR: ArgSpec = ArgSpec {
    flag: "--selector",
    value: "CSS",
    help: "CSS selector (DOM.querySelectorAll)",
};
const CDP_TEXT: ArgSpec = ArgSpec {
    flag: "--text",
    value: "SUB",
    help: "case-insensitive substring of the node's words (AX tree; lifted to the enclosing button / link)",
};
const CDP_NODE: ArgSpec = ArgSpec {
    flag: "--node",
    value: "ID",
    help: "backend DOM node id from page-find / page-text rows",
};
const SNAPSHOT: ArgSpec = ArgSpec {
    flag: "--snapshot",
    value: "",
    help: "write the bounded tree to the receipt before acting",
};
const EXPECT_GONE: ArgSpec = ArgSpec {
    flag: "--expect",
    value: "gone",
    help: "checkable postcondition: the handle reads back as absent",
};

const BROWSER_PROFILE: ArgSpec = ArgSpec {
    flag: "--browser-profile",
    value: "SUB",
    help: "case-insensitive substring of the window's browser_profile (Chromium profile name)",
};

const ALLOW_BROWSER_CHROME: ArgSpec = ArgSpec {
    flag: "--allow-browser-chrome",
    value: "",
    help: "with --window and no --name: write browser chrome (omnibox, toolbar, tab strip) instead of refusing",
};

pub const VERBS: &[VerbSpec] = &[
    // ---------------------------------------------------------------- windows
    VerbSpec {
        name: "capabilities",
        command: "capabilities",
        aliases: &["caps"],
        scope: Scope::Observe,
        family: Family::System,
        summary: "per-target capability matrix for this host",
        usage: "capabilities",
        args: &[],
        details: r#"What the selected target can observe and actuate, verb group by verb
group, with a typed reason for anything this host does not support.
`caps` is the MCU spelling."#,
    },
    VerbSpec {
        name: "permissions",
        command: "permissions",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::System,
        summary: "permission state, affected verbs and repair guidance",
        usage: "permissions",
        args: &[],
        details: r#"Reports the current host's permission model without opening settings or
claiming a grant that the platform cannot inspect. Each permission names the verbs it gates and
the exact repair surface when one exists. This is a read-only status facade; system consent stays
with the user and `permissions` never attempts to bypass it."#,
    },
    VerbSpec {
        name: "doctor",
        command: "doctor",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::System,
        summary: "bounded host, permission and desktop-bridge health report",
        usage: "doctor",
        args: &[],
        details: r#"Reuses the canonical capabilities and permissions declarations, then
runs bounded window and display inventory probes. A failed probe remains a typed check in the
successful diagnostic document; `doctor` never mutates setup, consent, helpers or foreground state."#,
    },
    VerbSpec {
        name: "windows",
        command: "windows",
        aliases: &["focused-window"],
        scope: Scope::Observe,
        family: Family::Windows,
        summary: "top-level window inventory, filtered and paged",
        usage: "windows [--pid N] [--app SUB] [--title SUB] [--focused [BOOL]] [--minimized [BOOL]]
        [--browser-profile SUB] [--offset N] [--max N]
focused-window                                      (= windows --focused true)",
        args: &[
            ArgSpec {
                flag: "--pid",
                value: "N",
                help: "owning process id",
            },
            ArgSpec {
                flag: "--app",
                value: "SUB",
                help: "application name substring",
            },
            ArgSpec {
                flag: "--title",
                value: "SUB",
                help: "window title substring",
            },
            ArgSpec {
                flag: "--focused",
                value: "[BOOL]",
                help: "only the focused window (true, default; reply adds focused_app + window) or unfocused windows",
            },
            ArgSpec {
                flag: "--minimized",
                value: "[BOOL]",
                help: "only minimized (true, default) or shown windows",
            },
            BROWSER_PROFILE,
            OFFSET,
            MAX,
        ],
        details: r#"Bare: the window array. With any filter or page flag the reply is
{windows, visited, matched, returned, offset, truncated}. Browser rows
carry browser_profile (the Chromium profile name from the window's
" - <App> - <profile>" identity suffix, read from the AX root when the
inventory title lacks it; the <App> segment is matched loosely against
app_name, because only macOS reports a display name there -- Linux
reports /proc/<pid>/comm and Windows the image name);
--browser-profile SUB keeps only those rows
(case-insensitive substring), so an agent can address one profile's
window of a multi-profile instance. Focus: the mechanism's own mark is
kept when it made one; otherwise the frontmost application (NSWorkspace
on macOS) decides -- its own AXFocusedWindow, else its topmost window in
the stacking order -- because the system-wide accessibility focus read
fails from a process outside the GUI front chain (tmux, SSH, an agent
bridge). Every object reply carries focus: {handle, via, reason}; with
--focused true (alias focused-window) it also carries focused_app
{name, pid, bundle_id} and window (the row, or null with reason
frontmost_app_has_no_inventory_window / no_frontmost_app), so a
"front window unchanged" check never reads an empty list as "none"."#,
    },
    VerbSpec {
        name: "windows-watch",
        command: "windows-watch",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::Windows,
        summary: "poll-diff over the window inventory",
        usage: "windows-watch [--pid N] [--app SUB] [--title SUB] [--duration-ms N] [--interval-ms N]
              [--max-events N]",
        args: &[
            ArgSpec {
                flag: "--pid",
                value: "N",
                help: "owning process id",
            },
            ArgSpec {
                flag: "--app",
                value: "SUB",
                help: "application name substring",
            },
            ArgSpec {
                flag: "--title",
                value: "SUB",
                help: "window title substring",
            },
            ArgSpec {
                flag: "--duration-ms",
                value: "N",
                help: "watch window; 0 (default) takes one extra sample",
            },
            ArgSpec {
                flag: "--interval-ms",
                value: "N",
                help: "poll interval",
            },
            ArgSpec {
                flag: "--max-events",
                value: "N",
                help: "stop after N events",
            },
        ],
        details: r#"poll-diff over the windows inventory (appeared / disappeared / changed +
field list). Not AXObserver. --duration-ms 0 (default) takes one extra
sample."#,
    },
    VerbSpec {
        name: "apps",
        command: "apps",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::Windows,
        summary: "running apps from top-level windows; --all adds installed",
        usage: "apps [--running] [--all]",
        args: &[
            ArgSpec {
                flag: "--running",
                value: "",
                help: "running applications only (the default set)",
            },
            ArgSpec {
                flag: "--all",
                value: "",
                help: "also the installed applications no window can reveal",
            },
        ],
        details: r#"Running apps from top-level windows (pids + window count). --all also
lists the applications installed on this host that no window can reveal,
each marked running or not."#,
    },
    VerbSpec {
        name: "ps",
        command: "ps",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::Process,
        summary: "bounded process inventory from the shared platform facade",
        usage: "ps [--pid N] [--parent N] [--name SUB] [--offset N] [--max N]",
        args: &[
            ArgSpec { flag: "--pid", value: "N", help: "exact process id" },
            ArgSpec { flag: "--parent", value: "N", help: "exact parent process id" },
            ArgSpec { flag: "--name", value: "SUB", help: "case-insensitive executable-name substring" },
            OFFSET,
            MAX,
        ],
        details: r#"Lists process id, parent id and executable name through the same
agenterm-platform process facade used by qjswasm process.list. Results are
sorted by pid and bounded before publication. MCU's --command, CPU, memory,
file and port filters remain typed migration gaps until their owning facades
land; this command never silently ignores them."#,
    },
    VerbSpec {
        name: "process-state",
        command: "process-state",
        aliases: &["process state"],
        scope: Scope::Observe,
        family: Family::Process,
        summary: "observe one pid with a stable process-start identity",
        usage: "process-state --pid N\nprocess state --pid N",
        args: &[ArgSpec {
            flag: "--pid",
            value: "N",
            help: "positive process id to observe",
        }],
        details: r#"Returns live, dead, or unknown through the shared process-observation
facade. A live result includes the platform start identity when available.
Unknown is fail-closed and never means dead. Future mutation commands must bind
their target to this identity rather than trusting a reusable pid alone."#,
    },
    VerbSpec {
        name: "process-usage",
        command: "process-usage",
        aliases: &["process usage"],
        scope: Scope::Observe,
        family: Family::Process,
        summary: "sample or watch identity-bound CPU, memory and page faults",
        usage: "process-usage --pid N [--watch-ms N --interval-ms N --max-samples N]\nprocess usage --pid N [--watch-ms N --interval-ms N --max-samples N]",
        args: &[
            ArgSpec { flag: "--pid", value: "N", help: "positive process id to sample" },
            ArgSpec { flag: "--watch-ms", value: "N", help: "bounded observation duration, 1..=86400000" },
            ArgSpec { flag: "--interval-ms", value: "N", help: "sample interval, 1..=60000 (default 1000)" },
            ArgSpec { flag: "--max-samples", value: "N", help: "maximum returned samples, 1..=4096 (default 120)" },
        ],
        details: r#"Reads one cumulative resource sample between two matching process-start
identity observations. CPU nanoseconds, resident bytes and page-fault counters
are decimal strings so JSON/JavaScript consumers cannot lose u64 precision.
Watch mode takes an immediate sample, then samples on a monotonic bounded
schedule while the same start identity remains live. It reports every sample
and whether the sample ceiling truncated the requested duration. Richer I/O
counters remain an explicit migration gap."#,
    },
    VerbSpec {
        name: "process-wait",
        command: "process-wait",
        aliases: &["process wait"],
        scope: Scope::Observe,
        family: Family::Process,
        summary: "wait for one identity-bound process instance to exit",
        usage: "process-wait --pid N --start-identity ID [--timeout-ms N]\nprocess wait --pid N --start-identity ID [--timeout-ms N]",
        args: &[
            ArgSpec { flag: "--pid", value: "N", help: "positive process id previously observed" },
            ArgSpec { flag: "--start-identity", value: "ID", help: "exact value returned by process-state" },
            ArgSpec { flag: "--timeout-ms", value: "N", help: "monotonic wait limit, 1..=86400000 (default 30000)" },
        ],
        details: r#"Opens a native stable process reference, verifies that its current start
identity equals the caller's prior process-state observation, then waits for
that exact process object. A timeout is a verified live result, not an error;
an identity mismatch fails before waiting. This avoids MCU's repeated PID
        polling and cannot silently follow a recycled pid."#,
    },
    VerbSpec {
        name: "process-watch",
        command: "process-watch",
        aliases: &["process watch"],
        scope: Scope::Observe,
        family: Family::Process,
        summary: "watch a bounded, identity-safe process-set diff",
        usage: "process-watch [--pid N] [--parent N] [--name SUB] [--all] [--duration-ms N --interval-ms N --max-events N --max-processes N]\nprocess watch [selectors] [limits]",
        args: &[
            ArgSpec { flag: "--pid", value: "N", help: "watch one process id" },
            ArgSpec { flag: "--parent", value: "N", help: "watch direct children of one parent" },
            ArgSpec { flag: "--name", value: "SUB", help: "watch executable-name substring" },
            ArgSpec { flag: "--all", value: "", help: "watch the whole bounded inventory" },
            ArgSpec { flag: "--duration-ms", value: "N", help: "monotonic duration, 1..=86400000 (default 30000)" },
            ArgSpec { flag: "--interval-ms", value: "N", help: "poll interval, 1..=60000 (default 1000)" },
            ArgSpec { flag: "--max-events", value: "N", help: "event ceiling, 1..=4096 (default 256)" },
            ArgSpec { flag: "--max-processes", value: "N", help: "matched inventory ceiling, 1..=5000 (default 1000)" },
        ],
        details: r#"Takes an immediate baseline, then emits started/exited diffs until the
monotonic duration or event ceiling is reached. Every row carries a native
start identity; pid reuse therefore appears as an exit plus a start. An
unverifiable identity on an exact PID fails closed. A broad selector excludes
unidentified processes and reports `coverage_complete: false` plus the count;
it never degrades those rows to pid-only polling. Oversized inventory fails
closed."#,
    },
    VerbSpec {
        name: "app",
        command: "app",
        aliases: &["launch", "quit", "hide", "show"],
        scope: Scope::Actuate,
        family: Family::Windows,
        summary: "hide / show / quit / launch a whole application",
        usage: "app <hide|show|quit|launch> [--window HANDLE] [--pid N] [--path P] [--snapshot --expect gone]
hide|show|quit --window HANDLE | --pid N        (alias of app <action>)
launch PATH | launch --path PATH                (alias of app launch)",
        args: &[
            ArgSpec {
                flag: "<action>",
                value: "",
                help: "hide | show | quit | launch (or --action <one of them>)",
            },
            WINDOW,
            ArgSpec {
                flag: "--pid",
                value: "N",
                help: "process id; show needs it because hiding removed the windows",
            },
            ArgSpec {
                flag: "--path",
                value: "P",
                help: "launch: installed application path",
            },
            SNAPSHOT,
            EXPECT_GONE,
        ],
        details: r#"Steps a whole application, not one of its windows. hide/show take it
aside and back (show takes --pid: hiding removed the windows the handle
named); quit is destructive and carries the same three-part gate as close
-- it presses the application's own Quit item and reads the process back.
launch --path starts an installed application; the reply says pid: null
because the launcher owns the process, so wait for its window if a pid is
needed. `launch PATH`, `quit`, `hide` and `show` are the MCU spellings of
`app <action>`."#,
    },
    VerbSpec {
        name: "unlock",
        command: "unlock",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::Windows,
        summary: "ask the app to build its full a11y tree",
        usage: "unlock --window HANDLE",
        args: &[WINDOW],
        details: r#"Asks the owning application to build its full accessibility tree,
reading the bounded tree before and after. The poke is per host and the
reply's poke field names the one that ran: macOS sets AXManualAccessibility
(plus AXEnhancedUserInterface on the application); Linux flips the
desktop-wide org.a11y.Status switch (IsEnabled + ScreenReaderEnabled on the
session-bus name org.a11y.Bus) that a Chromium renderer watches before it
publishes a web tree; Windows has no separate poke to make, because a
Chromium process turns accessibility on when it answers WM_GETOBJECT for
its window and the UIA tree walk sends that itself -- there poked is false
WITH a reason, which is not a failure.

Reports poked (the request was delivered), grew and returned_before
separately, because the poke's own status is not the outcome: AppKit calls
the attribute unsupported even when it lands, and the Linux flags may
already be set, so only the re-read can claim anything about the tree.

Evidence: macOS is proven on a real Brave instance
(scripts/cu-brave-live-smoke.sh). The Linux and Windows paths are
code-complete but have no live run yet; scripts/cu-linux-smoke.sh and
scripts/qjs/cu-windows-browser-smoke.qjs carry the journey and exit with a
typed SKIP until a host with a Chromium-family browser runs them."#,
    },
    VerbSpec {
        name: "close",
        command: "close",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Windows,
        summary: "close one window through its own close control (gated)",
        usage: "close --window HANDLE [--pid N] [--title T] --snapshot --expect gone",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--pid",
                value: "N",
                help: "bind the handle to this process in the same inventory read",
            },
            ArgSpec {
                flag: "--title",
                value: "T",
                help: "bind the handle to this exact title in the same inventory read",
            },
            SNAPSHOT,
            EXPECT_GONE,
        ],
        details: r#"The destructive verb: closes one top-level window in the background
through the platform's own close control (macOS AXCloseButton + AXPress).
The gate is three parts, all checked before anything is touched: an exact
target (--window, bound to --pid / exact --title in the same inventory
read), a prior snapshot (--snapshot: the bounded tree written to the
receipt) and a checkable postcondition (--expect gone: the handle read back
as absent). Missing any -> "refused" (detail.reason destructive_gate,
missing [...]) with nothing performed."#,
    },
    VerbSpec {
        name: "activate",
        command: "activate",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Windows,
        summary: "make one exact window the desktop foreground owner",
        usage: "activate --window HANDLE",
        args: &[WINDOW],
        details: r#"Makes one exact top-level window the desktop foreground owner and
then polls the public window inventory until that exact handle reads
focused. This is the whole-window operation MCU spells `focus HANDLE`.

It is deliberately not `focus`, which targets one accessibility node
inside a window, and not `raise`, which changes only an application's own
window order without activating the application. A mechanism refusal or
missing focused read-back fails typed; sending an activation request is
not by itself success."#,
    },
    VerbSpec {
        name: "raise",
        command: "raise",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Windows,
        summary: "lift one window inside its own app's z-order",
        usage: "raise --window HANDLE",
        args: &[WINDOW],
        details: r#"Lifts one window in front of the other windows OF ITS OWN APPLICATION
(macOS: AXRaise on the window element), then reads that order back. It
does not activate the application and does not change the system
frontmost application: the frontmost pid is read before and after and the
reply carries both plus frontmost_app_unchanged. If it moved anyway, the
verb fails typed "foreground_changed" rather than reporting success.

raise is NOT focus. `focus` gives one accessibility NODE inside a window
the keyboard focus and never touches stacking; `raise` moves a whole
WINDOW in front of its siblings and never moves the accessibility focus.
Neither is `orderwin`, which orders one window against another named
window in the DESKTOP-wide order.

Verification is the application-local stacking read-back: rank 0 of its
app's windows. A host that reports no stacking order answers verified
false with reason stacking_unreadable rather than letting the absence of
a contradiction read as success; a window the order refuses to move
fails typed "window_order_not_applied"."#,
    },
    VerbSpec {
        name: "minimize",
        command: "minimize",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Windows,
        summary: "minimize one window through its own affordance (gated)",
        usage: "minimize --window HANDLE --expect minimized",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--expect",
                value: "minimized",
                help: "checkable postcondition: the window reads back minimized",
            },
        ],
        details: r#"Sends one window to the dock through the window's own minimize
affordance (macOS: the window attribute AXMinimized set to true). Never a
keyboard shortcut, never by activating the application; the frontmost pid
is read before and after and reported.

Gated like `close`, minus the snapshot: an exact target (--window HANDLE)
and a checkable postcondition (--expect minimized). Missing either ->
"refused" (detail.reason destructive_gate, missing [...]) with nothing
performed. A window that is ALREADY minimized is a verified no-op
(performed false, verified true, reason already_minimized) -- the same
contract `invoke set-checked` has for a state that already holds.

Read-back is the window's own minimized state, polled until it settles.
Note for macOS: the `windows` inventory is on-screen only, so a minimized
window LEAVES it rather than appearing with minimized true; the reply
reports both after.minimized (the window's own state) and
after.inventory_present so the two are never confused."#,
    },
    VerbSpec {
        name: "restore",
        command: "restore",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Windows,
        summary: "un-minimize one window without activating its app (gated)",
        usage: "restore --window HANDLE --expect restored",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--expect",
                value: "restored",
                help: "checkable postcondition: the window reads back not minimized",
            },
        ],
        details: r#"Brings one minimized window back (macOS: AXMinimized set to false)
without activating its application. Same gate as `minimize` with
--expect restored, the same already-restored verified no-op, and the same
minimized read-back; the window handle is stable across minimize and
restore, so the handle `windows` gave before the minimize is the one to
pass here."#,
    },
    VerbSpec {
        name: "receipts",
        command: "receipts",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::Windows,
        summary: "read the target's crash-persistent receipt file",
        usage: "receipts [--window HANDLE] [--max N]",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--max",
                value: "N",
                help: "lines to return (default 50, at most 1000)",
            },
        ],
        details: r#"The target's crash-persistent receipt file (<audit dir>/cu-receipts/
<target>.jsonl) read back in order: every invoke / menu invoke / click /
focus / close appends a "reserved" line before the mechanism and a
"completed" / "failed" line after the read-back; a "reserved" line with no
partner is the crash signature (uncertain, never "did not happen").
Default 50, at most 1000."#,
    },
    VerbSpec {
        name: "spaces",
        command: "spaces",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::Windows,
        summary: "macOS managed Space inventory (read-only)",
        usage: "spaces",
        args: &[],
        details: r#"macOS SkyLight managed Space inventory (read-only). linux/windows answer
typed unsupported."#,
    },
    VerbSpec {
        name: "displays",
        command: "displays",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::Windows,
        summary: "native screen frames",
        usage: "displays",
        args: &[],
        details: r#"Native screen frames via agt_screen_list (MCU system group)."#,
    },
    // ----------------------------------------------------------- a11y observe
    VerbSpec {
        name: "tree",
        command: "tree",
        aliases: &["elements"],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "bounded a11y tree of a window; --flat numbers nodes",
        usage: "tree [--window HANDLE] [--depth N] [--max-nodes N] [--flat]
elements [--window HANDLE] [--depth N] [--max-nodes N]     (alias of tree --flat)",
        args: &[
            WINDOW,
            DEPTH,
            MAX_NODES,
            ArgSpec {
                flag: "--flat",
                value: "",
                help: "number nodes (index, depth) in walk order",
            },
        ],
        details: r#"Depth (root=0, <=64) and node budget (1..20000) apply while the platform
walks; the reply carries truncated / visited / returned. --flat numbers
nodes (index, depth) in walk order -- the same identity query reports.
`elements` is the MCU spelling of tree --flat."#,
    },
    VerbSpec {
        name: "query",
        command: "query",
        aliases: &["inspect", "find", "read"],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "bounded, filtered flat node list",
        usage: "query --window HANDLE|App#N | HANDLE [--depth N] [--max-nodes N] [--role R,R]
      [--text T | --text-exact T] [--identifier ID] [--actionable] [--within X,Y,W,H]
      [--offset N] [--max N] [--selector PATH]
inspect HANDLE [flags]           (alias of query; --app is a migration gap)
find HANDLE TEXT [flags]         (alias of query --text TEXT)
read HANDLE SELECTOR [flags]     (alias of query --selector SELECTOR)",
        args: &[
            WINDOW,
            DEPTH,
            MAX_NODES,
            ArgSpec {
                flag: "--role",
                value: "R,R",
                help: "roles to keep (AXTextArea or text-area spellings)",
            },
            ArgSpec {
                flag: "--text",
                value: "T",
                help: "name / value substring",
            },
            ArgSpec {
                flag: "--text-exact",
                value: "T",
                help: "exact name / value (not with --text)",
            },
            ArgSpec {
                flag: "--identifier",
                value: "ID",
                help: "accessibility identifier",
            },
            ArgSpec {
                flag: "--actionable",
                value: "",
                help: "nodes that offer an action",
            },
            ArgSpec {
                flag: "--within",
                value: "X,Y,W,H",
                help: "screen rectangle the node must intersect",
            },
            OFFSET,
            MAX,
            ArgSpec {
                flag: "--selector",
                value: "PATH",
                help: "MCU Role[idx] / Role@title / *@title / #desc path",
            },
        ],
        details: r#"Bounded, filtered flat node list with visited / matched / returned /
truncated; roles accept AXTextArea or text-area; an unknown flag fails
before the walk. inspect is query (MCU `inspect HANDLE`; `--app` stays
MCU). find HANDLE TEXT is query --text; read HANDLE SELECTOR is query
--selector. MCU selectors: Role[idx] / Role@title / *@title / #desc."#,
    },
    VerbSpec {
        name: "hit",
        command: "hit",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "the a11y node under a screen point",
        usage: "hit --window HANDLE --x X --y Y [--depth N] [--max-nodes N]",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--x",
                value: "X",
                help: "screen x (the space node bounds and query --within use)",
            },
            ArgSpec {
                flag: "--y",
                value: "Y",
                help: "screen y",
            },
            DEPTH,
            MAX_NODES,
        ],
        details: r#"Screen coordinates -> the node under them, in the same shape `query`
returns (index, depth, id, role, name, bounds, ...), so the id goes
straight into invoke --node / click --node. `containing` lists every node
whose rectangle holds the point, so an ambiguous spot is visible rather
than hidden behind the winner.

Ranking: deepest wins, then smallest area, then the later position in
walk order (a sibling drawn afterwards sits on top). A zero-area
rectangle is never a hit. Nothing here reads or moves the pointer.

The point is resolved inside the window's own bounded walk, not through
the platform's point-to-element call (macOS
AXUIElementCopyElementAtPosition): that call returns a live element with
no address in the id space tree / query publish, so its answer could not
be handed to invoke --node -- which is the whole reason to ask. A miss is
typed "a11y_node_not_found" with the walk's visited / truncated counts."#,
    },
    VerbSpec {
        name: "focused",
        command: "focused",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "the app's own focused control inside the window",
        usage: "focused --window HANDLE [--role ROLE] [--max-value-bytes N]",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--role",
                value: "ROLE",
                help: "expected role (\"unverified\" on mismatch)",
            },
            ArgSpec {
                flag: "--max-value-bytes",
                value: "N",
                help: "value preview size (default 4096; 0 keeps only value_bytes)",
            },
        ],
        details: r#"The application's own focused control inside the window (id / role /
name / identifier / states / value preview), read without the foreground;
--role binds the expected role ("unverified" on mismatch); default preview
4096 bytes, 0 keeps only value_bytes."#,
    },
    VerbSpec {
        name: "observe",
        command: "observe",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "bounded a11y event stream by poll-diff",
        usage: "observe --window HANDLE (--duration S | --duration-ms N) [--depth N] [--max-nodes N]
        [--max-events N] [--notification A,B] [--interval-ms N] [--mode poll-diff|notifications]
        [--ready-path PATH]",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--duration",
                value: "S",
                help: "seconds, fractions allowed, within (0, 120]",
            },
            ArgSpec {
                flag: "--duration-ms",
                value: "N",
                help: "exact milliseconds (not with --duration)",
            },
            DEPTH,
            MAX_NODES,
            ArgSpec {
                flag: "--max-events",
                value: "N",
                help: "stop after N events (<= 5000, default 200)",
            },
            ArgSpec {
                flag: "--notification",
                value: "A,B",
                help: "event kinds to keep",
            },
            ArgSpec {
                flag: "--interval-ms",
                value: "N",
                help: "poll interval",
            },
            ArgSpec {
                flag: "--mode",
                value: "poll-diff|notifications",
                help: "event source",
            },
            ArgSpec {
                flag: "--ready-path",
                value: "PATH",
                help: "atomic marker after the complete poll-diff baseline",
            },
        ],
        details: r#"Bounded event stream by poll-diff over the same bounded tree:
ValueChanged / TitleChanged / StateChanged / FocusChanged / Created /
Destroyed with monotonic seq and t_ms; stops at --max-events (<= 5000,
default 200) with truncated true; reports polls / emitted / filtered /
stopped. --ready-path publishes a caller-owned no-overwrite JSON marker
after the complete poll-diff baseline and before its duration starts; native
notifications reject it until subscription readiness is available."#,
    },
    VerbSpec {
        name: "snapshot",
        command: "snapshot",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "name a bounded tree as a baseline for diff",
        usage: "snapshot --window HANDLE [--depth N] [--max-nodes N] [--out PATH]",
        args: &[
            WINDOW,
            DEPTH,
            MAX_NODES,
            ArgSpec {
                flag: "--out",
                value: "PATH",
                help: "also write the baseline itself to this path",
            },
        ],
        details: r#"Captures the bounded tree once and stores it as a named baseline, so
`diff` can answer "what changed since" without the caller holding the
tree. The reply carries snapshot_id, the node count and truncated.

Baselines live beside the receipts, under the same audit directory:
<audit dir>/cu-snapshots/<target>/w<window>/<snapshot_id>.json, so
AGENTERM_CU_AUDIT_PATH relocates audit, receipts and baselines together.
Each window keeps its 32 newest baselines and older ones are dropped, so
a poll loop cannot grow the store without end."#,
    },
    VerbSpec {
        name: "diff",
        command: "diff",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "what changed since a snapshot baseline",
        usage: "diff --window HANDLE [--base SNAPSHOT_ID] [--advance] [--max N]",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--base",
                value: "SNAPSHOT_ID",
                help: "baseline to compare against (default: the window's most recent)",
            },
            ArgSpec {
                flag: "--advance",
                value: "",
                help: "store the walk just read as the next baseline",
            },
            ArgSpec {
                flag: "--max",
                value: "N",
                help: "changes per bucket (default 200, at most 2000)",
            },
        ],
        details: r#"Walks the window again and returns added / removed / changed nodes in
the same shape `query` returns; a changed row carries `changed` with the
names of the fields that differ (role, name, parent_id, states, bounds,
actions, text, identifier). The current walk reuses the BASELINE's budget
so a difference is a difference in the window, never a difference in how
far each side looked.

Without --base the window's most recent baseline is used and
base_selected_by says "most-recent"; base carries the id that was used.
--advance stores the walk it just compared as the next baseline in the
same call, so an agent polls a window with one verb per tick and misses
nothing between replies; next_base names the id to expect.

Node ids are positional paths (/0/3/1) -- which is what makes them
usable with invoke --node -- so inserting a sibling renumbers the ones
after it and one real insertion can read as one added plus several
changed. The field names say which: a renumbered node changes name /
role, a moved one changes only bounds.

Each bucket is capped at --max with truncated set; walk_truncated says
the tree walk itself hit its budget on either side."#,
    },
    VerbSpec {
        name: "verify",
        command: "verify",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "one tree read checked against --expect items",
        usage: "verify --window HANDLE --expect '[{\"node\"|\"index\"|\"name\"|\"titleIncludes\"[+\"role\"]|\"identifier\"|\"role\",
                                   \"value\"?, \"checked\"?, \"expanded\"?, \"focused\"?}, ...]'",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--expect",
                value: "JSON",
                help: "array of closed-shape items; an unknown key is a usage error",
            },
        ],
        details: r#"One tree read; all met -> ok + verified, a mismatch -> "unverified", a
state the node does not expose -> "unsupported" (fail closed), an unknown
key -> usage. name/titleIncludes alone is page identity (WebArea title)."#,
    },
    VerbSpec {
        name: "menu-inspect",
        command: "menu-inspect",
        aliases: &["menu inspect"],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "read the menu bar in the background",
        usage: "menu inspect --window HANDLE [--depth N] [--max-nodes N] [--title T [--exact]]
             [--enabled true|false] [--offset N] [--max N]",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--depth",
                value: "N",
                help: "0 = bar items, default 1, at most 8",
            },
            ArgSpec {
                flag: "--max-nodes",
                value: "N",
                help: "node budget 1..5000",
            },
            ArgSpec {
                flag: "--title",
                value: "T",
                help: "title filter (substring; --exact for equality)",
            },
            ArgSpec {
                flag: "--exact",
                value: "",
                help: "match --title exactly",
            },
            ArgSpec {
                flag: "--enabled",
                value: "true|false",
                help: "keep only enabled / disabled items",
            },
            OFFSET,
            MAX,
        ],
        details: r#"The application's menu bar read in the background (never opens a menu,
never activates): items with exact title paths, depth (0 = bar items,
default 1, <= 8), enabled / checked / has_submenu; node budget 1..5000;
counts visited / matched / returned / truncated."#,
    },
    VerbSpec {
        name: "get-text",
        command: "get-text",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "independent AT-SPI Text.GetText on a named / focused node",
        usage: "get-text --window HANDLE [--name PAT] [--role ROLE]",
        args: &[WINDOW, NAME_PAT, ROLE],
        details: r#"One-shot independent AT-SPI Text.GetText on the unique showing named
node, or with no --name on the node carrying the AT-SPI focused state --
the same text authority wait --text-equals polls, without a timeout. Not
send-text / paste / copy matched.text, last_text_write_via, the WebKit eval
helper queued-job OK, or a tree snapshot text. Missing Text typed-fails
(a11y_text_unavailable). Never XTest / --coords / screenshot."#,
    },
    VerbSpec {
        name: "get-extents",
        command: "get-extents",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "independent AT-SPI Component.GetExtents(Screen)",
        usage: "get-extents --window HANDLE --name PAT [--role ROLE]",
        args: &[WINDOW, NAME_PAT, ROLE],
        details: r#"Independent AT-SPI Component.GetExtents(Screen). Snapshot node.bounds do
not count. Empty extents typed-fail (a11y_extents_unavailable)."#,
    },
    VerbSpec {
        name: "get-selection",
        command: "get-selection",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "independent AT-SPI Text selection read-back",
        usage: "get-selection --window HANDLE --name PAT [--role ROLE]",
        args: &[WINDOW, NAME_PAT, ROLE],
        details: r#"Independent AT-SPI Text.GetNSelections + GetSelection(0). Not the select
reply payload. Missing Text typed-fails (a11y_selection_unavailable). n=0
is empty success."#,
    },
    VerbSpec {
        name: "get-caret",
        command: "get-caret",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "independent AT-SPI caret offset read-back",
        usage: "get-caret --window HANDLE --name PAT [--role ROLE]",
        args: &[WINDOW, NAME_PAT, ROLE],
        details: r#"Independent AT-SPI Text.CaretOffset / GetCaretOffset. Not the set-caret
reply payload. Missing Text typed-fails (a11y_caret_unavailable)."#,
    },
    VerbSpec {
        name: "wait",
        command: "wait",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "poll until a window / node / text condition holds",
        usage: "wait --timeout-ms MS (--window-count-gte N | --window-title-contains PAT | --focused-handle HANDLE
                      | --window HANDLE --expect JSON
                      | --node-name-contains PAT [--node-role ROLE] [--window HANDLE]
                      | --text-equals TEXT --name PAT [--role ROLE] --window HANDLE
                      | --text-contains SUB --name PAT [--role ROLE] --window HANDLE) [-- TEXT]",
        args: &[
            ArgSpec {
                flag: "--timeout-ms",
                value: "MS",
                help: "deadline (default 5000); timeout is typed with the last observation",
            },
            ArgSpec {
                flag: "--window-count-gte",
                value: "N",
                help: "at least N top-level windows",
            },
            ArgSpec {
                flag: "--window-title-contains",
                value: "PAT",
                help: "some window title contains PAT",
            },
            ArgSpec {
                flag: "--focused-handle",
                value: "HANDLE",
                help: "that window holds focus",
            },
            ArgSpec {
                flag: "--expect",
                value: "JSON",
                help: "same matcher as verify; polls until every item is met",
            },
            ArgSpec {
                flag: "--node-name-contains",
                value: "PAT",
                help: "a showing node's name contains PAT (--node-role narrows)",
            },
            ArgSpec {
                flag: "--text-equals",
                value: "TEXT",
                help: "independent Text.GetText on --name equals TEXT",
            },
            ArgSpec {
                flag: "--text-contains",
                value: "SUB",
                help: "independent Text.GetText on --name contains SUB",
            },
            NAME_PAT,
            ROLE,
            WINDOW,
        ],
        details: r#"--window HANDLE --expect JSON uses the same matcher as verify: polls
until every item is met, ambiguity / unobservable state fail at once,
timeout is typed with the last observation.
--text-equals / --node-text-equals and --text-contains / --node-text-contains
poll AT-SPI Text.GetText on the unique showing named node until that
independent text equals TEXT or contains SUB. send-text / paste / copy
matched.text, last_text_write_via, and the WebKit eval helper's queued-job
OK are not this condition. Timeout is typed ("timeout") and reports the
last GetText. Never screenshot / XTest / --coords. `--` ends flag parsing."#,
    },
    VerbSpec {
        name: "screenshot",
        command: "screenshot",
        aliases: &["shot"],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "capture a window or the screen to a PNG file",
        usage: "screenshot [--out PATH | PATH] [--window HANDLE]",
        args: &[
            ArgSpec {
                flag: "--out",
                value: "PATH",
                help: "PNG path (default: a temp file named by pid)",
            },
            WINDOW,
        ],
        details: r#"Last resort, never the primary observation: the a11y verbs are. --window
is never a positional path. `shot` is the MCU spelling."#,
    },
    VerbSpec {
        name: "zoom",
        command: "zoom",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "crop one region of a window capture to a PNG",
        usage: "zoom --window HANDLE --region X,Y,W,H --out PATH [--replace] [--pad N]",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--region",
                value: "X,Y,W,H",
                help: "screen rectangle to crop (the space node bounds use)",
            },
            ArgSpec {
                flag: "--out",
                value: "PATH",
                help: "PNG path",
            },
            ArgSpec {
                flag: "--replace",
                value: "",
                help: "overwrite an existing file",
            },
            ArgSpec {
                flag: "--pad",
                value: "N",
                help: "pixels of context kept around the region (default 8, at most 512)",
            },
        ],
        details: r#"Crops one region out of the window's own capture, so a caller can look
at a detail without a full-screen image. --region is in screen
coordinates -- the same space node bounds and query --within use -- so a
node's bounds can be passed straight in.

A region that does not intersect the window is typed
"region_outside_window" and NO file is written; a region that straddles
an edge is clipped to the window. --pad adds context around a region that
already intersects; it never rescues one that misses.

The crop is a clip of the window capture, never a screen grab: the reply
reports the capture size and the point -> pixel scale it applied (a
Retina window is captured at 2x, and the region is scaled into that
space). Still the last resort, like `screenshot`: the a11y verbs are the
primary observation."#,
    },
    VerbSpec {
        name: "pointer-position",
        command: "pointer-position",
        aliases: &["cursor"],
        scope: Scope::Observe,
        family: Family::A11yObserve,
        summary: "absolute pointer coordinates, no event injected",
        usage: "pointer-position
cursor                                              (alias)",
        args: &[],
        details: r#"Observes absolute screen coordinates without injecting any pointer event
(macOS: a read-only CGEvent sample; the journey reads it around every
click / close to prove the real pointer stayed put). `cursor` is the MCU
spelling."#,
    },
    // ----------------------------------------------------------- a11y actuate
    VerbSpec {
        name: "invoke",
        command: "invoke",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::A11yActuate,
        summary: "one semantic a11y action on one node",
        usage: "invoke --window HANDLE (--node PATH | --index N | --name PAT [--role ROLE] | --identifier ID
                        | --focused [--role ROLE] | --selector PATH)
       <press | set-value TEXT | select-option NAME | set-checked true|false
        | set-expanded true|false | increment | decrement | scroll-to
        | set-selection START:LENGTH>",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--node",
                value: "PATH",
                help: "node path id from tree / query",
            },
            ArgSpec {
                flag: "--index",
                value: "N",
                help: "flat index from tree --flat",
            },
            NAME_PAT,
            ROLE,
            ArgSpec {
                flag: "--identifier",
                value: "ID",
                help: "accessibility identifier",
            },
            ArgSpec {
                flag: "--focused",
                value: "",
                help: "the application's own focused control (combine only with --role)",
            },
            ArgSpec {
                flag: "--selector",
                value: "PATH",
                help: "MCU selector path (not with --node/--index/--name/--identifier)",
            },
            ArgSpec {
                flag: "<action>",
                value: "[VALUE]",
                help: "exactly one action after the flags, at most one VALUE",
            },
        ],
        details: r#"One semantic a11y action; never activates or raises the window. Two
showing matches -> "ambiguous", none -> "a11y_node_not_found", an action
the node does not offer -> "unsupported". set-checked / set-expanded are
desired states (already there = verified no-op). The reply carries verified
true|false with the reason and a receipt (target, node, action, before /
after state). --focused acts on the application's own focused control (what
`focused` reports), bound by id / role / identifier in the same tree read;
--role narrows it ("unverified" when the focused control has another role)."#,
    },
    VerbSpec {
        name: "menu-invoke",
        command: "menu-invoke",
        aliases: &["menu invoke"],
        scope: Scope::Actuate,
        family: Family::A11yActuate,
        summary: "press one menu item by exact path in the background",
        usage: "menu invoke --window HANDLE --path 'Menu/Item' | '[\"Menu\",\"Item\"]'",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--path",
                value: "PATH",
                help: "'Menu/Item' or a JSON array of exact titles",
            },
        ],
        details: r#"Press one menu item by exact path in the background; every segment must
be exactly one enabled item before anything is pressed
("a11y_menu_item_not_found" / "..._ambiguous" / "..._disabled"), the last
must be a leaf ("..._not_leaf"), a bare menu is "invalid_input"; verified
by mark read-back / tree diff."#,
    },
    VerbSpec {
        name: "click",
        command: "click",
        aliases: &["dclick", "rclick"],
        scope: Scope::Actuate,
        family: Family::A11yActuate,
        summary: "click a node by --node / --name, or --coords --degraded",
        usage: "click (--window HANDLE --node ID | --window HANDLE --name PAT [--role ROLE] | --coords X,Y --degraded)
      [--button left|right|middle] [--clicks N]
dclick ...                                          (alias of click --clicks 2)
rclick ...                                          (alias of click --button right)",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--to",
                value: "HANDLE",
                help: "MCU spelling of --window (not both; --to desktop is not mapped)",
            },
            ArgSpec {
                flag: "--node",
                value: "ID",
                help: "node path id",
            },
            NAME_PAT,
            ROLE,
            ArgSpec {
                flag: "--coords",
                value: "X,Y",
                help: "screen coordinates; requires --degraded",
            },
            ArgSpec {
                flag: "--degraded",
                value: "",
                help: "admit the coordinate fallback",
            },
            ArgSpec {
                flag: "--button",
                value: "left|right|middle",
                help: "pointer button (default left)",
            },
            ArgSpec {
                flag: "--clicks",
                value: "N",
                help: "click count (default 1)",
            },
        ],
        details: r#"--name reuses wait NodeNameContains matching, then the --node AT-SPI path.
`dclick` is click --clicks 2 and `rclick` is click --button right (MCU
spellings)."#,
    },
    VerbSpec {
        name: "drag",
        command: "drag",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::A11yActuate,
        summary: "press, move, release as one gesture (--degraded)",
        usage: "drag --window HANDLE --from X,Y --to X,Y [--button left|right|middle] [--steps N] [--degraded]",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--from",
                value: "X,Y",
                help: "screen point of the press; must be inside the window",
            },
            ArgSpec {
                flag: "--to",
                value: "X,Y",
                help: "screen point of the release",
            },
            ArgSpec {
                flag: "--button",
                value: "left|right|middle",
                help: "pointer button (default left)",
            },
            ArgSpec {
                flag: "--steps",
                value: "N",
                help: "intermediate moves between press and release (default 12, at most 64)",
            },
            ArgSpec {
                flag: "--degraded",
                value: "",
                help: "admit the path that moves the user's real pointer",
            },
        ],
        details: r#"One press, a bounded series of moves and one release, delivered as one
gesture. There is no semantic a11y path for a drag, so this is the
pointer, and the reply always says WHICH path ran (`path`) plus whether a
window-local one existed (window_local_available).

macOS has no window-local pointer injection at all: mouse events posted
to a pid arrive with no window for AppKit to route them through, so the
only working path is the global one that MOVES THE USER'S REAL CURSOR.
That makes --degraded a required opt-in there, exactly as for
click --coords; without it the verb refuses and names the path it would
have taken. Nothing is injected before that refusal.

--from must be inside the window (else typed "drag_outside_window",
nothing performed); --to may leave it, and to_inside_window says so. The
read-back is where the pointer ended up: the release happened at --to, so
pointer_after must equal it, and the receipt records pointer_before and
pointer_after either way. tree_changed reports whether the window's tree
moved, as supporting evidence."#,
    },
    VerbSpec {
        name: "focus",
        command: "focus",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::A11yActuate,
        summary: "focus a node by --node or --window --name",
        usage: "focus [--window HANDLE] (--node ID | --window HANDLE --name PAT [--role ROLE])",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--node",
                value: "ID",
                help: "node path id",
            },
            NAME_PAT,
            ROLE,
        ],
        details: r#"Gives one node the accessibility focus through the a11y tree; never
raises or activates the window."#,
    },
    VerbSpec {
        name: "send-text",
        command: "send-text",
        aliases: &["type"],
        scope: Scope::Actuate,
        family: Family::A11yActuate,
        summary: "write text into a named / focused node, or type into focus",
        usage: "send-text [--window HANDLE [--name PAT [--role ROLE]]] [--allow-browser-chrome] [--] <text...>
type ...                                            (alias)",
        args: &[
            WINDOW,
            NAME_PAT,
            ROLE,
            ALLOW_BROWSER_CHROME,
            ArgSpec {
                flag: "--",
                value: "",
                help: "ends flag parsing so the text may start with a dash",
            },
        ],
        details: r#"--name writes via AT-SPI EditableText (SetTextContents / InsertText) or
AT-SPI Text + toolkit set-value when EditableText is absent (Chrome
renderer AX; WebKitGTK AT-SPI id + eval helper); a node with no writeable
text interface typed-fails (never XTest). --window without --name writes
that same path on the showing focused node (same innermost Text candidate
as get-text --window). Never XTest when --window is set. Without --window
stays the plain type-into-focused inject. `--` ends flag parsing. `type`
is the MCU spelling.
When the window's focused control is browser chrome (omnibox, toolbar, tab
strip) the write is refused with focused_node_is_browser_chrome unless
--allow-browser-chrome is passed; pass --name to address a page control
instead."#,
    },
    VerbSpec {
        name: "send-keys",
        command: "send-keys",
        aliases: &["key"],
        scope: Scope::Actuate,
        family: Family::A11yActuate,
        summary: "deliver key chords to a named / focused node, or the focus",
        usage: "send-keys [--window HANDLE [--name PAT [--role ROLE]]] [--allow-browser-chrome] [--] <keys...>
key ...                                             (alias)",
        args: &[
            WINDOW,
            NAME_PAT,
            ROLE,
            ALLOW_BROWSER_CHROME,
            ArgSpec {
                flag: "--",
                value: "",
                help: "ends flag parsing so a chord may start with a dash",
            },
        ],
        details: r#"--name delivers AT-SPI Device/key events (DeviceEventListener
NotifyEvent); a node with no key interface typed-fails (never XTest).
--window without --name targets the showing focused node (same innermost
Text candidate as get-text --window). Prefers DeviceEventListener; plain
typeable text falls back to the AT-SPI EditableText/Text write path when
that interface is absent (con Command; Chrome; Reasonix). Never XTest when
--window is set. Without --window stays the plain focused inject. `--`
ends flag parsing. e.g. ctrl+c / enter / k. `key` is the MCU spelling.
When the window's focused control is browser chrome (omnibox, toolbar, tab
strip) the write is refused with focused_node_is_browser_chrome unless
--allow-browser-chrome is passed; pass --name to address a page control
instead."#,
    },
    VerbSpec {
        name: "scroll",
        command: "scroll",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::A11yActuate,
        summary: "AT-SPI Component.ScrollTo(TopEdge) on a named node",
        usage: "scroll --window HANDLE --name PAT [--role ROLE]",
        args: &[WINDOW, NAME_PAT, ROLE],
        details: r#"One-shot AT-SPI Component.ScrollTo(TopEdge). addressing=accessibility-tree
via=scroll-to. Missing / false / UnknownMethod typed-fails
(a11y_scroll_unavailable). Never Action scroll*, XTest wheel, --coords, or
screenshot."#,
    },
    VerbSpec {
        name: "select",
        command: "select",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::A11yActuate,
        summary: "AT-SPI Text.SetSelection on a named node",
        usage: "select --window HANDLE --name PAT --start N --end M [--role ROLE]",
        args: &[
            WINDOW,
            NAME_PAT,
            ArgSpec {
                flag: "--start",
                value: "N",
                help: "selection start offset",
            },
            ArgSpec {
                flag: "--end",
                value: "M",
                help: "selection end offset",
            },
            ROLE,
        ],
        details: r#"One-shot AT-SPI Text.SetSelection(0, start, end).
addressing=accessibility-tree via=set-selection. Missing Text /
UnknownMethod typed-fails (a11y_selection_unavailable). SetSelection false
typed-fails (a11y_selection_no_effect). Never XTest, mouse-drag, --coords,
or screenshot. The reply is not proof; observe with get-selection."#,
    },
    VerbSpec {
        name: "set-caret",
        command: "set-caret",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::A11yActuate,
        summary: "AT-SPI Text.SetCaretOffset on a named node",
        usage: "set-caret --window HANDLE --name PAT --offset N [--role ROLE]",
        args: &[
            WINDOW,
            NAME_PAT,
            ArgSpec {
                flag: "--offset",
                value: "N",
                help: "caret offset",
            },
            ROLE,
        ],
        details: r#"One-shot AT-SPI Text.SetCaretOffset. addressing=accessibility-tree
via=set-caret-offset. Missing Text / UnknownMethod typed-fails
(a11y_caret_unavailable). SetCaretOffset false typed-fails
(a11y_caret_no_effect). Never XTest, --coords, or screenshot. The reply is
not proof; observe with get-caret."#,
    },
    VerbSpec {
        name: "pointer-move",
        command: "pointer-move",
        aliases: &["move"],
        scope: Scope::Actuate,
        family: Family::A11yActuate,
        summary: "move the pointer to absolute screen coordinates",
        usage: "pointer-move --to desktop --x X --y Y
move ...                                            (alias)",
        args: &[
            ArgSpec {
                flag: "--to",
                value: "desktop",
                help: "explicit global coordinates; --to <handle> answers typed unsupported",
            },
            ArgSpec {
                flag: "--x",
                value: "X",
                help: "signed 32-bit screen x",
            },
            ArgSpec {
                flag: "--y",
                value: "Y",
                help: "signed 32-bit screen y",
            },
        ],
        details: r#"Moves to absolute screen coordinates without any press / release / click
/ drag / wheel side effect. Window-local pointer is not mapped: use click
--window. `move` is the MCU spelling."#,
    },
    // ---------------------------------------------------------------- browser
    VerbSpec {
        name: "page-js",
        command: "page-js",
        aliases: &["page read"],
        scope: Scope::Observe,
        family: Family::Browser,
        summary: "CDP Runtime.evaluate against one page target",
        usage: "page-js [--window HANDLE] --expression EXPR [--port N]
        [--target-id ID | --target-url SUB | --target-title SUB]
page read --js EXPR [...]                           (MCU spelling)",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--expression",
                value: "EXPR",
                help: "JavaScript expression (MCU: --js)",
            },
            PORT,
            TARGET_ID,
            TARGET_URL,
            TARGET_TITLE,
        ],
        details: r#"Second knife: CDP Runtime.evaluate on 127.0.0.1:N (default 9222).
MAIN-world Function constructor is refused. No listener -> typed
unsupported with backend debugger-runtime-evaluate. One selector picks the
page target (tab): exact id, or a case-insensitive substring of its url /
title. No match -> "cdp_target_not_found", more than one ->
"cdp_target_ambiguous" (candidates in error.detail). None keeps the first
page. The chosen id/url/title is echoed; a background tab is evaluated in
place, never selected or raised. Chrome / Brave must be started with
--remote-debugging-port=9222 for every CDP path; that port answers any
local process, so open it only while needed."#,
    },
    VerbSpec {
        name: "page-targets",
        command: "page-targets",
        aliases: &["page targets"],
        scope: Scope::Observe,
        family: Family::Browser,
        summary: "the CDP /json target inventory",
        usage: "page-targets [--port N] [--browser-profile SUB]
page targets [--port N] [--browser-profile SUB]     (MCU spelling)",
        args: &[PORT, BROWSER_PROFILE],
        details: r#"The CDP /json inventory: id, url, title, type, attached, websocket
(offered or not). Pick a --target-id here. No listener -> typed
unsupported. Chrome / Brave must be started with
--remote-debugging-port=9222 for every CDP path; that port answers any
local process, so open it only while needed.

--browser-profile SUB joins the inventory to one profile: only targets
whose title equals (exactly) a tab title of a window whose
browser_profile contains SUB are returned, each with profile_match:
"title", window and browser_profile. This is a heuristic and the reply
says so: one CDP port serves every profile of an instance and a target
carries no profile field, so a title shared across profiles matches each
of them and a differently spelled strip title (memory-saver suffix,
unset document title) is left out. No such window ->
browser_window_not_found before any socket is opened."#,
    },
    VerbSpec {
        name: "page-text",
        command: "page-text",
        aliases: &["page text"],
        scope: Scope::Observe,
        family: Family::Browser,
        summary: "visible page text in reading order (a11y tree or CDP)",
        usage: "page-text --window HANDLE [--max-bytes N] [--within X,Y,W,H] [--depth N] [--max-nodes N]
page-text [--port N] (--target-id ID | --target-url SUB | --target-title SUB) [--max-bytes N]
page text ...  |  page read [...]                   (MCU spellings)",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--max-bytes",
                value: "N",
                help: "text budget (default 16 KiB, max 1 MiB)",
            },
            ArgSpec {
                flag: "--within",
                value: "X,Y,W,H",
                help: "a11y only: screen rectangle the rows must intersect",
            },
            ArgSpec {
                flag: "--depth",
                value: "N",
                help: "a11y only: walk depth (default 64)",
            },
            ArgSpec {
                flag: "--max-nodes",
                value: "N",
                help: "a11y only: walk budget (default 6000)",
            },
            PORT,
            TARGET_ID,
            TARGET_URL,
            TARGET_TITLE,
        ],
        details: r#"The page's visible text in reading order as compact rows {id, role,
text} (+ name when it differs, focused, editable / actionable) -- never a
screenshot. Two backends, one row shape, so the caller does not care
which answered:

--window HANDLE reads the a11y tree of that window (rows carry bounds;
backend ax / uia / at-spi2). On macOS Chromium only the active tab has a
web-area there, so a background tab needs `tab select` first. Web
static-text keeps its words in AXValue; a link / button is one row (its
inner text merged). A container role is never a row of its own even when
the backend hands back its whole concatenated text (Windows UIA reports a
Chromium web root as Document, whose TextPattern is the entire page): the
rows are the nodes inside it, each with an id worth clicking. Then invoke
--node / click --node. Walk budget defaults depth 64 / 6000 nodes because
the platform's 1000-node breadth-first budget is spent on browser chrome
before web content.

--target-id | --target-url | --target-title [--port N] reads the CDP page
target instead (backend "cdp", via Accessibility.getFullAXTree; fallback
Runtime.evaluate innerText walk when that domain is unavailable). This
reaches a background tab in a background window and changes nothing
about which tab or window is active (focus_changed: false). Row `id` /
`node` is then the backend DOM node id that page click --node / page
fill --node take; no bounds (page find carries the box). One backend per
call. Default 16 KiB (max 1 MiB), truncated flag."#,
    },
    VerbSpec {
        name: "page-find",
        command: "page-find",
        aliases: &["page find"],
        scope: Scope::Observe,
        family: Family::Browser,
        summary: "nodes of one CDP page target by CSS, text or role",
        usage: "page-find [--port N] (--target-id ID | --target-url SUB | --target-title SUB)
        (--selector CSS | --text SUB | --role R [--name SUB])
page find ...                                       (MCU spelling)",
        args: &[
            PORT,
            TARGET_ID,
            TARGET_URL,
            TARGET_TITLE,
            CDP_SELECTOR,
            CDP_TEXT,
            ArgSpec {
                flag: "--role",
                value: "R",
                help: "AX role (button, link, textbox, StaticText, ...; case-insensitive)",
            },
            ArgSpec {
                flag: "--name",
                value: "SUB",
                help: "with --role: case-insensitive substring of the accessible name",
            },
        ],
        details: r#"The matching nodes of one CDP page target (a background tab in a
background window included; nothing is activated): {node, path, tag,
role, name, text, value, editable, ax_id, box}. --selector runs
DOM.querySelectorAll; --text and --role filter Accessibility.getFullAXTree
(a text hit inside a button / link is lifted to that control, so the
click lands on it). `node` is the backend DOM node id the actuators take
(page click --node / page fill --node); `box` is the layout box in
viewport CSS px. Zero matches -> cdp_node_not_found; more than 20 are
counted (total) and cut (truncated). The target selector rules are those
of page-js (cdp_target_not_found / cdp_target_ambiguous)."#,
    },
    VerbSpec {
        name: "page-click",
        command: "page-click",
        aliases: &["page click"],
        scope: Scope::Actuate,
        family: Family::Browser,
        summary: "click one node of a CDP page target in place",
        usage: "page-click [--port N] (--target-id ID | --target-url SUB | --target-title SUB)
        (--selector CSS | --text SUB | --node ID) [--button left|right|middle] [--clicks N]
page click ...                                      (MCU spelling)",
        args: &[
            PORT,
            TARGET_ID,
            TARGET_URL,
            TARGET_TITLE,
            CDP_SELECTOR,
            CDP_TEXT,
            CDP_NODE,
            ArgSpec {
                flag: "--button",
                value: "B",
                help: "left (default) | right | middle",
            },
            ArgSpec {
                flag: "--clicks",
                value: "N",
                help: "1 (default) ..= 3 press / release pairs",
            },
        ],
        details: r#"Resolves exactly one node (zero -> cdp_node_not_found, more ->
cdp_node_ambiguous with candidates; narrow the selector or pass --node),
DOM.scrollIntoViewIfNeeded, takes the DOM.getBoxModel content centre, and
dispatches Input.dispatchMouseEvent mouseMoved + pressed + released on
that page target. The tab is not selected and the window is not raised
(focus_changed: false); focus emulation is switched on for the click and
off after so an unfocused page handles it normally. A node without a
layout box -> cdp_node_not_visible, nothing dispatched. Verified by
reading the document (url, title, text length, active element) and the
node (text, value, checked, attributes) back: performed says the events
were accepted, verified says something observable changed
(verification.changed lists what; no_observable_change is honest, not a
failure). Receipt reserved before the dispatch, completed after."#,
    },
    VerbSpec {
        name: "page-fill",
        command: "page-fill",
        aliases: &["page fill"],
        scope: Scope::Actuate,
        family: Family::Browser,
        summary: "type into one field of a CDP page target in place",
        usage: "page-fill [--port N] (--target-id ID | --target-url SUB | --target-title SUB)
        (--selector CSS | --node ID) --text TEXT [--clear] [--submit]
page fill ...                                       (MCU spelling)",
        args: &[
            PORT,
            TARGET_ID,
            TARGET_URL,
            TARGET_TITLE,
            CDP_SELECTOR,
            CDP_NODE,
            ArgSpec {
                flag: "--text",
                value: "TEXT",
                help: "what to insert (<= 64 KiB; empty only with --clear)",
            },
            ArgSpec {
                flag: "--clear",
                value: "",
                help: "select everything first so the text replaces the field",
            },
            ArgSpec {
                flag: "--submit",
                value: "",
                help: "then dispatch Enter key down / up",
            },
        ],
        details: r#"Resolves exactly one editable node (an enabled input / textarea /
contenteditable; anything else -> cdp_node_not_editable, nothing
written), DOM.focus, optional select-all (--clear), Input.insertText,
then reads .value (or the text content) back; --submit dispatches Enter
key events afterwards and echoes the document state. Focus emulation is
switched on for the write and off after, so a background tab accepts it
without being brought forward (focus_changed: false). Verified when the
read-back equals TEXT (--clear) or grew by exactly TEXT (append at the
caret); a mismatch is performed but unverified (value_mismatch), for a
page that rewrites its own field. Receipt reserved before the write,
completed after."#,
    },
    VerbSpec {
        name: "page-nav",
        command: "page-nav",
        aliases: &["page nav"],
        scope: Scope::Actuate,
        family: Family::Browser,
        summary: "navigate one CDP page target without selecting it",
        usage: "page-nav [--port N] (--target-id ID | --target-url SUB | --target-title SUB) --url URL [--wait-ms N]
page nav ...                                        (MCU spelling)",
        args: &[
            PORT,
            TARGET_ID,
            TARGET_URL,
            TARGET_TITLE,
            ArgSpec {
                flag: "--url",
                value: "URL",
                help: "where to go (needs a scheme: https:, data:, file:, about:)",
            },
            ArgSpec {
                flag: "--wait-ms",
                value: "N",
                help: "how long to wait for Page.loadEventFired (default 10000, max 120000)",
            },
        ],
        details: r#"Page.navigate on that page target -- a background tab stays a
background tab, the window is not raised (focus_changed: false) -- then
waits up to --wait-ms for Page.loadEventFired. A navigation Chromium
refuses at once (errorText, e.g. a DNS failure) -> cdp_navigation_failed.
Verified when the load event fired (or readyState reads complete on the
new url); a timeout is performed but unverified (load_timeout) with the
url / title read so far. Reply: final_url, final_title, waited_ms,
receipt."#,
    },
    VerbSpec {
        name: "page-screenshot",
        command: "page-screenshot",
        aliases: &["page screenshot"],
        scope: Scope::Observe,
        family: Family::Browser,
        summary: "PNG of one CDP page target (background may refuse)",
        usage: "page-screenshot [--port N] (--target-id ID | --target-url SUB | --target-title SUB) --out PATH [--replace] [--activate]
page screenshot ...                                 (MCU spelling)",
        args: &[
            PORT,
            TARGET_ID,
            TARGET_URL,
            TARGET_TITLE,
            ArgSpec {
                flag: "--out",
                value: "PATH",
                help: "where the PNG is written (refuses an existing file without --replace)",
            },
            ArgSpec {
                flag: "--replace",
                value: "",
                help: "overwrite --out",
            },
            ArgSpec {
                flag: "--activate",
                value: "",
                help: "actuate: Page.bringToFront first (the only CDP verb that changes the active tab)",
            },
        ],
        details: r#"Page.captureScreenshot (PNG) of that page target, written to --out with
its sha256 in the reply. Chromium does not paint a tab that is not
visible, so a background or occluded tab may answer
cdp_screenshot_unavailable; this verb never activates the tab to get a
picture. --activate is the one explicit opt-in: it runs Page.bringToFront
first, needs the actuate grant, writes a receipt, and replies
focus_changed: true. Prefer page text / page find, which need no pixels."#,
    },
    VerbSpec {
        name: "tab-list",
        command: "tab-list",
        aliases: &["tab list"],
        scope: Scope::Observe,
        family: Family::Browser,
        summary: "the browser tab strip through the a11y tree",
        usage: "tab-list --window HANDLE
tab list --window HANDLE                            (MCU spelling)",
        args: &[WINDOW],
        details: r#"The browser tab strip through the a11y tree: index, title, selected per
tab. The strip is read on every backend, whatever it calls the roles:
macOS AX tab-group / radio-button, AT-SPI2 and UIA both "page tab list" /
"page tab" (roles are compared on their alphanumeric core, so separator
and AX-prefix spellings all match). A background tab's content is never in
the tree -- only its row is; use tab select, or the CDP page verbs.

Evidence: macOS is proven on a real Brave instance
(scripts/cu-brave-live-smoke.sh). Linux and Windows are code-complete with
no live run yet; scripts/cu-linux-smoke.sh and
scripts/qjs/cu-windows-browser-smoke.qjs carry the journey and exit with a
typed SKIP until a host with a Chromium-family browser runs them."#,
    },
    VerbSpec {
        name: "tab-select",
        command: "tab-select",
        aliases: &["tab select"],
        scope: Scope::Actuate,
        family: Family::Browser,
        summary: "select one tab in the background",
        usage: "tab-select --window HANDLE (--title SUB | --index N)
tab select ...                                      (MCU spelling)",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--title",
                value: "SUB",
                help: "tab title substring (not with --index)",
            },
            ArgSpec {
                flag: "--index",
                value: "N",
                help: "tab index from tab-list (not with --title)",
            },
        ],
        details: r#"Presses one tab-strip row in the background so it becomes the window's
active tab; never raises or activates the window. No such tab ->
"a11y_tab_not_found", two title hits -> "a11y_tab_ambiguous"; verified by
reading selected back (already selected = verified no-op). The a11y
fallback when no CDP port is open."#,
    },
    VerbSpec {
        name: "tab-close",
        command: "tab-close",
        aliases: &["tab close"],
        scope: Scope::Actuate,
        family: Family::Browser,
        summary: "close one tab through its own close button or CDP (gated)",
        usage: "tab-close --window HANDLE (--title T --exact | --index N) --expect gone [--port N]
tab close ...                                       (MCU spelling)",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--title",
                value: "T",
                help: "the tab's exact title (from tab-list); needs --exact",
            },
            ArgSpec {
                flag: "--index",
                value: "N",
                help: "the tab's strip index from tab-list (0-based); the exact selector for same-title duplicates",
            },
            ArgSpec {
                flag: "--exact",
                value: "",
                help: "required with --title: the title is matched exactly, never as a substring",
            },
            ArgSpec {
                flag: "--expect",
                value: "gone",
                help: "checkable postcondition: one fewer strip row with that title reads back",
            },
            ArgSpec {
                flag: "--port",
                value: "N",
                help: "CDP port: close by Target.closeTarget when the title names exactly one page target of the instance; else the a11y path",
            },
        ],
        details: r#"Destructive, so gated like close: --window H plus --title T --exact
(exact, case-sensitive title equality; no match -> a11y_tab_not_found,
two -> a11y_tab_ambiguous, name one with --index) or --index N (tab-list
order) names the tab, the strip snapshot is written to the receipt
before anything is pressed, and --expect gone is the postcondition read
back from the tab strip (one fewer row with that title, up to 2.5 s).
Any missing part -> refused (destructive_gate) with nothing performed.
The press goes to the tab row's own close button (the button child of
the Chromium tab radio-button). macOS Chromium exposes that button on
the selected (or hovered) tab only, so a background tab is selected
first in the same window (never raising it), closed, and the previously
selected tab is pressed again; the reply says selection_restored
true|false and select_first. With --port N and a listener, a title that
names exactly one page target of the whole instance is closed by
Target.closeTarget instead (via cdp-close-target, no selection change);
no listener, no such target, or two targets fall back to the a11y path
(cdp_fallback says why). A keyboard shortcut is never substituted.
Reply: performed, verified, via, before / after strip rows, receipt."#,
    },
    VerbSpec {
        name: "browser-profiles",
        command: "browser-profiles",
        aliases: &["browser profiles"],
        scope: Scope::Observe,
        family: Family::Browser,
        summary: "Chromium profiles of the running browser, with windows",
        usage: "browser-profiles [--app SUB]
browser profiles [--app SUB]                        (MCU spelling)",
        args: &[ArgSpec {
            flag: "--app",
            value: "SUB",
            help: "Brave Origin | Brave Browser | Google Chrome (substring; default: the one running)",
        }],
        details: r#"Reads the application's Chromium `Local State` (profile.info_cache:
directory -> display name; profile.last_used) and joins each profile to
the inventory windows whose browser_profile equals its name. Rows: {name,
directory, last_used, windows: [handles]}; browser windows whose profile
name is not in Local State are listed under unlisted_windows. --app is a
catalog substring (Brave Origin -> ~/Library/Application
Support/BraveSoftware/Brave-Origin, Brave Browser -> .../Brave-Browser,
Google Chrome -> .../Google/Chrome; Linux under ~/.config); omitted, the
one running catalog application is used (none -> browser_app_not_found,
several -> browser_app_ambiguous). Any other application -> typed
unsupported. Never touches the browser."#,
    },
    VerbSpec {
        name: "browser-open",
        command: "browser-open",
        aliases: &["browser open"],
        scope: Scope::Actuate,
        family: Family::Browser,
        summary: "open one profile's window / URL in the running browser",
        usage: "browser-open --profile NAME [--url URL] [--app SUB] [--timeout-ms N]
browser open ...                                    (MCU spelling)",
        args: &[
            ArgSpec {
                flag: "--profile",
                value: "NAME",
                help: "profile name from browser-profiles (exact, else unique case-insensitive substring)",
            },
            ArgSpec {
                flag: "--url",
                value: "URL",
                help: "open this URL in the profile (a tab of its window when it has one)",
            },
            ArgSpec {
                flag: "--app",
                value: "SUB",
                help: "Brave Origin | Brave Browser | Google Chrome (substring; default: the one running)",
            },
            ArgSpec {
                flag: "--timeout-ms",
                value: "N",
                help: "how long to wait for the window (default 8000, max 120000)",
            },
        ],
        details: r#"Resolves NAME to its profile directory (exact name first, then a unique
case-insensitive substring; browser_profile_not_found /
browser_profile_ambiguous carry the candidates) and runs macOS `open -na
<app> --args --profile-directory=<dir> [URL]`: the Chromium process
singleton hands that command line to the running instance, which opens a
window of the profile (or, with a URL, a tab in the profile's existing
window) and the user's browser is never quit or restarted. Then polls the
window inventory (default 8000 ms) until a window with that
browser_profile appears that was not in the before snapshot, or -- when
the profile already had a window and a URL was given -- until that
window's title changes. Reply: {handle, browser_profile, title, created,
tab_index, tab_title, tabs: {before, after}} -- the new tab is the
selected row the window's strip gained (diffed against the strip read
before the launch; null when none can be told apart) -- plus the receipt
(reserved before `open`, completed / failed after the read-back); timeout
-> browser_window_not_found. `open` activates the browser, so this is
actuation; nothing here needs a CDP port. Without --url the profile's own
last session is restored: after a browser restart only the last-used
profile comes back by itself, and `browser open --profile X` is how the
other profiles' windows are brought back."#,
    },
    VerbSpec {
        name: "page",
        command: "page",
        aliases: &[],
        scope: Scope::Observe,
        family: Family::Browser,
        summary: "MCU page group; unmapped page verbs answer typed",
        usage: "page read [--js EXPR] | page targets | page text | page find | page click | page fill | page nav | page screenshot
page [<other>]                                      (typed unsupported)",
        args: &[],
        details: r#"MCU page: `page read --js` -> page-js, `page read` (no --js) -> the CDP
page-text, `page targets` -> page-targets, `page text` -> page-text (a11y
with --window, CDP with a target selector), and the CDP background-tab
verbs `page find` / `page click` / `page fill` / `page nav` / `page
screenshot` -> page-find / page-click / page-fill / page-nav /
page-screenshot. Any other page sub-verb answers typed unsupported."#,
    },
    // -------------------------------------------------------------- clipboard
    VerbSpec {
        name: "clipboard-read",
        command: "clipboard-read",
        aliases: &["clip", "clipboard", "clipboard read"],
        scope: Scope::Observe,
        family: Family::Clipboard,
        summary: "clipboard text, or one native type as bounded bytes",
        usage: "clipboard-read [--type T] [--max-bytes N] [--out PATH [--replace]]
clipboard [read] [T] [...]                          (MCU spelling)
clip                                                (text only)",
        args: &[
            ArgSpec {
                flag: "--type",
                value: "T",
                help: "one native type name (positional T also accepted)",
            },
            ArgSpec {
                flag: "--max-bytes",
                value: "N",
                help: "byte budget (default 1 MiB, max 16 MiB); requires --type",
            },
            ArgSpec {
                flag: "--out",
                value: "PATH",
                help: "write the bytes (0600); requires --type",
            },
            ArgSpec {
                flag: "--replace",
                value: "",
                help: "overwrite --out",
            },
        ],
        details: r#"No --type: Unicode text plus host type names. --type T: one native type
as bounded bytes (default 1 MiB, max 16 MiB), sha256, utf8 or base64.
--out writes the bytes (0600; --replace overwrites). Requires observe.
`clip` takes no arguments and reads text; `clipboard` with no sub-command
is clipboard-read."#,
    },
    VerbSpec {
        name: "clipboard-write",
        command: "clipboard-write",
        aliases: &["clipboard write"],
        scope: Scope::Actuate,
        family: Family::Clipboard,
        summary: "publish one native type from a regular file",
        usage: "clipboard-write --type T --path P
clipboard write T P                                 (MCU spelling)",
        args: &[
            ArgSpec {
                flag: "--type",
                value: "T",
                help: "native type name",
            },
            ArgSpec {
                flag: "--path",
                value: "P",
                help: "regular file (<= 16 MiB)",
            },
        ],
        details: r#"Publish one native type from a regular file (<= 16 MiB) and read it back
(actuate)."#,
    },
    VerbSpec {
        name: "clipboard-write-file",
        command: "clipboard-write-file",
        aliases: &["clipboard write-file"],
        scope: Scope::Actuate,
        family: Family::Clipboard,
        summary: "put a file reference on the clipboard",
        usage: "clipboard-write-file --path P
clipboard write-file P                              (MCU spelling)",
        args: &[ArgSpec {
            flag: "--path",
            value: "P",
            help: "file to reference",
        }],
        details: r#"Put a file reference on the clipboard (macOS POSIX file / Linux
text/uri-list / Windows CF_HDROP), not the file bytes (actuate)."#,
    },
    VerbSpec {
        name: "clipboard-clear",
        command: "clipboard-clear",
        aliases: &["clipboard clear"],
        scope: Scope::Actuate,
        family: Family::Clipboard,
        summary: "empty the clipboard (planned unless --apply)",
        usage: "clipboard-clear [--apply]
clipboard clear [--apply]                           (MCU spelling)",
        args: &[ArgSpec {
            flag: "--apply",
            value: "",
            help: "perform it; without --apply the reply is planned only",
        }],
        details: r#"Empty the clipboard. Without --apply this is planned and performs nothing
(actuate)."#,
    },
    VerbSpec {
        name: "copy",
        command: "copy",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Clipboard,
        summary: "copy a node's AT-SPI text onto the native clipboard",
        usage: "copy --window HANDLE [--name PAT [--role ROLE]]",
        args: &[WINDOW, NAME_PAT, ROLE],
        details: r#"Copies AT-SPI Text.GetText onto the native clipboard (Linux X11:
SetSelectionOwner, not xclip). addressing=accessibility-tree via=gettext.
--name targets the unique showing named node. --window without --name
copies that same path on the showing focused node (same innermost Text
candidate as get-text --window; con Command via=gettext on a second con
that never steals the resident control socket; Chrome GetTextField;
Reasonix Message Reasonix... under scripts/reasonix-desktop-a11y.sh). Never
XTest when --window is set. A node with no Text interface typed-fails
(never XTest / --coords / screenshot). Close the circuit with paste
--window (no --text / no --name) then get-text --window / wait
--text-equals; copy matched.text does not count."#,
    },
    VerbSpec {
        name: "paste",
        command: "paste",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Clipboard,
        summary: "write clipboard text into a node via AT-SPI",
        usage: "paste --window HANDLE [--name PAT [--role ROLE]] [--allow-browser-chrome] [--text TEXT] [-- TEXT]",
        args: &[
            WINDOW,
            NAME_PAT,
            ROLE,
            ALLOW_BROWSER_CHROME,
            ArgSpec {
                flag: "--text",
                value: "TEXT",
                help: "seed the clipboard first (the write still reads the clipboard)",
            },
            ArgSpec {
                flag: "--",
                value: "",
                help: "ends flag parsing so --text may start with a dash",
            },
        ],
        details: r#"Writes clipboard text via native AT-SPI EditableText / Text
(addressing=accessibility-tree). --text only seeds the clipboard; the field
write always reads the clipboard. --name targets the unique showing named
field. --window without --name writes that same path on the showing
focused node (same innermost Text candidate as get-text --window; con
Command via=editable-text on a second con that never steals the resident
control socket; Chrome GetTextField; Reasonix Message Reasonix...). Never
XTest when --window is set. A node with no writeable text interface
typed-fails (never XTest / --coords / screenshot). Close the circuit with
get-text --window / wait --text-equals; paste matched.text does not count.
`--` ends flag parsing.
When the window's focused control is browser chrome (omnibox, toolbar, tab
strip) the write is refused with focused_node_is_browser_chrome unless
--allow-browser-chrome is passed; pass --name to address a page control
instead."#,
    },
    // -------------------------------------------------------------- placement
    VerbSpec {
        name: "window-place",
        command: "window-place",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Placement,
        summary: "one of the 18 placement actions on a window",
        usage: "window-place --action <id> [--window HANDLE] [--x X --y Y --width W --height H]
    ids: center|fullscreen|left-half|right-half|top-half|bottom-half
         upper-left|lower-left|upper-right|lower-right
         next-third|previous-third|next-display|previous-display
         larger|smaller|undo|redo|move|resize|frame  (or SpectacleWindowAction* constants)",
        args: &[
            ArgSpec {
                flag: "--action",
                value: "ID",
                help: "placement action id (positional ID also accepted)",
            },
            WINDOW,
            ArgSpec {
                flag: "--x/--y/--width/--height",
                value: "N",
                help: "the rect for --action frame; all four or none",
            },
        ],
        details: r#"The product placement catalog shared with the desktop-host menu and
global shortcuts. `frame`, `movewin`, `resize` and `maximize` are the MCU
shorthands and answer as window-place."#,
    },
    VerbSpec {
        name: "frame",
        command: "window-place",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Placement,
        summary: "window-place --action frame",
        usage: "frame HANDLE|--window H --x X --y Y --width W --height H   (or HANDLE X Y W H)",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--x/--y/--width/--height",
                value: "N",
                help: "the requested rect (positional X Y W H also accepted)",
            },
        ],
        details: r#"Alias of window-place --action frame. The reply's command is
window-place."#,
    },
    VerbSpec {
        name: "movewin",
        command: "window-place",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Placement,
        summary: "window-place move (keeps the current size)",
        usage: "movewin HANDLE|--window H --x X --y Y   (or HANDLE X Y)",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--x/--y",
                value: "N",
                help: "new origin (positional X Y also accepted)",
            },
        ],
        details: r#"Alias of window-place move; keeps the current size. The reply's command
is window-place."#,
    },
    VerbSpec {
        name: "resize",
        command: "window-place",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Placement,
        summary: "window-place resize (keeps the current origin)",
        usage: "resize HANDLE|--window H --width W --height H   (or HANDLE W H)",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--width/--height",
                value: "N",
                help: "new size (positional W H also accepted)",
            },
        ],
        details: r#"Alias of window-place resize; keeps the current origin. The reply's
command is window-place."#,
    },
    VerbSpec {
        name: "maximize",
        command: "window-place",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Placement,
        summary: "window-place --action fullscreen",
        usage: "maximize HANDLE|--window H",
        args: &[WINDOW],
        details: r#"Alias of window-place --action fullscreen. The reply's command is
window-place."#,
    },
    VerbSpec {
        name: "orderwin",
        command: "orderwin",
        aliases: &[],
        scope: Scope::Actuate,
        family: Family::Placement,
        summary: "relative z-order: raise --window above / below --relative",
        usage: "orderwin --window HANDLE --relation above|below --relative HANDLE",
        args: &[
            WINDOW,
            ArgSpec {
                flag: "--relation",
                value: "above|below",
                help: "above raises --window; below raises --relative",
            },
            ArgSpec {
                flag: "--relative",
                value: "HANDLE",
                help: "the other window",
            },
        ],
        details: r#"MCU relative z-order: above raises --window, below raises --relative,
then reads the order back and answers from what it read:
window_order_not_applied when the raise did not take. Linux raises with
the EWMH _NET_RESTACK_WINDOW (no focus change); macOS AXRaise cannot
reorder a background application's windows, so it refuses rather than
activating it."#,
    },
    // ------------------------------------------------------------- transports
    VerbSpec {
        name: "exec",
        command: "exec",
        aliases: &[],
        scope: Scope::Unscoped,
        family: Family::Transports,
        summary: "run one JSON command (the ssh / vnc worker mode)",
        usage: "exec [--grant observe,actuate] [--grant-id ID] [--grant-store PATH] --json '<command-json>'
exec [...] --json -  |  --json-stdin  |  -            (JSON command on stdin)",
        args: &[
            ArgSpec {
                flag: "--json",
                value: "JSON|-",
                help: "the command payload, or - for stdin",
            },
            ArgSpec {
                flag: "--json-stdin",
                value: "",
                help: "read the payload from stdin",
            },
            ArgSpec {
                flag: "--grant",
                value: "S",
                help: "strict scopes (also --grant=S); CLI wins over AGENTERM_CU_GRANT",
            },
            ArgSpec {
                flag: "--grant-id",
                value: "ID",
                help: "persisted grant selector (exclusive with every other source)",
            },
            ArgSpec {
                flag: "--grant-store",
                value: "PATH",
                help: "store override; valid only with --grant-id",
            },
        ],
        details: r#"Executes one Command serialized as JSON (the same shape `verbs --json`
consumers build). The ssh and vnc transports spawn `agenterm-cu --grant …
exec --json -` on the worker side; global flags may precede `exec`. The
command's own target and scope apply; grant sources never union."#,
    },
    // ------------------------------------------------------------------- host
    VerbSpec {
        name: "grant",
        command: "grant",
        aliases: &[],
        scope: Scope::Unscoped,
        family: Family::Host,
        summary: "create / list / revoke bounded persisted grants",
        usage: "grant create --target current --scopes S --ttl-ms N (--one-shot|--max-uses N) [--grant-store PATH]
grant list [--grant-store PATH]
grant revoke --grant-id ID [--grant-store PATH]",
        args: &[
            ArgSpec {
                flag: "--scopes",
                value: "S",
                help: "observe, actuate or observe,actuate",
            },
            ArgSpec {
                flag: "--ttl-ms",
                value: "N",
                help: "lifetime",
            },
            ArgSpec {
                flag: "--one-shot",
                value: "",
                help: "single use",
            },
            ArgSpec {
                flag: "--max-uses",
                value: "N",
                help: "bounded use count",
            },
            ArgSpec {
                flag: "--grant-store",
                value: "PATH",
                help: "explicit store (test / admin seam)",
            },
        ],
        details: r#"Grant management is local/current only. It refuses ambient
AGENTERM_CU_GRANT* and AGENTERM_CU_AUTH* selectors; --grant-store is an
explicit test/admin seam. A created grant is selected on later commands
with --grant-id ID, which is mutually exclusive with --grant and the
environment."#,
    },
    VerbSpec {
        name: "host",
        command: "host",
        aliases: &["hotkeys"],
        scope: Scope::Unscoped,
        family: Family::Host,
        summary: "resident desktop menu and global shortcuts",
        usage: "host
hotkeys                                             (compatibility alias)",
        args: &[],
        details: r#"Runs this binary as the resident desktop host: a status menu and global
shortcuts that route the placement catalog through the same Command ->
Executor chain as the CLI. `hotkeys` is the compatibility alias."#,
    },
    VerbSpec {
        name: "help",
        command: "help",
        aliases: &[],
        scope: Scope::Unscoped,
        family: Family::Host,
        summary: "this list, or one verb's full reference",
        usage: "help [<verb> | ssh | vnc | rdp]
<verb> --help                                       (same as help <verb>)",
        args: &[],
        details: r#"With no argument: the grouped command list. With a verb (any spelling):
usage, arguments and the long reference for that verb. The topics ssh,
vnc and rdp describe the transports. An unknown name is a typed usage
error that lists near matches."#,
    },
    VerbSpec {
        name: "verbs",
        command: "verbs",
        aliases: &[],
        scope: Scope::Unscoped,
        family: Family::Host,
        summary: "the verb table (JSON when piped, or --json / --text)",
        usage: "verbs [--json | --text]",
        args: &[
            ArgSpec {
                flag: "--json",
                value: "",
                help: "JSON array (the default when stdout is not a terminal)",
            },
            ArgSpec {
                flag: "--text",
                value: "",
                help: "aligned text table",
            },
        ],
        details: r#"Every row of the verb table: name, command, aliases, grant, family,
summary, usage and args. The JSON form is the machine-readable surface for
agents and generated docs; it is exactly what `--help` and `help <verb>`
render."#,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn names_and_aliases_are_unique_and_lower_kebab() {
        let mut seen = BTreeSet::new();
        for spec in VERBS {
            for spelling in spec.spellings() {
                assert!(seen.insert(spelling), "duplicate spelling {spelling:?}");
                assert!(
                    spelling.chars().all(|c| c.is_ascii_lowercase()
                        || c.is_ascii_digit()
                        || c == '-'
                        || c == ' '),
                    "{spelling:?} is not lower kebab"
                );
                assert!(spelling.split(' ').count() <= 2, "{spelling:?}");
            }
        }
    }

    #[test]
    fn every_alias_resolves_to_its_canonical_verb() {
        for spec in VERBS {
            for alias in spec.aliases {
                let resolved = match alias.split_once(' ') {
                    Some((head, tail)) => resolve(head, Some(tail)),
                    None => resolve(alias, None),
                };
                let resolved = resolved.unwrap_or_else(|| panic!("{alias:?} did not resolve"));
                assert_eq!(resolved.name, spec.name, "{alias:?}");
            }
            assert_eq!(lookup(spec.name).map(|s| s.name), Some(spec.name));
        }
    }

    #[test]
    fn group_words_resolve_to_their_first_subcommand() {
        assert_eq!(lookup("menu").map(|s| s.name), Some("menu-inspect"));
        assert_eq!(
            resolve("menu", Some("invoke")).map(|s| s.name),
            Some("menu-invoke")
        );
        assert_eq!(
            resolve("menu", Some("bogus")).map(|s| s.name),
            Some("menu-inspect")
        );
        assert_eq!(lookup("tab").map(|s| s.name), Some("tab-list"));
        assert_eq!(
            resolve("tab", Some("select")).map(|s| s.name),
            Some("tab-select")
        );
        assert_eq!(
            resolve("tab", Some("close")).map(|s| s.name),
            Some("tab-close")
        );
        assert_eq!(lookup("browser").map(|s| s.name), Some("browser-profiles"));
        assert_eq!(
            resolve("browser", Some("open")).map(|s| s.name),
            Some("browser-open")
        );
        assert_eq!(
            resolve("browser", Some("profiles")).map(|s| s.name),
            Some("browser-profiles")
        );
        assert_eq!(
            resolve("page", Some("text")).map(|s| s.name),
            Some("page-text")
        );
        assert_eq!(
            resolve("page", Some("click")).map(|s| s.name),
            Some("page-click")
        );
        assert_eq!(
            resolve("page", Some("fill")).map(|s| s.name),
            Some("page-fill")
        );
        assert_eq!(
            resolve("page", Some("nav")).map(|s| s.name),
            Some("page-nav")
        );
        assert_eq!(
            resolve("page", Some("find")).map(|s| s.name),
            Some("page-find")
        );
        assert_eq!(
            resolve("page", Some("screenshot")).map(|s| s.name),
            Some("page-screenshot")
        );
        assert_eq!(resolve("page", Some("zoom")).map(|s| s.name), Some("page"));
        assert_eq!(
            resolve("clipboard", Some("write")).map(|s| s.name),
            Some("clipboard-write")
        );
        assert_eq!(
            resolve("clipboard", Some("--type")).map(|s| s.name),
            Some("clipboard-read")
        );
        assert!(lookup("no-such-verb").is_none());
    }

    #[test]
    fn every_verb_has_usage_summary_and_family_header() {
        for spec in VERBS {
            assert!(!spec.usage.trim().is_empty(), "{} has no usage", spec.name);
            let head = spec.usage.lines().next().unwrap_or("");
            assert!(
                spec.spellings().any(|s| head.starts_with(s)),
                "{}: usage must start with one of its spellings: {head:?}",
                spec.name
            );
            assert!(!spec.summary.is_empty(), "{} has no summary", spec.name);
            assert!(
                spec.summary.len() <= 60,
                "{} summary is {} chars; keep --help scannable",
                spec.name,
                spec.summary.len()
            );
            assert!(!spec.family.header().is_empty());
            assert!(!spec.command.is_empty());
        }
        for family in Family::ALL {
            assert!(by_family(family).next().is_some(), "{family:?} is empty");
        }
    }

    #[test]
    fn table_json_round_trips() {
        let rows = table_json();
        let json = serde_json::to_string(&rows).expect("serialize");
        let back: Vec<VerbJson> = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, rows);
        assert_eq!(back.len(), VERBS.len());
        let first = &back[0];
        assert_eq!(first.name, "capabilities");
        assert_eq!(first.grant, Scope::Observe);
        let value: serde_json::Value = serde_json::from_str(&json).expect("value");
        assert_eq!(value[0]["grant"], "observe");
        assert_eq!(value[0]["family"], "system");
        let exec = back
            .iter()
            .find(|row| row.name == "exec")
            .expect("exec row");
        assert_eq!(serde_json::to_value(exec.grant).unwrap(), "none");
    }

    #[test]
    fn near_matches_are_bounded_and_helpful() {
        let matches = near_matches("menu");
        assert!(matches.contains(&"menu inspect"), "{matches:?}");
        assert!(matches.contains(&"menu invoke"), "{matches:?}");
        let typo = near_matches("windws");
        assert!(typo.contains(&"windows"), "{typo:?}");
        assert!(near_matches("zzzzzzzz").is_empty());
        assert!(near_matches("c").len() <= 6);
    }

    #[test]
    fn scopes_follow_command_required_grant() {
        // The table's scope column must agree with Command::required_grant:
        // every actuate verb here is one of the actuate variants there.
        let actuate: BTreeSet<&str> = VERBS
            .iter()
            .filter(|spec| spec.scope == Scope::Actuate)
            .map(|spec| spec.command)
            .collect();
        for expected in [
            "pointer-move",
            "invoke",
            "menu-invoke",
            "click",
            "focus",
            "send-text",
            "copy",
            "clipboard-write",
            "clipboard-write-file",
            "clipboard-clear",
            "paste",
            "send-keys",
            "scroll",
            "select",
            "set-caret",
            "window-place",
            "orderwin",
            "close",
            "tab-select",
            "tab-close",
            "browser-open",
            "page-click",
            "page-fill",
            "page-nav",
            "app",
            "activate",
            "raise",
            "minimize",
            "restore",
            "drag",
        ] {
            assert!(actuate.contains(expected), "{expected} must be actuate");
        }
        // The four observers added with them stay observe: reading a tree,
        // a baseline, a point or a crop actuates nothing.
        for expected in ["hit", "zoom", "snapshot", "diff"] {
            assert!(!actuate.contains(expected), "{expected} must be observe");
        }
        assert_eq!(actuate.len(), 30, "{actuate:?}");
    }
}
