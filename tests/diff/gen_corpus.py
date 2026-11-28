"""Generate the ec2 rows of the differential corpus, mechanically.

`corpus/ec2.jsonl` holds, in the model's operation order:

- one row per ec2 operation the native route admits, which is every ec2
  row of `crates/model-tools/tests/verdicts/verdicts.tsv` (read, never
  written) whose standing is not `refused:*`: the service, the operation
  and its required arguments, each filled as below;
- hand rows for each argument argrename renames on an admitted
  operation (`--<new name>`, and for a boolean the other spelling too),
  and for each argument toplevelbool pulls up: `--flag`, `--no-flag`,
  `--flag Value=false`, and both halves together, which raises
  `validate_boolean_mutex_groups`' error and sends nothing.

Required members are the input shape's, idempotency tokens aside. A value
is filled from its shape: a string is its first enum value or `x`
repeated to its `min` length, an integer or long its `min` or 1, a
float or double 1.5, a boolean `--name`, a timestamp
`2015-08-30T12:36:00Z`, a blob `AAAA`, a structure `{}` with its own
required members filled, a list one filled member, a map `{}`; anything
not a scalar is compact JSON. A toplevelbool member is given as `--flag`.

Every row answers each request with the operation's empty response
document (`<{Operation}Response xmlns=".../"/>`, ec2's `text/xml`), and
declares `"requests": 0` where botocore's validator rejects the filled
parameters (with any idempotency token filled, as botocore fills it) or
the row raises before the call.

`--out FILE` writes the corpus; `--check FILE` fails unless FILE is what
the generator writes. Either first attacks the check with drifts it must
refuse (a row dropped, added or swapped, an argument added, a request
declaration dropped, a reply changed, the last newline dropped) and
checks the rows' shape (`self_test`). Run it in the differential harness's venv, which
`cargo xtask diff` provisions, so it reads the in-tree botocore:

    target/diff/venv/bin/python -B tests/diff/gen_ec2_corpus.py \\
        --aws-cli-source aws-cli \\
        --verdicts crates/model-tools/tests/verdicts/verdicts.tsv \\
        --check tests/diff/corpus/ec2.jsonl
"""

from __future__ import annotations

import argparse
import ast
import json
import sys
from pathlib import Path
from typing import Any, Final

API_VERSION: Final = "2016-11-15"
MODEL: Final = f"aws-cli/awscli/botocore/data/ec2/{API_VERSION}/service-2.json"
XMLNS: Final = f"http://ec2.amazonaws.com/doc/{API_VERSION}/"


def admitted(verdicts: Path) -> set[str]:
    """The ec2 operations, by CLI name, whose standing is not a refusal."""
    found = set()
    lines = verdicts.read_text(encoding="utf-8").splitlines()
    header = lines[0].split("\t")
    command, operation, standing = (
        header.index("command"),
        header.index("operation"),
        header.index("standing"),
    )
    for line in lines[1:]:
        fields = line.split("\t")
        if fields[command] == "ec2" and not fields[standing].startswith("refused"):
            found.add(fields[operation])
    return found


def renames(source: Path) -> list[tuple[str, str, str]]:
    """argrename's ec2 rows: (operation or `*`, argument, new name)."""
    tree = ast.parse((source / "awscli/customizations/argrename.py").read_text())
    rows = []
    for node in tree.body:
        if isinstance(node, ast.Assign) and getattr(node.targets[0], "id", "") == "ARGUMENT_RENAMES":
            for key, new in ast.literal_eval(node.value).items():
                portion, argument = key.rsplit(".", 1)
                command, operation = portion.split(".", 1)
                if command == "ec2":
                    rows.append((operation, argument, new))
    return rows


def pulled_up(shape: Any) -> bool:
    """toplevelbool's `_qualifies_for_simplification`."""
    return (
        shape.type_name == "structure"
        and list(shape.members) == ["Value"]
        and shape.members["Value"].type_name == "boolean"
    )


