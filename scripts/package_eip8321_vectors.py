#!/usr/bin/env python3
"""Package unchanged official vectors from the pinned consensus-specs checkout."""
import gzip
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tarfile

COMMIT = "12c8b24bc09b12f62688d439ba20ef00b1fdfe1f"
CATEGORIES = [
    "operations/randao", "operations/randao_commitment_registration",
    "epoch_processing/pending_randao_commitments", "epoch_processing/pending_deposits",
    "sanity/slots", "sanity/blocks", "finality/finality", "fork/fork",
]

def main():
    spec = Path(sys.argv[1]).resolve()
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=spec, text=True).strip()
    if head != COMMIT:
        raise SystemExit(f"Expected consensus-specs {COMMIT}, got {head}")
    dirty = subprocess.check_output(
        ["git", "diff", "HEAD", "--", "specs", "tests", "presets", "configs"],
        cwd=spec, text=True,
    )
    if dirty:
        raise SystemExit("Official spec/test/preset/config sources must be unchanged")
    output = Path(__file__).resolve().parent.parent / "test-vectors"
    root = spec / "reftests"
    files, inventory = [], {}
    for preset in ["minimal", "mainnet"]:
        for category in CATEGORIES:
            directory = root / preset / "eip8321" / category
            count = len(list(directory.rglob("pre.ssz_snappy")))
            if not count:
                raise SystemExit(f"Missing vector cases: {directory}")
            inventory[f"{preset}/{category}"] = count
            files.extend(sorted(path for path in directory.rglob("*") if path.is_file()))
    expected = json.loads((output / "EIP8321_INVENTORY.json").read_text())
    if inventory != expected:
        raise SystemExit(f"Fixture inventory changed: {inventory}")
    archive = output / "eip8321-vectors.tar.gz"
    with archive.open("wb") as raw:
        with gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=0, compresslevel=9) as gz:
            with tarfile.open(fileobj=gz, mode="w|") as tar:
                for path in files:
                    info = tar.gettarinfo(str(path), str(path.relative_to(root)))
                    info.mtime = info.uid = info.gid = 0
                    info.uname = info.gname = ""
                    info.mode = 0o644
                    with path.open("rb") as source:
                        tar.addfile(info, source)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (output / "eip8321.sha256").write_text(f"{digest}  {archive.name}\n")
    (output / "EIP8321_SOURCE_COMMIT").write_text(COMMIT + "\n")
    print(f"Packaged {sum(inventory.values())} cases, SHA256 {digest}")

if __name__ == "__main__":
    main()
