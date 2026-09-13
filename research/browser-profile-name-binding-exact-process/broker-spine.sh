#!/bin/sh
# External ledger, admission and stage-persistence broker for the exact-process
# experiment (third, independent precommitment).
#
# Provenance: the ledger/journal/receipt discipline below is copied with
# provenance from `research/browser-profile-name-binding-v2/run-current-host.sh`
# (its `run_broker` Perl heredoc). It is NOT a repair, rerun, import or rename
# of the v2 court: this broker has its own experiment id, schemas, state root,
# ordinals and input digest, and it never reads or writes v2's state. The v2
# experiment's sources, results, ledgers, receipts and exhausted markers are
# read-only history. Neither exhausted court reached a verdict, so no value
# from them is authoritative here.
#
# This broker exposes the admission machinery only. It launches no browser,
# cannot produce a design fact and (in `self-test`) reserves nothing in the
# formal state root. See `plan/design-browser-profile-name-binding-exact-process-experiment.md`.
#
#   broker.sh self-test                      # isolated, disposable, reserves nothing formal
#   broker.sh inspect                        # read-only: print the formal ledger
#
# Environment:
#   AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT   overrides the state root
#   AGENTERM_PROFILE_BINDING_EXACT_BROKER_MODE  set to `self-test` for the self-test

set -eu

EXPERIMENT='acu.dynamic.075.profile-name-binding-exact-process'
LEDGER_SCHEMA='agenterm.profile-binding-exact-process-attempt/v1'
JOURNAL_SCHEMA='agenterm.profile-binding-exact-process-stage/v1'
RECEIPT_SCHEMA='agenterm.profile-binding-exact-process-receipt/v1'
INPUT_DOMAIN='agenterm-cu/profile-binding-exact-process/input/v1'

# The self-test never touches the formal root. When the caller is the self-test
# this variable points at a disposable directory created by the runner.
STATE_ROOT=${AGENTERM_PROFILE_BINDING_EXACT_STATE_ROOT:-}

TEMPLATE_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
TEMPLATE="$TEMPLATE_DIR/result-template.json"

