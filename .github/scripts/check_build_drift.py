#!/usr/bin/env python3
"""Compare the build this client announces against the one the vendor ships.

At logon the client states a build number and a build letter. Those are
constants in `src/config.rs`, and the vendor moves theirs every few weeks
without telling anyone. Today a stale one is tolerated because the logon also
marks the client as a rolling-release build, which skips the server's
allow-list check — but that is one flag standing between a stale constant and
a refused login, and nothing here notices when the gap widens.

So this notices. It reads the constants, reads the versions the vendor
publishes, and reports the difference. It does not change anything: what a
client announces at logon decides what the server does with it, and that is
not a thing to bump unattended.

    check_build_drift.py --self-check   # prove the parsing, no network
    check_build_drift.py                # compare, exit 1 on drift
"""

import json
import re
import sys
import urllib.request

CONFIG = "src/config.rs"

# The vendor publishes a version file per release channel.
CHANNELS = {
    "latest": "https://download2.interactivebrokers.com/installers/ibgateway/latest-standalone/version.json",
    "stable": "https://download2.interactivebrokers.com/installers/ibgateway/stable-standalone/version.json",
}


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
    print("self-check ok")


def main():
    if "--self-check" in sys.argv:
        return self_check()

    with open(CONFIG) as f:
        build, version = announced(f.read())
    print(f"this client announces build {build}, letter {version!r}")

    drifted = []
    for channel, url in CHANNELS.items():
        with urllib.request.urlopen(url, timeout=30) as r:
            published = unwrap(r.read().decode())["buildVersion"]
        their_build, their_version = split_version(published)
        match = "same" if (their_build, their_version) == (build, version) else "DIFFERENT"
        print(f"  {channel:>6}: {published}  -> build {their_build}, letter {their_version!r}  [{match}]")
        if match == "DIFFERENT":
            drifted.append(f"{channel} is {published}")

    if drifted:
        print("\ndrift: " + "; ".join(drifted))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main() or 0)
