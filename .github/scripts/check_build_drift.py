#!/usr/bin/env python3
"""Compare the build this client announces against the one the vendor ships.

At logon the client states a build number and a build letter. Those are
constants in `src/config.rs`, and the vendor moves theirs every few weeks
without telling anyone. Today a stale one is tolerated because the logon also
marks the client as a rolling-release build, which skips the server's
allow-list check — but that is one flag standing between a stale constant and
a refused login, and nothing here notices when the gap widens.

So this notices. What it reports on is the vendor publishing something new,
not the gap between their build and ours: that gap is deliberate, and a check
that reports it reports every night, which is the same as reporting nothing.
The build we last looked at is written down, and this speaks up when the
vendor moves past it. It changes nothing on its own — what a client announces
at logon decides what the server does with it, and that is not a thing to bump
unattended.

    check_build_drift.py --self-check   # prove the parsing, no network
    check_build_drift.py                # compare, exit 1 when the vendor moves
"""

import json
import re
import sys
import urllib.request

CONFIG = "src/config.rs"
# The published build this repository has already looked at and made a decision
# about. Update it in the same change that acts on a new one.
SEEN = ".github/gateway-build.json"

# The vendor publishes a version file per release channel.
CHANNELS = {
    "latest": "https://download2.interactivebrokers.com/installers/ibgateway/latest-standalone/version.json",
    "stable": "https://download2.interactivebrokers.com/installers/ibgateway/stable-standalone/version.json",
}

# Only one channel decides whether this reports. The other is printed for
# context and nothing more: a check that reports every night reports nothing,
# because the issue it keeps open stops being read after the second one.
TRACKED = "stable"


def unwrap(body):
    """The version files are served as JSONP: `name_callback({...});`."""
    m = re.search(r"\{.*\}", body, re.S)
    if not m:
        raise ValueError(f"no object in response: {body[:80]!r}")
    return json.loads(m.group(0))


def split_version(published):
    """"10.49.1d" -> ("10491", "d"), the two forms the logon carries."""
    m = re.fullmatch(r"(\d+(?:\.\d+)*)([a-z]*)", published.strip())
    if not m:
        raise ValueError(f"unrecognised version {published!r}")
    return m.group(1).replace(".", ""), m.group(2)


def announced(source):
    """The build and letter the client states, read from the Rust constants."""
    build = re.search(r'IB_BUILD:\s*&str\s*=\s*"([^"]+)"', source)
    version = re.search(r'IB_VERSION:\s*&str\s*=\s*"([^"]+)"', source)
    if not build or not version:
        raise ValueError(f"no IB_BUILD/IB_VERSION in {CONFIG}")
    return build.group(1), version.group(1)


def self_check():
    assert split_version("10.49.1d") == ("10491", "d")
    assert split_version("10.45.1i") == ("10451", "i")
    assert split_version("10.40.1c") == ("10401", "c")
    # A channel that drops the letter is still a version.
    assert split_version("10.50") == ("1050", "")
    for bad in ("", "beta", "10.49.1-rc1"):
        try:
            split_version(bad)
        except ValueError:
            pass
        else:
            raise AssertionError(f"{bad!r} should not parse")
    assert unwrap('cb({"buildVersion":"10.49.1d"});')["buildVersion"] == "10.49.1d"
    assert unwrap('{"buildVersion":"10.49.1d"}')["buildVersion"] == "10.49.1d"
    assert announced('pub const IB_BUILD: &str = "10401";\npub const IB_VERSION: &str = "c";') == ("10401", "c")
    # The deliberate gap between what we announce and what the vendor ships is
    # not what this reports on, so a build we have already looked at is quiet
    # however far it is from ours.
    with open(SEEN) as f:
        seen = json.load(f)
    assert TRACKED in seen, f"{SEEN} names no {TRACKED} build"
    print("self-check ok")


def main():
    if "--self-check" in sys.argv:
        return self_check()

    with open(CONFIG) as f:
        build, version = announced(f.read())
    with open(SEEN) as f:
        seen = json.load(f)
    print(f"this client announces build {build}, letter {version!r}")

    moved = None
    for channel, url in CHANNELS.items():
        with urllib.request.urlopen(url, timeout=30) as r:
            published = unwrap(r.read().decode())["buildVersion"]
        their = split_version(published)
        tracked = channel == TRACKED
        note = "tracked" if tracked else "for context"
        known = seen.get(channel)
        state = "as last seen" if published == known else f"NEW (last seen {known})"
        print(f"  {channel:>6}: {published}  -> build {their[0]}, letter {their[1]!r}  [{state}, {note}]")
        if tracked and published != known:
            moved = published

    if moved:
        print(f"\nthe {TRACKED} channel is now {moved}, and was {seen.get(TRACKED)} when last looked at.")
        print(f"This client announces {build}{version}. Decide whether to move, then record {moved} in {SEEN}.")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main() or 0)
