# Building on a shared machine

The build environment, not this repo, decides where artifacts go. The machine
config (`~/.cargo/config.toml`, policy `GLOBAL-RUST-BUILD-PERF-20261024` in
`~/knowledge/rust-build-performance.md`) puts each worktree's intermediates in
its own `build.build-dir` under `~/.cache/cargo/build`, leaves only final
binaries in `target/`, and shares compiled dependencies across worktrees
through a `RUSTC_WRAPPER` cache. Many sessions build at once, so:

- Do not run `cargo clean`. Fingerprints are per worktree, so there is nothing
  stale to clear, and a clean throws away the whole worktree's build. If one
  crate looks stale, run `cargo clean -p <crate>` and say why.
- Do not set `CARGO_TARGET_DIR`, `CARGO_BUILD_BUILD_DIR`, `RUSTC_WRAPPER` or
  `CARGO_INCREMENTAL`, and do not add them to `.cargo/config.toml`, `.envrc` or
  scripts. The environment owns them. In particular, never point two worktrees
  at one build-dir: Cargo identifies workspace crates by their path relative to
  the workspace root, so one worktree then silently builds and tests the
  other's sources.
- Pick one feature set for the session (`cargo build`, `cargo test`, or
  `--features web`) and stay on it. Do not run `--all-features` or
  `--no-default-features` sweeps to be safe; each switch recompiles every crate
  whose features changed.
- `Blocking waiting for file lock on build directory` means another cargo or
  rust-analyzer in this worktree is compiling. Wait. Do not kill the other
  cargo, and do not work around it with a different target or build dir.
- Run tests with `cargo nextest run`; it uses the same artifacts as
  `cargo test`.
- Do not fix slow builds by editing `[profile.*]`, `jobs`, linker or debuginfo
  settings; those are policy. Report the slow crates from
  `cargo build --timings` instead.

When builds are slow or the disk is filling, use the `rust-build-hygiene` skill.
Its `verify` command shows whether this worktree is on the policy build-dir.
