#!/bin/sh
set -eu

attempt="${1:-}"
case "$attempt" in
  1|2) ;;
  *) printf '%s\n' '{"ok":false,"code":"attempt_invalid"}'; exit 2 ;;
esac

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"
research_dir=research/mcp-persisted-session
target_lane=target/research-mcp-persisted-session
source_sha=$(git rev-parse HEAD)
experiment_base=9354e8c4505c87a3a893dfb109a187baf5485693

git merge-base --is-ancestor "$source_sha" origin/main || {
  printf '%s\n' '{"ok":false,"code":"source_not_reachable"}'
  exit 2
}

expected_files='Cargo.lock
Cargo.toml
README.md
RESULTS.md
fixture.json
harness.rs
result-template.json
run.sh'
actual_files=$(git ls-tree -r --name-only "$source_sha" -- "$research_dir" | sed "s#^$research_dir/##")
[ "$actual_files" = "$expected_files" ] || {
  printf '%s\n' '{"ok":false,"code":"tracked_file_set_mismatch"}'
  exit 2
}
git diff --quiet "$source_sha" -- plan/design-mcp-persisted-session-experiment.md || {
  printf '%s\n' '{"ok":false,"code":"specification_dirty"}'
  exit 2
}
git diff --name-only -z "$experiment_base" "$source_sha" | python3 -c 'import sys
names=[p.decode() for p in sys.stdin.buffer.read().split(b"\0") if p]
bad=[p for p in names if not (p.startswith("research/mcp-persisted-session/") or p=="plan/design-mcp-persisted-session-experiment.md")]
raise SystemExit(9 if bad else 0)' || {
  printf '%s\n' '{"ok":false,"code":"product_source_changed"}'
  exit 2
}
git diff-tree --no-commit-id --name-only -z -r "$source_sha" | python3 -c 'import sys
names=[p.decode() for p in sys.stdin.buffer.read().split(b"\0") if p]
raise SystemExit(9 if any(not p.startswith("research/mcp-persisted-session/") for p in names) else 0)' || {
  printf '%s\n' '{"ok":false,"code":"product_source_changed"}'; exit 2;
}

for name in $expected_files; do
  git diff --quiet "$source_sha" -- "$research_dir/$name" || {
    printf '%s\n' '{"ok":false,"code":"digest_input_dirty"}'
    exit 2
  }
done
[ -z "$(git ls-files --others --exclude-standard -- "$research_dir")" ] || {
  printf '%s\n' '{"ok":false,"code":"research_file_untracked"}'
  exit 2
}

input_digest=$(python3 - "$source_sha" <<'PY'
import hashlib, pathlib, struct, sys
names = [
    "plan/design-mcp-persisted-session-experiment.md",
    "research/mcp-persisted-session/Cargo.toml",
    "research/mcp-persisted-session/Cargo.lock",
    "research/mcp-persisted-session/README.md",
    "research/mcp-persisted-session/harness.rs",
    "research/mcp-persisted-session/fixture.json",
    "research/mcp-persisted-session/result-template.json",
    "research/mcp-persisted-session/run.sh",
]
h = hashlib.sha256(b"agenterm-research/mcp-persisted-session-input/v1\0")
for name in names:
    data = pathlib.Path(name).read_bytes()
    encoded = name.encode()
    h.update(struct.pack(">Q", len(encoded)))
    h.update(encoded)
    h.update(struct.pack(">Q", len(data)))
    h.update(data)
print(h.hexdigest())
PY
)
contract_digest=$(python3 - <<'PY'
import hashlib, pathlib, struct
h=hashlib.sha256(b"agenterm-research/mcp-persisted-session-contract/v1\0")
for p in ["plan/design-mcp-persisted-session-experiment.md", "research/mcp-persisted-session/result-template.json"]:
 d=pathlib.Path(p).read_bytes(); h.update(struct.pack(">Q",len(d))); h.update(d)
print(h.hexdigest())
PY
)
criteria_digest=$(python3 - <<'PY'
import hashlib, pathlib
s=pathlib.Path("plan/design-mcp-persisted-session-experiment.md").read_text()
h=pathlib.Path("research/mcp-persisted-session/harness.rs").read_text()
parts=[s[s.index("## 4."):s.index("## 5.")],
 h[h.index("fn criteria_for("):h.index("fn tie_inventory(")],
 h[h.index("fn expected_phase("):h.index("fn monotone(")]]
print(hashlib.sha256("\0".join(parts).encode()).hexdigest())
PY
)
decision_digest=$(python3 - <<'PY'
import hashlib, pathlib
s=pathlib.Path("plan/design-mcp-persisted-session-experiment.md").read_text()
h=pathlib.Path("research/mcp-persisted-session/harness.rs").read_text()
parts=[s[s.index("## 5."):s.index("## 6.")],
 h[h.index("fn tie_inventory("):h.index("fn expected_denial(")]]
print(hashlib.sha256("\0".join(parts).encode()).hexdigest())
PY
)

