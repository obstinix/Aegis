//! cargo-fuzz target: virtqueue processing.
//! The harness and its invariants are [`aegis_fuzz::virtqueue`].

#![no_main]

libfuzzer_sys::fuzz_target!(init: aegis_fuzz::fuzz_init(), |data: &[u8]| {
    aegis_fuzz::fuzz_input("virtqueue", data);
});
