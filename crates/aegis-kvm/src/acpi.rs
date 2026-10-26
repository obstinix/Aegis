//! ACPI tables for SMP discovery and virtio-mmio devices (roadmap M4h).
//!
//! The test guest kernel is built without `CONFIG_X86_MPPARSE` and without
//! `CONFIG_VIRTIO_MMIO_CMDLINE_DEVICES`. ACPI is therefore how the guest
//! learns about its CPUs (MADT) and its virtio-mmio devices (DSDT).
//!
//! The tables are built with rust-vmm's `acpi_tables`:
//! - **RSDP** (revision 2) → **XSDT** → **FADT** and **MADT**. The FADT points
//!   at the **DSDT**.
//! - The FADT sets `HW_REDUCED_ACPI`. There are no PM1/GPE blocks or SCI, and
//!   none are needed: the guest powers off by triple fault (`reboot=t`).
//! - The MADT has one enabled Processor Local APIC per vCPU (UID = APIC ID =
//!   vCPU index, matching KVM's default LAPIC IDs and the CPUID values Aegis
//!   sets) and one I/O APIC at `0xFEC0_0000` with GSI base 0 (KVM's in-kernel
//!   IOAPIC).
//! - The DSDT declares each virtio-mmio device as `\_SB.VRnn` with
//!   `_HID "LNRO0005"` (the ACPI ID that Linux's virtio-mmio driver matches),
//!   `_UID` = nn, and a `_CRS` holding a 32-bit fixed memory range and one
//!   level-triggered, active-high, exclusive interrupt.
//!
//! The tables live at [`ACPI_BASE`], inside the e820 hole between 640 KiB and
//! 1 MiB (never reported as RAM), each on a 16-byte boundary. The RSDP sits
//! first, so it is also found by the kernel's legacy scan of `0xE0000`–
//! `0xFFFFF`.

use acpi_tables::Aml;
use acpi_tables::aml::{Device, Interrupt, Memory32Fixed, Name, Path, ResourceTemplate, Scope};
use acpi_tables::fadt::{FADTBuilder, Flags};
use acpi_tables::madt::{
    EnabledStatus, IoApic, LocalInterruptController, MADT, ProcessorLocalApic,
};
use acpi_tables::rsdp::Rsdp;
use acpi_tables::sdt::Sdt;
use acpi_tables::xsdt::XSDT;

/// Where the tables start (the RSDP).
pub const ACPI_BASE: u64 = 0xE_0000;
/// The tables must end below this (start of the 1 MiB kernel area).
pub const ACPI_LIMIT: u64 = 0x10_0000;
/// Local APIC base (architectural default).
pub const LAPIC_ADDR: u32 = 0xFEE0_0000;
/// KVM in-kernel I/O APIC base.
pub const IOAPIC_ADDR: u32 = 0xFEC0_0000;
/// Most vCPUs the tables describe (local APIC IDs fit in a byte).
pub const MAX_VCPUS: u8 = 8;

const OEM_ID: [u8; 6] = *b"AEGIS ";
const OEM_REVISION: u32 = 1;
/// XSDT size: header plus two 64-bit entries (FADT, MADT).
const XSDT_LEN: usize = 36 + 2 * 8;

/// A virtio-mmio device to declare in the DSDT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtioMmioSpec {
    /// MMIO window base (guest-physical, below 4 GiB).
    pub base: u32,
    /// MMIO window length.
    pub len: u32,
    /// I/O APIC input (GSI) for the device's interrupt.
    pub gsi: u32,
}

/// What the tables describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpiConfig {
    /// Number of vCPUs, 1..=[`MAX_VCPUS`].
    pub vcpus: u8,
    /// virtio-mmio devices, at most 100 (named `VR00`..`VR99`).
    pub virtio_mmio: Vec<VirtioMmioSpec>,
}