attempt_one_receipt="$target_lane/invocation-1/receipt.json"
attempt_ledger="$target_lane/attempt-ledger.jsonl"
ledger_state=$(python3 - "$attempt_ledger" <<'PY'
import json, pathlib, sys
p=pathlib.Path(sys.argv[1])
if not p.exists(): print("empty"); raise SystemExit(0)
rows=[json.loads(line) for line in p.read_text().splitlines() if line]
expected=[(1,"reserved"),(1,"finished"),(2,"reserved"),(2,"finished")]
for i,row in enumerate(rows):
 if i>=len(expected) or (row.get("attempt"),row.get("state")) != expected[i] or not all(k in row for k in ("source_sha","chain_digest")):
  raise SystemExit(8)
print("empty" if not rows else f"{rows[-1]['attempt']}-{rows[-1]['state']}")
PY
) || { printf '%s\n' '{"ok":false,"code":"attempt_ledger_invalid"}'; exit 2; }
reviewed_digest=$(sed -n 's/^Attempt 1 review: receipt_sha256=\([0-9a-f]\{64\}\) failure_class=fixture-contract-failure repair_marker_sha256=[0-9a-f]\{64\}$/\1/p' "$research_dir/RESULTS.md")
repair_marker=$(sed -n 's/^Attempt 1 review: receipt_sha256=[0-9a-f]\{64\} failure_class=fixture-contract-failure repair_marker_sha256=\([0-9a-f]\{64\}\)$/\1/p' "$research_dir/RESULTS.md")
actual_reviewed=
if [ "$attempt" = 1 ]; then
  [ "$ledger_state" = empty ] && [ ! -e "$attempt_one_receipt" ] && [ -z "$reviewed_digest" ] || {
    printf '%s\n' '{"ok":false,"code":"attempt_budget_consumed"}'; exit 2;
  }
  chain_digest=$(printf '%s\0%s' "$source_sha" "$input_digest" | shasum -a 256 | awk '{print $1}')
else
  [ "$ledger_state" = 1-finished ] || {
    printf '%s\n' '{"ok":false,"code":"attempt_one_not_finished"}'; exit 2;
  }
  [ -f "$attempt_one_receipt" ] && [ ${#reviewed_digest} -eq 64 ] && [ ${#repair_marker} -eq 64 ] || {
    printf '%s\n' '{"ok":false,"code":"attempt_repair_chain_missing"}'; exit 2;
  }
  actual_reviewed=$(shasum -a 256 "$attempt_one_receipt" | awk '{print $1}')
  [ "$actual_reviewed" = "$reviewed_digest" ] || {
    printf '%s\n' '{"ok":false,"code":"attempt_review_digest_mismatch"}'; exit 2;
  }
  attempt_one_source=$(python3 - "$attempt_one_receipt" "$contract_digest" "$criteria_digest" "$decision_digest" <<'PY'
import json, pathlib, sys
v=json.loads(pathlib.Path(sys.argv[1]).read_text())
if not (v.get("terminal") is True and v.get("attempt")==1
        and v.get("failure_class")=="fixture-contract-failure"
        and v.get("decision")=="INCONCLUSIVE_FIXTURE_REPAIRABLE"
        and v.get("contract_digest")==sys.argv[2]
        and v.get("criteria_digest")==sys.argv[3]
        and v.get("decision_digest")==sys.argv[4]):
    raise SystemExit(8)
print(v["source_sha"])
PY
  ) || { printf '%s\n' '{"ok":false,"code":"attempt_one_contract_invalid"}'; exit 2; }
  git merge-base --is-ancestor "$attempt_one_source" "$source_sha" || {
    printf '%s\n' '{"ok":false,"code":"attempt_repair_source_not_descendant"}'; exit 2;
  }
  git diff --name-only -z "$attempt_one_source" "$source_sha" | python3 -c 'import sys
names=[p.decode() for p in sys.stdin.buffer.read().split(b"\0") if p]
allowed={
 "research/mcp-persisted-session/RESULTS.md",
 "research/mcp-persisted-session/fixture.json",
 "research/mcp-persisted-session/README.md",
}
raise SystemExit(9 if any(p not in allowed for p in names) else 0)' || {
    printf '%s\n' '{"ok":false,"code":"attempt_repair_scope_invalid"}'; exit 2;
  }
  for frozen in run.sh result-template.json Cargo.toml Cargo.lock harness.rs; do
    git diff --quiet "$attempt_one_source" "$source_sha" -- "$research_dir/$frozen" || {
      printf '%s\n' '{"ok":false,"code":"attempt_contract_surface_changed"}'; exit 2;
    }
  done
  python3 - "$attempt_one_source" <<'PY' || {
import json, pathlib, subprocess, sys
path="research/mcp-persisted-session/fixture.json"
before=json.loads(subprocess.check_output(["git","show",sys.argv[1]+":"+path]))
after=json.loads(pathlib.Path(path).read_text())
changed={key for key in set(before)|set(after) if before.get(key)!=after.get(key)}
allowed={"grant_selector","store_selector","binding","idempotency_key","payload_digest","marker"}
raise SystemExit(9 if changed-allowed else 0)
PY
    printf '%s\n' '{"ok":false,"code":"attempt_fixture_field_invalid"}'; exit 2;
  }
  expected_marker=$(printf '%s\0%s\0%s' "$actual_reviewed" "$attempt_one_source" 'fixture-observation-repair' | shasum -a 256 | awk '{print $1}')
  [ "$repair_marker" = "$expected_marker" ] || {
    printf '%s\n' '{"ok":false,"code":"attempt_repair_marker_invalid"}'; exit 2;
  }
  chain_digest=$(printf '%s\0%s\0%s' "$actual_reviewed" "$source_sha" "$repair_marker" | shasum -a 256 | awk '{print $1}')
