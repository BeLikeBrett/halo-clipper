# Build and release Halo Clipper

The public repository is `BeLikeBrett/halo-clipper`. GitHub Issues receive bug
reports and user-attached support reports; nothing is silently collected.

## Publish an update

1. Make the change and update `[package].version` in Cargo.toml. Run `cargo check`
   to refresh the workspace package version in Cargo.lock. Add release notes.
2. Pass `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
   `cargo test --release --locked`. Exercise the changed UI and exports.
3. Commit and push to main. Wait for the Checks workflow to pass.
4. Tag the exact reviewed commit: `git tag v0.7.1 && git push origin v0.7.1`.
5. The Release workflow builds and tests on Ubuntu and native Windows, packages
   the setup EXE and portable ZIP, signs update metadata, and publishes a stable
   GitHub release. Existing app installations discover that release on their
   next enabled startup check or manual check. Users choose when to install it.

The version in the tag, Cargo.toml, and signed manifest must agree. Incomplete
builds never publish a release. Do not manually publish a tag before its assets
and signed metadata are ready: clients would see an incomplete release.

The first stable release is v0.7.0. Its update manifest is also the reference
for subsequent updater versions. Normal app runs on v0.7.0 report current when
v0.7.0 is the latest stable release.

## Signing

The repository secret **HALO_UPDATE_SIGNING_KEY** contains a PEM Ed25519 private
key. The app embeds only `assets/update-public-key.bin`. `scripts/sign-release.py`
checks that the private key matches that public key and signs the exact bytes of
`update.json`. Clients verify the signature before trusting any download, then
verify the asset's SHA-256 hash and size. HTTPS is required for all requests.

Keep the private key outside this repository and back it up securely. Brett's
local key is stored in `~/.config/halo-clipper-release/signing-key.pem` with mode
0600; it is ignored by git. Never paste the key into an issue, log, or workflow.
Changing trust keys requires a deliberate key-rotation release; replacing the
public key abruptly prevents older clients from verifying later updates.
The custom signing signature is separate from Windows Authenticode signing.

## Native Windows build

Use Rust's MSVC toolchain, Visual Studio C++ Build Tools, Python, 7-Zip, and NSIS.
The release workflow is the executable specification:

- `python scripts/fetch-windows-runtime.py dist/runtime` downloads pinned runtime
  archives and verifies their hashes before extracting them.
- Generate `mpv.lib` from the DLL exports with `dumpbin` and `lib`, as shown in
  `.github/workflows/release.yml`. Add the import-library directory using
  `RUSTFLAGS=-L native=<absolute-directory>`. Put libmpv and the FFmpeg `bin`
  directory on PATH for tests.
- Run `cargo test --release --locked` and `cargo build --release --locked`.
- Run `python scripts/package-windows.py dist/runtime target/release/halo-clipper.exe`.
- Run `makensis /DVERSION=<version> packaging/windows.nsi`.

The setup EXE includes all runtime executables and DLLs; users do not need Rust,
FFmpeg, or mpv installed separately. No admin rights are requested. Installation
logs / panic logs remain in the user's data folder after uninstallation.

A MinGW cross-build is also supported from Linux: install the
`x86_64-pc-windows-gnu` Rust target and MinGW tools, make `libmpv.a` point to the
runtime's `libmpv.dll.a`, and pass its directory through RUSTFLAGS. Native
Windows CI is still required before publishing a normal release.

## Runtime refreshes

`packaging/runtime-lock.json` pins download URLs and SHA-256 digests. Use dated
upstream tags and versioned archive names, not moving `latest` links. Update
both URL and digest from the upstream release, inspect the included licensing
and source-provenance information, rerun runtime fetch / packaging, and perform
Windows playback, export, and installer checks. Runtime archives and private
signing keys never belong in the source repository.

## Support workflow

Users export a support report through Help / Updates, inspect it, and attach it
to an issue. Ask for reproducible steps, app / OS versions, codec metadata, and
export settings. A small nonprivate sample is optional. Local logs are bounded,
redact registered media paths, and contain no media contents or tokens.

Windows updates use a short-lived PowerShell helper to wait for the app to exit,
run the verified per-user installer silently, and restart. Linux user-local
updates atomically rename a verified replacement over the executable and retain
a `.previous` backup. System package installations defer to the package manager.