/// One table placed in guest memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedTable {
    /// Four-character signature (`RSD PTR ` is shown as `RSDP`).
    pub signature: [u8; 4],
    /// Guest-physical address.
    pub addr: u64,
    /// Table bytes.
    pub bytes: Vec<u8>,
}

/// Why the tables could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpiError {
    /// Zero vCPUs or more than [`MAX_VCPUS`].
    VcpuCount(u8),
    /// More than 100 virtio-mmio devices.
    TooManyDevices(usize),
    /// The tables do not fit between [`ACPI_BASE`] and [`ACPI_LIMIT`].
    TooLarge(u64),
}

impl std::fmt::Display for AcpiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VcpuCount(n) => write!(
                f,
                "{n} vCPUs requested; ACPI tables support 1..={MAX_VCPUS}"
            ),
            Self::TooManyDevices(n) => write!(f, "{n} virtio-mmio devices; at most 100"),
            Self::TooLarge(end) => {
                write!(f, "ACPI tables would end at {end:#x}, past {ACPI_LIMIT:#x}")
            }
        }
    }
}

impl std::error::Error for AcpiError {}

fn bytes_of(table: &dyn Aml) -> Vec<u8> {
    let mut out = Vec::new();
    table.to_aml_bytes(&mut out);
    out
}

/// The 16-byte-aligned address after a table of `len` bytes at `addr`.
fn next_table(addr: u64, len: usize) -> Result<u64, AcpiError> {
    u64::try_from(len)
        .ok()
        .and_then(|len| addr.checked_add(len))
        .and_then(|end| end.checked_add(15))
        .map(|a| a & !15)
        .ok_or(AcpiError::TooLarge(u64::MAX))
}

/// The DSDT: `\_SB_.VRnn` for each device, with `_UID` nn. `_UID` must be
/// unique among devices with the same `_HID`. `build` allows at most 100
/// devices, so every index fits the two-digit name and a `u8`.
fn dsdt(devices: &[VirtioMmioSpec]) -> Vec<u8> {
    let ids: Vec<u8> = (0..=u8::MAX).take(devices.len()).collect();
    let names: Vec<String> = ids.iter().map(|i| format!("VR{i:02}")).collect();
    let resources: Vec<(Memory32Fixed, Interrupt)> = devices
        .iter()
        .map(|d| {
            (
                Memory32Fixed::new(true, d.base, d.len),
                // consumer, level-triggered, active-high, exclusive
                Interrupt::new(true, false, false, false, d.gsi),
            )
        })
        .collect();
    let crs: Vec<ResourceTemplate<'_>> = resources
        .iter()
        .map(|(m, i)| ResourceTemplate::new(vec![m, i]))
        .collect();
    let hids: Vec<Name> = devices
        .iter()
        .map(|_| Name::new(Path::new("_HID"), &"LNRO0005"))
        .collect();
    let uids: Vec<Name> = ids
        .iter()
        .map(|id| Name::new(Path::new("_UID"), id))
        .collect();
    let crs_names: Vec<Name> = crs
        .iter()
        .map(|c| Name::new(Path::new("_CRS"), c))
        .collect();
    let device_objs: Vec<Device<'_>> = names
        .iter()
        .zip(hids.iter().zip(uids.iter().zip(crs_names.iter())))
        .map(|(name, (hid, (uid, crs)))| Device::new(Path::new(name), vec![hid, uid, crs]))
        .collect();
    let children: Vec<&dyn Aml> = device_objs.iter().map(|d| d as &dyn Aml).collect();
    let sb = Scope::new(Path::new("\\_SB_"), children);

    let mut table = Sdt::new(*b"DSDT", 36, 6, OEM_ID, *b"AEGISDSD", OEM_REVISION);
    table.append_slice(&bytes_of(&sb));
    table.as_slice().to_vec()
}

