"""The settings a gateway keeps in its own configuration file.

A gateway is a process, and a process is configured by a file next to it and a
window in front of it. This client is a library, so the same settings belong on
the client where a caller can set them in code and read them back.

The same settings are stated on the Rust client as ``EClientConfig.gateway``,
where they belong to the session that states them. This is the same list,
reached the other way: what is set here is what a session that states none of
its own falls back to.

Each setting below names the IB Gateway setting it corresponds to. A few of
those have no meaning without a local process to configure — a port to listen
on, the addresses allowed to reach it, how much heap the runtime may take — and
are named at the bottom rather than dropped, so nobody goes looking for them.

Settings are read when a session opens, so set them before ``connect()``.
Setting one afterwards affects the next session, not the running one.

The three logging settings are read earlier still. A process has one logger and
importing ``ibx`` installs it, so ``IBX_LOG_LEVEL``, ``IBX_LOG_DIR`` and
``IBX_LOG_QUEUE`` are read at that moment and belong in the environment before
it. :func:`configure` refuses them rather than storing a value nothing will
read.
"""

from __future__ import annotations

import os

#: Each setting, the variable it is held in, and the equivalent IB Gateway
#: setting. Held in the environment because that is where this client already
#: reads them from; the names here are the interface, the variables are not.
#
# ponytail: environment-backed because every read site already does env::var
# lazily. If settings ever need to differ between two sessions in one process,
# this becomes a per-session struct passed through connect().
_SETTINGS: dict[str, tuple[str, str]] = {
    "timezone": ("IBX_TZ", "the time zone announced at logon"),
    "log_level": ("IBX_LOG_LEVEL", "verbose logging"),
    "log_dir": ("IBX_LOG_DIR", "log directory"),
    "log_queue": ("IBX_LOG_QUEUE", "how many records logging buffers before dropping them"),
    "market_data_host": ("IBX_FARM_HOST", "the host every farm connection is opened on"),
    "port": ("IBX_MISC_PORT", "the port a farm connection opens on, where the routing names none"),
    "registration_timeout_ms": (
        "IBX_REGISTRATION_TIMEOUT_MS",
        "how long to wait to be admitted",
    ),
    "locale": ("IBX_LOCALE", "session locale"),
    "build": ("IBX_BUILD", "the build announced at logon"),
    "version": ("IBX_VERSION", "the version announced at logon"),
    "encoded": ("IBX_ENCODED", "the longer string announced with them"),
    "hardware_id": ("IBX_HWID", "the machine identity presented at logon"),
    "execution_reports": (
        "IBX_EXECUTION_REPORTS",
        "which executions arrive when a session opens: 'today' or 'all'",
    ),
    "island_for_nasdaq": (
        "IBX_ISLAND_FOR_NASDAQ",
        "whether a US stock on Nasdaq is handed back under the older spelling",
    ),
}

#: Gateway settings with nothing to stand in for here, and why. Named rather
#: than dropped: a caller migrating from a gateway will look for them, and
#: "there is no such thing here" is an answer where silence is not.
UNAVAILABLE: dict[str, str] = {
    "ApiMsgsPerSlice": "nothing paces what a caller sends; the pacing here is the subscription burst a reconnect replays, stated on ReconnectConfig",
    "ApiTimeSliceMillis": "nothing paces what a caller sends; the pacing here is the subscription burst a reconnect replays, stated on ReconnectConfig",
    "TimestampZone": "no setting chooses the zone a timestamp is stated in; a bar is stated on the zone the venue names beside it, or as seconds since the epoch where the request asked for that",
    "useSsl": "the connection to the venue is encrypted and there is nothing to turn off: every session is opened through TLS",
    "UseSSL": "the connection to the venue is encrypted and there is nothing to turn off: every session is opened through TLS",
    "reconnectOnSocketErr": "stated per session rather than once for a process: `policy` on ReconnectConfig, which is Automatic by default and Manual for a caller that would rather be told and decide",
    "RemoteHostOrderRouting": "stated per session rather than once for a process: `host` on the client config, which is where to knock — the venue names the server this account belongs on and the session moves there",
    "RemotePortOrderRouting": "stated per session rather than once for a process: `port` on the client config",
    "Select_account_type": "stated per session rather than once for a process: `paper` on the client config",
    "Local_FIX_Server_Settings": "no local socket to listen on; this client is the client",
    "LocalServerPort": "no local socket to listen on; this client is the client",
    "LocalApiPort": "no local socket to listen on; this client is the client",
    "TrustedIPs": "nothing connects to this client, so nothing needs trusting",
    "ApiOnly": "stated per session rather than once for a process: `readonly` on the client config, or connect(readonly=True)",
    "MainWindow.Width": "no window",
    "MainWindow.Height": "no window",
    "vmoptions": "no runtime to size",
}


#: The three that belong to the process rather than to a session. A process has
#: one logger, and importing ``ibx`` installs it, so a value set from here
#: arrives after the only moment it could have been read. Refused rather than
#: stored: stored, it reads back as a log level that was set and did nothing.
_INSTALLED_AT_IMPORT = ("log_level", "log_dir", "log_queue")


def configure(**settings) -> None:
    """Set one or more settings. Returns nothing; raises on a name it does not have.

    Raising rather than ignoring: a misspelled setting that is silently dropped
    leaves a caller believing a session is configured a way it is not.

        ibx.configure(timezone="America/New_York", execution_reports="today")

    The three logging settings are the exception, and are refused here: set
    them in the environment before ``import ibx``, which is when the logger is
    installed.
    """
    unknown = set(settings) - set(_SETTINGS)
    if unknown:
        raise ValueError(
            f"no such setting: {', '.join(sorted(unknown))}. "
            f"Known: {', '.join(sorted(_SETTINGS))}"
        )
    too_late = sorted(set(settings) & set(_INSTALLED_AT_IMPORT))
    if too_late:
        raise ValueError(
            f"{', '.join(too_late)} belongs to the process, not one session: "
            "importing ibx installs the logger, so set "
            f"{', '.join(_SETTINGS[name][0] for name in too_late)} in the "
            "environment before that"
        )
    for name, value in settings.items():
        var, _ = _SETTINGS[name]
        if value is None:
            os.environ.pop(var, None)
        else:
            os.environ[var] = str(value)


def settings() -> dict[str, str | None]:
    """Every setting and what it is currently set to, unset ones as ``None``."""
    return {name: os.environ.get(var) for name, (var, _) in _SETTINGS.items()}


def describe() -> str:
    """Every setting, its value, and the IB Gateway setting it corresponds to."""
    lines = ["settings:"]
    for name, (var, stands_for) in sorted(_SETTINGS.items()):
        value = os.environ.get(var)
        shown = "unset" if value is None else repr(value)
        lines.append(f"  {name:24s} {shown:28s} {stands_for}")
    lines.append("")
    lines.append("no counterpart here:")
    for name, why in sorted(UNAVAILABLE.items()):
        lines.append(f"  {name:24s} {why}")
    return "\n".join(lines)


def _names_match_the_rust_client() -> list[str]:
    """The settings this module carries, for the test that checks both lists.

    Two lists of the same settings drift the moment one is added to. The test
    beside this compares them, so a setting added on one side and not the other
    fails rather than being quietly available in one language.
    """
    return sorted(_SETTINGS)
