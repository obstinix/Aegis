//! cargo-fuzz target: virtio-mmio register handling, with the console behind it.
//! The harness and its invariants are [`aegis_fuzz::virtio_mmio`].

#![no_main]

libfuzzer_sys::fuzz_target!(init: aegis_fuzz::fuzz_init(), |data: &[u8]| {
    aegis_fuzz::fuzz_input("virtio_mmio", data);
});
