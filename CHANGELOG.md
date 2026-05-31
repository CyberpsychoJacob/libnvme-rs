# Changelog

All notable changes to `libnvme-sys` and `libnvme` are recorded here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) with the
caveat that pre-1.0 minor-version bumps may include breaking changes.

## [0.9.0] – 2026-05-26

API-stabilization pass ahead of the 1.0 freeze, driven by a full
public-API consistency audit. This is the last release that intentionally
breaks compatibility before 1.0; pre-1.0 minor bumps are allowed to break.

### Breaking

- `Error::InvalidArgument` now wraps `Cow<'static, str>` (was `&'static str`)
  so caller-input errors can carry the offending value. Match arms binding
  the message still work; `&str` deref is unchanged.
- All caller-input validation now returns `Error::InvalidArgument` instead
  of `Error::Os(InvalidInput)` (io.rs nlb/buffer-length checks, fabrics
  interior-NUL, `read_to_vec`/`error_log` overflow). `Error::NotAvailable`
  is no longer used for input or overflow errors — it's reserved for
  "libnvme returned NULL" on handle accessors. The `take_owned_cstr`
  free-function NULL path now maps to `Error::Os` (preserving `errno`).
- `Sanitize::execute` returns `Result<u32>` (was `Result<()>`), matching
  every other command builder.
- `DsmRange` fields are now private; construct via `DsmRange::new` (1-based
  block count) + `with_context`. Removes the field-vs-constructor 0/1-based
  footgun.
- The buffer-setter on `ReservationReport`, `ZnsMgmtRecv`, and `DirectiveRecv`
  is renamed `into()` → `buffer()` (avoids colliding with the `Into` trait).
- Spec-mirroring enums are now `#[non_exhaustive]` (`SecureErase`,
  `ProtectionInfo`, `FirmwareAction`, `FeatureSelect`, `SanitizeAction`,
  `SelfTestAction`, the reservation actions, and the ZNS actions/filter), so
  future NVMe-spec variants can be added without a major bump. Add a `_`
  arm when matching.

### Added

- `#[must_use]` on every builder type and every lazy iterator type
  (`Hosts`/`Subsystems`/`Controllers`/`Namespaces`/`Paths`), so dropping a
  builder or iterator without consuming it now warns.
- `nvme_resv_status` is re-exported from the `libnvme` crate so callers of
  `ReservationReport` can name and decode the report without a direct
  `libnvme-sys` dependency.
- Marshalling unit tests (no live device): `encode_nlb` 1-based conversion,
  `IoControl`/`DsmAttr` bit layouts, `DsmRange`/`CopyRange` encoding,
  buffer-length validation, and every admin-enum discriminant. 37 unit
  tests total (was 21).
- Documentation filled in for previously-undocumented public items
  (`PassthruArgs`/`LockdownArgs`/`GetLbaStatusArgs` fields, builder
  `execute()`/setter methods, `NvmeVersion`/`LbaFormat` fields).

### Changed

- `bindgen` build-dependency bumped 0.71 → 0.72 (regenerates `libnvme-sys`
  FFI bindings; the reason this release is a breaking minor).
- CI gained `cargo-deny`, `cargo-semver-checks`, `cargo-machete`, a nightly
  heads-up build, an MSRV job that reads `rust-version` from `Cargo.toml`,
  concurrency cancellation, and per-job timeouts. `deny.toml`
  `multiple-versions` is now `deny` (the tree has no duplicates).
- A headless QEMU integration workflow (`tests/qemu/ci.sh` + `qemu.yml`)
  boots the NVMe fixture on a hosted runner and runs the suite + `io_smoke`
  over SSH (manual + weekly; not PR-blocking yet).

## [0.8.0] – 2026-05-21

### Added

Three command-set groups that close the gap between the v0.7 surface and
"100% safe coverage of libnvme's command surface" — the v1.0 goal.

