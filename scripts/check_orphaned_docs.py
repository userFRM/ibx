"""Catch a new function inserted underneath somebody else's doc comment.

Adding a function directly below an existing `///` block silently steals that
block: the new function takes the doc, and the one it was written for is left
undocumented. `#![deny(missing_docs)]` cannot see it — the count of documented
items does not change, only which item each doc is attached to.

Four of these were found by reading. Three came from one edit, and a fourth was
introduced by the commit that fixed the other three, which is what this is for.

The signal is the doc run directly above an added `fn`: if any line in it is
*unchanged*, that prose was written for something else and the new function is
now wearing it. A function that arrives with its own doc has an all-added run
and does not match — including when it brings a doc of its own and still lands
under an older block, which is the shape that got through a hand review.

Editing an existing function's signature also shows up as an added `fn` under
an unchanged doc, so a name that is removed and added back is an edit, not an
insertion, and is not reported.

    python scripts/check_orphaned_docs.py [base [head]]

`base` defaults to HEAD~1 and `head` to the working tree. Exits 1 and names the
file and function if any match, 0 otherwise.
"""

import re
import subprocess
import sys

DECLARES = r"(?:pub(?:\([^)]*\))?\s+)?(?:default\s+|const\s+|async\s+|unsafe\s+|extern\s+\"[^\"]*\"\s+)*fn\s+(\w+)"
ADDED_FN = re.compile(r"^\+\s*" + DECLARES)
REMOVED_FN = re.compile(r"^-\s*" + DECLARES)
DOC = re.compile(r"^[ +]\s*(?:///|#\[)")
UNCHANGED_DOC = re.compile(r"^ \s*///")
FILE = re.compile(r"^\+\+\+ b/(.*)")
NOISE = ("@@", "--- ", "+++ ", "diff ", "index ", "new file", "deleted file",
         "similarity ", "rename ", "old mode", "new mode", "Binary ")


def orphaned(diff):
    """Every added function wearing a doc comment it did not bring.

    Walks back from the `fn` through the doc run above it — comment lines and
    attributes both, since an attribute sits between a doc and its item — and
    reports the function if any line of that run was already there.
    """
    edited = {m.group(1) for line in diff.splitlines()
              if (m := REMOVED_FN.match(line))}
    lines = [line for line in diff.splitlines()
             if not line.startswith(NOISE)]

    # Which file each surviving line belongs to.
    paths, path = [], "?"
    for line in diff.splitlines():
        if named := FILE.match(line):
            path = named.group(1)
        elif not line.startswith(NOISE):
            paths.append(path)

    found = []
    for i, line in enumerate(lines):
        fn = ADDED_FN.match(line)
        if not fn or fn.group(1) in edited:
            continue
        inherited = None
        for above in reversed(lines[:i]):
            if not DOC.match(above):
                break
            if UNCHANGED_DOC.match(above):
                # Walking backwards, so the last one seen is the block's own
                # opening line — which is what names the item it was for.
                inherited = above.strip()
        if inherited:
            found.append((paths[i], fn.group(1), inherited))
    return found


def resolved(ref):
    """`ref` if this checkout has it, else None.

    A first push to a branch states an all-zero base, and a shallow checkout
    has no base at all. Neither is a reason to fail; both mean compare against
    the parent instead.
    """
    if not ref or set(ref) <= {"0"}:
        return None
    got = subprocess.run(["git", "rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}"],
                         capture_output=True, text=True)
    return ref if got.returncode == 0 else None


def main():
    base = resolved(sys.argv[1] if len(sys.argv) > 1 else None) or "HEAD~1"
    refs = [base] + ([sys.argv[2]] if len(sys.argv) > 2 else [])
    diff = subprocess.run(
        ["git", "diff", "-U15", *refs, "--", "*.rs"],
        capture_output=True, text=True, check=True,
    ).stdout

    found = orphaned(diff)
    for path, fn, doc in found:
        print(f"{path}: `{fn}` was inserted under a doc comment written for "
              f"something else, and now reads as its own: {doc}")
    if found:
        print(f"\n{len(found)} function(s) took a doc comment written for "
              f"something else. Move the block down to the item it describes, "
              f"or give the new function its own.")
        return 1
    return 0


def demo():
    """Every shape, including the one that got through."""
    stolen = ("+++ b/src/thing.rs\n"
              " /// What the old one does.\n"
              "+fn inserted(body: &[u8]) -> u32 {\n")
    assert orphaned(stolen) == [
        ("src/thing.rs", "inserted", "/// What the old one does."),
    ], orphaned(stolen)

    # The shape a hand review missed: the new function brings a doc of its own
    # and still lands under an older block, which stays attached to it.
    both = ("+++ b/src/thing.rs\n"
            " /// Parse the frame.\n"
            " ///   Header: [2B misc][2B stag]\n"
            "+/// The subscription this section belongs to.\n"
            "+fn header_stag(body: &[u8]) -> Option<u32> {\n")
    assert orphaned(both) == [
        ("src/thing.rs", "header_stag", "/// Parse the frame."),
    ], orphaned(both)

    # A function arriving with only its own doc brings it.
    brought = ("+++ b/src/thing.rs\n"
               "+/// What the new one does.\n"
               "+fn inserted(body: &[u8]) -> u32 {\n")
    assert orphaned(brought) == [], orphaned(brought)

    # A signature edit removes and re-adds the same name; not an insertion.
    edit = ("+++ b/src/thing.rs\n"
            " /// What it does.\n"
            "-fn same(a: u32) {}\n"
            "+fn same(a: u64) {}\n")
    assert orphaned(edit) == [], orphaned(edit)

    # No doc above it at all is not this defect.
    bare = "+++ b/src/thing.rs\n fn neighbour() {}\n+fn inserted() {}\n"
    assert orphaned(bare) == [], orphaned(bare)

    # An attribute between the doc and the item does not break the run.
    attributed = ("+++ b/src/thing.rs\n"
                  " /// What the old one does.\n"
                  "+#[inline]\n"
                  "+fn inserted() {}\n")
    assert len(orphaned(attributed)) == 1, orphaned(attributed)
    print("ok")


if __name__ == "__main__":
    sys.exit(demo() if "--demo" in sys.argv else main())
