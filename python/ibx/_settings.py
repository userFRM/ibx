"""The settings that used to live in the gateway's own file.

A gateway is a process, and a process is configured by a file next to it and a
window in front of it. This client is a library, so the same settings belong on
the client where a caller can set them in code and read them back.

Each setting below names the one it stands in for. A few of the gateway's have
no meaning without a gateway — a port to listen on, the addresses allowed to
reach it, how much heap the runtime may take — and those are named at the bottom
rather than quietly dropped, so nobody goes looking for them.

Settings are read when a session opens, so set them before ``connect()``.
Setting one afterwards affects the next session, not the running one.
"""

from __future__ import annotations

import os

#: Each setting, the variable it is held in, and the gateway setting it stands
#: in for. Held in the environment because that is where this client already
#: reads them from; the names here are the interface, the variables are not.
#
# ponytail: environment-backed because every read site already does env::var
# lazily. If settings ever need to differ between two sessions in one process,
# this becomes a per-session struct passed through connect().
_SETTINGS: dict[str, tuple[str, str]] = {
    "timezone": ("IBX_TZ", "the gateway's time zone"),
    "log_level": ("IBX_LOG_LEVEL", "verbose logging"),
    "log_dir": ("IBX_LOG_DIR", "where the gateway wrote its logs"),
    "log_queue": ("IBX_LOG_QUEUE", "the gateway's log buffering"),
    "market_data_host": ("IBX_FARM_HOST", "the market data connection"),
    "port": ("IBX_MISC_PORT", "the port the gateway reached the venue on"),
    "registration_timeout_ms": (
        "IBX_REGISTRATION_TIMEOUT_MS",
        "how long the gateway waited to be admitted",
    ),
    "locale": ("IBX_LOCALE", "the gateway's locale"),
    "build": ("IBX_BUILD", "the build the gateway announced itself as"),
    "version": ("IBX_VERSION", "the version the gateway announced itself as"),
    "hardware_id": ("IBX_HWID", "the machine identity the gateway presented"),
}

#: Gateway settings with nothing to stand in for here, and why. Named rather
#: than dropped: a caller migrating from a gateway will look for them, and
#: "there is no such thing here" is an answer where silence is not.
UNAVAILABLE: dict[str, str] = {
    "LocalServerPort": "no local socket to listen on; this client is the client",
    "LocalApiPort": "no local socket to listen on; this client is the client",
    "TrustedIPs": "nothing connects to this client, so nothing needs trusting",
    "ApiOnly": "carried as connect(readonly=True), which is per session",
    "MainWindow.Width": "no window",
    "MainWindow.Height": "no window",
    "vmoptions": "no runtime to size",
}


def configure(**settings) -> None:
    """Set one or more settings. Returns nothing; raises on a name it does not have.

    Raising rather than ignoring: a misspelled setting that is silently dropped
    leaves a caller believing a session is configured a way it is not.

        ibx.configure(timezone="America/New_York", log_level="debug")
    """
    unknown = set(settings) - set(_SETTINGS)
    if unknown:
        raise ValueError(
            f"no such setting: {', '.join(sorted(unknown))}. "
            f"Known: {', '.join(sorted(_SETTINGS))}"
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
    """Every setting, its value, and the gateway setting it stands in for."""
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
