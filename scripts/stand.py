#!/usr/bin/env python3
"""Build the stand the chain simulator compiles against (stand.toml).

    python3 scripts/stand.py --into DIR [--soft3 REV] [--bump-only]

Fetches soft3 (default: the manifest's `main`) and every sibling at its
pinned revision into DIR/<name>, side by side with this repository (which
must already be DIR/simulation or be linked there), then applies the
manifest's stand-local bumps. Prints `name rev` for every checkout, so a
receipt can carry the exact stand. Stdlib only.
"""

import argparse
import pathlib
import re
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent


def git(*args, cwd=None, check=True):
    return subprocess.run(["git", *args], cwd=cwd, check=check, capture_output=True, text=True)


def fetch(into: pathlib.Path, name: str, url: str, rev: str) -> str:
    d = into / name
    if not (d / ".git").exists():
        d.mkdir(parents=True, exist_ok=True)
        git("init", "-q", cwd=d)
        git("remote", "add", "origin", url, cwd=d)
    r = git("fetch", "-q", "--depth", "1", "origin", rev, cwd=d, check=False)
    if r.returncode != 0:
        # a short sha or a ref GitHub will not serve shallowly: fetch everything
        git("fetch", "-q", "origin", cwd=d)
        git("checkout", "-q", "--detach", rev, cwd=d)
    else:
        git("checkout", "-q", "--detach", "FETCH_HEAD", cwd=d)
    return git("rev-parse", "HEAD", cwd=d).stdout.strip()


def bump(into: pathlib.Path, rules):
    for rule in rules:
        pat = re.compile(rule["from"], re.M)
        for f in rule["files"]:
            p = into / f
            if not p.exists():
                print(f"bump: {f} missing", file=sys.stderr)
                continue
            s = p.read_text()
            n = pat.sub(rule["to"], s)
            if n != s:
                p.write_text(n)
                print(f"bump: {f}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--into", required=True, type=pathlib.Path)
    ap.add_argument("--manifest", default=ROOT / "stand.toml", type=pathlib.Path)
    ap.add_argument("--soft3", help="soft3 revision (default: the manifest's)")
    ap.add_argument("--bump-only", action="store_true", help="apply the bumps to an existing stand")
    a = ap.parse_args()
    m = tomllib.loads(a.manifest.read_text())
    into = a.into.resolve()
    into.mkdir(parents=True, exist_ok=True)
    if not a.bump_only:
        s = m["soft3"]
        print("soft3", fetch(into, "soft3", s["url"], a.soft3 or s["rev"]))
        for sib in m["sibling"]:
            print(sib["name"], fetch(into, sib["name"], sib["url"], sib["rev"]))
    bump(into, m.get("bump", []))


if __name__ == "__main__":
    main()
