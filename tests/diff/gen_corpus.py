"""Generate corpus files of the differential corpus, mechanically.

Each corpus (`CORPORA`) is over a list of services. `ec2` writes
`corpus/ec2.jsonl`, which holds, in the model's operation order:

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

`command-adders` writes `corpus/command-adders.jsonl`: for each service
whose table a command adder edits (`ADDERS`: the awscli initializers that
set hand-written commands on a root table, which the native route covers
for the service's other operations), in that list's order,

- up to three operations the native route admits (none for emr, whose
  parser hook acts on every operation), filled as above, the
  most structurally varied (`varied`): in turn, the one whose filled
  required arguments bring the most value kinds not yet chosen (a string,
  an enum, a number, a boolean, a timestamp, a blob, a list, a map, a
  structure, nothing required), the first in model order on a tie;
- one row per command of the service's table, as the in-tree CLI builds it
  (`create_clidriver`), that is no model operation of its own name: each
  a command adder sets, adds or puts in place of an operation, which the
  native route leaves to Python (`unsupported_subcommand`). `wait` and
  `wizard` are the cross-cutting ones, and not listed. Each is given an
  option no command has, which Python's parser refuses before any
  request, so the row declares none.

Each reply is the protocol's empty answer: ec2's and query's empty
`<{Operation}Response>` document in the service's namespace, json's and
rest-json's `{}`, rest-xml's empty body, and rpc-v2-cbor's empty map.

`--out FILE` writes the corpus; `--check FILE` fails unless FILE is what
the generator writes. Either first attacks the check with drifts it must
refuse (a row dropped, added or swapped, an argument added, a request
declaration dropped, a reply changed, the last newline dropped) and
checks the rows' shape (`self_test`). Run it in the differential harness's venv, which
`cargo xtask diff` provisions, so it reads the in-tree botocore:

    target/diff/venv/bin/python -B tests/diff/gen_corpus.py ec2 \\
        --aws-cli-source aws-cli \\
        --verdicts crates/model-tools/tests/verdicts/verdicts.tsv \\
        --check tests/diff/corpus/ec2.jsonl
    target/diff/venv/bin/python -B tests/diff/gen_corpus.py command-adders \\
        --aws-cli-source aws-cli \\
        --verdicts crates/model-tools/tests/verdicts/verdicts.tsv \\
        --check tests/diff/corpus/command-adders.jsonl
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

# The root commands whose tables the command adders edit, by CLI name.
ADDERS: Final = [
    "cloudformation",
    "cloudfront",
    "cloudtrail",
    "codeartifact",
    "codecommit",
    "configservice",
    "datapipeline",
    "deploy",
    "dlm",
    "dsql",
    "ec2-instance-connect",
    "ecr",
    "ecr-public",
    "ecs",
    "eks",
    "emr",
    "emr-containers",
    "gamelift",
    "lambda-web",
    "lightsail",
    "logs",
    "rds",
    "servicecatalog",
    "ssm",
    "sso",
]
# The cross-cutting commands a service's table may hold that are no
# operation: `add_waiters`' and the wizards'.
CROSS_CUTTING: Final = {"wait", "wizard"}
# An option no command defines.
NO_SUCH_OPTION: Final = "--no-such-option"
SAMPLED: Final = 3


def admitted(verdicts: Path, command_name: str = "ec2") -> set[str]:
    """The operations of `command_name`, by CLI name, whose standing is
    not a refusal."""
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
        if fields[command] == command_name and not fields[standing].startswith("refused"):
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


def reply(operation: Any) -> dict[str, Any]:
    """The protocol's empty answer to `operation`."""
    model = operation.service_model
    protocol = model.protocol
    if protocol in ("ec2", "query"):
        namespace = XMLNS if protocol == "ec2" else model.metadata["xmlNamespace"]
        return {
            "status": 200,
            "headers": [["Content-Type", "text/xml;charset=UTF-8"]],
            "body": f'<{operation.name}Response xmlns="{namespace}"/>',
        }
    if protocol == "json":
        version = model.metadata.get("jsonVersion", "1.0")
        return {
            "status": 200,
            "headers": [["Content-Type", f"application/x-amz-json-{version}"]],
            "body": "{}",
        }
    if protocol == "rest-json":
        return {"status": 200, "headers": [["Content-Type", "application/json"]], "body": "{}"}
    if protocol == "rest-xml":
        return {"status": 200, "headers": [], "body": ""}
    if protocol == "smithy-rpc-v2-cbor":
        return {
            "status": 200,
            "headers": [["Content-Type", "application/cbor"], ["smithy-protocol", "rpc-v2-cbor"]],
            "body_base64": "oA==",
        }
    raise SystemExit(f"{model.service_name}: no empty answer for {protocol}")


