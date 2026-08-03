#!/usr/bin/env python3
"""Transform XERJ's exact pinned MiniLM export into its fused FP32 artifact."""

from __future__ import annotations

import argparse
import collections
import contextlib
import copy
import ctypes
import errno
import hashlib
import importlib.metadata
import json
import os
import pathlib
import platform
import re
import shutil
import sys
import tempfile
from typing import Any, Iterator

import onnx
from onnxruntime.transformers.optimizer import optimize_model


FORMAT = "xerj-onnx-minilm-fp32-recipe-v1"
MODEL_FILENAME = "model.bert-fused-fp32.onnx"
MANIFEST_FILENAME = "model.bert-fused-fp32.manifest.json"
HERE = pathlib.Path(__file__).resolve().parent
CONTRACT_PATH = HERE / "artifact-contract.json"
CONTRACT = json.loads(CONTRACT_PATH.read_text())

SOURCE_SHA256 = CONTRACT["source"]["model"]["sha256"]
SOURCE_BYTES = CONTRACT["source"]["model"]["bytes"]
TOKENIZER_SHA256 = CONTRACT["source"]["tokenizer"]["sha256"]
TOKENIZER_BYTES = CONTRACT["source"]["tokenizer"]["bytes"]
CANDIDATE_SHA256 = CONTRACT["artifact"]["sha256"]
CANDIDATE_BYTES = CONTRACT["artifact"]["bytes"]
PACKAGE_VERSIONS = CONTRACT["optimizer"]["packages"]
EXPECTED_INPUTS = CONTRACT["graph"]["inputs"]
EXPECTED_OUTPUTS = CONTRACT["graph"]["outputs"]
EXPECTED_CANDIDATE_OPERATORS = CONTRACT["graph"]["operators"]
EXPECTED_OPSETS = CONTRACT["graph"]["opsets"]
EXPECTED_IR_VERSION = CONTRACT["graph"]["ir_version"]
OPTIMIZER_ARGUMENT_SCHEMA = {
    "hidden_size": int,
    "model_type": str,
    "num_heads": int,
    "only_onnxruntime": bool,
    "opt_level": int,
    "use_external_data_format": bool,
    "use_gpu": bool,
}
OPTIMIZER_ENTRYPOINT = "onnxruntime.transformers.optimizer.optimize_model"


class RecipeError(RuntimeError):
    """A fail-closed recipe or artifact-contract error."""


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def verify_file(
    path: pathlib.Path, *, label: str, expected_bytes: int, expected_sha256: str
) -> None:
    if not path.is_file():
        raise RecipeError(f"{label} does not exist or is not a regular file: {path}")
    actual_bytes = path.stat().st_size
    if actual_bytes != expected_bytes:
        raise RecipeError(
            f"{label} byte length mismatch: expected {expected_bytes}, got "
            f"{actual_bytes}; refusing to optimize an unknown asset"
        )
    actual_sha256 = sha256(path)
    if actual_sha256 != expected_sha256:
        raise RecipeError(
            f"{label} SHA-256 mismatch: expected {expected_sha256}, got "
            f"{actual_sha256}; refusing to optimize an unknown asset"
        )


def _checked_value_type(name: str, value: Any, expected: type) -> None:
    # bool is a subclass of int, but is not a valid integer optimizer setting.
    if type(value) is not expected:
        raise RecipeError(
            f"optimizer.arguments.{name} must be {expected.__name__}, "
            f"got {type(value).__name__}"
        )


def optimizer_configuration(
    contract: dict[str, Any] | None = None,
) -> tuple[dict[str, Any], bool]:
    """Return the strictly validated optimizer and serializer configuration."""
    selected = CONTRACT if contract is None else contract
    optimizer = selected.get("optimizer")
    if not isinstance(optimizer, dict):
        raise RecipeError("contract optimizer must be an object")
    if optimizer.get("entrypoint") != OPTIMIZER_ENTRYPOINT:
        raise RecipeError(
            f"optimizer.entrypoint must be {OPTIMIZER_ENTRYPOINT!r}"
        )
    arguments = optimizer.get("arguments")
    if not isinstance(arguments, dict):
        raise RecipeError("optimizer.arguments must be an object")
    expected_names = set(OPTIMIZER_ARGUMENT_SCHEMA)
    actual_names = set(arguments)
    if actual_names != expected_names:
        raise RecipeError(
            "optimizer.arguments keys mismatch: expected "
            f"{sorted(expected_names)!r}, got {sorted(actual_names)!r}"
        )
    for name, expected_type in OPTIMIZER_ARGUMENT_SCHEMA.items():
        _checked_value_type(name, arguments[name], expected_type)
    if arguments["hidden_size"] <= 0 or arguments["num_heads"] <= 0:
        raise RecipeError("optimizer hidden_size and num_heads must be positive")
    if arguments["hidden_size"] % arguments["num_heads"] != 0:
        raise RecipeError("optimizer hidden_size must be divisible by num_heads")
    if not arguments["model_type"]:
        raise RecipeError("optimizer model_type must not be empty")
    optimize_arguments = dict(arguments)
    external_data = optimize_arguments.pop("use_external_data_format")
    return optimize_arguments, external_data


