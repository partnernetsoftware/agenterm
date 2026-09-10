#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$ROOT/../.." && pwd)
STATE_ROOT=${AGENTERM_PROFILE_BINDING_V2_STATE_ROOT:-"$REPO/.agenterm-research-state/browser-profile-name-binding-v2"}
TEMPLATE="$ROOT/result-template.json"

emit_failure() {
  printf '%s\n' "{\"schema\":\"agenterm.profile-binding-v2-runner-error/v1\",\"code\":\"$1\"}"
  exit 2
}

read_browser_version() {
  perl -e '
    use POSIX qw(:sys_wait_h);
    my ($exe)=@ARGV; pipe(my $read,my $write) or exit 2; my $pid=fork(); defined($pid) or exit 2;
    if (!$pid) { close($read); open(STDOUT,">&",$write) or exit 2; open(STDERR,">","/dev/null") or exit 2; exec {$exe} $exe,"--version"; exit 127 }
    close($write); binmode($read); my $expired=0;
    local $SIG{ALRM}=sub{$expired=1; kill "TERM",$pid; select undef,undef,undef,0.05; kill "KILL",$pid}; alarm 5;
    my $b=""; while (1) { my $chunk=""; my $n=sysread($read,$chunk,4097-length($b)); defined($n) or exit 3; last if $n==0; $b.=$chunk; length($b)<=4096 or do { kill "KILL",$pid; waitpid($pid,0); exit 6 } }
    close($read); waitpid($pid,0); alarm 0; exit 124 if $expired; $? == 0 or exit 4;
    $b=~s/^[\x09-\x0d\x20]+//; $b=~s/[\x09-\x0d\x20]+\z//;
    length($b)>0 && $b=~/^[\x20-\x7e]+$/ or exit 5; print $b;
  ' "$1"
}

