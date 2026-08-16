"""Run what continuous integration runs, and fail if this stops matching it.

A gate kept by hand drifts from the one that decides. This session pushed a
change that broke a suite nobody had run locally, because the local list was
five of the seven files the workflow names — the two missing ones are the slow
ones, which is exactly why they were dropped and exactly why they matter.

So the list is not written here. It is read out of the workflow, and a suite
added there without being added to any local run is picked up by reading it
again rather than by somebody noticing.

    python scripts/gate.py            # everything the workflow runs
    python scripts/gate.py --list     # what that is, without running it

The live suites need a session and are not part of this; the workflow does not
run them either.
"""

import re
import subprocess
import sys
import pathlib

WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / ".github/workflows/tests.yml"


def suites_ci_runs():
    """Every `--test <name>` the workflow names, in the order it names them."""
    text = WORKFLOW.read_text()
    return re.findall(r"--test\s+(\S+)", text)


def scripts_ci_runs():
    """Every script under `scripts/` the workflow runs, in its order.

    The suites were read out of the workflow and the scripts were not, so a
    commit passed every local suite and failed on a script gate that had never
    been run — the doc check, which this repository wrote itself. Read both,
    for the same reason.
    """
    # A command the workflow wrapped across lines is one command.
    text = re.sub(r"\\\n\s*", " ", WORKFLOW.read_text())
    seen = []
    for line in re.findall(r"python (scripts/\S+\.py[^\n]*)", text):
        # The workflow passes it the commit a pull request is measured against.
        # Locally that is the commit before this one, which is what a push adds.
        line = re.sub(r'"\$\{\{[^}]*\}\}"', "HEAD~1", line).strip()
        if line not in seen:
            seen.append(line)
    return seen


def toolchain():
    """CI resolves `stable`; a local default that is older lints differently."""
    return ["rustup", "run", "stable"]


def steps(suites):
    return [
        (["cargo", "clippy", "--lib", "--all-targets", "--features", "dev-tools", "--", "-D", "warnings"], {}),
        (["cargo", "clippy", "--lib", "--all-targets", "--features", "python",
          "--", "-D", "warnings"], {}),
        (["cargo", "test", "--lib"], {}),
        (["cargo", "test", "--lib", "--features", "python"], {}),
        # The registration timeout is overridden for the same reason the
        # workflow overrides it: without it every call with no engine to answer
        # waits the full timeout, which is minutes across these.
        (["cargo", "test", *sum([["--test", s] for s in suites], [])],
         {"IBX_REGISTRATION_TIMEOUT_MS": "20"}),
        # The workflow builds the documentation and fails on a warning. Run
        # locally only as clippy was: a broken doc link is invisible to every
        # step above it, and three of them reached main because this line was
        # not here.
        (["cargo", "doc", "--no-deps", "--lib"], {"RUSTDOCFLAGS": "-D warnings"}),
        # And the Python suite, which reads the Rust source in two places. Both
        # went stale in a refactor that every Rust suite passed.
        ([".venv/bin/python", "-m", "pytest", "tests/python", "-q"], {}),
        # The paper suite's offline half — the manifests that check this client
        # against the reference client's surface. Its live phases refuse to run
        # without credentials, which is deliberate, so they are told to skip.
        # A manifest here read a file that moved and could not pass at all,
        # and no other step in this list builds the target.
        (["cargo", "test", "--test", "ib_paper_compat"],
         {"IBX_ALLOW_SKIP_NO_CREDS": "1"}),
    ]


def generated_docs_are_current():
    """What the workflow checks after running the generators: nothing moved."""
    for tree in ("docs/", ".gates/"):
        subprocess.run(["git", "add", "-A", tree], check=False)
    done = subprocess.run(["git", "diff", "--cached", "--quiet", "docs/", ".gates/"])
    if done.returncode != 0:
        print("\nFAILED: generated docs are stale — the generators just moved them, "
              "so commit what they produced")
    return done.returncode


def main():
    suites = suites_ci_runs()
    if not suites:
        print(f"no suites named in {WORKFLOW}; has the workflow moved?")
        return 1

    if "--list" in sys.argv:
        print("\n".join(suites))
        return 0

    import os
    for command, extra_env in steps(suites):
        printable = " ".join(command)
        print(f"\n=== {printable}", flush=True)
        env = {**os.environ, **extra_env}
        # The toolchain pin is for cargo. Everything else runs as it is.
        prefix = toolchain() if command[0] == "cargo" else []
        done = subprocess.run([*prefix, *command], env=env)
        if done.returncode != 0:
            print(f"\nFAILED: {printable}")
            return done.returncode

    for line in scripts_ci_runs():
        print(f"\n=== python {line}", flush=True)
        done = subprocess.run([sys.executable, *line.split()])
        if done.returncode != 0:
            print(f"\nFAILED: python {line}")
            return done.returncode

    if code := generated_docs_are_current():
        return code

    print(f"\nall of it passed, across {len(suites)} suites: {' '.join(suites)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
