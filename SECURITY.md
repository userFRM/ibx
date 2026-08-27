# Security

## Reporting a vulnerability

Use GitHub's private vulnerability reporting on this repository: **Security → Report a vulnerability**. That opens a private advisory visible only to the maintainers, which is the right place for anything that would put an account at risk if it were public.

Please do not open a public issue for a security problem, and please do not include real credentials, account numbers or session captures from a live account in a report. A redacted transcript or a synthetic reproduction says the same thing without putting your account in a public thread.

What helps most: the version, the call that triggered it, whether the session was paper or live, and what you expected instead. If it reproduces offline, say so, because that makes it much faster to confirm.

## What this software holds

This client authenticates directly and holds a live session. That means it handles a username, a password and, on live accounts, a second factor. A few consequences worth stating plainly:

Credentials are read from the environment or passed to `connect`. They are never written to a log by this client, and the logging layer redacts identifiers rather than printing them whole. If you find a path that writes a credential anywhere, that is a vulnerability and it is worth reporting.

Never commit a credentials file. The repository ignores the usual local paths, but the safe habit is to keep them outside the working tree entirely and read them from the environment.

One login holds one session. A second login takes the first one over, so credentials shared between processes will disconnect each other rather than run side by side.

A session with trading permissions can place orders that cost real money. Test against a paper account first. Paper and live speak the same protocol here, so a program that works on paper is exercising the same code paths.

## Supported versions

The `main` branch is where fixes land. There is no long term support branch, so the answer to a security report is a fix on `main` and a release from it.