broker() {
  perl - "$STATE_ROOT" "$TEMPLATE" "$EXPERIMENT" "$LEDGER_SCHEMA" \
      "$JOURNAL_SCHEMA" "$RECEIPT_SCHEMA" "$INPUT_DOMAIN" "$@" <<'PERL'
use strict;
use warnings;
use Fcntl qw(:DEFAULT :flock O_NOFOLLOW);
use JSON::PP ();
use B ();
use Digest::SHA qw(sha256_hex);
use IO::Handle ();
use File::Basename qw(dirname);
use File::Spec ();
use Cwd ();

my ($state_root_arg, $template_path, $EXPERIMENT, $LEDGER_SCHEMA,
    $JOURNAL_SCHEMA, $RECEIPT_SCHEMA, $INPUT_DOMAIN, $operation, @args) = @ARGV;

# A state root may legitimately sit under a symlinked ancestor (on macOS
# `$TMPDIR` lives under `/var`, which is a symlink to `/private/var`). Resolve
# the root once, up front, so the plain-directory assertions below test the
# real tree rather than a symlinked ancestor.
my $state_root = File::Spec->rel2abs($state_root_arg // '');
if (my $real = eval { Cwd::realpath($state_root) }) {
  $state_root = $real if defined $real && $real ne '';
} else {
  # The root does not exist yet: resolve the deepest existing ancestor and
  # re-append the remaining components.
  my @parts = File::Spec->splitdir($state_root);
  my @tail;
  while (@parts) {
    my $candidate = File::Spec->catdir(@parts);
    my $real = eval { Cwd::realpath($candidate) };
    if (defined $real && $real ne '') {
      $state_root = @tail ? File::Spec->catdir($real, @tail) : $real;
      last;
    }
    unshift @tail, pop @parts;
  }
}

my $json = JSON::PP->new->canonical(1)->allow_nonref(0)->utf8(1);
my $ZERO = sha256_hex("$INPUT_DOMAIN.journal-zero/v1\0");
my %CRITERION_VALUE = map { $_ => 1 } qw(not-run pass fail rejected);

sub fail { die "$_[0]\n" }
sub is_bool { JSON::PP::is_bool($_[0]) }

sub slurp_raw {
  my ($path) = @_;
  sysopen(my $fh, $path, O_RDONLY | O_NOFOLLOW) or fail('read_failed');
  binmode($fh); local $/; my $b = <$fh>; close($fh) or fail('read_close_failed');
  return $b;
}

# The terminal vocabulary, the criteria keys and the persistence-failure
# record shape are owned by the experiment's result template, not by this
# broker. Reading them here prevents the broker from inventing a code or a key
# the specification does not admit.
my $TEMPLATE = do {
  my $bytes = slurp_raw($template_path);
  my $v = eval { $json->decode($bytes) };
  $@ and fail('template_json_invalid');
  ref($v) eq 'HASH' or fail('template_not_object');
  $v->{experiment} eq $EXPERIMENT or fail('template_identity');
  $v;
};

my $PERSISTENCE_CODE = $TEMPLATE->{persistence_failure_stdout}{code};
$PERSISTENCE_CODE && $PERSISTENCE_CODE =~ /^[A-Z_]+$/
  or fail('template_persistence_code');
my %TERMINAL = map { $_ => 1 } @{$TEMPLATE->{terminals} // []};
%TERMINAL or fail('template_terminals');
$TERMINAL{$PERSISTENCE_CODE} or fail('template_persistence_code_not_terminal');

my @CRITERIA_KEYS = @{$TEMPLATE->{criteria_keys} // []};
@CRITERIA_KEYS or fail('template_criteria_keys');
my @PERSISTENCE_STDOUT_KEYS =
  @{$TEMPLATE->{persistence_failure_stdout}{required_keys} // []};
@PERSISTENCE_STDOUT_KEYS or fail('template_persistence_keys');
my @DESIGN_KEYS = @{$TEMPLATE->{design_keys} // []};

# Terminal -> the criteria shapes that terminal is allowed to publish, taken
# from the specification's V1-V7 table and its decision tree (sections 3, 4).
#
# Each terminal maps to a LIST of legal alternatives. A terminal with more than
# one alternative may be reached by more than one gate failing, so each
# alternative names the criteria that must be `fail` and the criteria that must
# be `pass`; every criterion in neither set must be `not-run`. This is what
# stops a `preflight`-only receipt from closing an attempt with an arbitrary
# code, and it keeps every legal cleanup shape explicit.
my %TERMINAL_CRITERIA = (
  INVALID_INPUT => [
    {fail => ['V1'], pass => []},
  ],
  INCONCLUSIVE_IDENTITY_SOURCE => [
    {fail => ['V2'], pass => ['V1']},
  ],
  INCONCLUSIVE_OWNERSHIP => [
    {fail => ['V3'], pass => ['V1', 'V2']},
  ],
  # Spec section 4 step 4: `V4 or V5 fails` stops with this terminal. The frozen
  # decision model's `cleanup-both-fail` case pins the inclusive reading: V4 and
  # V5 failing together is still this terminal, not a new one.
  INCONCLUSIVE_CLEANUP => [
    {fail => ['V4'], pass => ['V1', 'V2', 'V3']},
    {fail => ['V5'], pass => ['V1', 'V2', 'V3', 'V4']},
    {fail => ['V4', 'V5'], pass => ['V1', 'V2', 'V3']},
  ],
  # Section 4 step 5: reached only after every earlier gate passed.
  INVALID_EVIDENCE => [
    {fail => ['V6'], pass => ['V1', 'V2', 'V3', 'V4', 'V5']},
  ],
  # Section 4 step 6: reached only after every earlier gate passed.
  INVALID_EXPERIMENT => [
    {fail => ['V7'], pass => ['V1', 'V2', 'V3', 'V4', 'V5', 'V6']},
  ],
  # Section 4 step 9: every validity gate passed and no design was selected.
  INCONCLUSIVE_MECHANISM => [
    {fail => [], pass => ['V1', 'V2', 'V3', 'V4', 'V5', 'V6', 'V7']},
  ],
);

# Terminals that can only be reached by selecting a design. This broker owns no
# design facts: it has no `TERMINAL_CRITERIA` entry for these codes, and its
# stage facts forbid D1-D3 outright, so it cannot verify the evidence that
# would justify a selection.
#
# While the first slice has no design-receipt contract, these codes are refused
# for EVERY kind, not only rehearsals. A `decision` attempt that could stage
# `A1_SELECTED` and `finish` would write an authoritative selection on zero
# design evidence, which is strictly worse than a rehearsal doing so. When a
# decision receipt contract exists, this is where it is admitted; this broker
# does not invent that contract.
my %DESIGN_SELECTION_TERMINAL = map { $_ => 1 } qw(A1_SELECTED B_SELECTED);

# Does this criteria shape satisfy ANY legal alternative for this terminal?
sub terminal_criteria_alternative_ok {
  my ($shape, $criteria) = @_;
  for my $key (@{$shape->{fail}}) {
    return 0 unless $criteria->{$key} eq 'fail';
  }
  for my $key (@{$shape->{pass}}) {
    return 0 unless $criteria->{$key} eq 'pass';
  }
  my %claimed = map { $_ => 1 } (@{$shape->{fail}}, @{$shape->{pass}});
  for my $key (@CRITERIA_KEYS) {
    next if $claimed{$key};
    return 0 unless $criteria->{$key} eq 'not-run';
  }
  return 1;
}

# A terminal with a mapping must publish one of its alternatives exactly. A
# terminal with no mapping is not implemented by this broker and fails closed;
# merely appearing in the template vocabulary is not evidence that this slice
# knows how to validate the terminal's criteria.
sub validate_terminal_criteria {
  my ($code, $criteria, $kind) = @_;
  my $alternatives = $TERMINAL_CRITERIA{$code}
    or fail('terminal_criteria_not_implemented');
  for my $shape (@$alternatives) {
    return 1 if terminal_criteria_alternative_ok($shape, $criteria);
  }
  fail('terminal_criteria_not_legal');
}

# A design-selection terminal may not close any attempt yet: the broker holds
# no design evidence and has no decision-receipt contract, so accepting one
# would make an unverified selection authoritative. Fail closed by name.
sub validate_terminal_kind {
  my ($code, $kind) = @_;
  $DESIGN_SELECTION_TERMINAL{$code} or return;
  fail('terminal_design_selection_not_implemented');
}

my %STAGES = %{$TEMPLATE->{stages} // {}};
%STAGES or fail('template_stages');
my %FACT_WHITELIST = map { $_ => 1 } @{$TEMPLATE->{stage_fact_whitelist} // []};
%FACT_WHITELIST or fail('template_fact_whitelist');
my %FORBIDDEN_KEY = map { $_ => 1 } @{$TEMPLATE->{stage_forbidden_keys} // []};
my $STAGE_FACT_LIMIT = $TEMPLATE->{limits}{stage_fact_limit} // 0;
my $STAGE_STRING_LIMIT = $TEMPLATE->{limits}{stage_string_limit} // 0;
# Journal bounds. These are read from the template and enforced on every read
# and every write, so a journal cannot grow without bound before a live ordinal
# is ever reserved.
my $JOURNAL_MAX_ROWS = $TEMPLATE->{limits}{journal_max_rows_exclusive} // 0;
my $JOURNAL_MAX_ROW_BYTES =
  $TEMPLATE->{limits}{journal_max_row_bytes_exclusive} // 0;
my $JOURNAL_MAX_TOTAL_BYTES =
  $TEMPLATE->{limits}{journal_max_total_bytes_exclusive} // 0;
$JOURNAL_MAX_ROWS && $JOURNAL_MAX_ROW_BYTES && $JOURNAL_MAX_TOTAL_BYTES
  or fail('template_journal_limits');

# The typed fact domain the template uses in its stage shapes.
my %FACT_TYPE = (
  sha256 => sub { sha($_[0]) },
  git_sha => sub {
    defined $_[0] && !ref($_[0]) && $_[0] =~ /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/
  },
  safe_integer => sub {
    json_number($_[0]) && $_[0] >= 0 && $_[0] <= 9007199254740991
  },
  string => sub { printable_ascii($_[0]) },
  boolean => sub { is_bool($_[0]) },
);

# The v3 ledger admits exactly one rehearsal and one decision ordinal, and
# neither is reusable. This mirrors the v2 discipline with a distinct budget.
my %ORDINAL_KIND = (R1 => 'rehearsal', D1 => 'decision');


sub exact_keys {
  my ($v, $keys, $where) = @_;
  ref($v) eq 'HASH' or fail("${where}_not_object");
  my @got = sort keys %$v;
  my @want = sort @$keys;
  join("\0", @got) eq join("\0", @want) or fail("${where}_keys");
}

# Keep the JSON domain restricted: no floats, no huge integers, ASCII only.
sub restricted {
  my ($v, $where) = @_;
  if (!ref $v) {
    return if !defined $v;
    my $flags = B::svref_2object(\$v)->FLAGS;
    if ($flags & (B::SVp_IOK() | B::SVp_NOK())) {
      $v == int($v) && abs($v) <= 9007199254740991
        or fail("${where}_unsafe_number");
      return;
    }
    ($flags & B::SVp_POK()) or fail("${where}_unsupported_scalar");
    $v =~ /^[\x20-\x7e]*$/ or fail("${where}_non_ascii");
    return;
  }
  return if is_bool($v);
  if (ref($v) eq 'ARRAY') { restricted($_, $where) for @$v; return }
  ref($v) eq 'HASH' or fail("${where}_unsupported_type");
  for my $k (keys %$v) {
    $k =~ /^[\x20-\x7e]+$/ or fail("${where}_non_ascii_key");
    restricted($v->{$k}, $where);
  }
}

sub canonical { my ($v) = @_; restricted($v, 'json'); return $json->encode($v) }

sub ensure_tree {
  my @parts = File::Spec->splitdir($state_root);
  my $acc = File::Spec->rootdir();
  for my $p (@parts) {
    next if $p eq '' || $p eq File::Spec->rootdir();
    $acc = File::Spec->catdir($acc, $p);
    if (!lstat($acc)) {
      mkdir($acc, 0700) or fail('state_mkdir_failed');
    }
    -d $acc && !-l $acc or fail('state_root_not_plain_directory');
  }
  # The root itself must be a plain directory too, and it must be the one we
  # resolved rather than a symlink standing in for it.
  -d $state_root && !-l $state_root or fail('state_root_not_plain_directory');
  # The stage journal lives in its own subdirectory. Create it under the same
  # plain-directory discipline so a symlink cannot redirect state writes.
  my $journal_dir = "$state_root/stage-journal";
  if (!lstat($journal_dir)) {
    mkdir($journal_dir, 0700) or fail('state_mkdir_failed');
  }
  -d $journal_dir && !-l $journal_dir or fail('state_root_not_plain_directory');
}

# Atomic publication with an enforced read-back.
#
# The sequence is: write a uniquely named temporary, `fsync` it so the bytes
# reach stable storage, `rename` it over the destination, then re-read the
# destination and require byte equality. This is process-crash evidence: after
# a crash the destination is either the old or the new complete bytes, never a
# torn write.
#
# Deliberate boundary: the parent directory is NOT fsynced. On a power loss the
# rename itself may not have reached stable storage, so this is not a
# power-loss durability claim. The live runner owns that stronger guarantee.
sub atomic_replace {
  my ($path, $bytes) = @_;
  if (lstat($path)) {
    -f _ && !-l _ or fail('atomic_destination_not_plain');
    my @old = stat($path);
    @old && $old[3] == 1 && $old[4] == $< or fail('atomic_destination_identity_invalid');
  }
  my $tmp = "$path.tmp.$$\." . int(rand(1_000_000));
  sysopen(my $fh, $tmp, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0600)
    or fail('atomic_temp_create_failed');
  binmode($fh); print {$fh} $bytes or fail('atomic_temp_write_failed');
  # Flush to stable storage before the rename, so the published bytes are
  # complete rather than merely buffered.
  $fh->flush or fail('atomic_temp_flush_failed');
  $fh->sync or fail('atomic_temp_sync_failed');
  close($fh) or fail('atomic_temp_close_failed');
  rename($tmp, $path) or fail('atomic_rename_failed');
  my $readback = slurp_raw($path);
  $readback eq $bytes or fail('atomic_readback_mismatch');
}

sub with_lock {
  my ($body) = @_;
  ensure_tree();
  my $lock_path = "$state_root/state.lock";
  sysopen(my $lock, $lock_path, O_RDWR | O_CREAT | O_NOFOLLOW, 0600)
    or fail('state_lock_open_failed');
  my @st = stat($lock);
  @st && (($st[2] & 0170000) == 0100000) && $st[3] == 1 && $st[4] == $<
    or fail('state_lock_identity_invalid');
  flock($lock, LOCK_EX | LOCK_NB) or fail('state_lock_busy');
  my $out = eval { $body->() };
  my $err = $@;
  close($lock) or fail('state_lock_close_failed');
  die $err if $err;
  return $out;
}

# A read-only reader. `inspect` must not create a state root, a stage-journal
# directory or a lock file, so it opens an existing lock without O_CREAT and
# refuses to manufacture anything. When no root exists it reports the typed
# `state_absent` condition instead of bringing one into being.
sub with_read_lock {
  my ($body) = @_;
  (-d $state_root && !-l $state_root) or fail('state_absent');
  my $lock_path = "$state_root/state.lock";
  # No O_CREAT: a reader never brings the lock into existence.
  sysopen(my $lock, $lock_path, O_RDWR | O_NOFOLLOW)
    or fail('state_lock_open_failed');
  my @st = stat($lock);
  @st && (($st[2] & 0170000) == 0100000) && $st[3] == 1 && $st[4] == $<
    or fail('state_lock_identity_invalid');
  flock($lock, LOCK_EX | LOCK_NB) or fail('state_lock_busy');
  my $out = eval { $body->() };
  my $err = $@;
  close($lock) or fail('state_lock_close_failed');
  die $err if $err;
  return $out;
}

sub ledger_path { "$state_root/attempt-ledger.jsonl" }

# The per-ordinal artifact stem. `stage` and `finish` both need it, and the
# authoritative-artifact re-validation below needs it for every finished row,
# so it is named once rather than repeated inline.
sub attempt_stem {
  my ($ordinal) = @_;
  return $ordinal eq 'D1' ? 'decision-1' : 'rehearsal-1';
}

sub receipt_path { "$state_root/" . attempt_stem($_[0]) . "-receipt.json" }
sub journal_path { "$state_root/stage-journal/" . attempt_stem($_[0]) . ".jsonl" }

sub read_lines {
  my ($path) = @_;
  return [] unless lstat($path);
  -f _ && !-l _ or fail('state_file_not_plain');
  my $b = slurp_raw($path);
  return [] if $b eq '';
  $b =~ /\n\z/ or fail('state_file_unterminated');
  my @rows;
  for my $line (split /\n/, $b) {
    next if $line eq '';
    my $v = eval { $json->decode($line) }; $@ and fail('state_json_invalid');
    canonical($v) eq $line or fail('state_json_not_canonical');
    push @rows, $v;
  }
  return \@rows;
}

sub write_lines {
  my ($path, $rows) = @_;
  atomic_replace($path, join('', map { canonical($_) . "\n" } @$rows));
}

sub publish_ledger_rows {
  my ($rows) = @_;
  validate_ledger($rows, 0);
  validate_authoritative_artifacts($rows);
  write_lines(ledger_path(), $rows);
  my $readback = read_lines(ledger_path());
  validate_ledger($readback, 0);
  validate_authoritative_artifacts($readback);
  @$readback == @$rows
    && canonical($readback->[-1]) eq canonical($rows->[-1])
    or fail('ledger_readback_mismatch');
}

sub sha { $_[0] =~ /^[0-9a-f]{64}$/ }

# Read the source revision from an independent, readable source rather than
# accepting the caller's word for it. The repository path is resolved
# no-follow, and a failure to read is a typed refusal.
sub read_git_head {
  my ($repo) = @_;
  (-d $repo && !-l $repo) or fail('frozen_input_repository_invalid');
  my $head_file = File::Spec->catfile($repo, '.git', 'HEAD');
  -f $head_file or fail('frozen_input_repository_not_git');
  my $head = slurp_raw($head_file);
  $head =~ s/\s+\z//;
  # A symbolic HEAD points at a ref, whose object name is the revision.
  if ($head =~ m{^ref:\s+(\S+)$}) {
    my $ref = $1;
    $ref =~ m{^refs/} or fail('frozen_input_repository_ref');
    $ref =~ m{\.\.} and fail('frozen_input_repository_ref');
    my $ref_file = File::Spec->catfile($repo, '.git', split m{/}, $ref);
    -f $ref_file or fail('frozen_input_repository_unreadable');
    my $value = slurp_raw($ref_file);
    $value =~ s/\s+\z//;
    $value =~ /^[0-9a-f]{40}$/ or fail('frozen_input_repository_revision');
    return $value;
  }
  $head =~ /^[0-9a-f]{40}$/ or fail('frozen_input_repository_revision');
  return $head;
}

# Hash one real file's bytes. No-follow semantics throughout: a symlink is
# refused rather than followed, so a manifest cannot point at one thing and
# have another read.
sub hash_real_file {
  my ($abs) = @_;
  (-f $abs && !-l $abs) or fail('frozen_input_not_plain_file');
  sysopen(my $fh, $abs, O_RDONLY | O_NOFOLLOW) or fail('frozen_input_open_failed');
  binmode($fh);
  my $d = Digest::SHA->new(256);
  $d->addfile($fh);
  close($fh) or fail('frozen_input_close_failed');
  return $d->hexdigest;
}

# Resolve a manifest-declared path inside the manifest's root, refusing any
# escape. The path must be relative, must not be absolute, must contain no `..`
# component, and the resolved absolute path must still sit under the root. This
# is what stops a manifest from reaching outside its own tree.
sub resolve_frozen_path {
  my ($root, $rel) = @_;
  defined $rel && !ref($rel) or fail('frozen_inputs_path');
  length($rel) or fail('frozen_inputs_path');
  $rel =~ m{^/} and fail('frozen_input_path_absolute');
  $rel =~ m{^~} and fail('frozen_input_path_absolute');
  my @parts = split m{/}, $rel;
  for my $part (@parts) {
    ($part eq '' || $part eq '.') and fail('frozen_input_path_component');
    $part eq '..' and fail('frozen_input_path_escape');
  }
  # Verify with the real ancestor: the root itself must exist and be a plain
  # directory, and the joined path must still begin with it.
  (-d $root && !-l $root) or fail('frozen_input_root_invalid');
  my $abs = File::Spec->catfile($root, @parts);
  my $real_root = Cwd::realpath($root);
  defined $real_root or fail('frozen_input_root_invalid');
  my $real_abs = Cwd::realpath($abs);
  # The file must exist for realpath to resolve it; a dangling entry is refused
  # rather than silently counted.
  defined $real_abs or fail('frozen_input_missing');
  $real_abs eq $real_root and fail('frozen_input_path_not_file');
  index($real_abs, "$real_root/") == 0 or fail('frozen_input_path_escape');
  return $real_abs;
}

# The frozen-input digest, recomputed from the real bytes of the files the
# manifest names.
#
# The caller's manifest is a claim, not evidence. For every declared path the
# broker opens and hashes the actual file under the manifest's own root, and
# requires the declared digest to equal what it read. Faking `files[].sha256`
# therefore fails: the recomputed value would disagree. The digest is then
# derived from the recomputed hashes, so it cannot be forged by editing the
# manifest either.
sub frozen_input_digest {
  my ($manifest) = @_;
  my $root = $manifest->{root};
  printable_ascii($root) or fail('frozen_inputs_root');
  my $s = Digest::SHA->new(256);
  # Domain separation uses the declared input domain exactly once. The manifest
  # must name that same domain, so a manifest built for another experiment
  # cannot be replayed here.
  $s->add($INPUT_DOMAIN . "\0");
  for my $entry (@{$manifest->{files}}) {
    ref($entry) eq 'HASH' or fail('frozen_inputs_entry');
    exact_keys($entry, [qw(path sha256)], 'frozen_inputs_entry');
    printable_ascii($entry->{path}) or fail('frozen_inputs_path');
    sha($entry->{sha256}) or fail('frozen_inputs_entry_sha');
    my $name = $entry->{path};
    my $abs = resolve_frozen_path($root, $name);
    my $actual = hash_real_file($abs);
    # The declared digest must match the bytes actually on disk.
    $actual eq $entry->{sha256} or fail('frozen_input_content_changed');
    $s->add(pack('Q<', length($name)), $name,
      pack('Q<', length($actual)), $actual);
  }
  return $s->hexdigest;
}
sub json_string {
  return 0 if !defined($_[0]) || ref($_[0]);
  my $flags = B::svref_2object(\$_[0])->FLAGS;
  return ($flags & B::SVp_POK()) ? 1 : 0;
}
sub printable_ascii { json_string($_[0]) && $_[0] =~ /^[\x20-\x7e]+$/ }
sub json_number {
  return 0 if !defined($_[0]) || ref($_[0]);
  my $flags = B::svref_2object(\$_[0])->FLAGS;
  return ($flags & (B::SVp_IOK() | B::SVp_NOK())) ? 1 : 0;
}

# The authoritative ledger state machine. Every transition keeps its identity
# fields, an ordinal is never reused, and a residual reservation is refused
# unless the caller explicitly asks to tolerate it (audit path).
sub validate_ledger {
  my ($rows, $reject_residual) = @_;
  my %attempt;
  for my $r (@$rows) {
    exact_keys($r, [qw(schema experiment kind ordinal run_id source_sha
      input_digest status receipt_sha256 terminal_code)], 'ledger');
    $r->{schema} eq $LEDGER_SCHEMA && $r->{experiment} eq $EXPERIMENT
      or fail('ledger_identity');
    (!$r->{kind} || !$r->{ordinal}) and fail('ledger_kind');
    my $expected = $ORDINAL_KIND{$r->{ordinal}}
      or fail('ledger_ordinal');
    $r->{kind} eq $expected or fail('ledger_kind_ordinal');
    $r->{run_id} =~ /^[0-9a-f]{32}$/ && sha($r->{input_digest})
      or fail('ledger_digest');
    $r->{source_sha} =~ /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/
      or fail('ledger_source_sha');
    my $key = "$r->{kind}:$r->{ordinal}";
    if ($r->{status} eq 'reserved') {
      !defined($r->{receipt_sha256}) && !defined($r->{terminal_code})
        or fail('ledger_reserved_terminal');
      !exists($attempt{$key}) or fail('ledger_ordinal_reused');
      $attempt{$key} = {row => $r, status => 'reserved'};
    } elsif ($r->{status} eq 'finished') {
      my $a = $attempt{$key};
      $a && $a->{status} eq 'reserved' or fail('ledger_finish_without_reserve');
      for my $field (qw(schema experiment kind ordinal run_id source_sha input_digest)) {
        $r->{$field} eq $a->{row}{$field} or fail('ledger_transition_identity');
      }
      $TERMINAL{$r->{terminal_code} // ''} or fail('ledger_finish_code');
      # Every finished row must bind a real receipt digest. There is no
      # exception: a persistence failure leaves the attempt `reserved` for
      # independent audit and never publishes a finished row, so a finished
      # row without a digest can only be corruption or forgery.
      sha($r->{receipt_sha256} // '') or fail('ledger_finish_receipt');
      $a->{status} = 'finished'; $a->{terminal_row} = $r;
    } elsif ($r->{status} eq 'abandoned') {
      my $a = $attempt{$key};
      $a && $a->{status} eq 'reserved' or fail('ledger_abandoned_without_reserve');
      for my $field (qw(schema experiment kind ordinal run_id source_sha input_digest)) {
        $r->{$field} eq $a->{row}{$field} or fail('ledger_transition_identity');
      }
      !defined($r->{receipt_sha256})
        && $r->{terminal_code} eq 'attempt_abandoned_after_independent_audit'
        or fail('ledger_abandoned');
      $a->{status} = 'abandoned'; $a->{terminal_row} = $r;
    } else { fail('ledger_status') }
  }
  if ($reject_residual) {
    for my $key (keys %attempt) {
      $attempt{$key}{status} ne 'reserved' or fail('residual_reservation');
    }
  }
  return \%attempt;
}

sub active_entry {
  my ($rows, $kind, $ordinal, $run_id) = @_;
  validate_ledger($rows, 0);
  # A caller acting on this ledger must not treat a forged finished row as
  # authoritative while it locates its own reservation.
  validate_authoritative_artifacts($rows);
  my @m = grep {
    $_->{kind} eq $kind && $_->{ordinal} eq $ordinal && $_->{run_id} eq $run_id
  } @$rows;
  @m == 1 && $m[0]{status} eq 'reserved' or fail('attempt_not_active');
  return $m[0];
}

# A versioned, chained stage journal. Every row carries the running digest, so a
# deleted or reordered row is detectable, and the final digest binds the receipt.
sub journal_digest {
  my ($rows) = @_;
  # Row count is bounded before anything is decoded, so an oversized journal
  # cannot be walked at all.
  @$rows < $JOURNAL_MAX_ROWS or fail('journal_row_count_limit');
  my $prev = $ZERO;
  my $seq = 0;
  my $total = 0;
  for my $r (@$rows) {
    exact_keys($r, [qw(schema experiment kind ordinal run_id seq prev_sha256
      stage producer deadline_ms elapsed_ms code facts)], 'journal');
    $r->{schema} eq $JOURNAL_SCHEMA && $r->{experiment} eq $EXPERIMENT
      or fail('journal_identity');
    json_number($r->{seq}) && $r->{seq} == ++$seq && $r->{prev_sha256} eq $prev
      or fail('journal_chain');
    json_number($r->{deadline_ms}) && json_number($r->{elapsed_ms})
      && $r->{deadline_ms} >= 0 && $r->{elapsed_ms} >= 0
      && $r->{deadline_ms} <= 9007199254740991
      && $r->{elapsed_ms} <= 9007199254740991
      or fail('journal_integer');
    printable_ascii($r->{producer}) && printable_ascii($r->{code})
      or fail('journal_text');
    # Producer and code are bounded by the same string limit as stage facts.
    # Without this a single row could be inflated past the row-byte bound.
    $STAGE_STRING_LIMIT
      && (length($r->{producer}) > $STAGE_STRING_LIMIT
          || length($r->{code}) > $STAGE_STRING_LIMIT)
      and fail('journal_text_limit');
    ref($r->{facts}) eq 'HASH' or fail('journal_facts');
    validate_stage_facts($r->{stage}, $r->{facts});
    my $encoded = canonical($r);
    # Single-row and cumulative byte bounds, both exclusive, both enforced on
    # the encoded form that is actually persisted.
    length($encoded) < $JOURNAL_MAX_ROW_BYTES or fail('journal_row_limit');
    $total += length($encoded) + 1;
    $total < $JOURNAL_MAX_TOTAL_BYTES or fail('journal_total_limit');
    $prev = sha256_hex($encoded);
  }
  return ($seq, $prev);
}

# A stage must be one the specification names, and its facts must satisfy that
# stage's typed required/optional shape, stay inside the whitelist, avoid every
# forbidden key, and respect the size limits. This is what keeps a stage row
# from smuggling a design fact, an argv, an environment value or a selector
# into durable state.
sub validate_stage_facts {
  my ($stage, $facts) = @_;
  my $shape = $STAGES{$stage} or fail('stage_unknown');
  ref($facts) eq 'HASH' or fail('stage_facts');
  $STAGE_FACT_LIMIT && scalar(keys %$facts) <= $STAGE_FACT_LIMIT
    or fail('stage_fact_count');
  my %allowed = map { $_ => 1 }
    (keys %{$shape->{required} // {}}, keys %{$shape->{optional} // {}});
  # A design fact is refused first and by its own name, so the diagnostic says
  # what is actually wrong rather than "not whitelisted". A design verdict is
  # the one thing no row in this experiment may ever publish.
  validate_no_design_fact($facts, 'stage_facts');
  for my $key (sort keys %$facts) {
    $FORBIDDEN_KEY{$key} and fail('stage_forbidden_key');
    $FACT_WHITELIST{$key} or fail('stage_fact_not_whitelisted');
    $allowed{$key} or fail('stage_fact_not_in_stage_shape');
  }
  for my $key (sort keys %{$shape->{required} // {}}) {
    exists $facts->{$key} or fail('stage_required_fact_missing');
  }
  my %types = (%{$shape->{required} // {}}, %{$shape->{optional} // {}});
  for my $key (sort keys %types) {
    next unless exists $facts->{$key};
    my $kind = $types{$key};
    my $check = $FACT_TYPE{$kind} or fail('stage_fact_type_unknown');
    $check->($facts->{$key}) or fail('stage_fact_type');
    if ($kind eq 'string' && $STAGE_STRING_LIMIT) {
      length($facts->{$key}) <= $STAGE_STRING_LIMIT or fail('stage_string_limit');
    }
  }
}

# Validate a CANDIDATE journal, including a row that has been appended but not
# yet published. Every template bound is enforced here:
#
#   rows  < journal_max_rows_exclusive
#   bytes < journal_max_row_bytes_exclusive   (per encoded row)
#   bytes < journal_max_total_bytes_exclusive (cumulative, incl. separators)
#
# `journal_digest` performs all three, so this is deliberately a named wrapper
# rather than an incidental second call: the stage path must invoke it AFTER
# appending and BEFORE `atomic_replace`, and the name makes that requirement
# legible at the call site. Omitting it would let an over-limit row reach the
# authoritative journal.
sub validate_candidate_journal {
  my ($rows) = @_;
  journal_digest($rows);
  return;
}

# Re-derive every `finished` transition from the artifacts on disk.
#
# `validate_ledger` is a pure structural check over the rows it is handed: it
# confirms the shape and vocabulary of a finished row but cannot see the
# receipt or journal that are supposed to justify it. On its own that lets a
# forged row -- a mapped terminal code plus a fabricated 64-hex digest, with no
# receipt on disk at all -- load as an authoritative completion. This routine
# closes that gap by repeating, for each finished row, the same re-derivation
# `finish` performs before it publishes one:
#
#   1. the receipt must exist, be a plain file, and hash to the digest the
#      ledger records;
#   2. the receipt must validate against the reserved row it belongs to;
#   3. the journal digest and final sequence must be recomputed from the
#      journal's actual bytes and match what the receipt claims;
#   4. the journal's last row must be the terminal stage, and its code, facts,
#      criteria and the ledger's `terminal_code` must all agree;
#   5. the terminal code must survive the kind and criteria gates.
#
# It is deliberately separate from `validate_ledger` so that the structural
# checker stays a pure function of its argument and does not read files. Every
# public path that treats a ledger as authoritative calls it.
sub validate_authoritative_artifacts {
  my ($rows) = @_;
  my %reserved;
  for my $r (@$rows) {
    # Remember each reservation as it is seen, so a finished row can only bind
    # to a reservation that precedes it.
    if ($r->{status} eq 'reserved') {
      $reserved{"$r->{kind}:$r->{ordinal}"} = $r;
      next;
    }
    next unless $r->{status} eq 'finished';
    my $key = "$r->{kind}:$r->{ordinal}";

    # The finished row must bind to the reservation it claims to close.
    my $entry = $reserved{$key} or fail('authoritative_finish_without_reserve');
    for my $field (qw(run_id source_sha input_digest)) {
      $r->{$field} eq $entry->{$field}
        or fail('authoritative_finish_identity');
    }

    # (1) The receipt must exist on disk and hash to the recorded digest.
    my $rp = receipt_path($r->{ordinal});
    (-f $rp && !-l $rp) or fail('authoritative_receipt_missing');
    my $actual_sha = sha256_hex(slurp_raw($rp));
    $actual_sha eq ($r->{receipt_sha256} // '')
      or fail('authoritative_receipt_digest');

    # (2) The receipt must validate against the reserved row.
    my $receipt = decode_request_file($rp);
    validate_receipt($receipt, $entry);

    # (3) The journal's real bytes must reproduce the receipt's claim.
    my $jp = journal_path($r->{ordinal});
    (-f $jp && !-l $jp) or fail('authoritative_journal_missing');
    my $journal = read_lines($jp);
    my ($seq, $digest) = journal_digest($journal);
    $receipt->{journal_final_seq} == $seq
      or fail('authoritative_journal_seq');
    $receipt->{journal_final_sha256} eq $digest
      or fail('authoritative_journal_digest');

    # (4) The terminal row must be the journal's last, and every field the
    #     ledger and receipt carry about it must agree.
    @$journal or fail('authoritative_journal_empty');
    $journal->[-1]{stage} eq 'terminal'
      or fail('authoritative_without_terminal_stage');
    my $terminal = $journal->[-1];
    my $code = $receipt->{code};
    $terminal->{code} eq $code or fail('authoritative_terminal_code');
    $code eq ($r->{terminal_code} // '')
      or fail('authoritative_ledger_terminal_code');
    ($terminal->{facts}{terminal_code} // '') eq $code
      or fail('authoritative_terminal_code_mismatch');
    canonical($terminal->{facts}) eq canonical($receipt->{facts})
      or fail('authoritative_terminal_facts');
    $terminal->{kind} eq $entry->{kind}
      or fail('authoritative_terminal_identity');
    $terminal->{ordinal} eq $entry->{ordinal}
      or fail('authoritative_terminal_identity');

    # (5) The terminal code must survive the kind and criteria gates. This is
    #     the same pair `stage` and `finish` apply, so a code this slice
    #     refuses to write cannot become authoritative merely by being loaded.
    #     The criteria live on the receipt: a journal row deliberately carries
    #     no criteria field, so the receipt is the only source for them here.
    $TERMINAL{$code} or fail('authoritative_terminal_code_unknown');
    validate_terminal_kind($code, $entry->{kind});
    validate_terminal_criteria($code, $receipt->{criteria}, $entry->{kind});
  }
  return 1;
}

sub receipt_for {
  my ($entry, $row, $seq, $digest, $criteria) = @_;
  return {
    schema => $RECEIPT_SCHEMA, experiment => $EXPERIMENT,
    kind => $entry->{kind}, ordinal => $entry->{ordinal},
    run_id => $entry->{run_id}, source_sha => $entry->{source_sha},
    input_digest => $entry->{input_digest},
    journal_final_seq => $seq, journal_final_sha256 => $digest,
    last_completed_stage => $row->{stage}, deadline_ms => $row->{deadline_ms},
    code => $row->{code}, facts => $row->{facts}, criteria => $criteria
  };
}

sub validate_criteria {
  my ($criteria, $kind) = @_;
  # The criteria are exactly the template's criteria keys (V1-V7).
  exact_keys($criteria, \@CRITERIA_KEYS, 'criteria');
  for my $key (@CRITERIA_KEYS) {
    !ref($criteria->{$key}) && $CRITERION_VALUE{$criteria->{$key} // ''}
      or fail("criteria_value_$key");
  }
  # A rehearsal measures V1-V7, so it may legitimately record a validity
  # failure or pass. What it may never carry is a design fact. `exact_keys`
  # already refuses a D-key inside criteria, so the real guard is the
  # forbidden/whitelist check on the facts and the design-key scan below.
}

# No D-key may appear as a fact in any row the broker publishes. This is the
# enforcement point the criteria check cannot reach, because a design verdict
# would be smuggled as a fact rather than as a criterion.
sub validate_no_design_fact {
  my ($facts, $where) = @_;
  ref($facts) eq 'HASH' or return;
  for my $key (@DESIGN_KEYS) {
    exists $facts->{$key} and fail('design_fact_present');
  }
}

sub validate_receipt {
  my ($receipt, $entry) = @_;
  exact_keys($receipt, [qw(schema experiment kind ordinal run_id source_sha
    input_digest journal_final_seq journal_final_sha256 last_completed_stage
    deadline_ms code facts criteria)], 'receipt');
  $receipt->{schema} eq $RECEIPT_SCHEMA && $receipt->{experiment} eq $EXPERIMENT
    or fail('receipt_identity');
  for my $field (qw(kind ordinal run_id source_sha input_digest)) {
    $receipt->{$field} eq $entry->{$field} or fail('receipt_attempt_identity');
  }
  json_number($receipt->{journal_final_seq}) && $receipt->{journal_final_seq} >= 1
    && sha($receipt->{journal_final_sha256})
    or fail('receipt_journal_identity');
  json_number($receipt->{deadline_ms}) && $receipt->{deadline_ms} >= 0
    && $receipt->{deadline_ms} <= 9007199254740991
    or fail('receipt_deadline');
  printable_ascii($receipt->{code}) or fail('receipt_code');
  ref($receipt->{facts}) eq 'HASH' or fail('receipt_facts');
  validate_criteria($receipt->{criteria}, $receipt->{kind});
}

# The fixed stdout record for a persistence failure: exactly seven keys, no
# design fact, and the caller must exit nonzero.
sub persistence_stdout_record {
  my ($entry) = @_;
  my $record = {
    experiment => $EXPERIMENT,
    kind => $entry->{kind},
    ordinal => $entry->{ordinal},
    run_id => $entry->{run_id},
    source_sha => $entry->{source_sha},
    input_digest => $entry->{input_digest},
    code => $PERSISTENCE_CODE
  };
  exact_keys($record, \@PERSISTENCE_STDOUT_KEYS, 'persistence_stdout');
  return $record;
}

sub decode_request_file {
  my ($path) = @_;
  (-f $path && !-l $path) or fail('request_not_plain');
  my $bytes = slurp_raw($path);
  my $v = eval { $json->decode($bytes) };
  $@ and fail('request_json_invalid');
  return $v;
}



sub emit { print canonical($_[0]), "\n" }
sub emit_persistence_failure {
  emit(persistence_stdout_record($_[0]));
  exit 1;
}

# ---------------------------------------------------------------------------
# Operations
# ---------------------------------------------------------------------------

$state_root // fail('state_root_missing');

# `inspect` is genuinely read-only: it acquires an existing lock without
# creating one, and reports `state_absent` rather than manufacturing a state
# root. A caller may therefore inspect a host that has never run an attempt.
if ($operation eq 'inspect') {
  my $out = with_read_lock(sub {
    my $rows = read_lines(ledger_path());
    validate_ledger($rows, 0);
    # `inspect` is the authoritative read. Re-derive every finished transition
    # from its receipt and journal so a forged row cannot be presented as a
    # legitimate completion.
    validate_authoritative_artifacts($rows);
    return {
      schema => 'agenterm.profile-binding-exact-process-state/v1',
      experiment => $EXPERIMENT, ledger => $rows
    };
  });
  emit($out); exit 0;
}

if ($operation eq 'reserve') {
  my ($kind, $ordinal, $run_id, $source_sha, $input_digest, $inputs_path) = @args;
  $kind && $kind =~ /^(?:rehearsal|decision)$/ or fail('reserve_kind');
  $ordinal && $ORDINAL_KIND{$ordinal} && $ORDINAL_KIND{$ordinal} eq $kind
    or fail('reserve_ordinal');
  $run_id && $run_id =~ /^[0-9a-f]{32}$/ or fail('reserve_run_id');
  $source_sha && $source_sha =~ /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/
    or fail('reserve_source_sha');
  $input_digest && sha($input_digest) or fail('reserve_input_digest');
  my $out = with_lock(sub {
    my $rows = read_lines(ledger_path());
    # Frozen-input recheck. When the caller supplies the frozen input manifest,
    # the digest is recomputed by reading the real bytes of every file the
    # manifest names, under the lock, and the pinned source revision is read
    # from an independent source. A claim is never trusted because it is
    # well-formed. A caller that omits the manifest gets no recheck, and the
    # record below says so explicitly.
    my $recheck = 'not-supplied';
    if (defined $inputs_path && $inputs_path ne '') {
      my $manifest = decode_request_file($inputs_path);
      exact_keys($manifest,
        [qw(experiment domain root repository files source_revision input_digest)],
        'frozen_inputs');
      $manifest->{experiment} eq $EXPERIMENT or fail('frozen_inputs_identity');
      $manifest->{domain} eq $INPUT_DOMAIN or fail('frozen_inputs_domain');
      ref($manifest->{files}) eq 'ARRAY' && @{$manifest->{files}}
        or fail('frozen_inputs_files');
      # Diagnose the source revision first: if the caller pinned a different
      # revision, that is the most specific thing that changed.
      $manifest->{source_revision} eq $source_sha
        or fail('reserve_source_revision_changed');
      # Independently read the source revision when the caller declared a
      # repository, so the pinned revision is not merely self-asserted. A
      # repository with no git available is a typed refusal, not a silent pass.
      if (defined $manifest->{repository} && $manifest->{repository} ne '') {
        my $head = read_git_head($manifest->{repository});
        $head eq $source_sha or fail('reserve_source_revision_changed');
      }
      my $computed = frozen_input_digest($manifest);
      $computed eq $input_digest or fail('reserve_frozen_input_changed');
      $manifest->{input_digest} eq $input_digest
        or fail('frozen_inputs_digest_disagrees');
      $recheck = 'verified';
    }
    # An ordinal is never reused. Check this before rejecting a residual
    # reservation, so the caller learns the precise reason.
    my %used = map { $_->{ordinal} => 1 }
      grep { $_->{status} eq 'reserved' } @$rows;
    !$used{$ordinal} or fail('reserve_ordinal_reused');
    validate_ledger($rows, 1);
    my $r = {
      schema => $LEDGER_SCHEMA, experiment => $EXPERIMENT, kind => $kind,
      ordinal => $ordinal, run_id => $run_id, source_sha => $source_sha,
      input_digest => $input_digest, status => 'reserved',
      receipt_sha256 => undef, terminal_code => undef
    };
    push @$rows, $r;
    publish_ledger_rows($rows);
    return {%$r, frozen_input_recheck => $recheck};
  });
  emit($out); exit 0;
}

if ($operation eq 'abandon') {
  my ($kind, $ordinal, $run_id) = @args;
  my $out = with_lock(sub {
    my $ledger = read_lines(ledger_path());
    my $entry = active_entry($ledger, $kind, $ordinal, $run_id);
    my $abandoned = {%$entry, status => 'abandoned', receipt_sha256 => undef,
      terminal_code => 'attempt_abandoned_after_independent_audit'};
    push @$ledger, $abandoned;
    publish_ledger_rows($ledger);
    return $abandoned;
  });
  emit($out); exit 0;
}

# Publish one stage row and its receipt with an enforced read-back of both.
# Publication is atomic and fsync'd (see `atomic_replace`); it is process-crash
# evidence, not a power-loss durability claim. If the journal or the receipt cannot be published and read back equal,
# the attempt ends as a persistence failure: a fixed seven-key stdout record,
# no design fact, nonzero exit, and the reservation left authoritative.
if ($operation eq 'stage') {
  my ($kind, $ordinal, $run_id, $request_path) = @args;
  my $req = decode_request_file($request_path);
  exact_keys($req, [qw(stage producer deadline_ms elapsed_ms code facts criteria)],
    'stage_request');
  my $entry;
  my $published = eval {
    with_lock(sub {
      my $ledger = read_lines(ledger_path());
      $entry = active_entry($ledger, $kind, $ordinal, $run_id);
      my $stem = $ordinal eq 'D1' ? 'decision-1' : 'rehearsal-1';
      my $jp = "$state_root/stage-journal/$stem.jsonl";
      my $rp = "$state_root/$stem-receipt.json";
      my $rows = read_lines($jp);
      my ($seq, $prev) = journal_digest($rows);
      validate_criteria($req->{criteria}, $kind);
      validate_stage_facts($req->{stage}, $req->{facts});
      # A terminal stage is the row that closes the attempt, so its code must
      # be one the specification admits and the row's own code must agree with
      # it. This is what stops a `PREFLIGHT_OK` row from being used as a close.
      if ($req->{stage} eq 'terminal') {
        $TERMINAL{$req->{code} // ''} or fail('stage_terminal_code');
        $req->{facts}{terminal_code} eq $req->{code}
          or fail('stage_terminal_code_mismatch');
        validate_terminal_kind($req->{code}, $kind);
        validate_terminal_criteria($req->{code}, $req->{criteria}, $kind);
      } else {
        $TERMINAL{$req->{code} // ''} and fail('stage_non_terminal_code');
      }
      my $row = {
        schema => $JOURNAL_SCHEMA, experiment => $EXPERIMENT,
        kind => $kind, ordinal => $ordinal, run_id => $run_id,
        seq => $seq + 1, prev_sha256 => $prev, stage => $req->{stage},
        producer => $req->{producer}, deadline_ms => $req->{deadline_ms},
        elapsed_ms => $req->{elapsed_ms}, code => $req->{code},
        facts => $req->{facts}
      };
      push @$rows, $row;
      # Validate the CANDIDATE journal -- including the row just appended --
      # before publishing it. All three template bounds (rows, single-row
      # bytes, total bytes) are enforced here, so an over-limit row is refused
      # rather than written and only noticed on the next read. The call is
      # explicit so the ordering cannot be lost in a later edit.
      validate_candidate_journal($rows);
      my $all = join('', map { canonical($_) . "\n" } @$rows);
      atomic_replace($jp, $all);
      my ($n, $dig) = journal_digest(read_lines($jp));
      my $receipt = receipt_for($entry, $row, $n, $dig, $req->{criteria});
      validate_receipt($receipt, $entry);
      atomic_replace($rp, canonical($receipt));
      return {accepted => JSON::PP::true,
        receipt_sha256 => sha256_hex(slurp_raw($rp))};
    });
  };
  my $error = $@;
  if ($error) {
    chomp(my $text = $error);
    # A failure to publish or read back persisted state is exactly the
    # persistence failure described in spec section 1.7: a fixed seven-key
    # stdout record, no design fact, nonzero exit, and the reservation left
    # authoritative. Any other failure is a typed refusal that changes no
    # state, and is reported as such.
    if ($text =~ /^(?:atomic_|state_file_|state_mkdir_|read_failed|read_close_failed)/) {
      emit_persistence_failure($entry // {
        kind => $kind, ordinal => $ordinal, run_id => $run_id,
        source_sha => ('0' x 40), input_digest => ('0' x 64)});
    }
    fail($text);
  }
  emit($published); exit 0;
}

# Close a reserved attempt against its published receipt and journal.
#
# This is the only path from `reserved` to `finished`. It re-reads both published
# artifacts, re-derives the journal's final digest from the rows on disk,
# requires the receipt to bind that exact digest, requires the receipt to be
# internally consistent, and requires the receipt's criteria to agree with its
# terminal code. A `finished` row therefore always points at a receipt that was
# independently re-verified at close time; a caller cannot assert one.
if ($operation eq 'finish') {
  my ($kind, $ordinal, $run_id, $receipt_sha) = @args;
  $receipt_sha && sha($receipt_sha) or fail('finish_receipt_sha');
  my $out = with_lock(sub {
    my $ledger = read_lines(ledger_path());
    my $entry = active_entry($ledger, $kind, $ordinal, $run_id);
    my $stem = $ordinal eq 'D1' ? 'decision-1' : 'rehearsal-1';
    my $jp = "$state_root/stage-journal/$stem.jsonl";
    my $rp = "$state_root/$stem-receipt.json";
    lstat($rp) or fail('finish_receipt_missing');
    my $bytes = slurp_raw($rp);
    sha256_hex($bytes) eq $receipt_sha or fail('finish_receipt_sha_mismatch');
    my $receipt = decode_request_file($rp);
    validate_receipt($receipt, $entry);
    # The journal on disk is the authority for the digest the receipt claims.
    my $rows = read_lines($jp);
    my ($seq, $digest) = journal_digest($rows);
    $receipt->{journal_final_seq} == $seq
      or fail('finish_journal_seq');
    $receipt->{journal_final_sha256} eq $digest or fail('finish_journal_digest');
    # The closing row must be the terminal stage, not an arbitrary earlier one.
    @$rows or fail('finish_journal_empty');
    $rows->[-1]{stage} eq 'terminal' or fail('finish_without_terminal_stage');
    $receipt->{last_completed_stage} eq 'terminal'
      or fail('finish_receipt_not_terminal');
    my $code = $receipt->{code};
    $TERMINAL{$code} or fail('finish_code_not_terminal');
    $receipt->{facts}{terminal_code} eq $code or fail('finish_terminal_code_mismatch');
    validate_terminal_kind($code, $entry->{kind});
    validate_terminal_criteria($code, $receipt->{criteria}, $entry->{kind});
    my $finished = {%$entry, status => 'finished',
      receipt_sha256 => $receipt_sha, terminal_code => $code};
    push @$ledger, $finished;
    publish_ledger_rows($ledger);
    return $finished;
  });
  emit($out); exit 0;
}

fail('unknown_broker_operation');
PERL
}

usage() {
  cat <<'USAGE'
usage: broker-spine.sh <operation> [args]
  self-test                       isolated broker self-test; reserves nothing formal
  inspect                         print the formal ledger (read-only)
  reserve KIND ORDINAL RUN_ID SOURCE_SHA INPUT_DIGEST [FROZEN_INPUTS_PATH]
  abandon KIND ORDINAL RUN_ID
  stage KIND ORDINAL RUN_ID REQUEST_PATH
  finish KIND ORDINAL RUN_ID RECEIPT_SHA
USAGE
}

[ $# -ge 1 ] || { usage; exit 2; }

OP=$1
shift

case "$OP" in
  self-test)
    # The self-test is implemented in a separate script so this file stays the
    # broker spine only.
    SELF_TEST="$TEMPLATE_DIR/broker-self-test.sh"
    [ -f "$SELF_TEST" ] || { echo '{"code":"self_test_missing"}'; exit 2; }
    exec "$SELF_TEST"
    ;;
  inspect|reserve|abandon|stage|finish)
    [ -n "$STATE_ROOT" ] || { echo '{"code":"state_root_missing"}'; exit 2; }
    broker "$OP" "$@"
    ;;
  *)
    usage
    exit 2
    ;;
esac
