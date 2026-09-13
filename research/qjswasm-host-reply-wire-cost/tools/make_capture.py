#!/usr/bin/env python3
"""Derive the reply-wire-cost capture copies from the pristine journeys.

The capture copies are *reporters*, not the experiment's control runs. Each one
is the pristine journey plus one added reporter that records, for every host
CLI reply, the exact two texts the guest receives -- the door envelope and the
payload text the journey parses -- so the reply set can be frozen once and
replayed by the pricing court.

Nothing else is changed: every replacement below is a byte-for-byte insertion
around the existing door call. The diff against the pristine script is recorded
in replies/<journey>/manifest.json.

Usage (from repository root):
    python3 research/qjswasm-host-reply-wire-cost/tools/make_capture.py
"""

import difflib
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
OUT = ROOT / "research" / "qjswasm-host-reply-wire-cost" / "capture"

SHIM = '''
// ---- reply-wire-cost capture shim (research reporter; research/ only) ----
// Appended by tools/make_capture.py. Records the door envelope text and the
// payload text the journey parses for every CLI reply, so the reply set can be
// frozen once and replayed by the pricing court. Byte counts are recomputed
// from the recorded files, never from `.length` (per-call O(n) on this engine).
const capture_dir = rh.env_or("AGENTERM_REPLY_CAPTURE_DIR", "");
let capture_seq = 0;
let capture_index = "";

function capture_pad(n) {
  const text = "" + n;
  if (text.length >= 4) { return text; }
  if (text.length === 3) { return "0" + text; }
  if (text.length === 2) { return "00" + text; }
  return "000" + text;
}

// `source`: "envelope" when the journey read the payload out of the door
// envelope, "file" when it read it back from a redirected file.
function capture_record(capture_arguments, envelope_text, output, source) {
  if (capture_dir === "") { return; }
  const name = capture_pad(capture_seq);
  capture_seq = capture_seq + 1;
  rh.write_text(rh.join(capture_dir, "env-" + name + ".txt"), envelope_text);
  if (source !== "envelope") {
    rh.write_text(rh.join(capture_dir, "pay-" + name + ".txt"), output.stdout);
  }
  if (output.stderr !== "") {
    rh.write_text(rh.join(capture_dir, "err-" + name + ".txt"), output.stderr);
  }
  const args = [];
  for (const a of capture_arguments) { args.push("" + a); }
  const line = JSON.stringify({
    seq: capture_seq - 1,
    arguments: args,
    source: source,
    exit_code: output.exit_code
  });
  capture_index = capture_index + line + "\\n";
  rh.write_text(rh.join(capture_dir, "index.jsonl"), capture_index);
}

// `rh.command_spec` with the envelope kept: same door calls, same order.
function capture_command_spec(spec, capture_arguments) {
  if (process_command(JSON.stringify(spec)) !== 0) { throw "process_command " + spec.program + ":" + tool_result(); }
  const envelope_text = tool_result();
  const output = JSON.parse(envelope_text);
  capture_record(capture_arguments, envelope_text, output, "envelope");
  return output;
}
// ---- end capture shim ----------------------------------------------------
'''

