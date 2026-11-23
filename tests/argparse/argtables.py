#!/usr/bin/env python3
"""Compare native tables with the in-tree AWS CLI's registered argument tables.

xtask supplies the dependency source path. The worker imports that source in a
cached venv holding the upstream AWS CLI's hash-pinned runtime lock.
"""

import argparse
import difflib
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any, Final

# This checker is executed by path, so its only import path is its own
# directory. The venv helpers live one level up, in tests/; append rather
# than prepend, as tests/blackbox/run.py does.
sys.path.append(str(Path(__file__).resolve().parents[1]))

import harness_venv

VENV_DEPS_REVISION: Final = "1"
UPSTREAM_RUNTIME_REQUIREMENTS: Final = (
    "requirements/download-deps/system-sandbox-lock.txt"
)


def describe_table(
    parser: Any, cli_types: dict[str, str]
) -> list[dict[str, Any]]:
    """Describe each argparse action in the form the drift check compares."""
    actions = {
        "_StoreAction": "store",
        "_StoreTrueAction": "store_true",
        "_StoreFalseAction": "store_false",
        "_AppendAction": "append",
        "_VersionAction": "version",
        "AppendFilter": "append_named",
        "CommandAction": "command",
    }
    result = []
    for action in parser._actions:
        name = (
            action.option_strings[0][2:]
            if action.option_strings
            else action.dest
        )
        kind = actions[type(action).__name__]
        cli_type = cli_types.get(name)
        result.append(
            {
                "name": name,
                "positional": not action.option_strings,
                "action": kind,
                "nargs": action.nargs if action.nargs != 0 else None,
                "const": (
                    action.const
                    if kind not in {"store_true", "store_false"}
                    else None
                ),
                "default": (
                    None
                    if action.default == argparse.SUPPRESS
                    else action.default
                ),
                "choices": list(action.choices or []),
                "dest": action.dest,
                "required": action.required,
                "cli_type": (
                    cli_type
                    if cli_type in {"integer", "map", "blob"}
                    else None
                ),
            }
        )
    return result


def upstream_tables() -> dict[str, Any]:
    """Build only parser tables; no command is called or credential resolved."""
    import awscli
    from awscli.argparser import ArgTableArgParser, CLIArgParser
    from awscli.arguments import CustomArgument
    from awscli.customizations.s3.s3 import (
        S3,
        register_s3_sync_strategies,
    )
    from botocore.loaders import Loader
    from botocore.session import Session

    session = Session()
    session.register_component(
        "data_loader",
        Loader(
            extra_search_paths=[str(Path(awscli.__file__).parent / "data")],
            include_default_search_paths=False,
        ),
    )
    register_s3_sync_strategies(session)
    source = session.get_component("data_loader").load_data("cli")["options"]
    globals_parser = CLIArgParser(add_help=False, conflict_handler="resolve")
    cli_types = {}
    for name, definition in source.items():
        data = {
            key: value
            for key, value in definition.items()
            if key not in {"type", "help"}
        }
        CustomArgument(name, **data).add_to_parser(globals_parser)
        if definition.get("type") == "int":
            cli_types[name] = "integer"

    def command_table(command: Any) -> dict[str, Any]:
        # This property fires sync-strategy registration before arg_table.
        children = command.subcommand_table
        arguments = command.arg_table
        parser = ArgTableArgParser(arguments)
        return {
            "name": command.name,
            "kind": "basic",
            "usage": getattr(command, "USAGE", None),
            "arguments": describe_table(
                parser,
                {name: arg.cli_type_name for name, arg in arguments.items()},
            ),
            "subcommands": (
                [command_table(child) for child in children.values()]
                if children
                else None
            ),
        }

    return {
        "globals": describe_table(globals_parser, cli_types),
        "s3": command_table(S3(session)),
    }


def describe_derived(parser: Any, arguments: dict[str, Any]) -> list[dict[str, Any]]:
    """An operation's parser and its table's objects, in the fields the
    derived check compares: the drift check's, then argparse's value type,
    `_UNDOCUMENTED` and `cli_type_name`."""
    types = {int: "int", float: "float"}
    described = describe_table(parser, {})
    for row, action, argument in zip(described, parser._actions, arguments.values()):
        del row["cli_type"]
        for key in ("const", "default"):
            row[key] = plain_json(row[key])
        row["choices"] = [plain_json(choice) for choice in row["choices"]]
        row["type"] = types.get(action.type)
        row["hidden"] = bool(getattr(argument, "_UNDOCUMENTED", False))
        row["cli_type_name"] = argument.cli_type_name
    return described


def isolate_home(home: Path) -> None:
    """Point the CLI at an empty home before it loads, as the native side's
    `--home` does: no config, alias file, plugins or user models of the
    caller's."""
    home.mkdir(parents=True, exist_ok=True)
    os.environ["HOME"] = str(home)
    os.environ["USERPROFILE"] = str(home)


def plain_json(value: Any) -> Any:
    """A value as JSON writes it, or its type's name for one it cannot
    write: a customization's sentinel default, say."""
    try:
        json.dumps(value)
    except TypeError:
        return f"<{type(value).__name__}>"
    return value


