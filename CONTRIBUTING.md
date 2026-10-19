# Contributing

Thanks for helping. Bug reports, device reports from other distributions and desktops, translations and pull requests are all welcome.

## Reporting a bug

Please include:

- your distribution, desktop and session type (Wayland or X11),
- the output of `linexusctl doctor --verbose`,
- what you did, what you expected and what happened,
- the relevant lines of `journalctl --user -u linexusd -n 100`.

Security problems go through a private report instead; see [SECURITY.md](SECURITY.md).

## Development setup

You need Rust 1.88 or newer, a C compiler, `pkg-config` and the libudev headers (`systemd-libs` on Arch, `libudev-dev` on Debian and Ubuntu).

```sh
cargo build --workspace
cargo test --workspace
```

You do not need the hardware for most work. The daemon can render into a PNG file instead of the device, and `--dry-run-actions` logs actions instead of running them:

```sh
cargo run -p nexus-daemon -- \
  --emulator-output /tmp/nexus-frame.png \
  --socket-path /tmp/nexus-dev.sock \
  --state-path /tmp/nexus-dev-state.json \
  --dry-run-actions
```

Point Studio at that daemon, and let it serve the frontend from the checkout so a browser reload picks up your changes:

```sh
NEXUS_STUDIO_ASSETS=apps/nexus-studio \
  cargo run -p nexus-studio -- --socket-path /tmp/nexus-dev.sock --port 19778
```

Render a single page or template to a PNG with `cargo run -p nexusctl -- render-preview --template overview --scale 2 --out /tmp/page.png`.

[docs/architecture.md](docs/architecture.md) explains how the crates fit together.

## Before you open a pull request

CI runs these, so run them locally first:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- Keep changes focused, and add or update tests for behaviour you change.
- Match the style of the code around your change: naming, comment density, error handling.
- User-facing text in Studio lives in `apps/nexus-studio/js/lib/strings.js`. Add every new key in English and German; if you do not speak German, add the English text in both places and say so in the pull request.
- New config fields need a serde default, so existing configs keep loading, and a mention in [docs/configuration.md](docs/configuration.md).
- Studio has no build step. Third-party browser code goes into `apps/nexus-studio/vendor/` with its license.

## Releasing

1. Set the new version in `Cargo.toml` (`[workspace.package]`) and `packaging/arch/PKGBUILD` (`pkgver`, and reset `pkgrel` to 1), then run `cargo update --workspace` so `Cargo.lock` follows.
2. Add a section for the version to [CHANGELOG.md](CHANGELOG.md). The release notes are taken from it.
3. Commit, then tag and push: `git tag -a vX.Y.Z -m "Linexus X.Y.Z" && git push origin master vX.Y.Z`.
4. The Release workflow builds the Arch package and the x86_64 tarball, adds `install.sh` and `SHA256SUMS`, and creates a draft release. Check the draft, then publish it. The installer only sees published releases.

## Clean-room rule

This project talks to the Nexus for interoperability. Do not contribute code, images, fonts, icons or text taken from Corsair software, even when it is easy to extract. Describing observed protocol behaviour in your own words is fine.

## License

By contributing you agree that your work is released under the MIT license of this project.
