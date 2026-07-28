#!/usr/bin/env python3
"""Launch one bounded XERJ profiling run and write a reproducibility manifest."""

import argparse
import datetime
import hashlib
import json
import os
import pathlib
import platform
import re
import shutil
import signal
import subprocess
import sys

PUBLICATION_GRACE_SECONDS = 5
ATTACHMENT_LABEL = re.compile(r"[a-zA-Z0-9_.-]+")


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def write_manifest(path, manifest):
    temporary = path.with_suffix(".json.tmp")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as target:
        target.write(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def parse_attachment(specification, option):
    if "=" not in specification:
        raise ValueError(f"{option} must be LABEL=PATH")
    label, source_text = specification.split("=", 1)
    if not ATTACHMENT_LABEL.fullmatch(label):
        raise ValueError(f"unsafe attachment label: {label}")
    return label, pathlib.Path(source_text).resolve()


def copy_attachment_atomic(source, destination):
    """Copy one attachment without exposing partial data or overwriting a file."""
    temporary = destination.with_name(f".{destination.name}.tmp")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "wb") as target, source.open("rb") as attached:
            shutil.copyfileobj(attached, target)
            target.flush()
            os.fsync(target.fileno())
        # A same-directory hard link publishes atomically and refuses an
        # existing destination on every supported platform.
        os.link(temporary, destination)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def collect_attachment(output, label, source, collection, process_exit_code=None):
    if not source.is_file():
        raise FileNotFoundError(f"attachment is not a file: {source}")
    destination = output / f"attachment-{label}{source.suffix}"
    copy_attachment_atomic(source, destination)
    metadata = {
        "file": destination.name,
        "source": str(source),
        "bytes": destination.stat().st_size,
        "sha256": sha256(destination),
        "collection": collection,
    }
    if process_exit_code is not None:
        metadata["process_exit_code_at_collection"] = process_exit_code
    return metadata


def read_optional(path):
    try:
        return pathlib.Path(path).read_text().strip()
    except OSError:
        return None


def cgroup_v2_snapshot():
    membership = read_optional("/proc/self/cgroup")
    relative = "/"
    if membership:
        for line in membership.splitlines():
            if line.startswith("0::"):
                relative = line[3:]
                break
    root = pathlib.Path("/sys/fs/cgroup") / relative.lstrip("/")
    return {
        "path": relative,
        "memory_max": read_optional(root / "memory.max"),
        "memory_current": read_optional(root / "memory.current"),
        "cpu_max": read_optional(root / "cpu.max"),
    }