fi

mkdir -p "$target_lane"

CARGO_TARGET_DIR="$target_lane" cargo build --locked --manifest-path "$research_dir/Cargo.toml" --bin mcp-persisted-session-harness --quiet
executable="$target_lane/debug/mcp-persisted-session-harness"
executable_digest=$(shasum -a 256 "$executable" | awk '{print $1}')
"$executable" --self-test >/dev/null

invocation="$target_lane/invocation-$attempt"
mkdir "$invocation" 2>/dev/null || {
  printf '%s\n' '{"ok":false,"code":"invocation_already_exists"}'
  exit 2
}
python3 - "$attempt_ledger" "$attempt" "$source_sha" "$chain_digest" reserved <<'PY'
import json, os, pathlib, sys
p=pathlib.Path(sys.argv[1])
with p.open("a") as f:
 f.write(json.dumps({"attempt":int(sys.argv[2]),"source_sha":sys.argv[3],"chain_digest":sys.argv[4],"state":sys.argv[5]},separators=(",",":"))+"\n")
 f.flush(); os.fsync(f.fileno())
PY
cp "$research_dir/fixture.json" "$invocation/fixture.json"
target=$(rustc -vV | sed -n 's/^host: //p')
python3 - "$invocation/metadata.json" "$source_sha" "$input_digest" "$executable_digest" "$target" "$attempt" "$chain_digest" "$experiment_base" "$contract_digest" "$criteria_digest" "$decision_digest" "$actual_reviewed" "$repair_marker" <<'PY'
import hashlib, json, pathlib, struct, subprocess, sys
changed = sorted(p for p in subprocess.check_output(
    ["git", "diff", "--name-only", "-z", sys.argv[8], sys.argv[2]]
).decode().split("\0") if p)
members_hash=hashlib.sha256(b"agenterm-research/mcp-persisted-session-source-members/v1\0")
for name in changed:
 encoded=name.encode(); members_hash.update(struct.pack(">Q",len(encoded))); members_hash.update(encoded)
product = [p for p in changed if not p.startswith("research/mcp-persisted-session/") and p != "plan/design-mcp-persisted-session-experiment.md"]
descriptors = [p for p in product if p.endswith(".json") or p.endswith(".toml")]
catalogs = [p for p in product if "catalog" in p or "alignment" in p or "gate" in p]
pathlib.Path(sys.argv[1]).write_text(json.dumps({
    "source_sha": sys.argv[2],
    "input_digest": sys.argv[3],
    "executable_digest": sys.argv[4],
    "target": sys.argv[5],
    "attempt": int(sys.argv[6]),
    "chain_digest": sys.argv[7],
    "chain_inputs": [sys.argv[2],sys.argv[3]] if int(sys.argv[6]) == 1 else [sys.argv[12],sys.argv[2],sys.argv[13]],
    "contract_digest": sys.argv[9],
    "criteria_digest": sys.argv[10],
    "decision_digest": sys.argv[11],
    "source_inventory": {
        "experiment_base_sha": sys.argv[8],
        "source_tree_oid": subprocess.check_output(["git","rev-parse",sys.argv[2]+"^{tree}"],text=True).strip(),
        "source_members_digest": members_hash.hexdigest(),
        "product_sources_changed": len(product),
        "descriptors_changed": len(descriptors),
        "catalogs_changed": len(catalogs),
        "source_members": changed,
    },
}, separators=(",", ":")) + "\n")
PY