/// Build and place the RSDP, XSDT, FADT, DSDT and MADT.
///
/// # Errors
///
/// [`AcpiError`] if the configuration is out of range or the tables do not fit.
pub fn build(cfg: &AcpiConfig) -> Result<Vec<PlacedTable>, AcpiError> {
    if cfg.vcpus == 0 || cfg.vcpus > MAX_VCPUS {
        return Err(AcpiError::VcpuCount(cfg.vcpus));
    }
    if cfg.virtio_mmio.len() > 100 {
        return Err(AcpiError::TooManyDevices(cfg.virtio_mmio.len()));
    }

    let dsdt_bytes = dsdt(&cfg.virtio_mmio);

    let mut madt = MADT::new(
        OEM_ID,
        *b"AEGISMAD",
        OEM_REVISION,
        LocalInterruptController::Address(LAPIC_ADDR),
    );
    for id in 0..cfg.vcpus {
        madt.add_structure(ProcessorLocalApic::new(id, id, EnabledStatus::Enabled));
    }
    madt.add_structure(IoApic::new(0, IOAPIC_ADDR, 0));
    let madt_bytes = bytes_of(&madt);

    // Layout: RSDP, XSDT (2 entries), FADT, DSDT, MADT. The XSDT has a fixed
    // size (header + 2 × 8 bytes), so every address is known up front.
    let xsdt_addr = next_table(ACPI_BASE, Rsdp::len())?;
    let fadt_addr = next_table(xsdt_addr, XSDT_LEN)?;
    let fadt = FADTBuilder::new(OEM_ID, *b"AEGISFAC", OEM_REVISION)
        .flag(Flags::HwReducedAcpi)
        .dsdt_64(0) // patched below once the DSDT address is known
        .finalize();
    let dsdt_addr = next_table(fadt_addr, bytes_of(&fadt).len())?;
    let fadt = FADTBuilder::new(OEM_ID, *b"AEGISFAC", OEM_REVISION)
        .flag(Flags::HwReducedAcpi)
        .dsdt_64(dsdt_addr)
        .finalize();
    let madt_addr = next_table(dsdt_addr, dsdt_bytes.len())?;
    let end = next_table(madt_addr, madt_bytes.len())?;
    if end > ACPI_LIMIT {
        return Err(AcpiError::TooLarge(end));
    }

    let mut xsdt = XSDT::new(OEM_ID, *b"AEGISXSD", OEM_REVISION);
    xsdt.add_entry(fadt_addr);
    xsdt.add_entry(madt_addr);
    let rsdp = Rsdp::new(OEM_ID, xsdt_addr);

    Ok(vec![
        PlacedTable {
            signature: *b"RSDP",
            addr: ACPI_BASE,
            bytes: bytes_of(&rsdp),
        },
        PlacedTable {
            signature: *b"XSDT",
            addr: xsdt_addr,
            bytes: bytes_of(&xsdt),
        },
        PlacedTable {
            signature: *b"FACP",
            addr: fadt_addr,
            bytes: bytes_of(&fadt),
        },
        PlacedTable {
            signature: *b"DSDT",
            addr: dsdt_addr,
            bytes: dsdt_bytes,
        },
        PlacedTable {
            signature: *b"APIC",
            addr: madt_addr,
            bytes: madt_bytes,
        },
    ])
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)] // fixed offsets into small tables
mod tests {
    use super::*;

