//! Aegis bare-metal core (ADR-0005), built for `x86_64-unknown-uefi`.
//!
//! Roadmap B0: the payload of the nested test harness (`cargo xtask
//! uefi-test`). Under OVMF it prints `AEGIS_B0_HELLO` on the firmware console,
//! which OVMF mirrors to the serial port. Then it powers the machine off
//! with `ResetSystem(EfiResetShutdown)`, so QEMU exits on its own. VMX comes
//! in B1.
//!
//! Firmware access uses `uefi-raw`'s type definitions (MIT OR Apache-2.0)
//! rather than the `uefi` crate that ADR-0003 names: `uefi` requires
//! `ucs2`, which is MPL-2.0 and outside the licence allowlist (ADR-0011).
//! B0 needs three firmware accesses, each a small `unsafe` block below.
//!
//! On any other target this builds as a stub that says how to build it, so
//! `cargo test --workspace` works on every host.

#![cfg_attr(target_os = "uefi", no_std, no_main)]
#![deny(unsafe_code)]
#![warn(clippy::undocumented_unsafe_blocks)]

#[cfg(target_os = "uefi")]
mod app {
    use core::fmt::{self, Write as _};

    use uefi_raw::protocol::console::SimpleTextOutputProtocol;
    use uefi_raw::table::runtime::ResetType;
    use uefi_raw::table::system::SystemTable;
    use uefi_raw::{Handle, Status};

    /// What the harness looks for on the serial console.
    const MARKER: &str = "AEGIS_B0_HELLO";

    /// UCS-2 code units buffered per `OutputString` call (plus the NUL).
    const CHUNK: usize = 64;

    /// The firmware's text console, valid while boot services are active.
    struct Console {
        out: *mut SimpleTextOutputProtocol,
        buf: [u16; CHUNK + 1],
        len: usize,
    }

    impl Console {
        #[allow(unsafe_code)]
        fn flush(&mut self) -> fmt::Result {
            if self.len == 0 {
                return Ok(());
            }
            self.buf[self.len] = 0;
            self.len = 0;
            // SAFETY: `out` is the system table's console, valid while boot
            // services are active (the whole run). The buffer is
            // NUL-terminated and outlives the call, and the firmware only
            // reads it.
            let status = unsafe { ((*self.out).output_string)(self.out, self.buf.as_ptr()) };
            if status.is_error() {
                Err(fmt::Error)
            } else {
                Ok(())
            }
        }

        fn push(&mut self, unit: u16) -> fmt::Result {
            if self.len == CHUNK {
                self.flush()?;
            }
            self.buf[self.len] = unit;
            self.len = self.len.saturating_add(1);
            Ok(())
        }
    }

    impl fmt::Write for Console {
        /// ASCII only: other characters print as `?`, and `\n` becomes the
        /// `\r\n` a UEFI console expects.
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for c in s.chars() {
                let byte = if c.is_ascii() {
                    u8::try_from(c).unwrap_or(b'?')
                } else {
                    b'?'
                };
                if byte == b'\n' {
                    self.push(u16::from(b'\r'))?;
                }
                self.push(u16::from(byte))?;
            }
            self.flush()
        }
    }

    /// The UEFI entry point.
    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    extern "efiapi" fn efi_main(_image: Handle, system_table: *const SystemTable) -> Status {
        // SAFETY: the firmware passes a valid system table, and it stays
        // valid while boot services are active, which they are throughout.
        let st = unsafe { &*system_table };
        let mut console = Console {
            out: st.stdout,
            buf: [0; CHUNK + 1],
            len: 0,
        };
        // If the console fails, the harness reports the missing marker.
        let _ = writeln!(
            console,
            "{MARKER}: aegis-uefi {} is running as a UEFI application \
             (UEFI revision {:#010x}, firmware revision {:#010x})",
            env!("CARGO_PKG_VERSION"),
            st.header.revision.0,
            st.firmware_revision
        );
        // SAFETY: `runtime_services` comes from the valid system table.
        // ResetSystem reads no data when `data_size` is 0, and does not
        // return.
        unsafe {
            ((*st.runtime_services).reset_system)(
                ResetType::SHUTDOWN,
                Status::SUCCESS,
                0,
                core::ptr::null(),
            )
        }
    }

    /// No console is reachable here, so spin. The harness's timeout
    /// reports the hang along with the serial log.
    #[panic_handler]
    fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
        loop {
            core::hint::spin_loop();
        }
    }
}

#[cfg(not(target_os = "uefi"))]
fn main() {
    eprintln!(
        "aegis-uefi is a UEFI application. Build it with \
         `cargo build -p aegis-uefi --target x86_64-unknown-uefi`, or boot it \
         under QEMU with `cargo xtask uefi-test` (Linux with KVM)."
    );
    std::process::exit(2);
}
