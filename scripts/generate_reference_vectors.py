#!/usr/bin/env python3
"""Generate selected, unmodified official vectors from a pinned consensus-specs checkout."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
METADATA = ROOT / "testing/reference-vectors"
SELECTION = json.loads((METADATA / "selection.json").read_text())
CHECKSUMS = json.loads((METADATA / "sha256.json").read_text())
COMMIT = SELECTION["source_commit"]


def run(args, cwd):
    display = list(map(str, args))
    if len(display) > 20:
        display = display[:12] + [f"... ({len(args)} arguments; exact selectors in selection.json)"]
    print("+ " + " ".join(display), flush=True)
    subprocess.run(list(map(str, args)), cwd=cwd, check=True)


def verify(output, suites, preset):
    selected = {
        path: digest for path, digest in CHECKSUMS.items()
        if Path(path).parts[1] in suites
        and (preset == "all" or Path(path).parts[0] == preset)
    }
    failures = []
    for name, digest in selected.items():
        path = output / name
        if not path.is_file():
            failures.append(f"missing {name}")
        elif hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            failures.append(f"checksum mismatch: {name}")
    # Refuse extra/missing cases, rather than silently changing the claimed coverage.
    for key, expected in SELECTION["inventory"].items():
        suite, case_preset, category = key.split("/", 2)
        if suite not in suites or (preset != "all" and case_preset != preset):
            continue
        actual = len(list((output / case_preset / suite / category).rglob("pre.ssz_snappy")))
        if actual != expected:
            failures.append(f"{key}: expected {expected} cases, found {actual}")
    return failures


def checkout_source(source):
    if not source.exists():
        source.mkdir(parents=True)
        run(["git", "init", "--quiet"], source)
        run(["git", "fetch", "--depth=1",
             "https://github.com/ethereum/consensus-specs.git", COMMIT], source)
        run(["git", "checkout", "--detach", "FETCH_HEAD"], source)
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
    if head != COMMIT:
        raise RuntimeError(f"Expected consensus-specs {COMMIT}, found {head}; checkout was not changed")
    dirty = subprocess.check_output(
        ["git", "status", "--porcelain", "--untracked-files=no", "--",
         "specs", "tests", "presets", "configs", "pyproject.toml", "uv.lock"],
        cwd=source, text=True,
    )
    if dirty:
        raise RuntimeError("Official generator sources must be unchanged:\n" + dirty)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", choices=["all", "heze", "eip8321"], default="all")
    parser.add_argument("--preset", choices=["all", "minimal", "mainnet"], default="all")
    parser.add_argument("--output", type=Path, default=ROOT / "test-vectors/reference")
    parser.add_argument("--spec-checkout", type=Path, default=os.environ.get("CONSENSUS_SPECS_DIR"))
    parser.add_argument("--reftests-source", type=Path,
                        help="Reuse already generated official vectors after checksum verification")
    parser.add_argument("--python", type=Path,
                        help="Reuse an already installed official generator environment")
    parser.add_argument("--workers", type=int, default=2)
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    if args.workers < 1:
        parser.error("--workers must be positive")
    suites = {"heze", "gloas"} if args.suite == "heze" else {"eip8321"}
    if args.suite == "all":
        suites |= {"heze", "gloas"}
    output = args.output.resolve()
    failures = verify(output, suites, args.preset)
    if not failures:
        print(f"Official reference vectors verified at {output} (spec {COMMIT})")
        return
    if args.verify_only:
        raise RuntimeError("\n".join(failures[:20]))

    source = (args.spec_checkout or ROOT / "test-vectors/consensus-specs").resolve()
    existing = args.reftests_source or (source / "reftests" if args.spec_checkout else None)
    if existing:
        reused = 0
        for name, digest in CHECKSUMS.items():
            parts = Path(name).parts
            if parts[1] not in suites or (args.preset != "all" and parts[0] != args.preset):
                continue
            original = existing / name
            destination = output / name
            if (original.is_file() and
                hashlib.sha256(original.read_bytes()).hexdigest() == digest):
                if (not destination.is_file() or
                    hashlib.sha256(destination.read_bytes()).hexdigest() != digest):
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(original, destination)
                    reused += 1
        print(f"Reused {reused} checksum-verified official files from {existing}")
        if not verify(output, suites, args.preset):
            return
    checkout_source(source)
    if args.python:
        python = args.python.absolute()
        if not python.is_file():
            raise RuntimeError(f"Python environment not found: {python}")
    else:
        if not shutil.which("uv"):
            raise RuntimeError("Install uv, or pass --python /path/to/consensus-specs/.venv/bin/python")
        run(["uv", "sync", "--locked", "--extra", "test", "--python", "3.12"], source)
        python = source / (".venv/Scripts/python.exe" if os.name == "nt" else ".venv/bin/python")
    run([python, "-m", "pysetup.generate_specs", "--all-forks"], source)
    output.mkdir(parents=True, exist_ok=True)
    for group in SELECTION["groups"]:
        if group["suite"] not in suites:
            continue
        if args.preset != "all" and group["preset"] != args.preset:
            continue
        nodes = []
        for case, node in group["cases"].items():
            expected_files = {name: digest for name, digest in CHECKSUMS.items()
                              if name.startswith(case + "/")}
            complete = all((output / name).is_file() and
                           hashlib.sha256((output / name).read_bytes()).hexdigest() == digest
                           for name, digest in expected_files.items())
            if not complete:
                nodes.append(node)
        if not nodes:
            continue
        command = [python, "-m", "pytest", "-q", "--preset=" + group["preset"],
                   "--reftests", "--reftests-output=" + str(output)]
        command += ["--fork=" + fork for fork in group["forks"]]
        if args.workers > 1:
            command += ["-n", str(args.workers), "--dist=worksteal"]
        # Exact node IDs select existing official functions, without patching tests.
        run(command + sorted(set(nodes)), source)
    checkout_source(source)
    failures = verify(output, suites, args.preset)
    if failures:
        raise RuntimeError("Generated data differs from the pinned reference:\n" +
                           "\n".join(failures[:20]))
    print(f"Generated and verified official reference vectors at {output}")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