def derived_tables(commands: list[str]) -> dict[str, Any]:
    """The root table's model commands, and each named root's command table
    and operation tables, from a CLI driver as `aws` builds one. Nothing is
    called; the tables are built as a command's parse would build them."""
    from awscli.argparser import ArgTableArgParser
    from awscli.clidriver import ServiceCommand, ServiceOperation, create_clidriver

    driver = create_clidriver()
    table = driver._get_command_table()
    roots = sorted(
        name for name, command in table.items() if isinstance(command, ServiceCommand)
    )
    services: dict[str, Any] = {}
    for name in commands:
        command = table.get(name)
        if not isinstance(command, ServiceCommand):
            services[name] = None
            continue
        children = command._get_command_table()
        operations = {}
        for operation_name, operation in list(children.items()):
            if not isinstance(operation, ServiceOperation):
                continue
            arguments = operation.arg_table
            operations[operation_name] = describe_derived(
                ArgTableArgParser(arguments), arguments
            )
        services[name] = {"choices": list(children), "operations": operations}
    return {"roots": roots, "services": services}


def compare_derived(upstream: dict[str, Any], native: dict[str, Any]) -> list[str]:
    """Every difference on a table the derivation renders natively, and a
    note for each it leaves to Python."""
    lines = []
    if upstream["roots"] != native["roots"]:
        lines.append("roots differ: " + diff_text(upstream["roots"], native["roots"]))
    for name, service in upstream["services"].items():
        mine = native["services"].get(name)
        if service is None or mine is None:
            if service != mine:
                lines.append(f"{name}: root on one side only")
            continue
        if mine["customized"]:
            print(f"{name}: command table customized; failures stay Python's")
        elif service["choices"] != mine["choices"]:
            lines.append(f"{name}: " + diff_text(service["choices"], mine["choices"]))
        for operation, arguments in service["operations"].items():
            derived = mine["operations"].get(operation)
            if derived is None:
                lines.append(f"{name} {operation}: not derived")
            elif derived["customized"]:
                print(f"{name} {operation}: table customized; failures stay Python's")
            elif derived["arguments"] != arguments:
                lines.append(f"{name} {operation}: " + diff_text(arguments, derived["arguments"]))
    return lines


def diff_text(upstream: Any, native: Any) -> str:
    left = json.dumps(upstream, indent=1).splitlines(keepends=True)
    right = json.dumps(native, indent=1).splitlines(keepends=True)
    return "".join(difflib.unified_diff(left, right, "in-tree AWS CLI", "derived"))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", action="store_true")
    parser.add_argument("--derived", nargs="*", metavar="COMMAND")
    parser.add_argument("--home", type=Path)
    parser.add_argument("--aws-cli-source")
    parser.add_argument("--cache-dir")
    parser.add_argument("--native-tables", type=Path)
    args = parser.parse_args()
    if args.worker:
        if args.derived is not None:
            isolate_home(args.home)
            print(json.dumps(derived_tables(args.derived)))
        else:
            print(json.dumps(upstream_tables()))
        return
    if not all(
        (
            args.aws_cli_source,
            args.cache_dir,
            args.native_tables,
        )
    ):
        parser.error("source, cache-dir and native-tables are required")
    harness_venv.validate_host_python("the argument table check")
    source = Path(args.aws_cli_source).resolve()
    cache_dir = Path(args.cache_dir).resolve()
    python = harness_venv.ensure_venv(
        cache_dir / "venv",
        [source / UPSTREAM_RUNTIME_REQUIREMENTS],
        VENV_DEPS_REVISION,
    )
    env = os.environ.copy()
    python_paths = [str(source)]
    if existing := env.get("PYTHONPATH"):
        python_paths.append(existing)
    env["PYTHONPATH"] = os.pathsep.join(python_paths)
    env["PYTHONPYCACHEPREFIX"] = str(cache_dir / "pycache")
    worker = [str(python), str(Path(__file__).resolve()), "--worker"]
    if args.derived is not None:
        if args.home is None:
            parser.error("--derived requires --home")
        worker += ["--home", str(args.home), "--derived", *args.derived]
    completed = subprocess.run(
        worker,
        env=env,
        capture_output=True,
        text=True,
        timeout=600,
    )
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr)
        raise SystemExit(completed.returncode)
    upstream = json.loads(completed.stdout)
    native = json.loads(args.native_tables.read_text())
    if args.derived is not None:
        differences = compare_derived(upstream, native)
        sys.stderr.writelines(line + "\n" for line in differences)
        if differences:
            raise SystemExit(1)
        print(f"Derived tables match the in-tree AWS CLI for {' '.join(args.derived)}.")
        return
    if native != upstream:
        left = json.dumps(upstream, indent=2).splitlines(keepends=True)
        right = json.dumps(native, indent=2).splitlines(keepends=True)
        sys.stderr.writelines(
            difflib.unified_diff(
                left, right, fromfile="in-tree AWS CLI", tofile="native tables"
            )
        )
        raise SystemExit(1)
    print(
        "Argument tables match in-tree AWS CLI globals and all nine S3 commands."
    )


if __name__ == "__main__":
    main()