def sample(shape: Any) -> Any:
    """A filled value of `shape`, as the parameters hold it."""
    kind = shape.type_name
    metadata = shape.metadata
    if kind == "string":
        enum = metadata.get("enum")
        return enum[0] if enum else "x" * max(1, int(metadata.get("min", 1)))
    if kind in ("integer", "long"):
        return max(1, int(metadata.get("min", 1)))
    if kind in ("float", "double"):
        return 1.5
    if kind == "boolean":
        return True
    if kind == "timestamp":
        return "2015-08-30T12:36:00Z"
    if kind == "blob":
        return "AAAA"
    if kind == "list":
        return [sample(shape.member)]
    if kind == "map":
        return {}
    return {name: sample(shape.members[name]) for name in shape.required_members}


def text(value: Any) -> str:
    """A filled value as argv spells it."""
    if isinstance(value, str):
        return value
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return str(value)
    return json.dumps(value, separators=(",", ":"))


def required(operation: Any) -> list[str]:
    shape = operation.input_shape
    if shape is None:
        return []
    return [
        name
        for name in shape.required_members
        if not shape.members[name].metadata.get("idempotencyToken")
    ]


def arguments(
    operation: Any, xform_name: Any, skip: str | None
) -> tuple[list[str], dict[str, Any]]:
    """The filled required arguments but `skip`, and the parameters they
    bind."""
    argv: list[str] = []
    params: dict[str, Any] = {}
    for name in required(operation):
        if name == skip:
            continue
        member = operation.input_shape.members[name]
        flag = f"--{xform_name(name, '-')}"
        if member.type_name == "boolean":
            argv.append(flag)
            params[name] = True
        elif pulled_up(member):
            argv.append(flag)
            params[name] = {"Value": True}
        else:
            value = sample(member)
            argv += [flag, text(value)]
            params[name] = value
    return argv, params


def valid(operation: Any, params: dict[str, Any]) -> bool:
    from botocore.exceptions import ParamValidationError
    from botocore.validate import validate_parameters

    shape = operation.input_shape
    if shape is None:
        return True
    # botocore fills an idempotency token the call leaves out.
    tokens = {
        name: "token"
        for name, member in shape.members.items()
        if member.metadata.get("idempotencyToken") and name not in params
    }
    try:
        validate_parameters({**params, **tokens}, shape)
    except ParamValidationError:
        return False
    return True


def row(
    operation: Any,
    cli_name: str,
    suffix: str,
    extra: list[str],
    extra_params: dict[str, Any],
    sends: bool,
    xform_name: Any,
) -> dict[str, Any]:
    """One row; the member `extra_params` binds is given by `extra` alone."""
    skip = next(iter(extra_params), None)
    argv, params = arguments(operation, xform_name, skip)
    params.update(extra_params)
    built: dict[str, Any] = {
        "id": f"ec2-{cli_name}{suffix}",
        "source": f"{MODEL}#{operation.name}",
        "argv": ["ec2", cli_name, *argv, *extra],
        "responses": [
            {
                "status": 200,
                "headers": [["Content-Type", "text/xml;charset=UTF-8"]],
                "body": f'<{operation.name}Response xmlns="{XMLNS}"/>',
            }
        ],
    }
    if not (sends and valid(operation, params)):
        built["requests"] = 0
    return built


def hand_rows(operation: Any, cli_name: str, rows: list[tuple[str, str, str]], xform_name: Any):
    """The renamed and pulled-up arguments' rows of one operation."""
    shape = operation.input_shape
    if shape is None:
        return
    members = {xform_name(name, "-"): name for name in shape.members}
    pairs = {cli: name for cli, name in members.items() if pulled_up(shape.members[name])}
    for cli, name in pairs.items():
        flag, negative = f"--{cli}", f"--no-{cli}"
        for suffix, extra, value, sends in [
            (f"-{cli}", [flag], {"Value": True}, True),
            (f"-no-{cli}", [negative], {"Value": False}, True),
            (f"-{cli}-valued", [flag, "Value=false"], {"Value": False}, True),
            (f"-{cli}-both", [flag, negative], {}, False),
        ]:
            yield row(operation, cli_name, suffix, extra, {name: value}, sends, xform_name)
    for scope, argument, new in rows:
        if scope not in ("*", cli_name):
            continue
        for cli, name in members.items():
            member = shape.members[name]
            if cli in pairs:
                continue
            if member.type_name == "boolean" and argument in (cli, f"no-{cli}"):
                other = f"--{cli}" if argument == f"no-{cli}" else f"--no-{cli}"
                positive = argument == cli
                yield row(operation, cli_name, f"-{new}", [f"--{new}"], {name: positive}, True, xform_name)
                yield row(operation, cli_name, f"-{new}-other", [other], {name: not positive}, True, xform_name)
            elif member.type_name != "boolean" and argument == cli:
                value = sample(member)
                yield row(operation, cli_name, f"-{new}", [f"--{new}", text(value)], {name: value}, True, xform_name)


