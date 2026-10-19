# Security policy

## Supported versions

Security fixes go into the latest release. Please update before reporting.

## Reporting a vulnerability

Please do not open a public issue. Use **Report a vulnerability** on the repository's Security tab (GitHub private vulnerability reporting). Include the version, the steps to reproduce and what an attacker gains.

You can expect a first answer within a week. Once a fix is released, the advisory is published with credit to you, unless you prefer otherwise.

## What is in scope

Linexus runs on your desktop with your user's rights, so these parts matter most:

- **Linexus Studio's HTTP server.** It listens on 127.0.0.1 only, rejects requests whose `Host` header is not local, and requires a per-start token for every write. A way for a web page or another local user to change the configuration or trigger actions is a vulnerability.
- **The daemon's IPC socket** in `$XDG_RUNTIME_DIR`, which only your user can open.
- **The input helper** (`linexus-inputd`), which can type and press keys through `/dev/uinput` for one user.
- **Actions.** Running programs is off unless the daemon is started with `--allow-shell-actions`. A configuration that runs commands without that flag is a vulnerability.
- **Imported `.nexuspack` files and images.** These are untrusted input.

Things that are expected and not vulnerabilities: anyone who can already edit your config file or run programs as your user can make the strip do anything you can do.
