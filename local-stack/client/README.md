# local-stack/client

Rust CLI crate (`deploy-client`) with binaries `client` and `pirate`.

## Responsibilities

- Pairing and secure handshake with deploy server.
- Artifact upload and deployment commands over gRPC.
- Local operator utilities (status checks, diagnostics, helper flows).

## Pirate CLI version and PATH

The line `client=…` from `pirate --version` is the **`deploy-client` crate version compiled into that binary** ([`src/version_info.rs`](src/version_info.rs): `CARGO_PKG_VERSION`).

### Default: user-local install (no administrator password)

The **PirateClient** desktop app keeps a **managed copy** of the bundled `pirate` under the user data directory and (once) appends that directory to your **user** `PATH`:

- **Windows:** `%LOCALAPPDATA%\PirateClient\bin` (and user `PATH` via PowerShell).
- **macOS:** `~/Library/Application Support/PirateClient/bin` (and a marked `export PATH=…` block in `~/.zprofile`, `~/.zshrc`, `~/.zlogin`, `~/.profile`, `~/.bash_profile`, `~/.bashrc` as needed; `~/.zlogin` runs last on login zsh so the managed `bin` stays ahead of `/usr/local/bin` after Oh My Zsh and similar).
- **Linux:** `$XDG_DATA_HOME/PirateClient/bin` or `~/.local/share/PirateClient/bin`, with the same shell rc files as macOS.

On each app launch the client **silently** copies the bundled CLI into that folder when the bundle is newer or the file is missing, then ensures the PATH snippet exists. **Existing** terminal windows do not reload `PATH` automatically; open a **new** terminal (or run `hash -r` / `rehash` in zsh) so `command -v pirate` points at the updated binary.

### Optional: system-wide install (password)

From the app you can still choose **install to `/usr/local/bin`** on macOS (Terminal + `sudo`) or Linux (`pkexec`). That path requires elevation and is only needed if you want a global `pirate` outside the managed user layout.

**Diagnose which binary the shell runs**

- macOS / Linux: `which -a pirate` then run each path with `--version` (e.g. `/usr/local/bin/pirate --version`).
- Windows: `where pirate` and the same for each path.

**Compare with the CLI bundled inside the app** (macOS example; adjust app name if needed):

`PirateClient.app/Contents/Resources/bundled/cli/pirate --version`

If the first `pirate` on your `PATH` is still an older copy (for example an old `/usr/local/bin/pirate` ahead of the user bin), either open a new shell after the user-local sync, use the in-app **user** install again, or use the optional system-wide install.

**Developers (repo clone):** `git pull` does not replace `/usr/local/bin/pirate`. Rebuild and reinstall from the repo root:

```bash
./scripts/install-pirate-cli-from-repo-macos.sh
```

Or manually: `cargo build -p deploy-client --bin pirate --release` then `sudo install -m 0755 target/release/pirate /usr/local/bin/pirate`. Check with `which -a pirate` and each path’s `--version`.

**macOS (optional system-wide from app):** choosing install to `/usr/local/bin` opens **Terminal** and runs `sudo install` so you type your Mac password **in the Terminal window**. The bundle must include `NSAppleEventsUsageDescription` so macOS allows controlling Terminal. The default in-app flow does **not** use Terminal or sudo.

Release builds must keep the repo-root [`VERSION`](../../VERSION) file in sync with the `version` in this `Cargo.toml`; CI runs [`scripts/check-deploy-client-version-matches-root-version.sh`](../../scripts/check-deploy-client-version-matches-root-version.sh).

## Related docs

- RU: [`docs/ru/local-client/README.md`](../../docs/ru/local-client/README.md)
- EN: [`docs/en/local-client/README.md`](../../docs/en/local-client/README.md)
