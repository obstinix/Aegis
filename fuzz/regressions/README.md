# Fuzz regression inputs

Inputs that once crashed a fuzz target, kept after the fix so the crash stays fixed. One directory per target, named as in `cargo fuzz list`:

```text
fuzz/regressions/virtqueue/
fuzz/regressions/virtio_mmio/
fuzz/regressions/vm_toml/
```

`cargo test --manifest-path fuzz/Cargo.toml` replays every file in them on stable Rust (`saved_regression_inputs_still_pass`), so CI catches a regression without a fuzzer.

To add one, after fixing the bug the fuzzer found:

1. Minimize the crash: `cargo +nightly fuzz tmin --features libfuzzer <target> fuzz/artifacts/<target>/crash-<hash>`.
2. Copy the minimized file here as `fuzz/regressions/<target>/<short-description>`, for example `virtqueue/desc-index-past-size`.
3. Check it replays cleanly: `cargo run --manifest-path fuzz/Cargo.toml --example reproduce -- <target> fuzz/regressions/<target>`.
4. Commit it with the fix, and name the input file in the commit message.

Keep inputs small (a few KiB at most). Files ending in `.md` are notes and are not replayed. None has been saved yet: no fuzzing campaign has been run (docs/fuzzing.md).