- **Reservations** (new `reservations` module) — multi-host shared-storage
  coordination (NVMe-oF clustering, SCSI Persistent-Reservation analog):
  - `Namespace::reservation_acquire().key(k).rtype(t).action(a).execute()`
  - `Namespace::reservation_register().new_key(k).action(a).ptpl(p).execute()`
  - `Namespace::reservation_release().key(k).rtype(t).action(a).execute()`
  - `Namespace::reservation_report().into(&mut buf).execute()` /
    `.execute_to_vec()`
  - Typed enums: `ReservationType` (WE/EA/WERO/EARO/WEAR/EAAR),
    `ReservationAcquireAction`, `ReservationRegisterAction`,
    `ReservationReleaseAction`, `PtplChange`
- **Directives** (new `directives` module) — workload hints (Streams,
  Identify):
  - `Namespace::directive_send(dtype, op).data(&buf).execute()`
  - `Namespace::directive_recv(dtype, op).into(&mut buf).execute()`
  - Typed enums: `DirectiveType` (Identify / Streams / Raw),
    `DirectiveSendOp`, `DirectiveRecvOp`
- **ZNS** (new `zns` module) — Zoned Namespaces (host-managed sequential-
  write SSDs):
  - `Namespace::zns_mgmt_send(slba, action).select_all().execute()`
    — Open / Close / Finish / Reset / Offline / SetDescriptorExtension /
    ZRWA Flush
  - `Namespace::zns_mgmt_recv(slba).filter(f).into(&mut buf).execute()`
    — Report Zones / Extended Report Zones
  - `Namespace::zns_append(zslba, nlb, &data).fua().execute() -> u64`
    — returns the assigned LBA the controller picked
  - Typed enums: `ZoneSendAction`, `ZoneRecvAction`, `ZoneReportFilter`

### Notes

- All three command groups round-trip via `libnvme-sys` directly to the
  kernel NVMe driver — no static-inline workarounds.
- All return-types follow the v0.7 convention: `Result<u32>` for
  result-dword-bearing ops, `Result<u64>` for `zns_append` (assigned LBA).