    fn sum(bytes: &[u8]) -> u8 {
        bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b))
    }

    fn u32_at(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
    }

    fn u64_at(b: &[u8], off: usize) -> u64 {
        u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
    }

    fn virtio() -> VirtioMmioSpec {
        VirtioMmioSpec {
            base: 0xD000_0000,
            len: 0x1000,
            gsi: 5,
        }
    }

    fn table(t: &[PlacedTable], sig: [u8; 4]) -> &PlacedTable {
        t.iter().find(|p| p.signature == sig).unwrap()
    }

    #[test]
    fn every_table_checksums_to_zero_and_has_its_declared_length() {
        let t = build(&AcpiConfig {
            vcpus: 4,
            virtio_mmio: vec![virtio()],
        })
        .unwrap();
        let rsdp = &table(&t, *b"RSDP").bytes;
        assert_eq!(&rsdp[..8], b"RSD PTR ");
        assert_eq!(sum(&rsdp[..20]), 0, "RSDP v1 checksum (first 20 bytes)");
        assert_eq!(sum(rsdp), 0, "RSDP extended checksum");
        assert_eq!(rsdp[15], 2, "revision 2 (ACPI 2.0+, has XSDT)");
        for sig in [*b"XSDT", *b"FACP", *b"DSDT", *b"APIC"] {
            let b = &table(&t, sig).bytes;
            assert_eq!(b[..4], sig);
            assert_eq!(
                u32_at(b, 4) as usize,
                b.len(),
                "{} length field",
                String::from_utf8_lossy(&sig)
            );
            assert_eq!(sum(b), 0, "{} checksum", String::from_utf8_lossy(&sig));
        }
    }

    #[test]
    fn tables_are_placed_aligned_in_the_e820_hole_and_linked() {
        let t = build(&AcpiConfig {
            vcpus: 2,
            virtio_mmio: vec![virtio()],
        })
        .unwrap();
        assert_eq!(
            t[0].addr, ACPI_BASE,
            "RSDP first, inside the legacy scan range"
        );
        let mut end = 0;
        for p in &t {
            assert_eq!(
                p.addr % 16,
                0,
                "{:?} aligned",
                String::from_utf8_lossy(&p.signature)
            );
            assert!(p.addr >= end, "no overlap");
            end = p.addr + p.bytes.len() as u64;
        }
        assert!(end <= ACPI_LIMIT);
        let rsdp = &table(&t, *b"RSDP").bytes;
        assert_eq!(u64_at(rsdp, 24), table(&t, *b"XSDT").addr, "RSDP -> XSDT");
        let xsdt = &table(&t, *b"XSDT").bytes;
        assert_eq!(xsdt.len(), 36 + 16, "two entries");
        assert_eq!(u64_at(xsdt, 36), table(&t, *b"FACP").addr);
        assert_eq!(u64_at(xsdt, 44), table(&t, *b"APIC").addr);
        let fadt = &table(&t, *b"FACP").bytes;
        assert_eq!(
            u64_at(fadt, 140),
            table(&t, *b"DSDT").addr,
            "FADT X_DSDT -> DSDT"
        );
        assert_ne!(u32_at(fadt, 112) & (1 << 20), 0, "HW_REDUCED_ACPI flag");
    }

    #[test]
    fn madt_lists_one_enabled_local_apic_per_vcpu_and_the_ioapic() {
        for vcpus in [1u8, 2, 8] {
            let t = build(&AcpiConfig {
                vcpus,
                virtio_mmio: vec![],
            })
            .unwrap();
            let m = &table(&t, *b"APIC").bytes;
            assert_eq!(u32_at(m, 36), LAPIC_ADDR);
            let mut off = 44;
            let (mut lapics, mut ioapics) = (Vec::new(), Vec::new());
            while off < m.len() {
                let (kind, len) = (m[off], usize::from(m[off + 1]));
                match kind {
                    0 => lapics.push((m[off + 2], m[off + 3], u32_at(m, off + 4))),
                    1 => ioapics.push((m[off + 2], u32_at(m, off + 4), u32_at(m, off + 8))),
                    other => panic!("unexpected MADT structure type {other}"),
                }
                off += len;
            }
            assert_eq!(off, m.len(), "structures exactly fill the table");
            let expected: Vec<_> = (0..vcpus).map(|i| (i, i, 1)).collect();
            assert_eq!(lapics, expected, "UID = APIC ID = index, enabled");
            assert_eq!(ioapics, [(0, IOAPIC_ADDR, 0)]);
        }
    }

    #[test]
    fn dsdt_declares_the_virtio_mmio_device_for_linux() {
        let t = build(&AcpiConfig {
            vcpus: 1,
            virtio_mmio: vec![virtio()],
        })
        .unwrap();
        let d = &table(&t, *b"DSDT").bytes;
        let find = |needle: &[u8]| d.windows(needle.len()).position(|w| w == needle);
        assert!(find(b"_SB_").is_some(), "\\_SB scope");
        assert!(find(b"VR00").is_some(), "device name");
        // NameOp "_HID" StringPrefix "LNRO0005" NUL
        assert!(find(b"\x08_HID\x0dLNRO0005\x00").is_some(), "_HID string");
        // NameOp "_UID" ZeroOp
        assert!(find(b"\x08_UID\x00").is_some(), "_UID 0");
        // Memory32Fixed: tag 0x86, length 9, read-write, base, length
        let mem = [
            0x86, 0x09, 0x00, 0x01, 0x00, 0x00, 0x00, 0xD0, 0x00, 0x10, 0x00, 0x00,
        ];
        assert!(
            find(&mem).is_some(),
            "_CRS memory range 0xD0000000 + 0x1000"
        );
        // Extended Interrupt: tag 0x89, length 6, flags (consumer, level,
        // active-high, exclusive) = 0x01, count 1, GSI 5
        let int = [0x89, 0x06, 0x00, 0x01, 0x01, 0x05, 0x00, 0x00, 0x00];
        assert!(
            find(&int).is_some(),
            "_CRS interrupt GSI 5, level, active-high"
        );
        assert!(find(&[0x79]).is_some(), "end tag");
    }

    #[test]
    fn dsdt_gives_each_device_its_own_name_and_uid() {
        let second = VirtioMmioSpec {
            base: 0xD000_1000,
            len: 0x1000,
            gsi: 6,
        };
        let t = build(&AcpiConfig {
            vcpus: 1,
            virtio_mmio: vec![virtio(), second, virtio()],
        })
        .unwrap();
        let d = &table(&t, *b"DSDT").bytes;
        let find = |needle: &[u8]| d.windows(needle.len()).position(|w| w == needle);
        // Each device's name comes before its own _UID, in order: ZeroOp,
        // OneOp, then BytePrefix 2.
        let mut at = 0;
        for (name, uid) in [
            (&b"VR00"[..], &b"\x08_UID\x00"[..]),
            (b"VR01", b"\x08_UID\x01"),
            (b"VR02", b"\x08_UID\x0a\x02"),
        ] {
            let n = find(name).unwrap_or_else(|| panic!("{name:?}"));
            let u = find(uid).unwrap_or_else(|| panic!("{uid:?}"));
            assert!(at <= n && n < u, "{name:?} at {n}, its _UID at {u}");
            at = u;
        }
        let int6 = [0x89, 0x06, 0x00, 0x01, 0x01, 0x06, 0x00, 0x00, 0x00];
        assert!(find(&int6).is_some(), "second device's GSI 6");
    }

    #[test]
    fn dsdt_without_devices_is_an_empty_system_bus() {
        let t = build(&AcpiConfig {
            vcpus: 2,
            virtio_mmio: vec![],
        })
        .unwrap();
        let d = &table(&t, *b"DSDT").bytes;
        assert!(d.windows(4).all(|w| w != b"VR00"));
        assert!(d.windows(4).any(|w| w == b"_SB_"));
    }

    #[test]
    fn rejects_out_of_range_configurations() {
        assert_eq!(
            build(&AcpiConfig {
                vcpus: 0,
                virtio_mmio: vec![]
            }),
            Err(AcpiError::VcpuCount(0))
        );
        assert_eq!(
            build(&AcpiConfig {
                vcpus: 9,
                virtio_mmio: vec![]
            }),
            Err(AcpiError::VcpuCount(9))
        );
        let many = vec![virtio(); 101];
        assert_eq!(
            build(&AcpiConfig {
                vcpus: 1,
                virtio_mmio: many
            }),
            Err(AcpiError::TooManyDevices(101))
        );
    }
}
