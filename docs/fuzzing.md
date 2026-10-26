# Fuzzing

Aegis parses input it does not control: rings and descriptors that a guest driver writes into guest memory, guest writes to device registers, and the `vm.toml` a user hands to `aegis run`. The `fuzz/` crate holds coverage-guided fuzz targets for these, plus stable-Rust smoke tests that run the same harnesses in CI.

## What is fuzzed

| Target | Code under test | Input | Invariants checked (besides "no panic") |
|---|---|---|---|
| `virtqueue` | `aegis_devices::virtio::queue::SplitQueue`: `enable`, `pop` (descriptor-chain walk), `push_used`, `wants_interrupt` | 8 bytes of queue configuration (size and the three ring addresses), then 64 KiB of guest RAM holding the rings and descriptors | An enabled queue has a power-of-two size up to 256 and aligned rings. Every popped chain has a head below the queue size, 1 to `size` buffers, at most `MAX_CHAIN_BYTES` in total, and no buffer whose end wraps the address space. |
| `virtio_mmio` | `aegis_devices::virtio::VirtioMmio` register handling with the virtio-console behind it: status and feature negotiation, queue configuration, notifications, interrupts, host input and the receive path | A script of register writes and reads (any offset below `0x200`, any width), host input and host-initiated processing, followed by the guest RAM image | The status only gains bits except through reset, and a reset clears it fully. `DEVICE_NEEDS_RESET` is set exactly when an error is recorded. `DRIVER_OK` implies `FEATURES_OK`. The interrupt line is asserted exactly when `InterruptStatus` is non-zero. Output and pending input stay within their caps. Delivered input never decreases and never exceeds the input accepted. |
| `vm_toml` | `aegis-cli`'s `config::parse`, compiled into the fuzz crate by path, unchanged | The config text | An accepted config has memory and vCPU counts in range, a command line without NUL, and stdin forwarding only with a virtio-console. On Unix, its kernel path is under the base directory unless the config gave an absolute one. Parsing is deterministic. |

The harnesses, with these invariants as assertions, are in [`fuzz/src/lib.rs`](../fuzz/src/lib.rs). Each returns a small report of how far its input got, which the smoke tests use to check that inputs reach past the early checks.

**Not fuzzed yet:** the bzImage loader in `aegis-kvm` (Linux-only, and it would need its parser split from the KVM setup); the `aegis-core` exit-dispatch loop, which ADR-0010 lists; the 16550 UART (a few registers, fully covered by unit tests).

## Isolation from the normal build

- `fuzz/` is a Cargo workspace of its own, which the root `Cargo.toml` excludes. `cargo build`, `cargo test --workspace`, clippy and `cargo deny check` on the main workspace never see it or its dependencies.
- It has its own `fuzz/Cargo.lock`. It was seeded from the workspace lockfile, so the shared dependencies (`serde`, `toml`) are at the same versions.
- `libfuzzer-sys` is optional, behind the `libfuzzer` feature, and only the cargo-fuzz targets need it. Without that feature, nothing builds libFuzzer or needs a C++ compiler.
- `fuzz/target/`, `fuzz/corpus/`, `fuzz/artifacts/` and `fuzz/coverage/` are ignored by git. Only `fuzz/regressions/` is tracked.

## On stable Rust, with no fuzzing tools

```bash
cargo test --manifest-path fuzz/Cargo.toml
```

This runs:
- each harness on its seed, checking that the seed reaches the deep paths (chains popped, `DRIVER_OK`, input delivered);
- 4000 deterministic inputs per harness, half of them mutated seeds and half random bytes. A fixed xorshift generator makes every run identical, so a failure always reproduces;
- every saved crash input under `fuzz/regressions/<target>/`;
- the `vm.toml` parser's own unit tests, as compiled into the fuzz crate.

CI runs this, plus `cargo fmt --check` and clippy on the fuzz crate, on Linux and Windows. It is a smoke test, not fuzzing: it explores far less than a coverage-guided fuzzer.

To replay one saved input, or a directory of them, on stable:

```bash
cargo run --manifest-path fuzz/Cargo.toml --example reproduce -- virtqueue path/to/crash-file
```

A broken invariant shows as a panic naming it (set `RUST_BACKTRACE=1` for the stack) and exit status 101; exit 0 means every input ran cleanly.

## Coverage-guided fuzzing

### What this machine has (checked 2026-10-27)

| | Windows (ASUS ROG Strix dev machine) | WSL2 Ubuntu 24.04 (same machine) |
|---|---|---|
| Nightly Rust | **yes**: `nightly-x86_64-pc-windows-msvc`, rustc 1.100.0-nightly (6eeff9a52 2026-09-23) | no (only 1.97.0) |
| `cargo-fuzz` | **no** | no |
| C++ compiler for libFuzzer | yes: MSVC Build Tools 14.44.35207 | **no** (no `clang`, `clang++` or `g++`) |
| AddressSanitizer runtime | yes: `clang_rt.asan_dynamic-x86_64.dll` in the MSVC `bin\HostX64\x64` directory, not on `PATH` by default | — |

Nothing was installed for this work.

### With cargo-fuzz (the standard way)

Not run here, since cargo-fuzz is not installed. These are the documented cargo-fuzz commands for this layout. Installing cargo-fuzz needs owner approval:

```bash
cargo install --locked cargo-fuzz
```

On Windows, put the MSVC AddressSanitizer runtime on `PATH` for the session first, for example in PowerShell:
`$env:PATH = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64;$env:PATH"`.