- Block-count encoding for `zns_append` is 1-based (matches the I/O
  module's convention); we subtract one for the spec's 0-based NLB.
- Coverage now stands at 122/122 of libnvme's extern functions either
  typed or reachable via `Controller::admin_passthru` / `io_passthru`.
  The v0.9 cycle will focus on API audit, QEMU CI automation, and
  per-feature smoke tests ahead of v1.0.

## [0.7.0] – 2026-05-21

### Added

- NVM I/O command surface in the new `io` module:
  - `Namespace::read(slba, nlb, &mut buf)` + `Namespace::read_to_vec(slba, nlb)`
  - `Namespace::write(slba, nlb, &buf)`
  - `Namespace::compare(slba, nlb, &buf)`
  - `Namespace::verify(slba, nlb)`
  - `Namespace::write_zeroes(slba, nlb)` (with `.deallocate()` /
    `.no_deallocate_after_zero()`)
  - `Namespace::write_uncorrectable(slba, nlb)`
  - `Namespace::flush()`
  - `Namespace::dsm(attrs).ranges(&[..])` for Dataset Management (TRIM)
  - `Namespace::copy(sdlba, &ranges)` for the Copy command
- Builder setters cover the full optional surface of `nvme_io_args`: FUA,
  Limited Retry, PI action/check-{ref,app,guard}, storage tag/check, ref/app
  tags, dataset-mgmt hint, directive (streams), per-command timeout, metadata
  buffer
- New public types: `Read`, `Write`, `Compare`, `Verify`, `WriteZeroes`,
  `WriteUncorrectable`, `Dsm`, `DsmAttr`, `DsmRange`, `Copy`, `CopyRange`
- `examples/io_smoke.rs` — round-trips write/read/compare/verify/write_zeroes
  /dsm/flush on the QEMU NVMe fixture (model-name safety latch)
- `Error::InvalidArgument(&'static str)` variant for input-validation
  failures (interior NUL bytes, controller-id lists exceeding the spec
  maximum, etc.). Previously these went through `Error::NotAvailable`,
  which is reserved for "libnvme returned NULL" cases.
- `# Warning` rustdoc blocks on every destructive entry point
  (`Format::execute`, `Sanitize::execute`, `Namespace::delete_namespace`,
  `Controller::fw_commit`, `Controller::lockdown`,
  `Namespace::write_uncorrectable`).
- `_not_send_sync: PhantomData<*const ()>` markers on every borrowed
  handle (`Controller`, `Namespace`, `Subsystem`, `Host`, `Path`) so the
  `!Send + !Sync` story is explicit at the type level rather than
  inherited from `Root`'s auto-trait quirks.
- `CONTRIBUTING.md` (build + style + symbol-probe guide + QEMU + release
  process) and `SECURITY.md` (private disclosure policy).
- CI now runs an MSRV (1.85) build, `cargo doc -D warnings`, `cargo audit`,
  and `cargo publish --dry-run` on `v*` tags.

### Fixed

- **Memory leak in `take_owned_cstr` UTF-8 error path** (root.rs). The
  libnvme-allocated string was leaked when its bytes didn't form valid
  UTF-8. Now we copy bytes out, free unconditionally, and validate UTF-8
  on the owned copy. Affects `generate_hostnqn`, `generate_hostid`,
  `hostnqn_from_file`, `hostid_from_file`.
- `Controller::attach_namespace` / `detach_namespace` returned
  `Error::NotAvailable` when the caller passed more than 2047 controller
  IDs; now returns `Error::InvalidArgument` with a descriptive message.

### Changed

- `Controller::ns_attach_op` uses `copy_from_slice` instead of a manual
  loop when building the `nvme_ctrl_list` payload.
- `io::base_args` uses `Default::default()` field-init instead of
  `mem::zeroed()` + per-field writes — matches the pattern already used
  in `Controller::get_log_page` and is more robust if libnvme adds a
  non-zero-default field.
- `fabrics_cstring` (`controller.rs`) promoted to `util::str_to_cstring`
  and reused from `Root::lookup_host`, removing a small duplication.
- Every `unsafe { ... }` block now carries a `// SAFETY: ...` comment
  explaining the invariant the call relies on. The workspace enables
  `clippy::undocumented_unsafe_blocks = "warn"`, and CI runs with
  `-D warnings`, so future unsafe blocks must come with a comment.
- `features.rs` (786 lines) split into `features/{mod,types,get,set}.rs`.
- I/O control flags hoisted from free constants into a `bitflags`-style
  `IoControl` struct inside `io.rs`, with builder setters in terms of
  named bits.

### Notes

- Block counts in the new I/O API are **1-based** (`nlb = 1` means a
  single LBA). We convert to the spec's 0-based encoding internally;
  this matches what most users naturally type and prevents silent
  off-by-one bugs.
- The reviewer-flagged crates.io 404 for `libnvme` reflected the
  registry lag right after the `0.6.2` publish; v0.7.0 is the second
  publish under this name.

## [0.6.2] – 2026-05-20

### Fixed

- docs.rs build for 0.6.1 failed because the docs.rs sandboxed builder
  doesn't have `libnvme-dev` installed, so `bindgen` couldn't find the
  headers. Resolved by vendoring an unmodified copy of `libnvme.h` and
  `nvme/*.h` (libnvme 1.16.1) into `libnvme-sys/vendored-headers/` and
  having both `build.rs` files use them when `DOCS_RS=1` is set. Normal
  user builds against system `libnvme-dev` are unchanged.

### Added

- `libnvme-sys/vendored-headers/` — LGPL-2.1+ libnvme headers, used only
  during docs.rs builds. Documented in `vendored-headers/README.md`.

## [0.6.1] – 2026-05-20

### Changed

- First crates.io publish. `Cargo.toml` metadata now includes `documentation`,
  `homepage`, and `rust-version` (1.85) fields. README leads with the example
  rather than the status section. Added this `CHANGELOG.md`. No source code
  changes versus 0.6.0.

## [0.6.0] – 2026-05-20

### Added

- `Controller::sanitize()` builder for Sanitize NVM (action, AUSE, pass count, overwrite invert, no-deallocate, pattern, NVMe 2.0 EMVS field — last is version-gated)
- `Controller::self_test(nsid, action)` for Device Self-Test
- `Controller::lockdown(args)` for NVMe 2.0+ Lockdown
- `Controller::security_send(...)` / `security_receive(...)` for security-protocol payloads
- `Controller::get_lba_status(args, buf)` for the LBA Status Descriptor admin command
- `Controller::set_property(offset, value)` / `get_property(offset)` for Fabrics controller registers
- `Controller::admin_passthru(args)` / `io_passthru(args)` — generic escape hatches
- Typed enums `SanitizeAction`, `SelfTestAction`
- New public types: `Sanitize` builder, `LockdownArgs`, `GetLbaStatusArgs`, `PassthruArgs`
- New struct-field probe pattern in `build.rs` (`has_sanitize_emvs`)

## [0.5.0] – 2026-05-19

### Added

- `Controller::features()` returning a `Features` accessor that wraps every
  per-feature Get/Set Features helper libnvme exposes — 69 typed methods total
  (32 set + 37 get)
- `FeatureSelect` enum (Current / Default / Saved / Supported)
- Re-exported feature-buffer types: `LbaRangeType`, `AutoPst`, `Timestamp`,
  `HostBehavior`, `PlmConfig`, `HostMemBufAttrs`
- Seven new `build.rs` probes for the NVMe 2.0 `*2` feature variants
  (`temp_thresh2`, `err_recovery2`, `lba_range2`, `host_mem_buf2`,
  `resv_mask2`, `resv_persist2`, `write_protect2`)

## [0.4.1] – 2026-05-19

### Fixed

- Build failure on libnvme 1.8 (Ubuntu 24.04): five Fabrics auth/TLS/discovery
  helpers (`set_dhchap_host_key`, `set_tls_key`, `set_tls_key_identity`,
  `set_keyring`, `set_unique_discovery_ctrl`/`is_unique_discovery_ctrl`) are
  newer than libnvme 1.8 and are now `#[cfg]`-gated
- Build failure on libnvme 1.8 for `nvmf_hostid_generate` /
  `nvmf_hostid_from_file` — also gated

## [0.4.0] – 2026-05-18

### Added

- `fabrics` module: `Transport` enum, `Connect` builder (14 chainable
  setters), `DiscoveryLog`, `DiscoveryLogEntry`
- `Root::default_host`, `Root::lookup_host`
- Free functions: `generate_hostnqn`, `generate_hostid`, `hostnqn_from_file`,
  `hostid_from_file`
- `Controller::disconnect` (consumes self), `reset`, `is_discovery_controller`,
  `was_discovered`, `is_unique_discovery_controller`, `is_persistent`,
  `set_persistent`, `discovery_log`, `set_dhchap_host_key`, `set_dhchap_key`,
  `set_tls_key`, `set_tls_key_identity`, `set_keyring`
- Bindgen allowlist widened in `libnvme-sys/build.rs` to include `nvmf_*`
  functions, types, and vars

## [0.3.0] – 2026-05-18

### Added

- `Controller::get_log_page<T>` — generic typed Get Log Page helper
- `Controller::fw_slot_log()` returning `FirmwareSlotLog`
- `Controller::error_log(max_entries)` returning `Vec<ErrorLogEntry>`
- `Path` and `Paths` multipath/ANA types; `Controller::paths()` and
  `Namespace::paths()` iterators
- `Namespace::format()` builder (LBA format, secure erase, protection info,
  metadata, timeout)
- `Controller::fw_download(image)` and `fw_commit(slot, action, bpid)`
- `Controller::create_namespace(template)`, `delete_namespace(nsid)`,
  `attach_namespace(nsid, ctrlids)`, `detach_namespace(...)`
- `admin` module with typed enums: `SecureErase`, `ProtectionInfo`,
  `ProtectionLocation`, `MetadataSettings`, `FirmwareAction`
- `examples/fw_info.rs` and `examples/format_smoke.rs` (the latter has a
  model-name safety latch — only formats when controller model is
  `"QEMU NVMe Ctrl"`)
- QEMU NVMe test fixture in `tests/qemu/`

## [0.2.1] – 2026-05-17

### Fixed

- `Controller::smart_log()` was masking `EACCES` as `EBADF` because the
  file descriptor returned by `nvme_ctrl_get_fd` (which opens the device
  lazily) wasn't checked before use. Fix introduces a `open_fd` helper
  that propagates the real `errno`.

### Added

- 9 new Controller sysfs accessors: `numa_node`, `queue_count`, `sq_size`,
  `phy_slot`, `subsystem_nqn`, `transport_address`, `transport_service_id`,
  `host_transport_address`, `host_interface`
- 7 new Namespace accessors: `generic_name`, `meta_size`, `lba_utilization`,
  `csi`, `model`, `serial`, `firmware`
- 4 version-gated Subsystem accessors: `model`, `firmware`, `iopolicy`,
  `application` (`build.rs` probes for the symbol presence)

## [0.2.0] – 2026-05-17

### Added

- `Controller::identify()` returning `IdentifyController` with ~25 decoded
  accessors over `nvme_id_ctrl`
- `Namespace::identify()` returning `IdentifyNamespace` with `LbaFormat` helper
- `Controller::smart_log()` returning `SmartLog` with 18 decoded SMART
  accessors
- `Error::Nvme(u32)` variant for device-reported NVMe status codes,
  `Error::Utf8` for invalid UTF-8 in libnvme-returned strings,
  `Error::NotAvailable` for NULL pointers
- `check_ret` helper mapping libnvme's `c_int` return convention (0 ok,
  negative = `-errno`, positive = NVMe status) to `Result`