def main():
    parser = argparse.ArgumentParser(
        description="Run a symbolized, bounded XERJ CPU/heap capture.",
        epilog=(
            "Validate with debug-profiling/inspect.py. Analysis prompts: "
            "PROMPT_CPU.md, PROMPT_HEAP.md, PROMPT_COMPARE.md."
        ),
    )
    parser.add_argument("--output", required=True, type=pathlib.Path)
    parser.add_argument("--cpu-seconds", type=int, default=0)
    parser.add_argument("--heap-seconds", type=int, default=0)
    parser.add_argument("--cpu-hz", type=int, default=100)
    parser.add_argument("--delay-seconds", type=int, default=0)
    parser.add_argument("--workload", required=True, help="stable workload/driver identifier")
    parser.add_argument("--corpus", required=True, help="stable corpus identifier or hash")
    parser.add_argument("--concurrency", required=True, help="worker/request concurrency")
    parser.add_argument(
        "--cache-state", required=True, choices=("cold", "warm", "mixed", "unknown")
    )
    parser.add_argument(
        "--build-features",
        required=True,
        help="declared comma-separated Cargo feature set used to build the binary",
    )
    parser.add_argument("--build-profile", required=True, help="declared Cargo profile")
    parser.add_argument(
        "--attach",
        action="append",
        default=[],
        metavar="LABEL=PATH",
        help="copy and hash evidence that already exists before server launch",
    )
    parser.add_argument(
        "--attach-after",
        action="append",
        default=[],
        metavar="LABEL=PATH",
        help="copy and hash evidence after the server process exits",
    )
    parser.add_argument(
        "--stop-after",
        type=int,
        help="terminate the server after this many seconds (default: capture max + 5)",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("server command is required after --")
    binary = pathlib.Path(command[0]).resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(f"server binary does not exist or is not executable: {binary}")
    if not 1 <= max(args.cpu_seconds, args.heap_seconds) <= 300:
        parser.error("at least one capture duration must be in 1..=300")
    if not 1 <= args.cpu_hz <= 1000:
        parser.error("--cpu-hz must be in 1..=1000")
    if not 0 <= args.delay_seconds <= 300:
        parser.error("--delay-seconds must be in 0..=300")
    capture_end = args.delay_seconds + max(args.cpu_seconds, args.heap_seconds)
    if (
        args.stop_after is not None
        and args.stop_after < capture_end + PUBLICATION_GRACE_SECONDS
    ):
        parser.error(
            f"--stop-after must allow {PUBLICATION_GRACE_SECONDS}s after capture "
            "for profile dump and atomic publication"
        )
    attachments_before = []
    attachments_after = []
    labels = set()
    for option, specifications, target in (
        ("--attach", args.attach, attachments_before),
        ("--attach-after", args.attach_after, attachments_after),
    ):
        for specification in specifications:
            try:
                label, source = parse_attachment(specification, option)
            except ValueError as error:
                parser.error(str(error))
            if label in labels:
                parser.error(f"duplicate attachment label: {label}")
            labels.add(label)
            target.append((label, source))
    for _, source in attachments_before:
        if not source.is_file():
            parser.error(f"attachment is not a file: {source}")

    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    output = args.output.resolve()
    manifest_path = output / "manifest.json"
    started = datetime.datetime.now(datetime.timezone.utc)
    manifest = {
        "schema": "xerj.debug_profile.v1",
        "status": "running",
        "started_utc": started.isoformat(),
        "command": command,
        "binary": str(binary),
        "binary_sha256": sha256(binary),
        "source_build_binding": "UNVERIFIED",
        "capture": {
            "cpu_seconds": args.cpu_seconds or None,
            "heap_seconds": args.heap_seconds or None,
            "cpu_hz": args.cpu_hz,
            "delay_seconds": args.delay_seconds,
        },
        "experiment": {
            "workload": args.workload,
            "corpus": args.corpus,
            "concurrency": args.concurrency,
            "cache_state": args.cache_state,
        },
        "declared_build_unverified": {
            "profile": args.build_profile,
            "features": [item for item in args.build_features.split(",") if item],
        },
        "host": {
            "platform": platform.platform(),
            "kernel": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "cpu_count": os.cpu_count(),
            "page_size": os.sysconf("SC_PAGE_SIZE"),
            "cgroup_v2": cgroup_v2_snapshot(),
        },
    }
    repo = pathlib.Path(__file__).resolve().parents[3]
    try:
        manifest["source"] = {
            "repo": str(repo),
            "git_head": subprocess.check_output(
                ["git", "-C", str(repo), "rev-parse", "HEAD"],
                text=True,
                stderr=subprocess.DEVNULL,
            ).strip(),
            "git_status": subprocess.check_output(
                ["git", "-C", str(repo), "status", "--short"],
                text=True,
                stderr=subprocess.DEVNULL,
            ).splitlines(),
        }
        tracked_diff = subprocess.check_output(
            ["git", "-C", str(repo), "diff", "--binary", "HEAD"],
            stderr=subprocess.DEVNULL,
        )
        manifest["source"]["tracked_diff_sha256"] = hashlib.sha256(tracked_diff).hexdigest()
    except (OSError, subprocess.CalledProcessError):
        manifest["source"] = None
    try:
        manifest["rustc"] = subprocess.check_output(
            ["rustc", "-Vv"], text=True, stderr=subprocess.DEVNULL
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        manifest["rustc"] = None
    manifest["cgroup"] = read_optional("/proc/self/cgroup")
    attachments = {}
    for label, source in attachments_before:
        attachments[label] = collect_attachment(
            output, label, source, collection="pre_launch"
        )
    manifest["attachments"] = attachments
    manifest["post_run_attachment_requests"] = {
        label: {"source": str(source)} for label, source in attachments_after
    }
    manifest["attachment_failures"] = []
    write_manifest(manifest_path, manifest)

    environment = os.environ.copy()
    environment["XERJ_DEBUG_PROFILE_DIR"] = str(output)
    environment["XERJ_DEBUG_CPU_HZ"] = str(args.cpu_hz)
    environment["XERJ_DEBUG_PROFILE_DELAY_SECONDS"] = str(args.delay_seconds)
    if args.cpu_seconds:
        environment["XERJ_DEBUG_CPU_SECONDS"] = str(args.cpu_seconds)
    if args.heap_seconds:
        environment["XERJ_DEBUG_HEAP_SECONDS"] = str(args.heap_seconds)

    log_path = output / "server.log"
    log_fd = os.open(log_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    log_file = os.fdopen(log_fd, "wb")
    process = subprocess.Popen(
        command,
        env=environment,
        stdout=log_file,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    timeout = args.stop_after or capture_end + PUBLICATION_GRACE_SECONDS
    timed_out = False

    def stop_group(first_signal, grace_seconds=10):
        os.killpg(process.pid, first_signal)
        try:
            process.wait(timeout=grace_seconds)
            return
        except subprocess.TimeoutExpired:
            pass
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=10)
            return
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()

    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        stop_group(signal.SIGTERM)
    except KeyboardInterrupt:
        stop_group(signal.SIGINT)
    finally:
        log_file.close()

    for label, source in attachments_after:
        try:
            attachments[label] = collect_attachment(
                output,
                label,
                source,
                collection="post_process",
                process_exit_code=process.returncode,
            )
        except OSError as error:
            manifest["attachment_failures"].append(
                {
                    "label": label,
                    "source": str(source),
                    "collection": "post_process",
                    "reason": str(error),
                }
            )

    artifacts = {}
    requested = []
    if args.cpu_seconds:
        requested.append("cpu.pb")
    if args.heap_seconds:
        requested.append("heap.pb.gz")
    for name in requested:
        path = output / name
        if path.exists():
            artifacts[name] = {"bytes": path.stat().st_size, "sha256": sha256(path)}
            if shutil.which("pprof"):
                top_path = output / f"{name}.top.txt"
                top_fd = os.open(top_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                with os.fdopen(top_fd, "w") as top:
                    result = subprocess.run(
                        ["pprof", "-top", str(path)],
                        stdout=top,
                        stderr=subprocess.STDOUT,
                        text=True,
                    )
                artifacts[top_path.name] = {
                    "bytes": top_path.stat().st_size,
                    "sha256": sha256(top_path),
                    "exit_code": result.returncode,
                }
    for name in ("cpu-error.json", "heap-error.json"):
        path = output / name
        if path.exists():
            artifacts[name] = {"bytes": path.stat().st_size, "sha256": sha256(path)}
    artifacts["server.log"] = {
        "bytes": log_path.stat().st_size,
        "sha256": sha256(log_path),
    }
    missing = [name for name in requested if name not in artifacts]
    expected_termination = timed_out and process.returncode == -signal.SIGTERM
    if missing:
        status = "profile_failed"
    elif process.returncode not in (0, None) and not expected_termination:
        status = "process_failed"
    elif manifest["attachment_failures"]:
        status = "attachment_failed"
    else:
        status = "complete"
    manifest.update(
        {
            "status": status,
            "ended_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "elapsed_seconds": (datetime.datetime.now(datetime.timezone.utc) - started).total_seconds(),
            "exit_code": process.returncode,
            "terminated_after_capture_window": timed_out,
            "missing_requested_artifacts": missing,
            "artifacts": artifacts,
        }
    )
    write_manifest(manifest_path, manifest)
    if missing:
        print(f"missing requested profile artifacts: {', '.join(missing)}", file=sys.stderr)
        return 2
    if status == "process_failed":
        return 3
    if status == "attachment_failed":
        print(
            "one or more post-run attachments could not be collected",
            file=sys.stderr,
        )
        return 4
    return 0


if __name__ == "__main__":
    sys.exit(main())