receipt="$invocation/receipt.json"
python3 - "$executable" "$invocation/state" "$invocation/fixture.json" "$invocation/metadata.json" "$receipt" <<'PY'
import pathlib, subprocess, sys
with pathlib.Path(sys.argv[5]).open("wb") as output:
    try:
        completed = subprocess.run(
            [sys.argv[1], "--run", sys.argv[2], sys.argv[3], sys.argv[4]],
            stdin=subprocess.DEVNULL,
            stdout=output,
            stderr=subprocess.DEVNULL,
            timeout=900,
            check=False,
        )
    except subprocess.TimeoutExpired:
        raise SystemExit(4)
if completed.returncode != 0:
    raise SystemExit(completed.returncode)
PY
python3 - "$receipt" "$research_dir/result-template.json" <<'PY'
import json, pathlib, re, sys
path = pathlib.Path(sys.argv[1])
raw = path.read_bytes()
if len(raw) > 262144:
    raise SystemExit(3)
def reject_duplicate_keys(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("duplicate JSON key")
        value[key] = item
    return value
try:
    value = json.loads(raw, object_pairs_hook=reject_duplicate_keys)
    template = json.loads(
        pathlib.Path(sys.argv[2]).read_text(),
        object_pairs_hook=reject_duplicate_keys,
    )
except (json.JSONDecodeError, ValueError):
    raise SystemExit(3)
schema = template.pop("_schema")
boolean_fields=set(schema["boolean_fields"])
optional_boolean_fields=set(schema["optional_boolean_fields"])
required_string_fields=set(schema["required_string_fields"])
optional_string_fields=set(schema["optional_string_fields"])
integer_fields=set(schema["integer_fields"])
boolean_maps=set(schema["boolean_maps"])
integer_arrays=set(schema["integer_arrays"])
string_arrays=set(schema["string_arrays"])
empty_array_paths=set(schema["empty_array_paths"])
if set(schema)!={"null_default","boolean_fields","optional_boolean_fields","required_string_fields","optional_string_fields","integer_fields","boolean_maps","integer_arrays","string_arrays","empty_array_paths"} or schema["null_default"]!="explicit-field-sets":
    raise SystemExit(3)
def shape(value, model, path=()):
    if isinstance(model, dict):
        return isinstance(value, dict) and set(value)==set(model) and all(shape(value[k],model[k],path+(k,)) for k in model)
    if isinstance(model, list):
        if not isinstance(value, list): return False
        if not model:
            dotpath=".".join(path)
            if dotpath in empty_array_paths: return len(value)==0
            if path and path[-1] in string_arrays: return all(isinstance(x,str) for x in value)
            return False
        if all(x is None for x in model):
            scalar_ok = (lambda x: isinstance(x,int) and not isinstance(x,bool)) if path[-1] in integer_arrays else (lambda x: isinstance(x,str))
            return len(value)==len(model) and all(scalar_ok(x) for x in value)
        if len(model)==1: return all(shape(x,model[0],path+("[]",)) for x in value)
        return len(value)==len(model) and all(shape(x,y,path+("[]",)) for x,y in zip(value,model))
    if model is None:
        field=path[-1]
        if len(path)>1 and path[-2] in boolean_maps: return isinstance(value,bool)
        if field in boolean_fields: return isinstance(value,bool)
        if field in optional_boolean_fields: return value is None or isinstance(value,bool)
        if field in integer_fields: return isinstance(value,int) and not isinstance(value,bool)
        if field in required_string_fields: return isinstance(value,str) and len(value)>0
        if field in optional_string_fields: return value is None or isinstance(value,str)
        return False
    return type(value) is type(model)
if set(value) != set(template) or set(value.get("compatibility_inventory", {})) != set(template["compatibility_inventory"]):
    raise SystemExit(3)
if value.get("schema_version") != 1 or value.get("precommitment") != "mcp-persisted-session-experiment" or value.get("privacy_matches") != 0 or value.get("terminal") is not True:
    raise SystemExit(3)
if any(not isinstance(value.get(k),str) or not value.get(k) for k in ("target","fixture_gate","decision")):
    raise SystemExit(3)
if not re.fullmatch(r"[a-z0-9_-]{1,128}",value["fixture_gate"]):
    raise SystemExit(3)
hex40=lambda text: isinstance(text,str) and len(text)==40 and all(c in "0123456789abcdef" for c in text)
hex64=lambda text: isinstance(text,str) and len(text)==64 and all(c in "0123456789abcdef" for c in text)
if not hex40(value.get("source_sha")) or any(not hex64(value.get(k)) for k in ("input_digest","executable_digest","chain_digest","contract_digest","criteria_digest","decision_digest")):
    raise SystemExit(3)
if not shape(value.get("compatibility_inventory"), template["compatibility_inventory"], ("compatibility_inventory",)):
    raise SystemExit(3)
compat=value["compatibility_inventory"]
if value.get("fixture_gate") == "passed":
    if compat.get("accepted_operations") != ["shell-exec"] or compat.get("protocol_members") != ["shell-exec"] or compat.get("job_device_constructible") is not False:
        raise SystemExit(3)
    if value.get("decision") not in {"REJECT_BOTH","SELECT_A1_BOUNDED_SESSION","SELECT_B_REQUEST_DIRECT","INCONCLUSIVE_MODEL"} or value.get("failure_class") is not None:
        raise SystemExit(3)
    if value.get("negative_control", {}).get("variant") != "a0":
        raise SystemExit(3)
    if not shape(value["negative_control"], template["negative_control"], ("negative_control",)):
        raise SystemExit(3)
    negative=value["negative_control"]
    if negative.get("lifecycle_codes") != ["operation-mismatch"]*3 or len(negative.get("slope",[])) != 3 or len(negative.get("tie_counts",[])) != 6:
        raise SystemExit(3)
    variants=value.get("variants", [])
    if sorted(v.get("variant") for v in variants) != ["a1", "b"]:
        raise SystemExit(3)
    if any(set(v.get("criteria", {})) != {"C1","C2","C3","C4","C5","C6","C7"} for v in variants):
        raise SystemExit(3)
    if not shape(variants, template["variants"], ("variants",)):
        raise SystemExit(3)
    for variant in variants:
        label=variant.get("variant")
        expected_denials=12 if variant.get("variant")=="a1" else 7
        if len(variant.get("phase_traces",[])) != 5 or len(variant.get("denial_traces",[])) != expected_denials or len(variant.get("slope",[])) != 3 or len(variant.get("tie_counts",[])) != 6:
            raise SystemExit(3)
        phases=variant["phase_traces"]
        if sorted(p.get("phase") for p in phases) != ["F-1","F0","F1","F2","F3"] or any(p.get("variant")!=label for p in phases):
            raise SystemExit(3)
        if any(p.get("session_teardown_observed") != (True if label=="a1" and p.get("phase")!="F2" else None) for p in phases):
            raise SystemExit(3)
        if any(d.get("variant")!=label for d in variant["denial_traces"]):
            raise SystemExit(3)
    if not shape(value.get("command_probes", []), template["command_probes"], ("command_probes",)):
        raise SystemExit(3)
    if sorted({p.get("variant") for p in value.get("command_probes", [])}) != ["a1", "b"]:
        raise SystemExit(3)
    if len(value.get("command_probes", [])) != 22 or any(sum(p.get("variant")==variant for p in value["command_probes"]) != 11 for variant in ("a1","b")):
        raise SystemExit(3)
else:
    if compat.get("accepted_operations") != [] or compat.get("protocol_members") != [] or compat.get("job_device_constructible") is not False:
        raise SystemExit(3)
    terminal=(value.get("decision"),value.get("failure_class"))
    allowed={
      ("INCONCLUSIVE_FIXTURE_REPAIRABLE","fixture-contract-failure"),
      ("INCONCLUSIVE_FIXTURE_EXHAUSTED","fixture-contract-failure"),
      ("REJECT_PRIVACY_SECURITY","privacy-security-failure"),
      ("INCONCLUSIVE_NONREPAIRABLE","experiment-execution-failure"),
    }
    if terminal not in allowed or value.get("negative_control") is not None or value.get("variants") or value.get("command_probes"):
        raise SystemExit(3)
sys.stdout.buffer.write(raw)
PY
python3 - "$attempt_ledger" "$attempt" "$source_sha" "$chain_digest" finished <<'PY'
import json, os, pathlib, sys
p=pathlib.Path(sys.argv[1])
with p.open("a") as f:
 f.write(json.dumps({"attempt":int(sys.argv[2]),"source_sha":sys.argv[3],"chain_digest":sys.argv[4],"state":sys.argv[5]},separators=(",",":"))+"\n")
 f.flush(); os.fsync(f.fileno())
PY
