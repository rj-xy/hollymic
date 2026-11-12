# Releasing

Not set up yet: there's no GitHub repository. This is the plan.

## Today: a local tarball

```bash
scripts/package.sh            # builds target/release, writes dist/hollymic-<version>-<os>-<arch>.tar.gz and .sha256
bash install.sh --file dist/hollymic-0.1.0-linux-x86_64.tar.gz
```

The tarball holds `hollymic`, `install.sh`, `README.md`, `VERSION`, `icons/hollymic-<size>.png`, and on Linux `hollymic.desktop` and `70-hollymic.rules`.

## Later: GitHub releases

1. Create `github.com/rj-xy/hollymic` (personal account `rj-xy`; in `~/rj-xy` always use `GH_TOKEN="$(gh auth token --user rj-xy)"`).
2. A release workflow on tag `v*` that runs `scripts/package.sh` on `ubuntu-latest` (x86_64), `ubuntu-24.04-arm` (aarch64), `macos-14` (aarch64) and `macos-13` (x86_64), and uploads each `dist/*.tar.gz` and `.sha256` to the release.
3. Fill in `download_latest` in `install.sh` (the commands are in its TODO comment), so `curl -fsSL https://raw.githubusercontent.com/rj-xy/hollymic/main/install.sh | bash` works.
4. Bump `version` in `Cargo.toml`, tag `vX.Y.Z`, push the tag.

Linux binaries built on Ubuntu link against its glibc; build on the oldest Ubuntu still supported to run on older distros.
