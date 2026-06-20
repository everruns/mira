#!/usr/bin/env python3
"""Verify that the expected version of each crate is live on crates.io.

Usage: verify_crates_publish.py --expected 0.2.0 mira-eval mira-cli mira-everruns

Polls the crates.io sparse index (no API-policy User-Agent gymnastics) until
each crate reports the expected version, or times out.
"""
import argparse
import json
import sys
import time
import urllib.request


def index_path(crate: str) -> str:
    n = len(crate)
    if n == 1:
        return f"1/{crate}"
    if n == 2:
        return f"2/{crate}"
    if n == 3:
        return f"3/{crate[0]}/{crate}"
    return f"{crate[:2]}/{crate[2:4]}/{crate}"


def published_versions(crate: str) -> set[str]:
    url = f"https://index.crates.io/{index_path(crate)}"
    req = urllib.request.Request(url, headers={"User-Agent": "mira-release-check"})
    with urllib.request.urlopen(req, timeout=20) as resp:
        body = resp.read().decode("utf-8")
    return {json.loads(line)["vers"] for line in body.splitlines() if line.strip()}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--expected", required=True)
    ap.add_argument("--timeout", type=int, default=600)
    ap.add_argument("crates", nargs="+")
    args = ap.parse_args()

    deadline = time.time() + args.timeout
    remaining = list(args.crates)
    while remaining and time.time() < deadline:
        still = []
        for crate in remaining:
            try:
                if args.expected in published_versions(crate):
                    print(f"✓ {crate} {args.expected} is live")
                    continue
            except Exception as e:  # 404 before first publish, transient errors
                print(f"… {crate}: {e}")
            still.append(crate)
        remaining = still
        if remaining:
            print(f"waiting for: {', '.join(remaining)}")
            time.sleep(15)

    if remaining:
        print(f"ERROR: not live after {args.timeout}s: {', '.join(remaining)}", file=sys.stderr)
        return 1
    print("All crates published.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