REPLACEMENTS = {
    # server-smoke: `run()` is the journey's single CLI choke point. On a Unix
    # host the answer is read back from a redirected file, so the payload is a
    # second door fetch; on Windows it rides inside the envelope.
    "server-smoke": [
        (
            '  if (process_command(JSON.stringify(spec)) !== 0) { throw "process_command:" + tool_result(); }\n'
            '  const output = JSON.parse(tool_result());\n'
            '  if (answer_path !== "") {\n'
            '    output.stdout = rh.exists(answer_path) ? rh.read_text(answer_path) : "";\n'
            '    rh.try_remove_file(answer_path);\n'
            '  }\n',
            '  if (process_command(JSON.stringify(spec)) !== 0) { throw "process_command:" + tool_result(); }\n'
            '  const envelope_text = tool_result();\n'
            '  const output = JSON.parse(envelope_text);\n'
            '  let capture_source = "envelope";\n'
            '  if (answer_path !== "") {\n'
            '    output.stdout = rh.exists(answer_path) ? rh.read_text(answer_path) : "";\n'
            '    rh.try_remove_file(answer_path);\n'
            '    capture_source = "file";\n'
            '  }\n'
            '  capture_record(arguments, envelope_text, output, capture_source);\n',
        ),
    ],
    # native-ipc-smoke: every reply crosses through `rh.command_spec`, called
    # from `invoke`, `probe` and `probe_protocol` (the last through
    # `run_filtered`). The envelope always carries stdout here.
    "native-ipc-smoke": [
        (
            "function run_filtered(spec) {",
            "function run_filtered(spec, capture_arguments) {",
        ),
        (
            '  spec.program = "sh";\n  spec.args = wrapped_args;\n  return rh.command_spec(spec);\n',
            '  spec.program = "sh";\n  spec.args = wrapped_args;\n'
            '  if (capture_arguments === undefined) { capture_arguments = []; }\n'
            "  return capture_command_spec(spec, capture_arguments);\n",
        ),
        (
            "  spec.timeout_ms = 15000;\n  const output = rh.command_spec(spec);\n",
            "  spec.timeout_ms = 15000;\n"
            "  const output = capture_command_spec(spec, arguments);\n",
        ),
        (
            "  spec.timeout_ms = 2000;\n  const output = rh.command_spec(spec);\n",
            "  spec.timeout_ms = 2000;\n"
            "  const output = capture_command_spec(spec, arguments);\n",
        ),
        (
            "  const output = unix_host !== 0 ? run_filtered(spec) : rh.command_spec(spec);\n",
            "  const output = unix_host !== 0\n"
            "    ? run_filtered(spec, arguments)\n"
            "    : capture_command_spec(spec, arguments);\n",
        ),
        (
            "      const alias_output = run_filtered(alias_spec);\n",
            "      const alias_output = run_filtered(alias_spec, alias_arguments);\n",
        ),
    ],
    # workbench-smoke: every CLI call goes through the harness's `run_cli` /
    # `invoke_cli`, which the reporter replaces locally (same door calls, the
    # same command record appended) so the envelope can be kept.
    "workbench-smoke": [
        (
            "import * as harness from \"lib/test_harness\";\n",
            "import * as harness from \"lib/test_harness\";\n" + SHIM,
        ),
        (
            "function json_cli(context, cli, args) {\n"
            "  const response = harness.invoke_cli(context, cli, args, 0, []);\n",
            "function json_cli(context, cli, args) {\n"
            "  const response = capture_invoke_cli(context, cli, args, 0, []);\n",
        ),
        (
            "function text_cli(context, cli, args) {\n"
            "  const response = harness.invoke_cli(context, cli, args, 0, []);\n",
            "function text_cli(context, cli, args) {\n"
            "  const response = capture_invoke_cli(context, cli, args, 0, []);\n",
        ),
        (
            "    const probe = harness.run_cli(context, cli, [\"ui-snapshot\"], 0, []);\n",
            "    const probe = capture_run_cli(context, cli, [\"ui-snapshot\"], 0, []);\n",
        ),
        (
            "  const output = harness.run_cli(context, cli, [\"ui-snapshot\"], 0, []);\n",
            "  const output = capture_run_cli(context, cli, [\"ui-snapshot\"], 0, []);\n",
        ),
    ],
}

# workbench's local replacements of the two harness entry points.
WORKBENCH_WRAPPERS = '''
// Local replacements of the harness entry points the journey already used
// (same door calls, same command record); they keep the door envelope.
function capture_run_cli(context, cli, args, expected_failure, redactions) {
  const all = ["--address", "" + context.address];
  for (const a of args) { all.push("" + a); }
  const spec = harness.configured_cli_spec(context, cli, all);
  if (process_command(JSON.stringify(spec)) !== 0) { throw "process_command " + spec.program + ":" + tool_result(); }
  const envelope_text = tool_result();
  const output = JSON.parse(envelope_text);
  capture_record(args, envelope_text, output, "envelope");
  harness.append_command_record(context, args, output, expected_failure, redactions);
  return output;
}

function capture_invoke_cli(context, cli, args, expected_failure, redactions) {
  const output = capture_run_cli(context, cli, args, expected_failure, redactions);
  if (expected_failure !== 0) {
    harness.require(!output.success, "test_harness_cli_unexpected_success");
  } else {
    harness.require(output.success, "test_harness_cli_failed:" + harness.redact_text(output.stderr, redactions));
  }
  return output;
}
'''


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    failures = []
    for journey, edits in REPLACEMENTS.items():
        source = (ROOT / "scripts" / "qjs" / f"{journey}.qjs").read_text()
        text = source
        if journey != "workbench-smoke":
            marker = 'import * as harness from "lib/test_harness";\n'
            if marker not in text:
                failures.append(f"{journey}: harness import marker missing")
                continue
            text = text.replace(marker, marker + SHIM, 1)
        for old, new in edits:
            count = text.count(old)
            if count != 1:
                failures.append(f"{journey}: {count} matches for {old.splitlines()[0]!r}")
                break
            text = text.replace(old, new, 1)
        else:
            if journey == "workbench-smoke":
                anchor = "function json_cli(context, cli, args) {"
                if text.count(anchor) != 1:
                    failures.append("workbench-smoke: wrapper anchor missing")
                    continue
                text = text.replace(anchor, WORKBENCH_WRAPPERS.lstrip("\n") + "\n" + anchor, 1)
            target = OUT / f"{journey}.capture.qjs"
            target.write_text(text)
            diff = difflib.unified_diff(
                source.splitlines(keepends=True),
                text.splitlines(keepends=True),
                fromfile=f"scripts/qjs/{journey}.qjs",
                tofile=f"research/qjswasm-host-reply-wire-cost/capture/{journey}.capture.qjs",
            )
            (OUT / f"{journey}.diff").write_text("".join(diff))
            print(f"{journey}: wrote {target.name} (+{len(text.splitlines()) - len(source.splitlines())} lines)")
    if failures:
        for failure in failures:
            print(f"FAIL {failure}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