Then, from the repository root:

```bash
cargo test --manifest-path fuzz/Cargo.toml -- --ignored write_seed_corpus   # seeds into fuzz/corpus/<target>/
cargo +nightly fuzz list
cargo +nightly fuzz run --features libfuzzer virtqueue -- -max_total_time=600
cargo +nightly fuzz run --features libfuzzer virtio_mmio -- -max_total_time=600
cargo +nightly fuzz run --features libfuzzer vm_toml -- -max_total_time=600
```

`--features libfuzzer` is required, because the targets depend on that feature. Crashes are saved under `fuzz/artifacts/<target>/`.

On WSL2 or another Linux host, fuzzing additionally needs, with owner approval: `rustup toolchain install nightly`, and a C++ compiler for libFuzzer, for example `sudo apt install clang`.

### Without cargo-fuzz (what was run here)

cargo-fuzz is a wrapper around one cargo invocation. The following PowerShell commands build the targets the same way, using only the tools listed above:

```powershell
$env:PATH = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64;$env:PATH"
$env:RUSTFLAGS = "-Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=4 " +
    "-Cllvm-args=-sanitizer-coverage-inline-8bit-counters -Cllvm-args=-sanitizer-coverage-pc-table " +
    "-Cllvm-args=-sanitizer-coverage-trace-compares --cfg fuzzing -Cdebug-assertions -Coverflow-checks " +
    "-Clink-arg=/include:main -Zsanitizer=address"
cargo +nightly build --manifest-path fuzz/Cargo.toml --features libfuzzer --release --bins `
    --target x86_64-pc-windows-msvc --target-dir fuzz/target/libfuzzer-nightly
Remove-Item Env:RUSTFLAGS
fuzz\target\libfuzzer-nightly\x86_64-pc-windows-msvc\release\virtqueue.exe `
    -max_total_time=180 -artifact_prefix=fuzz/artifacts/virtqueue/ fuzz/corpus/virtqueue
```

Some of these details were found by trial on this machine:
- **`--target` is required.** Without it, `RUSTFLAGS` also instruments build scripts, which then fail to link.
- **`/include:main`** pulls in libFuzzer's `main`, which the MSVC linker otherwise drops (error `LNK1561`).
- **AddressSanitizer is required on Windows**, even for Rust code with no `unsafe`. It is what defines the coverage section symbols; without `-Zsanitizer=address`, linking fails on `__start___sancov_cntrs`. That is why Windows fuzzing needs nightly.

### Crashes on Windows

libfuzzer-sys turns a panic into an abort. On Windows a Rust abort is a fast-fail exception, which libFuzzer cannot intercept, so it would not save the crashing input. The targets work around this:
- They call `aegis_fuzz::fuzz_init()` from their `init` block. On Windows it installs a panic hook that writes the current input to `<artifact prefix>crash-<FNV-1a hash>` and then aborts.
- On other systems it does nothing, and libFuzzer saves `crash-<sha1>` as usual.

## Results so far

All of these ran on the **Windows development machine** (ASUS ROG Strix, Intel Core Ultra 9 275HX) on 2026-10-27, using the build above. Nothing has been fuzzed on Linux or on the ThinkPad.

| Target | Time | Executions | Coverage (`cov` / `ft`) | Corpus | Crashes |
|---|---|---|---|---|---|
| `virtqueue` | 181 s | 3,778,201 | 189 / 690 | 90 | **0** |
| `virtio_mmio` | 181 s | 1,514,566 | 436 / 2056 | 405 | **0** |
| `vm_toml` | 181 s | 498,535 | 2897 / 9562 | 1823 | **0** |

The three ran in parallel, from the seed corpus. Three minutes each is a smoke run, not a campaign. Longer runs are worth doing before a release.

**Detection check (mutation).** On a scratch change that was reverted afterwards, the descriptor-index check in `SplitQueue::walk` was weakened to accept indices up to `0x1000`. Results:
- The fuzzer found it after about 110 executions, under a second: `head 256 of 8`. The Windows hook saved the input.
- `reproduce` on stable failed on that file with the same message (exit 101).
- After the check was restored, the same file replayed cleanly (exit 0).
- The stable smoke test `random_and_mutated_inputs_keep_the_virtqueue_invariants` also failed while the check was weakened.

## When the fuzzer finds a crash

1. Reproduce it on stable: `cargo run --manifest-path fuzz/Cargo.toml --example reproduce -- <target> fuzz/artifacts/<target>/crash-…`.
2. Fix the bug, and add a unit test in the crate that owns the code.
3. Minimize the input (`cargo +nightly fuzz tmin --features libfuzzer <target> <file>`), and save it as `fuzz/regressions/<target>/<short-description>`. See [`fuzz/regressions/README.md`](../fuzz/regressions/README.md).
4. `cargo test --manifest-path fuzz/Cargo.toml` now replays it on every CI run.

## Licensing

`libfuzzer-sys`'s metadata says "(MIT OR Apache-2.0) AND NCSA". The bundled libFuzzer files themselves are tagged `Apache-2.0 WITH LLVM-exception`. The ADR-0011 allowlist names neither NCSA nor `Apache-2.0 WITH LLVM-exception`. The crate is used only by the fuzz targets and is never linked into Aegis. See the [licensing review §3.2](../research/licensing-review.md#32-fuzz-only-dependencies-2026-10-27).