def model_path(operation: Any) -> str:
    model = operation.service_model
    return (
        f"aws-cli/awscli/botocore/data/{model.service_name}/{model.api_version}"
        f"/service-2.json"
    )


def row(
    operation: Any,
    cli_name: str,
    suffix: str,
    extra: list[str],
    extra_params: dict[str, Any],
    sends: bool,
    xform_name: Any,
    command_name: str = "ec2",
) -> dict[str, Any]:
    """One row; the member `extra_params` binds is given by `extra` alone."""
    skip = next(iter(extra_params), None)
    argv, params = arguments(operation, xform_name, skip)
    params.update(extra_params)
    built: dict[str, Any] = {
        "id": f"{command_name}-{cli_name}{suffix}",
        "source": f"{model_path(operation)}#{operation.name}",
        "argv": [command_name, cli_name, *argv, *extra],
        "responses": [reply(operation)],
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


def ec2_corpus(source: Path, verdicts: Path) -> str:
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


def kinds(shape: Any, seen: set[str] | None = None) -> set[str]:
    """The value kinds a filled `shape` holds, nested members' too."""
    seen = set() if seen is None else seen
    kind = shape.type_name
    if kind == "string" and shape.metadata.get("enum"):
        seen.add("enum")
    elif kind in ("integer", "long", "float", "double"):
        seen.add("number")
    else:
        seen.add(kind)
    if kind == "list":
        kinds(shape.member, seen)
    elif kind == "structure":
        for name in shape.required_members:
            kinds(shape.members[name], seen)
    return seen


def operation_kinds(operation: Any) -> set[str]:
    """The value kinds `operation`'s filled required arguments hold, or
    `nothing` for none."""
    names = required(operation)
    if not names:
        return {"nothing"}
    found: set[str] = set()
    for name in names:
        kinds(operation.input_shape.members[name], found)
    return found


def varied(operations: list[Any], count: int) -> list[Any]:
    """Up to `count` of `operations`, in turn the one that brings the most
    kinds not yet chosen, the first on a tie; in their own order."""
    chosen: list[Any] = []
    covered: set[str] = set()
    pending = list(operations)
    while pending and len(chosen) < count:
        best = max(pending, key=lambda op: len(operation_kinds(op) - covered))
        chosen.append(best)
        covered |= operation_kinds(best)
        pending.remove(best)
    return [op for op in operations if op in chosen]


def python_commands(driver: Any, command_name: str, model: Any, xform_name: Any) -> list[str]:
    """The names of `command_name`'s table, as the CLI builds it, that are
    no model operation of their own name, but the cross-cutting ones."""
    from awscli.clidriver import ServiceOperation

    table = driver._get_command_table()[command_name].subcommand_table
    names = []
    for name, command in table.items():
        if name in CROSS_CUTTING:
            continue
        own = (
            type(command) is ServiceOperation
            and xform_name(command._operation_model.name, "-") == name
            and command._operation_model.service_model.service_name == model.service_name
        )
        if not own:
            names.append(name)
    return names


def adders_corpus(source: Path, verdicts: Path) -> str:
    sys.path.insert(0, str(source))
    import awscli  # noqa: F401  (aliases botocore to the in-tree copy)
    from awscli.clidriver import create_clidriver
    from botocore import xform_name

    driver = create_clidriver()
    rows = []
    for command_name in ADDERS:
        service = driver._get_command_table()[command_name]
        model = service._get_service_model()
        wanted = admitted(verdicts, command_name)
        operations = [
            model.operation_model(name)
            for name in model.operation_names
            if xform_name(name, "-") in wanted
        ]
        for operation in varied(operations, SAMPLED):
            cli_name = xform_name(operation.name, "-")
            rows.append(row(operation, cli_name, "", [], {}, True, xform_name, command_name))
        kept = python_commands(driver, command_name, model, xform_name)
        if not kept:
            raise SystemExit(f"{command_name}: no command kept on Python")
        for name in kept:
            rows.append(
                {
                    "id": f"{command_name}-{name}-python",
                    "source": f"awscli command table of {command_name}",
                    "argv": [command_name, name, NO_SUCH_OPTION],
                    "responses": [{"status": 200, "headers": [], "body": ""}],
                    "requests": 0,
                }
            )
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


def attack(written: str) -> None:
    """Attack `--check` with each evasion."""
    for name, evasion in evasions(written):
        if matches(written, evasion):
            raise SystemExit(f"--check accepts {name}")


def adders_self_test(written: str, verdicts: Path) -> None:
    """Attack `--check`, then check the rows' shape: each service of
    `ADDERS` has three rows of admitted operations, or one per operation
    the verdicts admit where they admit fewer, each of an
    operation the verdicts admit, and at least one row kept on Python,
    which declares no request and gives the unknown option."""
    attack(written)
    rows = [json.loads(line) for line in written.splitlines()]
    ids = [row["id"] for row in rows]
    if len(ids) != len(set(ids)):
        raise SystemExit("row ids repeat")
    for command_name in ADDERS:
        mine = [row for row in rows if row["argv"][0] == command_name]
        native = [row for row in mine if not row["id"].endswith("-python")]
        python = [row for row in mine if row["id"].endswith("-python")]
        wanted = admitted(verdicts, command_name)
        if len(native) != min(SAMPLED, len(wanted)):
            raise SystemExit(f"{command_name}: {len(native)} admitted rows")
        for row in native:
            if row["argv"][1] not in wanted:
                raise SystemExit(f"{row['id']} is not admitted")
        if not python:
            raise SystemExit(f"{command_name}: no row kept on Python")
        for row in python:
            if row["argv"][1] in wanted or row.get("requests") != 0:
                raise SystemExit(f"{row['id']} must be no admitted operation and send nothing")
            if row["argv"][2:] != [NO_SUCH_OPTION]:
                raise SystemExit(f"{row['id']} must give only {NO_SUCH_OPTION}")
    if {row["argv"][0] for row in rows} != set(ADDERS):
        raise SystemExit("rows of a service outside ADDERS")
    print(f"self-test: {len(evasions(written))} evasions refused, {len(rows)} rows well-formed")


def self_test(written: str, wanted: set[str]) -> None:
    """Attack `--check` with each evasion, then check the rows' shape:
    one base row per admitted operation, four per pulled-up pair, and a
    request declared absent on every `-both` row."""
    attack(written)
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


CORPORA: Final = {
    "ec2": (ec2_corpus, lambda written, verdicts: self_test(written, admitted(verdicts))),
    "command-adders": (adders_corpus, adders_self_test),
}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("corpus", choices=sorted(CORPORA))
    parser.add_argument("--aws-cli-source", type=Path, required=True)
    parser.add_argument("--verdicts", type=Path, required=True)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--out", type=Path)
    group.add_argument("--check", type=Path)
    args = parser.parse_args()
    generate, check = CORPORA[args.corpus]
    written = generate(args.aws_cli_source.resolve(), args.verdicts)
    check(written, args.verdicts)
    if args.out is not None:
        args.out.write_text(written, encoding="utf-8")
        return
    if not matches(written, args.check.read_text(encoding="utf-8")):
        raise SystemExit(f"{args.check} is not what gen_corpus.py {args.corpus} writes")
    print(f"{args.check}: {written.count(chr(10))} rows, as generated")


if __name__ == "__main__":
    main()
