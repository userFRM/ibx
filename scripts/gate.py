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
VENV = pathlib.Path(__file__).resolve().parents[1] / ".venv/bin/python"


def script_python():
    """The interpreter the workflow runs these scripts with.

    Not the one running this file. The workflow names `.venv/bin/python` because
    one of these scripts imports the built extension, and started with any other
    interpreter it fails to import rather than reporting on the tree. Read that
    way, a local run passed everything the workflow runs except the one step the
    workflow spells out an interpreter for.
    """
    return str(VENV) if VENV.exists() else sys.executable


def jobs_in_workflow():
    """Each job the workflow declares, as `(name, body)`.

    A job key sits at two spaces of indent under `jobs:`. Anything above that
    line is triggers and permissions, which carry keys at the same depth.
    """
    text = WORKFLOW.read_text()
    at = text.find("\njobs:\n")
    if at < 0:
        return
    text = text[at:]
    starts = [(m.start(), m.group(1)) for m in re.finditer(r"(?m)^  ([A-Za-z0-9_-]+):$", text)]
    for i, (start, name) in enumerate(starts):
        end = starts[i + 1][0] if i + 1 < len(starts) else len(text)
        yield name, text[start:end]


def python_ci_pins():
    """The interpreter the workflow runs the Python suite on, where it names one.

    Read out of the job that runs the suite rather than out of the file. The
    first `python-version` in this workflow belongs to the documentation job,
    and taking that one answered for a run it has nothing to do with — the
    suite's own job installs its interpreter another way and names no version
    at all. `None` says that, which is the honest answer; a version borrowed
    from another job is worse than no answer, because it reads like one.
    """
    for _, body in jobs_in_workflow():
        if "pytest" not in body:
            continue
        found = re.search(r'python-version:\s*"?(\d+\.\d+)"?', body)
        return found.group(1) if found else None
    return None


def python_version_here():
    """`major.minor` of the interpreter this gate runs the Python suite with."""
    if not VENV.exists():
        return None
    done = subprocess.run(
        [str(VENV), "-c",
         "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}')"],
        capture_output=True, text=True, check=False,
    )
    return done.stdout.strip() or None


def suites_ci_runs():
    """Every `--test <name>` the workflow names, in the order it names them."""
    text = WORKFLOW.read_text()
    return re.findall(r"--test\s+(\S+)", text)


def base_ref():
    """What a local run measures a change against.

    `origin/main` where there is one, so a gate run over several commits reads
    all of them. Falling back to the commit before this one, which is right for
    a single commit and wrong for a batch — and being wrong that way is silent.
    """
    done = subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", "origin/main"],
        capture_output=True, text=True, check=False,
    )
    return "origin/main" if done.returncode == 0 else "HEAD~1"


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
        # The workflow passes it the commit the change is measured against.
        # Locally that is what has already been pushed, not the commit before
        # this one: a batch of three is three commits past `HEAD~1`, so a check
        # run that way reads only the last of them. One of these gates is
        # exactly the kind that a batch defeats — an item takes its neighbour's
        # documentation in the first commit and the check never looks there —
        # and the push hook, which reads the whole range, is what caught it.
        line = re.sub(r'"\$\{\{[^}]*\}\}"', base_ref(), line).strip()
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
        # Off by default, so no step above builds it. Left out, a break in it
        # reaches main having compiled nowhere.
        (["cargo", "clippy", "--lib", "--all-targets", "--features", "async",
          "--", "-D", "warnings"], {}),
        (["cargo", "test", "--lib"], {}),
        # Both configurations the workflow builds. Run with only the helpers,
        # three tests that need them exist and a failure belonging to the plain
        # feature set passes here; run with only the plain set, those three do
        # not exist at all.
        (["cargo", "test", "--lib", "--features", "python"], {}),
        (["cargo", "test", "--lib", "--features", "python,test-helpers"], {}),
        (["cargo", "test", "--lib", "--features", "async"], {}),
        # The examples in the documentation are compiled by nothing else. Two
        # of them were broken and building, because `cargo doc` renders an
        # example without compiling it and no suite collects them.
        (["cargo", "test", "--doc", "--features", "async"], {}),
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
        # The extension the Python suite imports, rebuilt first. Without this
        # the suite runs against whatever was built last: a change to any
        # `#[pymethods]` body is invisible here, the Rust half recompiles and
        # passes, and the same change fails in the workflow, which does build
        # it. Two runs are only comparable if both are testing the same
        # extension.
        ([".venv/bin/maturin", "develop", "--features", "python,extension-module,test-helpers"], {}),
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
    """What the workflow checks after running the generators: nothing moved.

    This compares the whole of `docs/`, so it cannot tell a page a generator
    rewrote from one a person edited and has not committed. Both fail, and both
    should: the push carries a documents tree that does not match what was
    committed either way. The message says what is known rather than guessing
    which of the two it was.
    """
    for tree in ("docs/",):
        subprocess.run(["git", "add", "-A", tree], check=False)
    done = subprocess.run(["git", "diff", "--cached", "--quiet", "docs/"])
    if done.returncode != 0:
        print("\nFAILED: docs/ does not match what is committed. Either a generator "
              "moved a page, or an edit is uncommitted. `git diff --cached docs/` "
              "says which; commit it either way.")
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

    runner = script_python()
    for line in scripts_ci_runs():
        print(f"\n=== {runner} {line}", flush=True)
        done = subprocess.run([runner, *line.split()])
        if done.returncode != 0:
            print(f"\nFAILED: {runner} {line}")
            return done.returncode

    if code := generated_docs_are_current():
        return code

    # Everything above passed, which is only the same answer the workflow gives
    # if the Python suite ran on the interpreter the workflow names. It did not
    # once: the suite passed here on one version and failed there on another for
    # a day of pushes, because a write through the attribute protocol is not the
    # same operation on both. So the verdict says which interpreter answered.
    pinned, here = python_ci_pins(), python_version_here()
    if here and pinned and pinned != here:
        print(f"\nevery suite passed, but the Python suite ran on {here} and the "
              f"workflow runs it on {pinned}. That is not the same evidence — run "
              f"it on {pinned} before reading this as what the workflow will say.")
        return 0
    if here and pinned is None:
        print(f"\nevery suite passed, with the Python suite on {here}. The workflow "
              f"names no interpreter for that job — it takes whatever the runner "
              f"ships — so an answer that depends on the version can still differ "
              f"there, and this run cannot tell you it will not.")
        return 0

    print(f"\nall of it passed, across {len(suites)} suites: {' '.join(suites)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