run_broker() {
  broker_state_root=${AGENTERM_PROFILE_BINDING_V2_STATE_ROOT:-$STATE_ROOT}
  perl - "$broker_state_root" "$TEMPLATE" "$@" <<'PERL'
use strict;
use warnings;
use Fcntl qw(:DEFAULT :flock O_NOFOLLOW);
use JSON::PP ();
use Digest::SHA qw(sha256_hex);
use File::Basename qw(dirname);
use File::Spec ();
use Cwd qw(abs_path);
use B qw(svref_2object SVp_IOK SVp_NOK SVp_POK);

my ($state_root, $template_path, $operation, @args) = @ARGV;
my $EXPERIMENT = 'acu.dynamic.075.profile-name-binding-v2';
my $LEDGER_SCHEMA = 'agenterm.profile-binding-v2-attempt/v1';
my $JOURNAL_SCHEMA = 'agenterm.profile-binding-v2-stage/v1';
my $RECEIPT_SCHEMA = 'agenterm.profile-binding-v2-receipt/v1';
my $ZERO = sha256_hex("acu.profile-binding-v2.journal-zero/v1\0");
my $json = JSON::PP->new->canonical(1)->allow_nonref(0)->utf8(1);
my %TERMINAL = map { $_ => 1 } qw(
  REHEARSAL_PASS
  INCONCLUSIVE_SOURCE INCONCLUSIVE_OWNERSHIP INCONCLUSIVE_CLEANUP
  INCONCLUSIVE_INVENTORY INCONCLUSIVE_BASELINE_MUTATION
  INCONCLUSIVE_DEPENDENCY INCONCLUSIVE_CANDIDATE_OBSERVER
  INCONCLUSIVE_FIXTURE INCONCLUSIVE_GROUND_TRUTH_CONFLICT
  INCONCLUSIVE_ACCEPTED_SURFACE_CHANGED
  INCONCLUSIVE_EDGE_ELIGIBILITY_UNKNOWN
  INCONCLUSIVE_BINDING_DESIGN SELECT_EXPLICIT_DURABLE_BINDING
  INCONCLUSIVE_EVIDENCE_PERSISTENCE
);
my %CRITERION_VALUE = map { $_ => 1 } qw(not-run pass fail rejected);

sub fail { die "$_[0]\n" }
sub is_bool { JSON::PP::is_bool($_[0]) }
sub exact_keys {
  my ($v, $keys, $where) = @_;
  ref($v) eq 'HASH' or fail("${where}_not_object");
  my @got = sort keys %$v;
  my @want = sort @$keys;
  join("\0", @got) eq join("\0", @want) or fail("${where}_keys");
}
sub restricted {
  my ($v, $where) = @_;
  if (!ref $v) {
    return if !defined $v;
    my $flags = svref_2object(\$v)->FLAGS;
    if ($flags & (SVp_IOK | SVp_NOK)) {
      $v == int($v) && abs($v) <= 9007199254740991
        or fail("${where}_unsafe_number");
      return;
    }
    ($flags & SVp_POK) or fail("${where}_unsupported_scalar");
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
sub slurp_raw {
  my ($path) = @_;
  sysopen(my $fh, $path, O_RDONLY | O_NOFOLLOW) or fail("read_failed");
  binmode($fh); local $/; my $b = <$fh>; close($fh) or fail("read_close_failed");
  return $b;
}
sub json_skip_ws { my ($s,$i)=@_; $$i++ while $$i<length($s) && substr($s,$$i,1)=~/\s/ }
sub json_scan_string {
  my ($s,$i)=@_; substr($s,$$i,1) eq '"' or fail('json_string_expected');
  my $start=$$i++;
  while ($$i<length($s)) {
    my $c=substr($s,$$i++,1);
    if ($c eq '"') { my $raw=substr($s,$start,$$i-$start); my $v=eval{JSON::PP->new->allow_nonref(1)->decode($raw)}; $@ and fail('json_string_invalid'); return $v }
    if ($c eq '\\') { $$i<length($s) or fail('json_escape_invalid'); my $e=substr($s,$$i++,1); if ($e eq 'u') { $$i+4<=length($s) && substr($s,$$i,4)=~/^[0-9a-fA-F]{4}$/ or fail('json_escape_invalid'); $$i+=4 } }
  }
  fail('json_string_unterminated');
}
sub json_scan_value {
  my ($s,$i)=@_; json_skip_ws($s,$i); $$i<length($s) or fail('json_value_missing');
  my $c=substr($s,$$i,1);
  if ($c eq '{') {
    $$i++; json_skip_ws($s,$i); my %seen;
    if (substr($s,$$i,1) eq '}') { $$i++; return }
    while (1) {
      json_skip_ws($s,$i); my $key=json_scan_string($s,$i); !$seen{$key}++ or fail('json_duplicate_key');
      json_skip_ws($s,$i); substr($s,$$i++,1) eq ':' or fail('json_colon_expected'); json_scan_value($s,$i); json_skip_ws($s,$i);
      my $d=substr($s,$$i++,1); last if $d eq '}'; $d eq ',' or fail('json_object_delimiter');
    }
    return;
  }
  if ($c eq '[') {
    $$i++; json_skip_ws($s,$i); if (substr($s,$$i,1) eq ']') { $$i++; return }
    while (1) { json_scan_value($s,$i); json_skip_ws($s,$i); my $d=substr($s,$$i++,1); last if $d eq ']'; $d eq ',' or fail('json_array_delimiter') }
    return;
  }
  if ($c eq '"') { json_scan_string($s,$i); return }
  my $rest=substr($s,$$i);
  if ($rest =~ /\A(?:true|false|null)/) { $$i += length($&); return }
  $rest =~ /\A(-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)/
    or fail('json_scalar_invalid');
  my $number=$1;
  $number =~ /^-?(?:0|[1-9][0-9]*)$/ or fail('json_number_not_integer');
  my $magnitude=$number; $magnitude =~ s/^-//;
  length($magnitude) < 16
    || (length($magnitude) == 16 && $magnitude le '9007199254740991')
    or fail('json_number_unsafe');
  $$i += length($number);
}
sub reject_duplicate_keys {
  my ($text)=@_; my $i=0; json_scan_value($text,\$i); json_skip_ws($text,\$i); $i==length($text) or fail('json_trailing_bytes');
}
sub decode_file {
  my ($path, $must_be_canonical) = @_;
  my $b = slurp_raw($path);
  $b =~ s/\n\z//;
  reject_duplicate_keys($b);
  my $v = eval { $json->decode($b) };
  $@ and fail('json_invalid');
  restricted($v, 'json');
  canonical($v) eq $b or ($must_be_canonical ? fail('json_not_rfc8785_subset') : 1);
  return $v;
}
sub assert_directory_chain {
  my ($path,$code)=@_;
  my $absolute=File::Spec->rel2abs($path);
  my ($volume,$directories)=File::Spec->splitpath($absolute,1);
  my @parts=File::Spec->splitdir($directories);
  my $cursor=$volume ne '' ? $volume : File::Spec->rootdir();
  assert_plain_directory($cursor,$code);
  for my $part (@parts) {
    next if $part eq '' || $part eq File::Spec->rootdir();
    $cursor=File::Spec->catdir($cursor,$part);
    assert_plain_directory($cursor,$code);
  }
  return $absolute;
}
sub assert_lane_candidate {
  my ($path) = @_;
  my $lane = $ENV{AGENTERM_PROFILE_BINDING_V2_LANE_ROOT} // fail('lane_root_missing');
  lstat($path) && -f _ && !-l _ or fail('candidate_not_direct_file');
  my @st = stat($path); @st && $st[3] == 1 && $st[4] == $< or fail('candidate_identity_invalid');
  my $parent=assert_directory_chain(dirname($path),'candidate_ancestor_not_plain');
  my $expected=assert_directory_chain($lane,'lane_ancestor_not_plain');
  $parent eq $expected or fail('candidate_outside_lane');
}
sub assert_plain_directory {
  my ($path, $code) = @_;
  lstat($path) && -d _ && !-l _ or fail($code);
  my @st = stat($path);
  @st && (($st[2] & 0170000) == 0040000) or fail($code);
}
sub assert_private_directory {
  my ($path, $code) = @_;
  assert_plain_directory($path, $code);
  my @st = stat($path);
  @st && $st[4] == $< && (($st[2] & 0777) == 0700) or fail($code);
}
sub ensure_tree {
  $state_root = File::Spec->rel2abs($state_root);
  my ($volume, $directories) = File::Spec->splitpath($state_root, 1);
  my @parts = File::Spec->splitdir($directories);
  my $cursor = $volume ne '' ? $volume : File::Spec->rootdir();
  assert_plain_directory($cursor, 'state_ancestor_not_plain_directory');
  for my $part (@parts) {
    next if $part eq '' || $part eq File::Spec->rootdir();
    $cursor = File::Spec->catdir($cursor, $part);
    if (lstat($cursor)) {
      assert_plain_directory($cursor, 'state_ancestor_not_plain_directory');
    } else {
      if (!mkdir($cursor, 0700)) {
        lstat($cursor) or fail('state_root_create_failed');
      }
      assert_plain_directory($cursor, 'state_root_create_invalid');
    }
  }
  assert_private_directory($state_root, 'state_root_identity_invalid');
  my $journal = "$state_root/stage-journal";
  if (!lstat($journal) && !mkdir($journal,0700)) {
    lstat($journal) or fail('journal_root_create_failed');
  }
  assert_private_directory($journal, 'journal_root_identity_invalid');
}
sub atomic_replace {
  my ($path, $bytes) = @_;
  if (lstat($path)) {
    -f _ && !-l _ or fail('atomic_destination_not_plain');
    my @old = stat($path); @old && $old[3] == 1 && $old[4] == $< or fail('atomic_destination_identity_invalid');
  }
  my $tmp = "$path.tmp.$$\." . int(rand(1_000_000));
  sysopen(my $fh, $tmp, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0600)
    or fail('atomic_temp_create_failed');
  binmode($fh); print {$fh} $bytes or fail('atomic_temp_write_failed');
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
sub ledger_path { "$state_root/attempt-ledger.jsonl" }
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
  validate_ledger($rows,0);
  write_lines(ledger_path(),$rows);
  my $readback=read_lines(ledger_path());
  validate_ledger($readback,0);
  @$readback == @$rows && canonical($readback->[-1]) eq canonical($rows->[-1])
    or fail('ledger_readback_mismatch');
}
sub sha { $_[0] =~ /^[0-9a-f]{64}$/ }
sub git_sha { $_[0] =~ /^[0-9a-f]{40}$/ }
sub json_string {
  return 0 if !defined($_[0]) || ref($_[0]);
  my $flags = svref_2object(\$_[0])->FLAGS;
  return ($flags & SVp_POK) ? 1 : 0;
}
sub printable_ascii { json_string($_[0]) && $_[0] =~ /^[\x20-\x7e]+$/ }
sub json_number {
  return 0 if !defined($_[0]) || ref($_[0]);
  my $flags = svref_2object(\$_[0])->FLAGS;
  return ($flags & (SVp_IOK | SVp_NOK)) ? 1 : 0;
}
sub file_sha { sha256_hex(slurp_raw($_[0])) }
sub frozen_input_digest {
  my ($repo)=@_;
  my @names=(
    'plan/design-browser-profile-name-binding-owned-lifecycle-experiment.md',
    'research/browser-profile-name-binding-v2/README.md',
    'research/browser-profile-name-binding-v2/result-template.json',
    'research/browser-profile-name-binding-v2/run-current-host.sh',
    'research/browser-profile-name-binding-v2/court-current-host.qjs',
    'research/browser-profile-name-binding-v2/binding-model.qjs',
    'research/browser-profile-name-binding-v2/fixtures/local-state.json',
    'research/browser-profile-name-binding-v2/fixtures/preferences.json',
    'crates/agenterm-cu/assets/browser-bridge/manifest.json',
    'crates/agenterm-cu/assets/browser-bridge/background.js');
  my $s=Digest::SHA->new(256); $s->add("agenterm-cu/profile-binding-owned-lifecycle-v2/input/v1\0");
  for my $n (@names) { my $b=slurp_raw("$repo/$n"); $s->add(pack('Q<',length($n)),$n,pack('Q<',length($b)),$b) }
  return $s->hexdigest;
}
sub validate_ledger {
  my ($rows, $reject_residual) = @_;
  my %attempt;
  for my $r (@$rows) {
    exact_keys($r, [qw(schema experiment kind ordinal run_id source_sha input_digest status receipt_sha256 terminal_code)], 'ledger');
    $r->{schema} eq $LEDGER_SCHEMA && $r->{experiment} eq $EXPERIMENT or fail('ledger_identity');
    $r->{kind} =~ /^(?:rehearsal|decision)$/ or fail('ledger_kind');
    (($r->{kind} eq 'rehearsal' && $r->{ordinal} =~ /^R[12]$/) || ($r->{kind} eq 'decision' && $r->{ordinal} eq 'D1')) or fail('ledger_ordinal');
    $r->{run_id} =~ /^[0-9a-f]{32}$/ && git_sha($r->{source_sha}) && sha($r->{input_digest}) or fail('ledger_digest');
    my $key = "$r->{kind}:$r->{ordinal}";
    if ($r->{status} eq 'reserved') {
      !defined($r->{receipt_sha256}) && !defined($r->{terminal_code}) or fail('ledger_reserved_terminal');
      !exists($attempt{$key}) or fail('ledger_ordinal_reused');
      $attempt{$key}={row=>$r,status=>'reserved'};
    } elsif ($r->{status} eq 'finished') {
      my $a=$attempt{$key}; $a && $a->{status} eq 'reserved' or fail('ledger_finish_without_reserve');
      for my $field (qw(schema experiment kind ordinal run_id source_sha input_digest)) {
        $r->{$field} eq $a->{row}{$field} or fail('ledger_transition_identity');
      }
      $TERMINAL{$r->{terminal_code}//''} or fail('ledger_finish_code');
      if ($r->{terminal_code} eq 'INCONCLUSIVE_EVIDENCE_PERSISTENCE') {
        !defined($r->{receipt_sha256}) || sha($r->{receipt_sha256}) or fail('ledger_finish_receipt');
      } else { sha($r->{receipt_sha256}//'') or fail('ledger_finish_receipt') }
      $a->{status}='finished'; $a->{terminal_row}=$r;
    } elsif ($r->{status} eq 'abandoned') {
      my $a=$attempt{$key}; $a && $a->{status} eq 'reserved' or fail('ledger_abandoned_without_reserve');
      for my $field (qw(schema experiment kind ordinal run_id source_sha input_digest)) {
        $r->{$field} eq $a->{row}{$field} or fail('ledger_transition_identity');
      }
      !defined($r->{receipt_sha256}) && $r->{terminal_code} eq 'attempt_abandoned_after_independent_audit' or fail('ledger_abandoned');
      $a->{status}='abandoned'; $a->{terminal_row}=$r;
    } else { fail('ledger_status') }
  }
  if ($reject_residual) {
    for my $key (keys %attempt) { $attempt{$key}{status} ne 'reserved' or fail('residual_reservation') }
  }
  return \%attempt;
}
sub load_template {
  my $t = decode_file($template_path, 0);
  exact_keys($t, [qw(schema experiment limits identity_domains journal_keys ledger_keys receipt_keys criteria_keys stages)], 'template');
  $t->{schema} eq 'agenterm.profile-binding-v2-result-template/v1' && $t->{experiment} eq $EXPERIMENT or fail('template_identity');
  exact_keys($t->{limits}, [qw(journal_max_rows_exclusive journal_max_row_bytes_exclusive journal_max_total_bytes_exclusive)], 'template_limits');
  $t->{limits}{journal_max_rows_exclusive} == 64
    && $t->{limits}{journal_max_row_bytes_exclusive} == 32768
    && $t->{limits}{journal_max_total_bytes_exclusive} == 1048576
    or fail('template_limit_value');
  my @domains=qw(
    acu.profile-binding-v2.owner-identity/v1
    acu.profile-binding-v2.browser-identity/v1
    acu.profile-binding-v2.host-identity/v1
    acu.profile-binding-v2.connection-identity/v1
    acu.profile-binding-v2.inventory-row/v1
    acu.profile-binding-v2.candidate-profile/v1
    acu.profile-binding-v2.candidate-installation/v1
    acu.profile-binding-v2.candidate-root/v1
    acu.profile-binding-v2.live-root/v1
    acu.profile-binding-v2.strict-status/v1
    acu.profile-binding-v2.construction-trace/v1
    acu.profile-binding-v2.surface-clause/v1
    acu.profile-binding-v2.ground-truth-pair/v1
    acu.profile-binding-v2.fault-matrix/v1);
  ref($t->{identity_domains}) eq 'ARRAY'
    && join("\0",@{$t->{identity_domains}}) eq join("\0",@domains)
    or fail('template_identity_domains');
  my @journal=qw(schema experiment kind ordinal run_id seq prev_sha256 stage producer deadline_ms elapsed_ms code facts);
  my @ledger=qw(schema experiment kind ordinal run_id source_sha input_digest status receipt_sha256 terminal_code);
  my @receipt=qw(schema experiment kind ordinal run_id source_sha input_digest journal_final_seq journal_final_sha256 last_completed_stage deadline_ms code facts criteria);
  my @criteria=qw(V1 V2 V3 V3a V3b V4a V4b G1 G2a G2b G3 G4a G4c G6 G7);
  for my $pair ([$t->{journal_keys},\@journal,'template_journal_keys'],[$t->{ledger_keys},\@ledger,'template_ledger_keys'],[$t->{receipt_keys},\@receipt,'template_receipt_keys'],[$t->{criteria_keys},\@criteria,'template_criteria_keys']) {
    ref($pair->[0]) eq 'ARRAY' && join("\0",@{$pair->[0]}) eq join("\0",@{$pair->[1]}) or fail($pair->[2]);
  }
  my @stages=qw(preflight baseline session-ready connection-ready fixture ownership candidate surface a0-control prefix-control binding-model stop termination-proof final-inventory registry-hygiene root-removal terminal);
  exact_keys($t->{stages},\@stages,'template_stages');
  my %stage_contract_sha256=(
    'a0-control'=>'af11b559f6979746bcccb6b66666639ae8842cc770fc155eecf7bc9c64d5c08e',
    baseline=>'6b1c220d3a5073586bee2dcb12c9cc0f9818ba596c8776deac7f1adf5eab329b',
    'binding-model'=>'9984b5438e9932c5ac2d360cc0ac83797f599b91ae31b1ba4d3f38d121a9c91f',
    candidate=>'1cb5e5bc9acdf25f773992bc02f05ce95163a82709768d2fd8d5b5f08438a247',
    'connection-ready'=>'492b43e2dc44d560c5fd93566f794e3b7fa81e10db7d025a3cee404a5445f4e7',
    'final-inventory'=>'cfb71bd8ef18dfeb35f7cc98c692a2b0f3b487c2a86c93c4f56f7dda0356fbaa',
    fixture=>'08b480d2afc5e2f5aa6549bbc19e626e566e452cf3219ef3d8afcc8486e09533',
    ownership=>'25783d0952fe6f3dbc3a02d73acb8e041b720da60fefbe8306e3cf05baee0d27',
    'prefix-control'=>'1f2eafe23d5f86e82c528bf9f9c117e60b828f604100df8fbfbe945e24486021',
    preflight=>'6f4417c6af9a45a7a676c597b3b422250ce1908b766c4cb9f22f5f55f99a2941',
    'registry-hygiene'=>'304d2db0eaa00edbd4a5ce28b1adb71ceb537c47c04c031187e84eb8045f023f',
    'root-removal'=>'7f6cf7a94cbe8653526ffdffab3ba830a8aa727011d44f134cf0895572971601',
    'session-ready'=>'483091ce0809117137a8db821f01779fdf723cf9adb06773e732a5098f8d7ba8',
    stop=>'00aab8fd67bc33605dc3fdfeefcc4625157985e6e2e23ba0a7fcbfa51163e910',
    surface=>'bccbefa219e9346b688f69056d3c46ca8a5636500977b97e899afd9248140b19',
    terminal=>'f3335468cb0a21334f13fd8fd46f1298dee416789238735b2e6b1cbd238ffd29',
    'termination-proof'=>'76b5cff2ff53d40584778ead9cb9ce8ea8ed38ccbfe492326a13548aeb53efa6');
  for my $stage (@stages) {
    my $variant = $stage eq 'candidate' || $stage eq 'connection-ready'
      || $stage eq 'ownership';
    exact_keys($t->{stages}{$stage},$variant ? [qw(required optional one_of)] : [qw(required optional)],'template_stage');
    ref($t->{stages}{$stage}{required}) eq 'HASH' && ref($t->{stages}{$stage}{optional}) eq 'HASH' or fail('template_stage_members');
    my @schemas=($t->{stages}{$stage});
    if ($variant) {
      ref($t->{stages}{$stage}{one_of}) eq 'ARRAY' && @{$t->{stages}{$stage}{one_of}} == 2 or fail('template_candidate_one_of');
      push @schemas,@{$t->{stages}{$stage}{one_of}};
    }
    for my $schema (@schemas) {
      ref($schema->{required}) eq 'HASH' && ref($schema->{optional}) eq 'HASH' or fail('template_stage_members');
      for my $type (values %{$schema->{required}}, values %{$schema->{optional}}) {
      $type =~ /^(?:sha256|git_sha|string|boolean|safe_integer|nullable_string|tristate)$/ or fail('template_unknown_type');
      }
    }
    sha256_hex(canonical($t->{stages}{$stage})) eq $stage_contract_sha256{$stage}
      or fail('template_stage_recursive_contract');
  }
  return $t;
}
sub validate_type {
  my ($v, $type) = @_;
  return sha($v // '') if $type eq 'sha256';
  return git_sha($v // '') if $type eq 'git_sha';
  return printable_ascii($v) if $type eq 'string';
  return is_bool($v) if $type eq 'boolean';
  return json_number($v) && $v =~ /^(?:0|[1-9][0-9]*)$/ && $v <= 9007199254740991 if $type eq 'safe_integer';
  return !defined($v) || printable_ascii($v) if $type eq 'nullable_string';
  return !ref($v) && defined($v) && $v =~ /^(?:yes|no|unknown)$/ if $type eq 'tristate';
  return 0;
}
sub facts_match_schema {
  my ($spec,$facts)=@_;
  return 0 unless ref($facts) eq 'HASH';
  for my $k (keys %{$spec->{required}}) { return 0 unless exists($facts->{$k}) }
  for my $k (keys %$facts) {
    my $type = $spec->{required}{$k} // $spec->{optional}{$k};
    return 0 unless defined($type) && validate_type($facts->{$k},$type);
  }
  return 1;
}
sub validate_facts {
  my ($template, $stage, $facts) = @_;
  my $spec = $template->{stages}{$stage} or fail('stage_not_allowed');
  ref($facts) eq 'HASH' or fail('facts_not_object');
  if (exists($spec->{one_of})) {
    my $matched=0; $matched++ for grep { facts_match_schema($_,$facts) } @{$spec->{one_of}};
    $matched == 1 or fail('facts_one_of');
    return;
  }
  facts_match_schema($spec,$facts) or fail('facts_schema');
}
sub validate_criteria {
  my ($template,$criteria,$kind)=@_;
  exact_keys($criteria,$template->{criteria_keys},'criteria');
  for my $key (@{$template->{criteria_keys}}) {
    !ref($criteria->{$key}) && $CRITERION_VALUE{$criteria->{$key}//''}
      or fail("criteria_value_$key");
  }
  if ($kind eq 'rehearsal') {
    for my $key (qw(G1 G2a G2b G3 G4a G4c G6 G7)) {
      $criteria->{$key} eq 'not-run' or fail('rehearsal_design_fact_present');
    }
  }
}
sub names {
  my ($kind, $ordinal) = @_;
  return $kind eq 'decision' ? ('decision-1', 'D1') : ($ordinal eq 'R1' ? ('rehearsal-1','R1') : ('rehearsal-2','R2'));
}
sub journal_digest {
  my ($rows,$template,$entry) = @_;
  @$rows < $template->{limits}{journal_max_rows_exclusive} or fail('journal_row_count_limit');
  my $prev = $ZERO; my $seq = 0;
  my $total = 0;
  for my $r (@$rows) {
    exact_keys($r, [qw(schema experiment kind ordinal run_id seq prev_sha256 stage producer deadline_ms elapsed_ms code facts)], 'journal');
    $r->{schema} eq $JOURNAL_SCHEMA && $r->{experiment} eq $EXPERIMENT or fail('journal_identity');
    if ($entry) {
      for my $field (qw(kind ordinal run_id)) { $r->{$field} eq $entry->{$field} or fail('journal_attempt_identity') }
    }
    json_number($r->{seq}) && $r->{seq} == ++$seq && $r->{prev_sha256} eq $prev or fail('journal_chain');
    json_number($r->{deadline_ms}) && json_number($r->{elapsed_ms})
      && $r->{deadline_ms} >= 0 && $r->{elapsed_ms} >= 0
      && $r->{deadline_ms} <= 9007199254740991 && $r->{elapsed_ms} <= 9007199254740991
      or fail('journal_integer');
    printable_ascii($r->{producer}) && printable_ascii($r->{code}) or fail('journal_text');
    validate_facts($template,$r->{stage},$r->{facts});
    my $encoded=canonical($r);
    length($encoded) < $template->{limits}{journal_max_row_bytes_exclusive}
      or fail('journal_row_limit');
    $total += length($encoded) + 1;
    $total < $template->{limits}{journal_max_total_bytes_exclusive}
      or fail('journal_total_limit');
    $prev = sha256_hex($encoded);
  }
  return ($seq, $prev);
}
sub stage_rank {
  my ($row)=@_;
  return 0 if $row->{stage} eq 'preflight';
  return $row->{code} eq 'BASELINE_CAPTURED' ? 1 : 4
    if $row->{stage} eq 'baseline';
  my %rank=('session-ready'=>2,'connection-ready'=>3,ownership=>5,fixture=>6,
    candidate=>7,'prefix-control'=>8,surface=>9,'a0-control'=>10,
    'binding-model'=>11,stop=>12,'termination-proof'=>13,
    'final-inventory'=>14,'registry-hygiene'=>15,'root-removal'=>16,terminal=>17);
  exists($rank{$row->{stage}}) or fail('journal_stage_rank');
  return $rank{$row->{stage}};
}
sub validate_stage_producer {
  my ($row)=@_;
  my %producer=(preflight=>'runner',baseline=>'public-cli',
    'session-ready'=>'public-cli','connection-ready'=>'public-cli',
    fixture=>'public-cli',ownership=>'independent-ps',surface=>'court',
    'a0-control'=>'court','prefix-control'=>'court',
    'binding-model'=>'binding-model',stop=>'public-cli',
    'termination-proof'=>'independent-ps','final-inventory'=>'public-cli',
    'registry-hygiene'=>'public-cli','root-removal'=>'filesystem-witness',
    terminal=>'court');
  if ($row->{stage} eq 'candidate') {
    my $expected=exists($row->{facts}{local_state_digest})
      ? 'independent-local-state' : 'independent-preferences';
    $row->{producer} eq $expected or fail('journal_candidate_producer');
    return;
  }
  $row->{producer} eq $producer{$row->{stage}}
    or fail('journal_stage_producer');
}
sub validate_mode_stage {
  my ($row,$kind)=@_;
  return unless $row->{stage} eq 'connection-ready' || $row->{stage} eq 'ownership';
  my $has_connection = exists($row->{facts}{connection_identity_digest});
  if ($kind eq 'rehearsal') {
    !$has_connection or fail('rehearsal_connection_metadata_present');
    if ($row->{stage} eq 'connection-ready') {
      exists($row->{facts}{row_count}) or fail('rehearsal_connection_count_missing');
    }
  } else {
    $has_connection or fail('decision_connection_metadata_missing');
    !exists($row->{facts}{row_count}) or fail('decision_rehearsal_count_present');
  }
}
sub validate_stage_history {
  my ($rows,$kind)=@_;
  my $prior=-1;
  for my $row (@$rows) {
    my $rank=stage_rank($row);
    $rank >= $prior or fail('journal_stage_order');
    $prior=$rank;
    validate_stage_producer($row);
    validate_mode_stage($row,$kind);
  }
}
sub stage_rows {
  my ($rows,$stage,$producer,$code)=@_;
  return grep { $_->{stage} eq $stage
      && (!defined($producer) || $_->{producer} eq $producer)
      && (!defined($code) || $_->{code} eq $code) } @$rows;
}
sub validate_criteria_witnesses {
  my ($rows,$criteria)=@_;
  my %proof=(
    V1=>['preflight','runner','PREFLIGHT_PASS'],
    V2=>['ownership','independent-ps','OWNERSHIP_PROVED'],
    V3a=>['termination-proof','independent-ps','TERMINATION_PROVED'],
    V3b=>['root-removal','filesystem-witness','ROOT_REMOVAL_PROVED'],
    V4a=>['baseline','public-cli','CURRENT_INVENTORY_PROVED'],
    V4b=>['final-inventory','public-cli','FINAL_INVENTORY_PROVED'],
    G1=>['fixture','public-cli','FIXTURE_IDENTITY_PROVED'],
    G2a=>['candidate','independent-local-state','PUBLIC_CANDIDATE_PROVED'],
    G2b=>['candidate','independent-preferences','INSTALLED_CANDIDATE_PROVED'],
    G6=>['prefix-control','court','PREFIX_CONTROL_PASS'],
    G7=>['binding-model','binding-model','BINDING_MODEL_PASS']);
  for my $criterion (keys %proof) {
    if ($criteria->{$criterion} eq 'pass') {
      my @found=stage_rows($rows,@{$proof{$criterion}});
      @found == 1 or fail("criteria_witness_$criterion");
    }
  }
  if ($criteria->{V3} eq 'pass') {
    $criteria->{V3a} eq 'pass' && $criteria->{V3b} eq 'pass'
      or fail('criteria_witness_V3');
  }
  if ($criteria->{G3} eq 'rejected') {
    my @found=stage_rows($rows,'a0-control','court',undef);
    @found == 1 or fail('criteria_witness_G3');
  }
  if ($criteria->{G4a} ne 'not-run' || $criteria->{G4c} ne 'not-run') {
    my @found=stage_rows($rows,'surface','court','SURFACE_PAIR_CLASSIFIED');
    @found == 4 or fail('criteria_witness_G4_complete_table');
  }
}
sub receipt_for {
  my ($entry, $row, $seq, $digest, $criteria) = @_;
  return {schema=>$RECEIPT_SCHEMA,experiment=>$EXPERIMENT,kind=>$entry->{kind},ordinal=>$entry->{ordinal},run_id=>$entry->{run_id},source_sha=>$entry->{source_sha},input_digest=>$entry->{input_digest},journal_final_seq=>$seq,journal_final_sha256=>$digest,last_completed_stage=>$row->{stage},deadline_ms=>$row->{deadline_ms},code=>$row->{code},facts=>$row->{facts},criteria=>$criteria};
}
sub validate_receipt {
  my ($template,$receipt,$entry,$must_terminal)=@_;
  exact_keys($receipt,$template->{receipt_keys},'receipt');
  $receipt->{schema} eq $RECEIPT_SCHEMA && $receipt->{experiment} eq $EXPERIMENT or fail('receipt_identity');
  for my $field (qw(kind ordinal run_id source_sha input_digest)) {
    $receipt->{$field} eq $entry->{$field} or fail('receipt_attempt_identity');
  }
  json_number($receipt->{journal_final_seq}) && $receipt->{journal_final_seq} >= 1
    && sha($receipt->{journal_final_sha256}) or fail('receipt_journal_identity');
  exists($template->{stages}{$receipt->{last_completed_stage}}) or fail('receipt_stage');
  json_number($receipt->{deadline_ms}) && $receipt->{deadline_ms} >= 0
    && $receipt->{deadline_ms} <= 9007199254740991 or fail('receipt_deadline');
  printable_ascii($receipt->{code}) or fail('receipt_code');
  validate_facts($template,$receipt->{last_completed_stage},$receipt->{facts});
  validate_criteria($template,$receipt->{criteria},$receipt->{kind});
  if ($must_terminal || $receipt->{last_completed_stage} eq 'terminal') {
    $receipt->{last_completed_stage} eq 'terminal'
      && $TERMINAL{$receipt->{code}//''}
      && $receipt->{facts}{terminal_code} eq $receipt->{code}
      or fail('receipt_terminal');
    !defined($receipt->{facts}{primary_cause})
      || $TERMINAL{$receipt->{facts}{primary_cause}}
      or fail('receipt_primary_cause');
    if ($receipt->{code} eq 'REHEARSAL_PASS'
        || $receipt->{code} eq 'SELECT_EXPLICIT_DURABLE_BINDING') {
      $receipt->{facts}{cleanup_status} eq 'pass'
        && $receipt->{facts}{final_inventory_status} eq 'pass'
        && $receipt->{criteria}{V3a} eq 'pass'
        && $receipt->{criteria}{V3b} eq 'pass'
        && $receipt->{criteria}{V4b} eq 'pass'
        or fail('receipt_authority_cleanup');
    }
    if ($receipt->{code} eq 'REHEARSAL_PASS') {
      for my $key (qw(V1 V2 V3 V3a V3b V4a V4b)) {
        $receipt->{criteria}{$key} eq 'pass' or fail('rehearsal_gate_not_proved');
      }
    }
    if ($receipt->{code} eq 'SELECT_EXPLICIT_DURABLE_BINDING') {
      for my $key (qw(V1 V2 V3 V3a V3b V4a V4b G1 G2a G2b G4a G4c G6 G7)) {
        $receipt->{criteria}{$key} eq 'pass' or fail('decision_gate_not_proved');
      }
      $receipt->{criteria}{G3} eq 'rejected' or fail('a0_not_rejected');
    }
  }
}
sub active_entry {
  my ($rows, $kind, $ordinal, $run_id) = @_;
  validate_ledger($rows,0);
  my @m = grep { $_->{kind} eq $kind && $_->{ordinal} eq $ordinal && $_->{run_id} eq $run_id } @$rows;
  @m == 1 && $m[0]{status} eq 'reserved' or fail('attempt_not_active');
  return $m[0];
}

sub validate_external_state {
  my ($ledger,$template)=@_;
  my $attempt=validate_ledger($ledger,0);
  for my $ordinal (qw(R1 R2 D1)) {
    my $kind=$ordinal eq 'D1' ? 'decision' : 'rehearsal';
    my $key="$kind:$ordinal"; my ($stem)=names($kind,$ordinal);
    my $jp="$state_root/stage-journal/$stem.jsonl";
    my $rp="$state_root/$stem-receipt.json";
    my $has_j=lstat($jp) ? 1 : 0; my $has_r=lstat($rp) ? 1 : 0;
    ($has_j || $has_r) && !exists($attempt->{$key}) and fail('profile_binding_v2_state_inconsistent');
    next unless exists($attempt->{$key});
    my $entry=$attempt->{$key}{row};
    my ($seq,$digest)=(0,$ZERO);
    my $journal_rows=[];
    if ($has_j) {
      $journal_rows=read_lines($jp);
      ($seq,$digest)=journal_digest($journal_rows,$template,$entry);
      validate_stage_history($journal_rows,$entry->{kind});
    }
    $has_r && !$has_j and fail('profile_binding_v2_state_inconsistent');
    if ($has_r) {
      my $bytes=slurp_raw($rp); my $receipt=decode_file($rp,1);
      my $terminal=$attempt->{$key}{terminal_row};
      validate_receipt($template,$receipt,$entry,$attempt->{$key}{status} eq 'finished'
        && ($terminal->{terminal_code}//'') ne 'INCONCLUSIVE_EVIDENCE_PERSISTENCE');
      validate_criteria_witnesses($journal_rows,$receipt->{criteria});
      if ($attempt->{$key}{status} ne 'finished'
          || ($terminal->{terminal_code}//'') ne 'INCONCLUSIVE_EVIDENCE_PERSISTENCE') {
        $receipt->{journal_final_seq} == $seq && $receipt->{journal_final_sha256} eq $digest
          or fail('profile_binding_v2_state_inconsistent');
      }
      if ($attempt->{$key}{status} eq 'finished' && defined($terminal->{receipt_sha256})) {
        sha256_hex($bytes) eq $terminal->{receipt_sha256}
          or fail('profile_binding_v2_state_inconsistent');
      }
    }
    if ($attempt->{$key}{status} eq 'finished'
        && ($attempt->{$key}{terminal_row}{terminal_code}//'') ne 'INCONCLUSIVE_EVIDENCE_PERSISTENCE') {
      $has_j && $has_r or fail('profile_binding_v2_state_inconsistent');
    }
  }
  return $attempt;
}

my $template = load_template();
if ($operation eq 'schema-check') {
  validate_facts($template,'fixture',{extension_id=>'synthetic',strict_status_digest=>('a'x64),extension_identity_match=>JSON::PP::true,status_identity_match=>JSON::PP::true,build_identity_match=>JSON::PP::true,native_host_identity_digest=>('b'x64),construction_trace_digest=>('c'x64),construction_trace_complete=>JSON::PP::true});
  validate_facts($template,'candidate',{local_state_digest=>('a'x64),local_state_row_count=>1,N_all=>1});
  validate_facts($template,'candidate',{preferences_digest=>('b'x64),preferences_row_count=>1,candidate_root_identity_digest=>('c'x64),live_root_identity_digest=>('d'x64),candidate_ancestor_symlinks_absent=>JSON::PP::true,live_ancestor_symlinks_absent=>JSON::PP::true,live_root_outside_candidate=>JSON::PP::true,root_file_objects_distinct=>JSON::PP::true,N=>1,I=>1,C=>0});
  validate_facts($template,'connection-ready',{host_identity_digest=>('a'x64),row_count=>1,ready=>JSON::PP::true,complete=>JSON::PP::true});
  validate_facts($template,'connection-ready',{host_identity_digest=>('a'x64),connection_identity_digest=>('b'x64),ready=>JSON::PP::true,complete=>JSON::PP::true});
  my $mixed=eval { validate_facts($template,'candidate',{local_state_digest=>('a'x64),local_state_row_count=>1,N_all=>1,N=>1}); 1 };
  !$mixed or fail('template_candidate_one_of_not_exact');
  my $numeric_string=eval { validate_facts($template,'a0-control',{outcome=>1}); 1 };
  !$numeric_string or fail('template_number_accepted_as_string');
  validate_facts($template,'a0-control',{outcome=>'1'});
  my $huge_numeric_token=eval { reject_duplicate_keys('{"digest":'.('1'x64).'}'); 1 };
  !$huge_numeric_token or fail('json_unsafe_numeric_token_accepted');
  reject_duplicate_keys('{"digest":"'.('1'x64).'"}');
  my $rehearsal_leak=eval { validate_mode_stage({stage=>'connection-ready',facts=>{connection_identity_digest=>('b'x64)}},'rehearsal'); 1 };
  !$rehearsal_leak or fail('template_rehearsal_connection_leak_accepted');
  my $decision_blind=eval { validate_mode_stage({stage=>'connection-ready',facts=>{row_count=>1}},'decision'); 1 };
  !$decision_blind or fail('template_decision_connection_omission_accepted');
  print canonical({schema=>'agenterm.profile-binding-v2-schema-check/v1',code=>'ok',template_sha256=>sha256_hex(slurp_raw($template_path))}), "\n";
  exit 0;
}
if ($operation eq 'selftest-reserve') {
  my $out=with_lock(sub {
    my $rows=read_lines(ledger_path()); validate_external_state($rows,$template); validate_ledger($rows,1);
    my $r={schema=>$LEDGER_SCHEMA,experiment=>$EXPERIMENT,kind=>'rehearsal',ordinal=>'R1',run_id=>'0123456789abcdef0123456789abcdef',source_sha=>('b'x40),input_digest=>('a'x64),status=>'reserved',receipt_sha256=>undef,terminal_code=>undef};
    push @$rows,$r; publish_ledger_rows($rows); return $r;
  }); print canonical($out),"\n"; exit 0;
}
if ($operation eq 'abandon') {
  my ($kind,$ordinal,$run_id)=@args;
  my $out=with_lock(sub {
    my $ledger=read_lines(ledger_path());
    my $entry=active_entry($ledger,$kind,$ordinal,$run_id);
    my $abandoned={%$entry,status=>'abandoned',receipt_sha256=>undef,
      terminal_code=>'attempt_abandoned_after_independent_audit'};
    push @$ledger,$abandoned;
    publish_ledger_rows($ledger);
    return $abandoned;
  });
  print canonical($out),"\n"; exit 0;
}
if ($operation eq 'reserve') {
  my ($kind, $run_id, $candidate_path) = @args;
  $kind && $kind =~ /^(?:rehearsal|decision)$/ or fail('reserve_kind');
  $run_id && $run_id =~ /^[0-9a-f]{32}$/ or fail('reserve_run_id');
  assert_lane_candidate($candidate_path);
  my $c = decode_file($candidate_path, 1);
  exact_keys($c, [qw(source_sha input_digest agenterm_sha256 agenterm_cu_sha256 browser_sha256 result_template_sha256 browser_family browser_version)], 'reserve_candidate');
  git_sha($c->{source_sha}) && sha($c->{input_digest}) && sha($c->{agenterm_sha256}) && sha($c->{agenterm_cu_sha256}) && sha($c->{browser_sha256}) && sha($c->{result_template_sha256}) or fail('reserve_candidate_value');
  my $out = with_lock(sub {
    my $rows = read_lines(ledger_path()); validate_external_state($rows,$template); validate_ledger($rows, 1);
    my $repo=$ENV{AGENTERM_PROFILE_BINDING_V2_REPO} // fail('reserve_repo_missing');
    my $agenterm=$ENV{AGENTERM_PROFILE_BINDING_V2_AGENTERM_EXE} // fail('reserve_agenterm_missing');
    my $cu=$ENV{AGENTERM_PROFILE_BINDING_V2_CU_EXE} // fail('reserve_cu_missing');
    my $browser=$ENV{AGENTERM_PROFILE_BINDING_V2_BROWSER_EXE} // fail('reserve_browser_missing');
    my $head; { open my $git,'-|','git','-C',$repo,'rev-parse','HEAD' or fail('reserve_git_failed'); $head=<$git>; close($git) or fail('reserve_git_failed'); chomp($head) }
    $head eq $c->{source_sha} && frozen_input_digest($repo) eq $c->{input_digest} && file_sha($agenterm) eq $c->{agenterm_sha256} && file_sha($cu) eq $c->{agenterm_cu_sha256} && file_sha($browser) eq $c->{browser_sha256} && file_sha($template_path) eq $c->{result_template_sha256} or fail('reserve_inputs_changed');
    my %used = map { $_->{ordinal} => 1 } grep { $_->{status} eq 'reserved' } @$rows;
    my $ordinal;
    if ($kind eq 'rehearsal') { $ordinal = !$used{R1} ? 'R1' : !$used{R2} ? 'R2' : fail('rehearsal_budget_exhausted') }
    else {
      !$used{D1} or fail('decision_budget_exhausted');
      my @success = grep { $_->{kind} eq 'rehearsal' && $_->{status} eq 'finished' && $_->{terminal_code} eq 'REHEARSAL_PASS' && $_->{source_sha} eq $c->{source_sha} && $_->{input_digest} eq $c->{input_digest} } @$rows;
      my $matched=0;
      for my $done (@success) {
        my ($stem)=names('rehearsal',$done->{ordinal}); my $jr=read_lines("$state_root/stage-journal/$stem.jsonl"); journal_digest($jr,$template,$done);
        my ($pre)=grep { $_->{stage} eq 'preflight' } @$jr;
        next unless $pre && $pre->{facts}{state_chain_valid};
        my $f=$pre->{facts};
        if ($f->{source_sha} eq $c->{source_sha} && $f->{input_digest} eq $c->{input_digest} && $f->{agenterm_sha256} eq $c->{agenterm_sha256} && $f->{agenterm_cu_sha256} eq $c->{agenterm_cu_sha256} && $f->{browser_sha256} eq $c->{browser_sha256} && ($f->{result_template_sha256}//'') eq $c->{result_template_sha256} && $f->{browser_family} eq $c->{browser_family} && $f->{browser_version} eq $c->{browser_version}) { $matched=1; last }
      }
      $matched or fail('decision_requires_matching_rehearsal'); $ordinal = 'D1';
    }
    my $r={schema=>$LEDGER_SCHEMA,experiment=>$EXPERIMENT,kind=>$kind,ordinal=>$ordinal,run_id=>$run_id,source_sha=>$c->{source_sha},input_digest=>$c->{input_digest},status=>'reserved',receipt_sha256=>undef,terminal_code=>undef};
    push @$rows,$r; publish_ledger_rows($rows); return $r;
  });
  print canonical($out),"\n"; exit 0;
}
if ($operation eq 'stage') {
  my ($kind,$ordinal,$run_id,$request_path)=@args;
  assert_lane_candidate($request_path);
  my $req=decode_file($request_path,1);
  exact_keys($req,[qw(stage producer deadline_ms elapsed_ms code facts criteria previous_receipt_sha256)],'stage_request');
  my $out=with_lock(sub {
    my $ledger=read_lines(ledger_path()); validate_external_state($ledger,$template); my $entry=active_entry($ledger,$kind,$ordinal,$run_id);
    my ($stem)=names($kind,$ordinal); my $jp="$state_root/stage-journal/$stem.jsonl"; my $rp="$state_root/$stem-receipt.json";
    my $rows=read_lines($jp); my ($seq,$prev)=journal_digest($rows,$template,$entry);
    validate_stage_history($rows,$kind);
    @$rows && $rows->[-1]{stage} eq 'terminal' and fail('stage_after_terminal');
    !@$rows && $req->{stage} ne 'preflight' && $req->{stage} ne 'terminal'
      and fail('first_stage_not_preflight_or_terminal');
    if ($kind eq 'rehearsal' && $req->{stage} =~ /^(?:fixture|candidate|surface|a0-control|prefix-control|binding-model)$/) {
      fail('rehearsal_design_stage_forbidden');
    }
    if (lstat($rp)) {
      defined($req->{previous_receipt_sha256}) && sha($req->{previous_receipt_sha256}) && $req->{previous_receipt_sha256} eq sha256_hex(slurp_raw($rp)) or fail('lane_mirror_disagrees');
      @$rows or fail('receipt_without_journal');
      my $prior=decode_file($rp,1); validate_receipt($template,$prior,$entry,0);
      validate_criteria_witnesses($rows,$prior->{criteria});
      $prior->{journal_final_seq} == $seq && $prior->{journal_final_sha256} eq $prev
        or fail('receipt_journal_disagrees');
    } else { !defined($req->{previous_receipt_sha256}) or fail('unexpected_previous_receipt') }
    !lstat($rp) && @$rows and fail('journal_without_receipt');
    validate_facts($template,$req->{stage},$req->{facts});
    validate_criteria($template,$req->{criteria},$kind);
    validate_mode_stage({stage=>$req->{stage},facts=>$req->{facts}},$kind);
    validate_stage_producer({stage=>$req->{stage},producer=>$req->{producer},
      code=>$req->{code},facts=>$req->{facts}});
    for ($req->{deadline_ms},$req->{elapsed_ms}) { defined($_) && !ref($_) && /^(?:0|[1-9][0-9]*)$/ && $_ <= 9007199254740991 or fail('stage_integer') }
    my $row={schema=>$JOURNAL_SCHEMA,experiment=>$EXPERIMENT,kind=>$kind,ordinal=>$ordinal,run_id=>$run_id,seq=>$seq+1,prev_sha256=>$prev,stage=>$req->{stage},producer=>$req->{producer},deadline_ms=>$req->{deadline_ms},elapsed_ms=>$req->{elapsed_ms},code=>$req->{code},facts=>$req->{facts}};
    my $encoded=canonical($row); length($encoded)<$template->{limits}{journal_max_row_bytes_exclusive} or fail('journal_row_limit');
    push @$rows,$row; @$rows<$template->{limits}{journal_max_rows_exclusive} or fail('journal_row_count_limit');
    validate_stage_history($rows,$kind);
    validate_criteria_witnesses($rows,$req->{criteria});
    my $all=join('',map { canonical($_)."\n" } @$rows); length($all)<$template->{limits}{journal_max_total_bytes_exclusive} or fail('journal_total_limit');
    atomic_replace($jp,$all); my ($n,$dig)=journal_digest(read_lines($jp),$template,$entry);
    my $receipt=receipt_for($entry,$row,$n,$dig,$req->{criteria});
    validate_receipt($template,$receipt,$entry,0);
    my $receipt_text=canonical($receipt);
    atomic_replace($rp,$receipt_text);
    return {accepted=>JSON::PP::true,receipt_sha256=>sha256_hex(slurp_raw($rp)),receipt_text=>$receipt_text};
  }); print canonical($out),"\n"; exit 0;
}
if ($operation eq 'finish') {
  my ($kind,$ordinal,$run_id,$candidate_path)=@args;
  assert_lane_candidate($candidate_path);
  my $candidate=decode_file($candidate_path,1);
  exact_keys($candidate,[qw(receipt_sha256)],'finish_candidate');
  my $out=with_lock(sub {
    my $ledger=read_lines(ledger_path()); validate_external_state($ledger,$template); my $entry=active_entry($ledger,$kind,$ordinal,$run_id);
    my ($stem)=names($kind,$ordinal); my $rp="$state_root/$stem-receipt.json"; my $jp="$state_root/stage-journal/$stem.jsonl";
    lstat($rp) or fail('receipt_missing');
    my $bytes=slurp_raw($rp); sha($candidate->{receipt_sha256}) && $candidate->{receipt_sha256} eq sha256_hex($bytes) or fail('lane_mirror_disagrees');
    my $r=decode_file($rp,1); validate_receipt($template,$r,$entry,1);
    my $journal_rows=read_lines($jp);
    my ($n,$dig)=journal_digest($journal_rows,$template,$entry);
    validate_stage_history($journal_rows,$kind);
    validate_criteria_witnesses($journal_rows,$r->{criteria});
    $r->{journal_final_seq} == $n && $r->{journal_final_sha256} eq $dig or fail('receipt_journal_disagrees');
    my $digest=sha256_hex($bytes); my $terminal=$r->{facts}{terminal_code};
    my $finished={%$entry,status=>'finished',receipt_sha256=>$digest,terminal_code=>$terminal};
    push @$ledger,$finished; publish_ledger_rows($ledger);
    return {accepted=>JSON::PP::true,receipt_sha256=>$digest,receipt_text=>$bytes};
  }); print canonical($out),"\n"; exit 0;
}
if ($operation eq 'evidence-fail') {
  my ($kind,$ordinal,$run_id)=@args;
  my $out=with_lock(sub {
    my $ledger=read_lines(ledger_path()); validate_ledger($ledger,0);
    my @same=grep { $_->{kind} eq $kind && $_->{ordinal} eq $ordinal && $_->{run_id} eq $run_id } @$ledger;
    if (@same == 2 && $same[1]{status} eq 'finished' && $same[1]{terminal_code} eq 'INCONCLUSIVE_EVIDENCE_PERSISTENCE') {
      return $same[1];
    }
    my $entry=active_entry($ledger,$kind,$ordinal,$run_id);
    my ($stem)=names($kind,$ordinal); my $rp="$state_root/$stem-receipt.json";
    my $receipt_sha;
    if (lstat($rp)) {
      my $ok=eval { my $r=decode_file($rp,1); validate_receipt($template,$r,$entry,0); $receipt_sha=sha256_hex(slurp_raw($rp)); 1 };
      $receipt_sha=undef unless $ok;
    }
    my $finished={%$entry,status=>'finished',receipt_sha256=>$receipt_sha,terminal_code=>'INCONCLUSIVE_EVIDENCE_PERSISTENCE'};
    push @$ledger,$finished; publish_ledger_rows($ledger);
    return $finished;
  }); print canonical($out),"\n"; exit 0;
}
if ($operation eq 'inspect') {
  my $out=with_lock(sub { my $rows=read_lines(ledger_path()); validate_external_state($rows,$template); validate_ledger($rows,0); return {schema=>'agenterm.profile-binding-v2-state/v1',experiment=>$EXPERIMENT,ledger=>$rows} });
  print canonical($out),"\n"; exit 0;
}
if ($operation eq 'verify-finished') {
  my ($kind,$ordinal,$run_id)=@args;
  my $out=with_lock(sub {
    my $rows=read_lines(ledger_path()); validate_external_state($rows,$template); validate_ledger($rows,0);
    my @m=grep { $_->{kind} eq $kind && $_->{ordinal} eq $ordinal && $_->{run_id} eq $run_id } @$rows;
    @m==2 && $m[0]{status} eq 'reserved' && $m[1]{status} eq 'finished' or fail('attempt_not_finished');
    if (defined($m[1]{receipt_sha256})) { my ($stem)=names($kind,$ordinal); my $rp="$state_root/$stem-receipt.json"; lstat($rp) && sha256_hex(slurp_raw($rp)) eq $m[1]{receipt_sha256} or fail('finished_receipt_disagrees') }
    return $m[1];
  }); print canonical($out),"\n"; exit 0;
}
fail('unknown_broker_operation');
PERL
}

fixed_persistence_record() {
  record_kind=$1
  record_ordinal=$2
  record_run_id=$3
  perl -MJSON::PP -e '
    my ($kind,$ordinal,$run_id,$source,$input)=@ARGV;
    $kind =~ /^(?:rehearsal|decision)$/ && $ordinal =~ /^(?:R1|R2|D1)$/
      && $run_id =~ /^[0-9a-f]{32}$/ && $source =~ /^[0-9a-f]{40}$/
      && $input =~ /^[0-9a-f]{64}$/ or exit 2;
    my $j=JSON::PP->new->canonical(1);
    print $j->encode({schema=>"agenterm.profile-binding-v2-persistence-failure/v1",
      experiment=>"acu.dynamic.075.profile-name-binding-v2",kind=>$kind,
      ordinal=>$ordinal,run_id=>$run_id,source_sha=>$source,
      input_digest=>$input,code=>"INCONCLUSIVE_EVIDENCE_PERSISTENCE"}),"\n";
  ' "$record_kind" "$record_ordinal" "$record_run_id" \
    "${AGENTERM_PROFILE_BINDING_V2_SOURCE_SHA:-}" \
    "${AGENTERM_PROFILE_BINDING_V2_INPUT_DIGEST:-}"
}

finish_evidence_failure() {
  failure_kind=$1
  failure_ordinal=$2
  failure_run_id=$3
  if failure_row=$(run_broker evidence-fail "$failure_kind" "$failure_ordinal" "$failure_run_id" 2>/dev/null); then
    printf '%s\n' "$failure_row"
  else
    fixed_persistence_record "$failure_kind" "$failure_ordinal" "$failure_run_id" \
      || printf '%s\n' '{"code":"INCONCLUSIVE_EVIDENCE_PERSISTENCE","schema":"agenterm.profile-binding-v2-persistence-failure-invalid-identity/v1"}'
  fi
  return 2
}

case "${1:-}" in
  __state)
    shift
    case "${1:-}" in stage|finish) ;; *) emit_failure state_operation_not_public ;; esac
    state_operation=$1
    state_kind=${2:-}
    state_ordinal=${3:-}
    state_run_id=${4:-}
    set +e
    if [ "${AGENTERM_PROFILE_BINDING_V2_DEBUG:-0}" = 1 ]; then
      broker_data=$(run_broker "$@")
      broker_status=$?
    else
      broker_data=$(run_broker "$@" 2>/dev/null)
      broker_status=$?
    fi
    set -e
    if [ "$broker_status" -eq 0 ]; then
      printf '%s\n' "$broker_data" | perl -MJSON::PP -e '
        local $/; my $j=JSON::PP->new->canonical(1); my $d=$j->decode(<>);
        print $j->encode({ok=>JSON::PP::true,data=>$d,error=>undef}),"\n";
      '
    else
      failure_record=$(run_broker evidence-fail "$state_kind" "$state_ordinal" "$state_run_id" 2>/dev/null || true)
      if [ -z "$failure_record" ]; then
        failure_record=$(fixed_persistence_record "$state_kind" "$state_ordinal" "$state_run_id" 2>/dev/null || true)
      fi
      printf '%s\n' "$failure_record" | perl -MJSON::PP -e '
        local $/; my $j=JSON::PP->new->canonical(1); my $text=<>;
        my $e = eval { $j->decode($text) };
        $e = {code=>"INCONCLUSIVE_EVIDENCE_PERSISTENCE"} if $@ || ref($e) ne "HASH";
        if (($e->{schema}//"") eq "agenterm.profile-binding-v2-attempt/v1") {
          $e={schema=>"agenterm.profile-binding-v2-persistence-failure/v1",
            experiment=>$e->{experiment},kind=>$e->{kind},ordinal=>$e->{ordinal},
            run_id=>$e->{run_id},source_sha=>$e->{source_sha},
            input_digest=>$e->{input_digest},code=>"INCONCLUSIVE_EVIDENCE_PERSISTENCE"};
        }
        print $j->encode({ok=>JSON::PP::false,data=>{accepted=>JSON::PP::false,receipt_sha256=>undef,receipt_text=>undef},error=>$e}),"\n";
      '
      exit 2
    fi
    exit 0
    ;;
  --schema-check)
    run_broker schema-check
    exit $?
    ;;
  --audit-abandon)
    [ "$#" -eq 4 ] || emit_failure audit_abandon_expected_kind_ordinal_run_id
    audit_kind=$2
    audit_ordinal=$3
    audit_run_id=$4
    case "$audit_kind:$audit_ordinal" in
      rehearsal:R1|rehearsal:R2|decision:D1) ;;
      *) emit_failure audit_abandon_identity_invalid ;;
    esac
    printf '%s' "$audit_run_id" | grep -Eq '^[0-9a-f]{32}$' \
      || emit_failure audit_abandon_run_id_invalid
    run_broker abandon "$audit_kind" "$audit_ordinal" "$audit_run_id"
    exit $?
    ;;
  --self-test)
    formal_state="$REPO/.agenterm-research-state/browser-profile-name-binding-v2"
    { [ ! -e "$formal_state" ] && [ ! -L "$formal_state" ]; } || emit_failure self_test_formal_state_present_before
    scratch=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-profile-binding-v2.XXXXXX")
    scratch=$(CDPATH= cd -- "$scratch" && pwd -P)
    trap 'rm -rf -- "$scratch"' EXIT HUP INT TERM
    AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/schema-only" "$0" --schema-check >/dev/null
    [ ! -e "$scratch/schema-only" ] || emit_failure self_test_schema_check_wrote_state
    broker_data=$(AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/state" run_broker selftest-reserve)
    printf '%s\n' "$broker_data" >"$scratch/reserved.json"
    ordinal=$(perl -MJSON::PP -e 'local $/; print JSON::PP->new->decode(<>)->{ordinal}' "$scratch/reserved.json")
    [ "$ordinal" = R1 ] || emit_failure self_test_reservation
    perl -MJSON::PP -e '
      my $j=JSON::PP->new->canonical(1); my %criteria=map { $_=>"not-run" } qw(V1 V2 V3 V3a V3b V4a V4b G1 G2a G2b G3 G4a G4c G6 G7);
      print $j->encode({stage=>"preflight",producer=>"runner",deadline_ms=>1,
        elapsed_ms=>0,code=>"SELF_TEST_PREFLIGHT",
        facts=>{source_sha=>("b"x40),input_digest=>("a"x64),
          agenterm_sha256=>("c"x64),agenterm_cu_sha256=>("d"x64),
          browser_sha256=>("e"x64),browser_family=>"synthetic",
          browser_version=>"synthetic",state_chain_valid=>JSON::PP::true,
          result_template_sha256=>("f"x64)},
        criteria=>\%criteria,previous_receipt_sha256=>undef}),"\n";
    ' >"$scratch/stage.json"
    AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/state" AGENTERM_PROFILE_BINDING_V2_LANE_ROOT="$scratch" "$0" __state stage rehearsal R1 0123456789abcdef0123456789abcdef "$scratch/stage.json" >"$scratch/stage-envelope.json"
    previous_sha=$(perl -MJSON::PP -e 'local $/; my $v=JSON::PP->new->decode(<>); $v->{ok} && $v->{data}{accepted} or exit 1; print $v->{data}{receipt_sha256}' "$scratch/stage-envelope.json") || emit_failure self_test_preflight_stage
    perl -MJSON::PP -e '
      my $j=JSON::PP->new->canonical(1); my %criteria=map { $_=>"not-run" } qw(V1 V2 V3 V3a V3b V4a V4b G1 G2a G2b G3 G4a G4c G6 G7);
      print $j->encode({stage=>"terminal",producer=>"court",deadline_ms=>1,
        elapsed_ms=>0,code=>"INCONCLUSIVE_SOURCE",
        facts=>{primary_cause=>"INCONCLUSIVE_SOURCE",cleanup_status=>"not-run",
          final_inventory_status=>"not-run",terminal_code=>"INCONCLUSIVE_SOURCE"},
        criteria=>\%criteria,previous_receipt_sha256=>$ARGV[0]}),"\n";
    ' "$previous_sha" >"$scratch/stage.json"
    AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/state" AGENTERM_PROFILE_BINDING_V2_LANE_ROOT="$scratch" "$0" __state stage rehearsal R1 0123456789abcdef0123456789abcdef "$scratch/stage.json" >"$scratch/stage-envelope.json"
    receipt_sha=$(perl -MJSON::PP -e 'local $/; my $v=JSON::PP->new->decode(<>); $v->{ok} && $v->{data}{accepted} or exit 1; print $v->{data}{receipt_sha256}' "$scratch/stage-envelope.json") || emit_failure self_test_stage
    perl -MJSON::PP -e 'my $j=JSON::PP->new->canonical(1); print $j->encode({receipt_sha256=>$ARGV[0]}),"\n"' "$receipt_sha" >"$scratch/finish.json"
    AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/state" AGENTERM_PROFILE_BINDING_V2_LANE_ROOT="$scratch" "$0" __state finish rehearsal R1 0123456789abcdef0123456789abcdef "$scratch/finish.json" >"$scratch/finish-envelope.json"
    perl -MJSON::PP -e 'local $/; my $v=JSON::PP->new->decode(<>); $v->{ok} && $v->{data}{accepted} or exit 1' "$scratch/finish-envelope.json" || emit_failure self_test_finish
    AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/state" run_broker inspect >/dev/null
    broker_data=$(AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/abandoned-state" run_broker selftest-reserve)
    printf '%s\n' "$broker_data" >"$scratch/abandoned-reserved.json"
    AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/abandoned-state" run_broker abandon rehearsal R1 0123456789abcdef0123456789abcdef >"$scratch/abandoned.json"
    perl -MJSON::PP -e '
      local $/; my $v=JSON::PP->new->decode(<>);
      $v->{status} eq "abandoned"
        && $v->{terminal_code} eq "attempt_abandoned_after_independent_audit"
        && !defined($v->{receipt_sha256}) or exit 1;
    ' "$scratch/abandoned.json" || emit_failure self_test_abandoned_transition
    AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/abandoned-state" run_broker inspect >/dev/null
    AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/terminal-first-state" run_broker selftest-reserve >/dev/null
    perl -MJSON::PP -e '
      my $j=JSON::PP->new->canonical(1); my %criteria=map { $_=>"not-run" } qw(V1 V2 V3 V3a V3b V4a V4b G1 G2a G2b G3 G4a G4c G6 G7);
      $criteria{V1}="fail";
      print $j->encode({stage=>"terminal",producer=>"court",deadline_ms=>1,
        elapsed_ms=>0,code=>"INCONCLUSIVE_SOURCE",
        facts=>{primary_cause=>"INCONCLUSIVE_SOURCE",cleanup_status=>"not-run",
          final_inventory_status=>"not-run",terminal_code=>"INCONCLUSIVE_SOURCE"},
        criteria=>\%criteria,previous_receipt_sha256=>undef}),"\n";
    ' >"$scratch/terminal-first-stage.json"
    AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/terminal-first-state" AGENTERM_PROFILE_BINDING_V2_LANE_ROOT="$scratch" "$0" __state stage rehearsal R1 0123456789abcdef0123456789abcdef "$scratch/terminal-first-stage.json" >"$scratch/terminal-first-envelope.json"
    terminal_first_sha=$(perl -MJSON::PP -e 'local $/; my $v=JSON::PP->new->decode(<>); $v->{ok} && $v->{data}{accepted} or exit 1; print $v->{data}{receipt_sha256}' "$scratch/terminal-first-envelope.json") || emit_failure self_test_terminal_first_stage
    perl -MJSON::PP -e 'my $j=JSON::PP->new->canonical(1); print $j->encode({receipt_sha256=>$ARGV[0]}),"\n"' "$terminal_first_sha" >"$scratch/terminal-first-finish.json"
    AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/terminal-first-state" AGENTERM_PROFILE_BINDING_V2_LANE_ROOT="$scratch" "$0" __state finish rehearsal R1 0123456789abcdef0123456789abcdef "$scratch/terminal-first-finish.json" >/dev/null
    mkdir "$scratch/plain"
    ln -s plain "$scratch/link"
    if AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/link/state" run_broker inspect >/dev/null 2>&1; then
      emit_failure self_test_symlink_ancestor_followed
    fi
    mkdir "$scratch/insecure-state"
    chmod 0755 "$scratch/insecure-state"
    if AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/insecure-state" run_broker inspect >/dev/null 2>&1; then
      emit_failure self_test_insecure_state_root_accepted
    fi
    mkdir -m 0700 "$scratch/insecure-journal-state"
    mkdir -m 0755 "$scratch/insecure-journal-state/stage-journal"
    if AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$scratch/insecure-journal-state" run_broker inspect >/dev/null 2>&1; then
      emit_failure self_test_insecure_journal_root_accepted
    fi
    printf '%s\n' '#!/bin/sh' "printf '  Brave Browser 152.1.94.121 \\n'" >"$scratch/version-probe"
    chmod 0700 "$scratch/version-probe"
    normalized_version=$(read_browser_version "$scratch/version-probe") \
      || emit_failure self_test_browser_version_probe_failed
    [ "$normalized_version" = 'Brave Browser 152.1.94.121' ] \
      || emit_failure self_test_browser_version_not_trimmed
    { [ ! -e "$formal_state" ] && [ ! -L "$formal_state" ]; } || emit_failure self_test_formal_state_present_after
    printf '%s\n' '{"schema":"agenterm.profile-binding-v2-self-test/v1","code":"ok","formal_attempts_consumed":false}'
    exit 0
    ;;
esac

KIND=${1:-}
case "$KIND" in rehearsal|decision) ;; *) emit_failure usage_expected_rehearsal_or_decision ;; esac
[ $# -eq 1 ] || emit_failure unexpected_argument

AGENTERM_EXE=${AGENTERM_EXE:-"$REPO/target/debug/agenterm"}
AGENTERM_CU_EXE=${AGENTERM_CU_EXE:-"$REPO/target/debug/agenterm-cu"}
CHROMIUM_EXE=${AGENTERM_CU_BROWSER_EXE:-}
APP=${AGENTERM_CU_BROWSER_APP:-}
COURT="$ROOT/court-current-host.qjs"
MODEL="$ROOT/binding-model.qjs"
SPEC="$REPO/plan/design-browser-profile-name-binding-owned-lifecycle-experiment.md"

for pair in "$TEMPLATE:result-template.json" "$COURT:court-current-host.qjs" "$MODEL:binding-model.qjs" "$SPEC:frozen specification" "$AGENTERM_EXE:AGENTERM_EXE" "$AGENTERM_CU_EXE:AGENTERM_CU_EXE" "$CHROMIUM_EXE:AGENTERM_CU_BROWSER_EXE"; do
  path=${pair%%:*}; label=${pair#*:}; [ -n "$path" ] && [ -f "$path" ] || emit_failure "missing_$label"
done
[ -n "$APP" ] || emit_failure missing_AGENTERM_CU_BROWSER_APP

SOURCE_SHA=$(git -C "$REPO" rev-parse HEAD) || emit_failure source_sha_unavailable
git -C "$REPO" merge-base --is-ancestor "$SOURCE_SHA" origin/main || emit_failure source_not_on_origin_main
STATUS=$(git -C "$REPO" status --porcelain --untracked-files=all -- research/browser-profile-name-binding-v2 plan/design-browser-profile-name-binding-owned-lifecycle-experiment.md crates/agenterm-cu/assets/browser-bridge/manifest.json crates/agenterm-cu/assets/browser-bridge/background.js)
[ -z "$STATUS" ] || emit_failure frozen_input_dirty_or_untracked

set -- \
  plan/design-browser-profile-name-binding-owned-lifecycle-experiment.md \
  research/browser-profile-name-binding-v2/README.md \
  research/browser-profile-name-binding-v2/result-template.json \
  research/browser-profile-name-binding-v2/run-current-host.sh \
  research/browser-profile-name-binding-v2/court-current-host.qjs \
  research/browser-profile-name-binding-v2/binding-model.qjs \
  research/browser-profile-name-binding-v2/fixtures/local-state.json \
  research/browser-profile-name-binding-v2/fixtures/preferences.json \
  crates/agenterm-cu/assets/browser-bridge/manifest.json \
  crates/agenterm-cu/assets/browser-bridge/background.js
for input do git -C "$REPO" cat-file -e "$SOURCE_SHA:$input" 2>/dev/null || emit_failure input_not_tracked; done
INPUT_DIGEST=$(perl -MDigest::SHA -e '
  my ($repo,@names)=@ARGV; my $s=Digest::SHA->new(256);
  $s->add("agenterm-cu/profile-binding-owned-lifecycle-v2/input/v1\0");
  for my $n (@names) { open my $f,"<:raw","$repo/$n" or die; local $/; my $b=<$f>; $s->add(pack("Q<",length($n)),$n,pack("Q<",length($b)),$b) }
  print $s->hexdigest
' "$REPO" "$@") || emit_failure input_digest_failed
AGENTERM_SHA=$(shasum -a 256 "$AGENTERM_EXE" | awk '{print $1}')
AGENTERM_CU_SHA=$(shasum -a 256 "$AGENTERM_CU_EXE" | awk '{print $1}')
CHROMIUM_SHA=$(shasum -a 256 "$CHROMIUM_EXE" | awk '{print $1}')
TEMPLATE_SHA=$(shasum -a 256 "$TEMPLATE" | awk '{print $1}')
BROWSER_VERSION=$(read_browser_version "$CHROMIUM_EXE") || emit_failure browser_version_unavailable
case "$BROWSER_VERSION" in
  "Chrome for Testing "*) PREFIX_FAMILY="Chrome for Testing"; EXPECTED_APP="" ;;
  "Google Chrome "*) PREFIX_FAMILY="Google Chrome"; BROWSER_FAMILY="chrome"; EXPECTED_APP="Google Chrome" ;;
  "Brave Browser Beta "*) PREFIX_FAMILY="Brave Browser Beta"; EXPECTED_APP="" ;;
  "Brave Browser "*) PREFIX_FAMILY="Brave Browser"; BROWSER_FAMILY="brave-browser"; EXPECTED_APP="Brave Browser" ;;
  "Microsoft Edge "*) PREFIX_FAMILY="Microsoft Edge"; EXPECTED_APP="" ;;
  "Chromium "*) PREFIX_FAMILY="Chromium"; EXPECTED_APP="" ;;
  *) emit_failure browser_family_unsupported ;;
esac
VERSION_SUFFIX=${BROWSER_VERSION#"$PREFIX_FAMILY "}
[ -n "$VERSION_SUFFIX" ] && [ "$VERSION_SUFFIX" != "$BROWSER_VERSION" ] \
  || emit_failure browser_version_missing
[ -n "$EXPECTED_APP" ] || emit_failure browser_public_catalog_unsupported
[ "$APP" = "$EXPECTED_APP" ] || emit_failure browser_app_family_mismatch
EXPECTED_BUILD_ID=$(perl -MDigest::SHA -e '
  my ($repo)=@ARGV; my $s=Digest::SHA->new(256);
  for my $n (qw(manifest.json background.js)) { my $p="$repo/crates/agenterm-cu/assets/browser-bridge/$n"; open my $f,"<:raw",$p or die; local $/; my $b=<$f>; $s->add(pack("Q<",length($n)),$n,pack("Q<",length($b)),$b) }
  print $s->hexdigest
' "$REPO") || emit_failure build_identity_failed

export AGENTERM_PROFILE_BINDING_V2_SOURCE_SHA="$SOURCE_SHA"
export AGENTERM_PROFILE_BINDING_V2_INPUT_DIGEST="$INPUT_DIGEST"
export AGENTERM_PROFILE_BINDING_V2_STATE_ROOT="$STATE_ROOT"

RUN_ID=$(perl -MDigest::SHA=sha256_hex -e 'print substr(sha256_hex(join("\0",@ARGV,time,$$)),0,32)' "$SOURCE_SHA" "$INPUT_DIGEST")
LANE=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-profile-binding-v2-run.XXXXXX")
LANE=$(CDPATH= cd -- "$LANE" && pwd -P)
trap 'rm -rf -- "$LANE"' EXIT HUP INT TERM
mkdir "$LANE/candidate"
SESSION="profile-binding-v2-$RUN_ID"

# The court's preflight is deliberately no-side-effect. It must emit the same
# canonical candidate object; reserve happens only after this independent
# handshake and a second broker-held state validation.
AGENTERM_PROFILE_BINDING_V2_LANE_ROOT="$LANE/candidate" "$AGENTERM_EXE" cli script run --profile tool --timeout-ms 10000 --max-operations 30000000 --max-host-operations 30000 --max-output-bytes 65536 --max-string-bytes 1048576 --project-root "$REPO" "$COURT" -- preflight "$REPO" "$AGENTERM_EXE" "$AGENTERM_CU_EXE" "$CHROMIUM_EXE" "$APP" "$SESSION" "$ROOT/run-current-host.sh" "$LANE/candidate" "$SOURCE_SHA" "$INPUT_DIGEST" "$AGENTERM_SHA" "$AGENTERM_CU_SHA" "$CHROMIUM_SHA" "$EXPECTED_BUILD_ID" "$KIND" UNRESERVED "$RUN_ID" >"$LANE/preflight.json" || emit_failure court_preflight_failed
perl -MJSON::PP -e '
  my ($path,@expected)=@ARGV; open my $f,"<:raw",$path or exit 2; local $/; my $b=<$f>; $b=~s/\n\z//;
  my $j=JSON::PP->new->canonical(1); my $v=eval{$j->decode($b)}; exit 3 if $@;
  my @keys=qw(source_sha input_digest agenterm_sha256 agenterm_cu_sha256 browser_sha256 result_template_sha256 browser_family browser_version);
  exit 4 unless join("\0",sort keys %$v) eq join("\0",sort @keys) && $j->encode($v) eq $b;
  for my $i (0..$#keys) { exit 5 unless $v->{$keys[$i]} eq $expected[$i] }
' "$LANE/preflight.json" "$SOURCE_SHA" "$INPUT_DIGEST" "$AGENTERM_SHA" "$AGENTERM_CU_SHA" "$CHROMIUM_SHA" "$TEMPLATE_SHA" "$BROWSER_FAMILY" "$BROWSER_VERSION" || emit_failure preflight_digest_disagreement
RESERVED=$(AGENTERM_PROFILE_BINDING_V2_LANE_ROOT="$LANE" AGENTERM_PROFILE_BINDING_V2_REPO="$REPO" AGENTERM_PROFILE_BINDING_V2_AGENTERM_EXE="$AGENTERM_EXE" AGENTERM_PROFILE_BINDING_V2_CU_EXE="$AGENTERM_CU_EXE" AGENTERM_PROFILE_BINDING_V2_BROWSER_EXE="$CHROMIUM_EXE" run_broker reserve "$KIND" "$RUN_ID" "$LANE/preflight.json") || emit_failure compare_and_reserve_failed
printf '%s\n' "$RESERVED"

ORDINAL=$(printf '%s\n' "$RESERVED" | perl -MJSON::PP -e 'local $/; print JSON::PP->new->decode(<>)->{ordinal}')
TIMEOUT=180000; [ "$KIND" = decision ] && TIMEOUT=240000
set +e
AGENTERM_PROFILE_BINDING_V2_LANE_ROOT="$LANE/candidate" "$AGENTERM_EXE" cli script run --profile tool --timeout-ms "$TIMEOUT" --max-operations 300000000 --max-host-operations 300000 --max-output-bytes 262144 --max-string-bytes 1048576 --project-root "$REPO" "$COURT" -- "$KIND" "$REPO" "$AGENTERM_EXE" "$AGENTERM_CU_EXE" "$CHROMIUM_EXE" "$APP" "$SESSION" "$ROOT/run-current-host.sh" "$LANE/candidate" "$SOURCE_SHA" "$INPUT_DIGEST" "$AGENTERM_SHA" "$AGENTERM_CU_SHA" "$CHROMIUM_SHA" "$EXPECTED_BUILD_ID" "$KIND" "$ORDINAL" "$RUN_ID"
court_status=$?
set -e
[ "$court_status" -eq 0 ] || {
  finish_evidence_failure "$KIND" "$ORDINAL" "$RUN_ID" || true
  exit 2
}
# The court performs the public finish call after atomically mirroring and
# reading back the terminal receipt. A successful court exit therefore means
# the finished ledger row already exists.
if ! FINISHED=$(run_broker verify-finished "$KIND" "$ORDINAL" "$RUN_ID" 2>/dev/null); then
  finish_evidence_failure "$KIND" "$ORDINAL" "$RUN_ID" || true
  exit 2
fi
printf '%s\n' "$FINISHED"