- `util::fixed_ascii_to_str` for decoding NVMe spec ASCII fields
- Examples: `id_ctrl`, `smart_log`
- Symbol-probing `build.rs` for `has_subsystem_serial` (the version-gating
  scheme); see `tests/qemu/` for the test fixture

## [0.1.0] – 2026-05-17

### Added

- Initial release
- Tree iteration: `Host` → `Subsystem` → `Controller` → `Namespace`
- Controller properties: `name`, `model`, `serial`, `firmware`, `transport`,
  `address`, `state`
- Namespace properties: `name`, `nsid`, `lba_size`, `lba_count`, `size_bytes`,
  `uuid`, `nguid`, `eui64`
- Subsystem and Host basics (`nqn`, `hostid`, `type`)
- `Root::scan()` entry point, `Drop` cascading `nvme_free_tree`
- `Error` / `Result` types
- `examples/scan.rs` and `examples/list_nvme.rs`
- Dual-license (MIT or Apache-2.0)
- CI (Ubuntu 24.04, libnvme 1.8) running `cargo build`, `cargo test`,
  `cargo clippy`, `cargo fmt --check`

[0.9.0]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.9.0
[0.8.0]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.8.0
[0.7.0]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.7.0
[0.6.2]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.6.2
[0.6.1]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.6.1
[0.6.0]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.6.0
[0.5.0]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.5.0
[0.4.1]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.4.1
[0.4.0]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.4.0
[0.3.0]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.3.0
[0.2.1]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.2.1
[0.2.0]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.2.0
[0.1.0]: https://github.com/Cyberpsych0s1s/libnvme-rs/releases/tag/v0.1.0