def required_python(
    contract: dict[str, Any] | None = None,
) -> tuple[str, tuple[int, int, int]]:
    selected = CONTRACT if contract is None else contract
    optimizer = selected.get("optimizer")
    if not isinstance(optimizer, dict):
        raise RecipeError("contract optimizer must be an object")
    value = optimizer.get("python")
    if not isinstance(value, str):
        raise RecipeError("optimizer.python must be a string")
    match = re.fullmatch(r"(CPython) ([0-9]+)\.([0-9]+)\.([0-9]+)", value)
    if match is None:
        raise RecipeError(
            "optimizer.python must have the form 'CPython <major>.<minor>.<patch>'"
        )
    return match.group(1), tuple(int(part) for part in match.groups()[1:])


def current_toolchain_matches_contract() -> bool:
    try:
        implementation, version = required_python()
        if platform.python_implementation() != implementation:
            return False
        if sys.version_info[:3] != version:
            return False
        return all(
            importlib.metadata.version(package) == expected
            for package, expected in PACKAGE_VERSIONS.items()
        )
    except (RecipeError, importlib.metadata.PackageNotFoundError):
        return False


def verify_toolchain() -> dict[str, str]:
    implementation, version = required_python()
    required = f"{implementation} {'.'.join(str(part) for part in version)}"
    actual_implementation = platform.python_implementation()
    if actual_implementation != implementation or sys.version_info[:3] != version:
        raise RecipeError(
            f"this byte-reproduction recipe requires {required}; create the "
            "documented locked environment instead of using another interpreter"
        )
    actual: dict[str, str] = {}
    for package, expected in PACKAGE_VERSIONS.items():
        try:
            version = importlib.metadata.version(package)
        except importlib.metadata.PackageNotFoundError as error:
            raise RecipeError(f"required package is missing: {package}=={expected}") from error
        actual[package] = version
        if version != expected:
            raise RecipeError(
                f"package version mismatch for {package}: expected {expected}, "
                f"got {version}; refusing a non-locked optimization"
            )
    return dict(sorted(actual.items()))


def _shape(value: onnx.ValueInfoProto) -> list[int | str]:
    dimensions: list[int | str] = []
    for dimension in value.type.tensor_type.shape.dim:
        if dimension.HasField("dim_value"):
            dimensions.append(dimension.dim_value)
        elif dimension.HasField("dim_param"):
            dimensions.append(dimension.dim_param)
        else:
            dimensions.append("")
    return dimensions


def _value_contract(value: onnx.ValueInfoProto) -> dict[str, Any]:
    return {
        "name": value.name,
        "element_type": onnx.TensorProto.DataType.Name(value.type.tensor_type.elem_type),
        "shape": _shape(value),
    }


def inspect_graph(path: pathlib.Path) -> dict[str, Any]:
    model = onnx.load(path, load_external_data=True)
    onnx.checker.check_model(model, full_check=True)
    operators = collections.Counter(node.op_type for node in model.graph.node)
    initializer_types = collections.Counter(
        onnx.TensorProto.DataType.Name(value.data_type)
        for value in model.graph.initializer
    )
    return {
        "bytes": path.stat().st_size,
        "sha256": sha256(path),
        "ir_version": model.ir_version,
        "opsets": dict(sorted((value.domain, value.version) for value in model.opset_import)),
        "nodes": len(model.graph.node),
        "operators": dict(sorted(operators.items())),
        "initializer_types": dict(sorted(initializer_types.items())),
        "inputs": [_value_contract(value) for value in model.graph.input],
        "outputs": [_value_contract(value) for value in model.graph.output],
        "uses_external_data": any(
            value.data_location == onnx.TensorProto.EXTERNAL
            for value in model.graph.initializer
        ),
    }


