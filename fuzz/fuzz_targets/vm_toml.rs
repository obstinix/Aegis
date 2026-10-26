//! cargo-fuzz target: the vm.toml parser.
//! The harness and its invariants are [`aegis_fuzz::vm_toml`].

#![no_main]

libfuzzer_sys::fuzz_target!(init: aegis_fuzz::fuzz_init(), |data: &[u8]| {
    aegis_fuzz::fuzz_input("vm_toml", data);
});
