# Contributing to libnvme-rs

Thanks for your interest in `libnvme-rs`. Issues, PRs, and design discussion
are all welcome.

## Quick start

```sh
git clone https://github.com/Cyberpsych0s1s/libnvme-rs
cd libnvme-rs

# Linux only — libnvme has no Windows/macOS counterpart.
sudo apt-get install -y build-essential pkg-config libnvme-dev clang libclang-dev

cargo build --workspace --all-targets
cargo test --workspace
```

CI runs on every PR:

| Job | What it checks |
|---|---|
| `test` | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo build`, `cargo test`, `cargo doc -D warnings` |
| `msrv` | Build with the toolchain in `Cargo.toml`'s `rust-version` |
| `nightly` | Heads-up build on nightly (non-fatal — informational only) |
| `deny` | `cargo deny check` — licenses, advisories, banned deps, sources (see `deny.toml`) |
| `semver` | `cargo semver-checks` — compares against the published version, catches accidental breaking changes |
| `machete` | `cargo machete` — unused-dependency detection |
| `publish-dry-run` | On `v*` tags only — verifies the workspace packages cleanly |

Plus two separate workflows that don't gate every PR:

- **QEMU integration** (`.github/workflows/qemu.yml`) — boots the QEMU NVMe
  fixture and runs `cargo test` + `io_smoke` inside the guest. Triggers on
  manual dispatch and weekly cron. Promote to PR-blocking after the harness
  has been stable for a few release cycles.
- **libnvme version matrix** (`.github/workflows/libnvme-matrix.yml`) —
  builds + tests against libnvme `v1.6` / `v1.11.1` / `v1.16.1` compiled
  from source, validating the `build.rs` symbol probes against real
  old/new headers. Triggers on main pushes, weekly cron, manual dispatch,
  and PRs that touch the build plumbing (`build.rs`, `wrapper.h`,
  `Cargo.toml`). If you add a new version-gated symbol, this is what
  proves the probe gates it correctly on older libnvme.

Make sure the non-QEMU jobs all pass locally before requesting review:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

## Style and conventions

- **`rustfmt` + `clippy::all` + `clippy::pedantic` clean** — no overrides.
- **`unsafe` blocks must carry a `// SAFETY:` comment** explaining the
  invariant the call relies on. The lint
  `clippy::undocumented_unsafe_blocks` is set to `warn` workspace-wide;
  CI runs with `-D warnings`, so an uncommented block fails CI.
- **Public API doc-comments include a `# Warning` block for destructive
  ops** (Format NVM, Sanitize, namespace delete, FW commit, Lockdown).
- **Lifetime story:** every borrowed handle (`Controller`, `Namespace`,
  `Subsystem`, `Host`, `Path`) carries `_marker: PhantomData<&'r Root>`
  *and* `_not_send_sync: PhantomData<*const ()>` so it inherits `Root`'s
  non-thread-safety even if `Root`'s auto-trait story changes.

## How to add a new symbol probe

`libnvme-rs` runs against any libnvme ≥ 1.6. Symbols added after 1.6 must
be `#[cfg]`-gated behind a probe so older distros (Ubuntu 24.04 still
ships libnvme 1.8) still build.

There are three probe shapes in
[`libnvme/build.rs`](libnvme/build.rs):

1. **Missing function** — grep the libnvme pkg-config include path for
   the symbol name in a `.h` file. Example:
   ```rust
   probe("has_dhchap_host_key", b"nvme_ctrl_set_dhchap_host_key");
   ```
2. **Missing struct field** — grep for the literal struct-field source
   line. Example:
   ```rust
   probe("has_sanitize_emvs", b"bool emvs");
   ```
3. **Sibling functions added in stages** — separate probes per symbol
   (don't probe one and assume the rest are present).

Then in the Rust code:

```rust
#[cfg(has_dhchap_host_key)]
use libnvme_sys::nvme_ctrl_set_dhchap_host_key;

#[cfg(has_dhchap_host_key)]
pub fn set_dhchap_host_key(&self, key: &str) -> Result<()> {
    /* ... */
}
```

Document the version gate in the rustdoc comment so users know why a method
is conditionally absent.

## QEMU test fixture

[`tests/qemu/`](tests/qemu/) boots an Ubuntu 24.04 guest with a virtual
NVMe controller attached as `/dev/nvme0`. Use this any time you touch the
destructive paths (Format, Sanitize, namespace mgmt, FW commit, I/O
commands).

```sh
bash tests/qemu/run.sh
# In the guest (login: tester / nvmenvme):
cd /mnt/host
sudo -E env "PATH=$PATH" cargo test --workspace
sudo -E env "PATH=$PATH" cargo run --example io_smoke -p libnvme
```

See [`tests/qemu/README.md`](tests/qemu/README.md) for the full workflow,
including how to re-provision the guest after editing cloud-init.

## Release process

1. Bump `version` in the workspace `Cargo.toml`.
2. Update `CHANGELOG.md` with the new section.
3. Update the path-dep version in `libnvme/Cargo.toml`
   (`libnvme-sys = { path = "...", version = "X.Y.Z" }`).
4. Commit and tag (`git tag -a vX.Y.Z -m "vX.Y.Z"`).
5. Push commit + tag. CI runs the `publish-dry-run` job on tags — wait
   for that to go green.
6. `cargo publish -p libnvme-sys` first; wait ~2 minutes for the index
   to update.
7. `cargo publish -p libnvme`.
8. Create the GitHub release pointing at the tag, with the CHANGELOG
   section as the body.

## License

By contributing you agree that your contributions are dual-licensed under
the project's MIT OR Apache-2.0 terms. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).