def verify_graph_contract(graph: dict[str, Any]) -> None:
    expected = {
        "inputs": EXPECTED_INPUTS,
        "outputs": EXPECTED_OUTPUTS,
        "operators": EXPECTED_CANDIDATE_OPERATORS,
        "opsets": EXPECTED_OPSETS,
        "ir_version": EXPECTED_IR_VERSION,
        "nodes": 46,
        "initializer_types": {"FLOAT": 77},
        "uses_external_data": False,
    }
    for key, expected_value in expected.items():
        if graph[key] != expected_value:
            raise RecipeError(
                f"candidate graph {key} mismatch: expected {expected_value!r}, "
                f"got {graph[key]!r}"
            )


def _fsync_file(path: pathlib.Path) -> None:
    with path.open("rb") as artifact:
        os.fsync(artifact.fileno())


def _fsync_directory(path: pathlib.Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _write_all(descriptor: int, payload: bytes) -> None:
    view = memoryview(payload)
    while view:
        written = os.write(descriptor, view)
        if written == 0:
            raise RecipeError("short write while staging the artifact manifest")
        view = view[written:]


def publish_directory_no_replace(
    staged: pathlib.Path, destination: pathlib.Path
) -> None:
    """Atomically publish one complete directory and refuse replacement."""
    if staged.parent != destination.parent:
        raise RecipeError("staged and destination directories must share one parent")
    _fsync_directory(staged)
    libc = ctypes.CDLL(None, use_errno=True)
    renameat2 = getattr(libc, "renameat2", None)
    if renameat2 is None:
        raise RecipeError(
            "atomic no-replace directory publication requires Linux renameat2"
        )
    renameat2.argtypes = [
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_uint,
    ]
    renameat2.restype = ctypes.c_int
    at_fdcwd = -100
    rename_noreplace = 1
    result = renameat2(
        at_fdcwd,
        os.fsencode(staged),
        at_fdcwd,
        os.fsencode(destination),
        rename_noreplace,
    )
    if result != 0:
        error_number = ctypes.get_errno()
        if error_number in (errno.EEXIST, errno.ENOTEMPTY):
            raise RecipeError(
                f"refusing to overwrite existing output directory: {destination}"
            )
        if error_number in (errno.ENOSYS, errno.EINVAL, errno.ENOTSUP):
            raise RecipeError(
                "the destination filesystem does not support atomic no-replace "
                "directory publication"
            )
        raise RecipeError(
            f"cannot atomically publish {destination}: "
            f"{os.strerror(error_number)}"
        )
    _fsync_directory(destination.parent)


@contextlib.contextmanager
def exclusive_lock(path: pathlib.Path) -> Iterator[None]:
    """Reserve a recipe output set using O_EXCL; stale locks require inspection."""
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    owner = os.fstat(descriptor)
    try:
        _write_all(descriptor, f"pid={os.getpid()}\n".encode())
        os.fsync(descriptor)
        _fsync_directory(path.parent)
        yield
    finally:
        os.close(descriptor)
        try:
            current = path.stat(follow_symlinks=False)
        except FileNotFoundError:
            current = None
        if (
            current is not None
            and current.st_dev == owner.st_dev
            and current.st_ino == owner.st_ino
        ):
            path.unlink()
            _fsync_directory(path.parent)


def observed_contract(
    *,
    source: dict[str, Any],
    tokenizer: pathlib.Path,
    candidate: dict[str, Any],
    packages: dict[str, str],
) -> dict[str, Any]:
    projection = copy.deepcopy(CONTRACT)
    projection["artifact"]["bytes"] = candidate["bytes"]
    projection["artifact"]["sha256"] = candidate["sha256"]
    projection["graph"] = {
        key: candidate[key]
        for key in (
            "ir_version",
            "initializer_types",
            "inputs",
            "nodes",
            "operators",
            "opsets",
            "outputs",
            "uses_external_data",
        )
    }
    projection["optimizer"]["packages"] = packages
    projection["source"]["model"] = {
        "bytes": source["bytes"],
        "sha256": source["sha256"],
    }
    projection["source"]["tokenizer"] = {
        "bytes": tokenizer.stat().st_size,
        "sha256": sha256(tokenizer),
    }
    return projection


def verify_full_contract(projection: dict[str, Any]) -> None:
    if projection != CONTRACT:
        raise RecipeError(
            "observed artifact metadata does not exactly match "
            f"{CONTRACT_PATH.name}; refusing publication"
        )


def build_manifest(
    *,
    source: dict[str, Any],
    tokenizer: pathlib.Path,
    candidate: dict[str, Any],
    packages: dict[str, str],
) -> dict[str, Any]:
    projection = observed_contract(
        source=source,
        tokenizer=tokenizer,
        candidate=candidate,
        packages=packages,
    )
    verify_full_contract(projection)
    return {
        "format": FORMAT,
        "artifact_contract": projection,
        "observed": {
            "source_graph": source,
            "candidate_graph": candidate,
            "toolchain": {
                "python": "CPython 3.13.5",
                "packages": packages,
            },
        },
        "recipe": (
            "demo/playbooks/onnx-model-optimization/optimize_minilm_fp32.py"
        ),
    }


def run(args: argparse.Namespace) -> dict[str, Any]:
    source = args.source.resolve()
    tokenizer = args.tokenizer.resolve()
    output_directory = args.output_dir.resolve()
    if output_directory.exists():
        raise RecipeError(
            f"refusing to overwrite existing output directory: {output_directory}"
        )

    verify_file(
        source,
        label="source model",
        expected_bytes=SOURCE_BYTES,
        expected_sha256=SOURCE_SHA256,
    )
    verify_file(
        tokenizer,
        label="tokenizer",
        expected_bytes=TOKENIZER_BYTES,
        expected_sha256=TOKENIZER_SHA256,
    )
    packages = verify_toolchain()
    source_graph = inspect_graph(source)
    if source_graph["inputs"] != EXPECTED_INPUTS or source_graph["outputs"] != EXPECTED_OUTPUTS:
        raise RecipeError("source model does not satisfy the pinned MiniLM I/O contract")

    output_directory.parent.mkdir(parents=True, exist_ok=True)
    lock = output_directory.with_name(f".{output_directory.name}.optimize.lock")
    try:
        lock_context = exclusive_lock(lock)
        lock_context.__enter__()
    except FileExistsError as error:
        raise RecipeError(
            f"another optimizer or an interrupted run owns {lock}; inspect the "
            "recorded PID before removing the lock"
        ) from error

    staged_directory: pathlib.Path | None = None
    try:
        staged_directory = pathlib.Path(
            tempfile.mkdtemp(
                prefix=f".{output_directory.name}.staging-",
                dir=output_directory.parent,
            )
        )
        staged_candidate = staged_directory / MODEL_FILENAME
        staged_report = staged_directory / MANIFEST_FILENAME

        optimize_arguments, use_external_data_format = optimizer_configuration()
        optimized = optimize_model(str(source), **optimize_arguments)
        optimized.save_model_to_file(
            str(staged_candidate),
            use_external_data_format=use_external_data_format,
        )
        verify_file(
            staged_candidate,
            label="optimized candidate",
            expected_bytes=CANDIDATE_BYTES,
            expected_sha256=CANDIDATE_SHA256,
        )
        candidate_graph = inspect_graph(staged_candidate)
        verify_graph_contract(candidate_graph)
        result = build_manifest(
            source=source_graph,
            tokenizer=tokenizer,
            candidate=candidate_graph,
            packages=packages,
        )
        report_bytes = (json.dumps(result, indent=2, sort_keys=True) + "\n").encode()
        descriptor = os.open(staged_report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            _write_all(descriptor, report_bytes)
            os.fsync(descriptor)
        finally:
            os.close(descriptor)

        _fsync_file(staged_candidate)
        publish_directory_no_replace(staged_directory, output_directory)
        staged_directory = None
        return result
    finally:
        if staged_directory is not None:
            shutil.rmtree(staged_directory, ignore_errors=True)
        lock_context.__exit__(None, None, None)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Transform the exact checksum-pinned MiniLM source into the expected "
            "fused FP32 model. "
            "Unknown inputs, tools, topology, or existing outputs fail closed."
        )
    )
    parser.add_argument("--source", type=pathlib.Path, required=True)
    parser.add_argument("--tokenizer", type=pathlib.Path, required=True)
    parser.add_argument(
        "--output-dir",
        type=pathlib.Path,
        required=True,
        help=(
            "new directory atomically published with fixed model and manifest "
            "filenames"
        ),
    )
    return parser.parse_args()


def main() -> int:
    try:
        result = run(parse_args())
    except (RecipeError, onnx.checker.ValidationError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
