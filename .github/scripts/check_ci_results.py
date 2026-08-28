#!/usr/bin/env python3

"""Fail a terminal CI job unless every serialized dependency succeeded.

Parent workflows pass GitHub's `toJSON(needs)` object through the NEEDS
environment variable. Treat skipped and cancelled dependencies as failures too:
for a required fan-in job, only an explicit success is safe to accept.

`ALLOW_SKIP` is an optional comma-separated list of `needs` keys whose
`skipped` result is accepted (used on forks that cannot run Bazel/sdk).
"""

import json
import os


def main() -> None:
    # Keep result policy in one script so blocking-ci and postmerge-ci cannot
    # drift in how they interpret dependency conclusions.
    needs = json.loads(os.environ["NEEDS"])
    allow_skip = {
        name.strip()
        for name in os.environ.get("ALLOW_SKIP", "").split(",")
        if name.strip()
    }
    failures = sorted(
        (name, dependency["result"])
        for name, dependency in needs.items()
        if not (
            dependency["result"] == "success"
            or (dependency["result"] == "skipped" and name in allow_skip)
        )
    )

    if failures:
        print("CI dependencies did not succeed:")
        for name, result in failures:
            print(f"{name}: {result}")
        raise SystemExit(1)

    print("All CI dependencies succeeded.")


if __name__ == "__main__":
    main()