def corpus(source: Path, verdicts: Path) -> str:
    sys.path.insert(0, str(source))
    import awscli  # noqa: F401  (aliases botocore to the in-tree copy)
    from botocore import xform_name
    from botocore.session import get_session

    model = get_session().get_service_model("ec2")
    if model.api_version != API_VERSION:
        raise SystemExit(f"ec2 is {model.api_version}, not {API_VERSION}")
    wanted = admitted(verdicts)
    ec2_renames = renames(source)
    rows = []
    for name in model.operation_names:
        cli_name = xform_name(name, "-")
        if cli_name not in wanted:
            continue
        operation = model.operation_model(name)
        rows.append(row(operation, cli_name, "", [], {}, True, xform_name))
        rows.extend(hand_rows(operation, cli_name, ec2_renames, xform_name))
    missing = wanted - {built["argv"][1] for built in rows}
    if missing:
        raise SystemExit(f"admitted operations the model lacks: {sorted(missing)}")
    return "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows)


def matches(expected: str, found: str) -> bool:
    """`--check`'s test: the file is the generator's output, byte for byte."""
    return found == expected


def evasions(written: str) -> list[tuple[str, str]]:
    """Files `--check` must refuse: each a plausible drift of `written`."""
    rows = written.splitlines(keepends=True)
    first = json.loads(rows[0])
    altered = dict(first, argv=[*first["argv"], "--dry-run"])
    undeclared = {key: value for key, value in json.loads(
        next(row for row in rows if '"requests": 0' in row)
    ).items() if key != "requests"}
    return [
        ("a row dropped", "".join(rows[1:])),
        ("a row added", written + rows[-1]),
        ("two rows swapped", "".join([rows[1], rows[0], *rows[2:]])),
        ("an argument added", json.dumps(altered, ensure_ascii=False) + "\n" + "".join(rows[1:])),
        ("a request declaration dropped", written.replace(
            next(row for row in rows if '"requests": 0' in row),
            json.dumps(undeclared, ensure_ascii=False) + "\n", 1)),
        ("a reply changed", written.replace("Response xmlns", "Result xmlns", 1)),
        ("the last newline dropped", written[:-1]),
    ]


def self_test(written: str, wanted: set[str]) -> None:
    """Attack `--check` with each evasion, then check the rows' shape:
    one base row per admitted operation, four per pulled-up pair, and a
    request declared absent on every `-both` row."""
    for name, evasion in evasions(written):
        if matches(written, evasion):
            raise SystemExit(f"--check accepts {name}")
    rows = [json.loads(line) for line in written.splitlines()]
    base = {row["argv"][1] for row in rows if row["id"] == f"ec2-{row['argv'][1]}"}
    if base != wanted:
        raise SystemExit(f"base rows differ from the admitted set: {sorted(base ^ wanted)}")
    for row in rows:
        if row["id"].endswith("-both") and row.get("requests") != 0:
            raise SystemExit(f"{row['id']} must declare no request")
    for suffix in ("-both", "-valued"):
        pairs = sum(row["id"].endswith(suffix) for row in rows)
        if pairs == 0:
            raise SystemExit(f"no {suffix} rows")
    print(f"self-test: {len(evasions(written))} evasions refused, {len(rows)} rows well-formed")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aws-cli-source", type=Path, required=True)
    parser.add_argument("--verdicts", type=Path, required=True)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--out", type=Path)
    group.add_argument("--check", type=Path)
    args = parser.parse_args()
    written = corpus(args.aws_cli_source.resolve(), args.verdicts)
    self_test(written, admitted(args.verdicts))
    if args.out is not None:
        args.out.write_text(written, encoding="utf-8")
        return
    if not matches(written, args.check.read_text(encoding="utf-8")):
        raise SystemExit(f"{args.check} is not what gen_ec2_corpus.py writes")
    print(f"{args.check}: {written.count(chr(10))} rows, as generated")


if __name__ == "__main__":
    main()
