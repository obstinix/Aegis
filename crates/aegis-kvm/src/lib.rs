//! Aegis hosted backend: Linux KVM via rust-vmm (ADR-0004, roadmap M2).
//!
//! `KvmVm` owns a KVM VM and its guest memory, built from a validated
//! [`aegis_core::GuestMemoryMap`]. `KvmVcpu` implements [`aegis_core::Vcpu`],
//! so the backend-neutral [`aegis_core::run_vcpu`] drives it. (This crate's
//! own types are not linked here because they exist only on Linux, while
//! this crate-level text is rendered on every OS.)
//!
//! Memory safety of registering guest memory with KVM, the `unsafe`
//! operation that matters, rests on two type-level guarantees:
//! - `KvmVm` drops its `VmFd` before the guest memory mapping.
//! - `KvmVcpu<'vm>` borrows its `KvmVm`, so no vCPU outlives the memory.
//!
//! The other `unsafe` blocks are plain libc calls on integers or on a
//! buffer the caller owns: `gettid` and `tgkill` in `smp`, and `poll` and
//! `read` in `input`. Each has its own SAFETY comment.
//!
//! Guests can be entered in real mode (M2 test guests) or booted as Linux
//! through the 64-bit boot protocol (M3, see `boot`). Linux needs the
//! in-kernel PIC/IOAPIC and PIT, enabled before any vCPU exists.
//! - `acpi` describes SMP vCPUs and virtio-mmio devices to the guest (M4h).
//! - `smp` runs one sandboxed thread per vCPU (M4h).
//! - `input` forwards host stdin to a device such as the virtio-console.
//! - `seccomp` holds the syscall allowlists for those threads.
//!
//! Limits:
//! - String (`rep ins/outs`) port I/O is reported as an unhandled exit.
//! - MMIO is served by devices on the bus. Unclaimed MMIO stops the VM.
//! - Memory can be read-only, but not non-executable: KVM memory slots
//!   cannot deny execution.
//! - Linux boot needs a single RAM region starting at 0, of at most 1 GiB.
//!
//! On non-Linux targets this crate is empty.

#![cfg(target_os = "linux")]
#![deny(unsafe_code)]
#![warn(clippy::undocumented_unsafe_blocks)]

use std::fmt;
use std::marker::PhantomData;

pub mod acpi;
pub mod boot;
pub mod input;
pub mod seccomp;
pub mod smp;

use aegis_core::{
    AccessSize, GuestAccessError, GuestIo, GuestMemoryAccess, GuestMemoryMap, InterruptLine,
    IoAccess, MmioAccess, Vcpu, VmExit,
};
use kvm_bindings::{
    KVM_MAX_CPUID_ENTRIES, KVM_MEM_READONLY, KVM_PIT_SPEAKER_DUMMY, kvm_cpuid_entry2,
    kvm_pit_config, kvm_regs, kvm_segment, kvm_userspace_memory_region,
};

use crate::boot::{BootError, BzImage};
use kvm_ioctls::{Cap, Kvm, VcpuExit, VcpuFd, VmFd};
use vm_memory::{Bytes, GuestAddress, GuestMemoryBackend, GuestMemoryMmap, GuestMemoryRegion};

/// RFLAGS bit 1 is reserved and must be set.
const RFLAGS_RESERVED: u64 = 0x2;
/// Three pages KVM needs on Intel for its internal TSS, just below 4 GiB and
/// outside any guest RAM Aegis maps (the address Firecracker also uses).
const KVM_TSS_ADDR: u64 = 0xFFFB_D000;
const KVM_TSS_END: u64 = 0xFFFC_0000;
const CR0_PE: u64 = 0x1;
const CR0_PG: u64 = 0x8000_0000;
const CR4_PAE: u64 = 0x20;
const EFER_LME: u64 = 0x100;
const EFER_LMA: u64 = 0x400;
/// Local APIC LVT LINT0 / LINT1 registers (Intel SDM Vol. 3A, 11.5.1).
const APIC_LVT_LINT0: std::ops::Range<usize> = 0x350..0x354;
const APIC_LVT_LINT1: std::ops::Range<usize> = 0x360..0x364;
const APIC_DELIVERY_MODE_MASK: u32 = 0x700;
const APIC_MODE_EXTINT: u32 = 0x700;
const APIC_MODE_NMI: u32 = 0x400;

/// Errors from the KVM backend.
#[derive(Debug)]
pub enum KvmError {
    /// A KVM ioctl failed.
    Kvm {
        /// What Aegis was doing.
        op: &'static str,
        /// The OS error.
        source: kvm_ioctls::Error,
    },
    /// Guest memory could not be allocated or accessed.
    Memory(String),
    /// The request needs something this backend does not support.
    Unsupported(&'static str),
    /// A host write to guest memory does not fit inside one guest region.
    OutOfBounds {
        /// Start of the write.
        gpa: u64,
        /// Length of the write.
        len: u64,
    },
    /// The kernel image or boot configuration was rejected.
    Boot(BootError),
}

impl From<BootError> for KvmError {
    fn from(e: BootError) -> Self {
        Self::Boot(e)
    }
}

impl fmt::Display for KvmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kvm { op, source } => write!(f, "{op}: {source}"),
            Self::Memory(e) => write!(f, "guest memory: {e}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::OutOfBounds { gpa, len } => {
                write!(
                    f,
                    "write of {len:#x} bytes at {gpa:#x} is outside guest memory"
                )
            }
            Self::Boot(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for KvmError {}

fn kvm_err(op: &'static str) -> impl FnOnce(kvm_ioctls::Error) -> KvmError {
    move |source| KvmError::Kvm { op, source }
}

/// A KVM virtual machine with its guest memory.
pub struct KvmVm {
    // Declaration order is drop order: the VM (and its memory slots) goes
    // before the memory those slots point at.
    vm: VmFd,
    memory: GuestMemoryMmap,
    map: GuestMemoryMap,
    kvm: Kvm,
}

impl fmt::Debug for KvmVm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KvmVm")
            .field("map", &self.map)
            .finish_non_exhaustive()
    }
}

impl KvmVm {
    /// Create a VM whose guest memory is exactly `map`.
    ///
    /// # Errors
    ///
    /// - [`KvmError::Unsupported`] if a region denies read or execute, or is
    ///   read-only on a kernel without `KVM_CAP_READONLY_MEM`.
    /// - [`KvmError::Kvm`] if `/dev/kvm` cannot be opened or an ioctl fails.
    /// - [`KvmError::Memory`] if the host cannot allocate the guest memory.
    pub fn new(map: GuestMemoryMap) -> Result<Self, KvmError> {
        if map
            .regions()
            .iter()
            .any(|r| !r.permissions.read || !r.permissions.execute)
        {
            return Err(KvmError::Unsupported(
                "KVM memory slots cannot deny read or execute",
            ));
        }
        let kvm = Kvm::new().map_err(kvm_err("open /dev/kvm"))?;
        let needs_readonly = map.regions().iter().any(|r| !r.permissions.write);
        if needs_readonly && !kvm.check_extension(Cap::ReadonlyMem) {
            return Err(KvmError::Unsupported(
                "read-only guest memory needs KVM_CAP_READONLY_MEM",
            ));
        }
        let vm = kvm.create_vm().map_err(kvm_err("KVM_CREATE_VM"))?;

        let ranges = map
            .regions()
            .iter()
            .map(|r| {
                usize::try_from(r.size)
                    .map(|size| (GuestAddress(r.start), size))
                    .map_err(|_| {
                        KvmError::Unsupported("guest region larger than the host address space")
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let memory = GuestMemoryMmap::<()>::from_ranges(&ranges)
            .map_err(|e| KvmError::Memory(e.to_string()))?;
        if memory.num_regions() != map.regions().len() {
            return Err(KvmError::Memory(
                "vm-memory merged or dropped guest regions".to_owned(),
            ));
        }

        // Both sequences are in ascending address order.
        for (slot, (region, spec)) in memory.iter().zip(map.regions()).enumerate() {
            let slot =
                u32::try_from(slot).map_err(|_| KvmError::Unsupported("too many memory slots"))?;
            let userspace_addr = u64::try_from(region.as_ptr().addr())
                .map_err(|_| KvmError::Unsupported("host address wider than 64 bits"))?;
            let mem_region = kvm_userspace_memory_region {
                slot,
                flags: if spec.permissions.write {
                    0
                } else {
                    KVM_MEM_READONLY
                },
                guest_phys_addr: region.start_addr().0,
                memory_size: region.len(),
                userspace_addr,
            };
            #[allow(unsafe_code)]
            // SAFETY: `userspace_addr` is the start of a live anonymous mapping
            // of exactly `memory_size` bytes owned by `memory`. `memory` is
            // stored in the returned `KvmVm`, which drops `vm` (and so every
            // slot) first, and no `KvmVcpu` can outlive the `KvmVm`, so KVM
            // never uses the mapping after it is unmapped. The ranges come from
            // a validated `GuestMemoryMap`, so slots do not overlap.
            unsafe { vm.set_user_memory_region(mem_region) }
                .map_err(kvm_err("KVM_SET_USER_MEMORY_REGION"))?;
        }

        Ok(Self {
            vm,
            memory,
            map,
            kvm,
        })
    }

    /// The guest memory map this VM was built from.
    #[must_use]
    pub fn memory_map(&self) -> &GuestMemoryMap {
        &self.map
    }

    /// Create the in-kernel interrupt controllers (PIC, IOAPIC, LAPICs) and
    /// PIT. Must run before any vCPU exists; taking `&mut self` makes the
    /// borrow checker enforce that, since a vCPU borrows the VM.
    ///
    /// # Errors
    ///
    /// [`KvmError::Unsupported`] if guest RAM overlaps KVM's TSS pages;
    /// [`KvmError::Kvm`] if an ioctl fails.
    pub fn enable_legacy_interrupts(&mut self) -> Result<(), KvmError> {
        if self
            .map
            .regions()
            .iter()
            .any(|r| r.start < KVM_TSS_END && r.end().is_none_or(|end| end > KVM_TSS_ADDR))
        {
            return Err(KvmError::Unsupported(
                "guest RAM overlaps KVM's TSS pages below 4 GiB",
            ));
        }
        let tss = usize::try_from(KVM_TSS_ADDR)
            .map_err(|_| KvmError::Unsupported("host address space too small"))?;
        self.vm
            .set_tss_address(tss)
            .map_err(kvm_err("KVM_SET_TSS_ADDR"))?;
        self.vm
            .create_irq_chip()
            .map_err(kvm_err("KVM_CREATE_IRQCHIP"))?;
        let pit = kvm_pit_config {
            flags: KVM_PIT_SPEAKER_DUMMY,
            ..kvm_pit_config::default()
        };
        self.vm.create_pit2(pit).map_err(kvm_err("KVM_CREATE_PIT2"))
    }

    /// Load a bzImage, optional initramfs and command line into guest memory
    /// for the 64-bit boot protocol, and return the entry state for
    /// [`KvmVcpu::set_linux_entry`].
    ///
    /// # Errors
    ///
    /// [`KvmError::Boot`] for an unusable image or configuration that does
    /// not fit; [`KvmError::Unsupported`] unless guest memory is one region
    /// starting at 0 of at most [`boot::IDENTITY_MAP_BYTES`].
    pub fn load_linux(
        &self,
        kernel: &[u8],
        initrd: Option<&[u8]>,
        cmdline: &str,
        acpi_rsdp: Option<u64>,
    ) -> Result<LinuxEntry, KvmError> {
        let ram = self.map.total_bytes();
        if self.map.regions().len() != 1 || self.map.region_for(0, ram).is_none() {
            return Err(KvmError::Unsupported(
                "Linux boot needs one guest RAM region starting at 0",
            ));
        }
        if ram > boot::IDENTITY_MAP_BYTES {
            return Err(KvmError::Unsupported(
                "Linux boot supports at most 1 GiB of guest RAM",
            ));
        }
        if cmdline.bytes().any(|b| b == 0) {
            return Err(KvmError::Unsupported("NUL byte in the kernel command line"));
        }
        let bz = BzImage::parse(kernel)?;
        let len = |n: usize| {
            u64::try_from(n).map_err(|_| KvmError::Unsupported("input larger than 2^64 bytes"))
        };

        let initrd_at = match initrd {
            Some(data) => {
                let size = len(data.len())?;
                Some((bz.place_initrd(ram, size)?, size))
            }
            None => None,
        };
        let zero_page = bz.zero_page(ram, len(cmdline.len())?, initrd_at, acpi_rsdp)?;

        self.write_guest(boot::KERNEL_ADDR, bz.protected_mode_kernel())?;
        if let (Some(data), Some((addr, _))) = (initrd, initrd_at) {
            self.write_guest(addr, data)?;
        }
        let mut cmd = cmdline.as_bytes().to_vec();
        cmd.push(0);
        self.write_guest(boot::CMDLINE_ADDR, &cmd)?;
        self.write_guest(boot::ZERO_PAGE_ADDR, &zero_page)?;
        let [pml4, pdpt, pd] = boot::identity_page_tables();
        self.write_guest(boot::PML4_ADDR, &pml4)?;
        self.write_guest(boot::PDPT_ADDR, &pdpt)?;
        self.write_guest(boot::PD_ADDR, &pd)?;
        self.write_guest(boot::GDT_ADDR, &boot::gdt())?;
        Ok(LinuxEntry {
            rip: bz.entry_64(),
            initrd: initrd_at,
        })
    }

    /// Write ACPI tables built by [`acpi::build`] into guest memory and
    /// return the RSDP address, for [`KvmVm::load_linux`].
    ///
    /// # Errors
    ///
    /// [`KvmError::OutOfBounds`] if a table is outside guest memory;
    /// [`KvmError::Unsupported`] if there is no RSDP.
    pub fn load_acpi(&self, tables: &[acpi::PlacedTable]) -> Result<u64, KvmError> {
        for table in tables {
            self.write_guest(table.addr, &table.bytes)?;
        }
        tables
            .iter()
            .find(|t| &t.signature == b"RSDP")
            .map(|t| t.addr)
            .ok_or(KvmError::Unsupported("ACPI tables without an RSDP"))
    }

    /// An interrupt line driving I/O APIC input `gsi` through `KVM_IRQ_LINE`.
    /// Needs [`KvmVm::enable_legacy_interrupts`].
    #[must_use]
    pub fn irq_line(&self, gsi: u32) -> KvmIrqLine<'_> {
        KvmIrqLine {
            vm: &self.vm,
            gsi,
            failures: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Copy `bytes` into guest memory at `gpa` (host-side load, e.g. a guest
    /// image; guest permissions do not apply).
    ///
    /// # Errors
    ///
    /// [`KvmError::OutOfBounds`] unless the whole range lies in one region.
    pub fn write_guest(&self, gpa: u64, bytes: &[u8]) -> Result<(), KvmError> {
        let len =
            u64::try_from(bytes.len()).map_err(|_| KvmError::OutOfBounds { gpa, len: u64::MAX })?;
        if self.map.region_for(gpa, len).is_none() {
            return Err(KvmError::OutOfBounds { gpa, len });
        }
        self.memory
            .write_slice(bytes, GuestAddress(gpa))
            .map_err(|e| KvmError::Memory(e.to_string()))
    }

    /// Read guest memory at `gpa` into `buf`.
    ///
    /// # Errors
    ///
    /// [`KvmError::OutOfBounds`] unless the whole range lies in one region.
    pub fn read_guest(&self, gpa: u64, buf: &mut [u8]) -> Result<(), KvmError> {
        let len =
            u64::try_from(buf.len()).map_err(|_| KvmError::OutOfBounds { gpa, len: u64::MAX })?;
        if self.map.region_for(gpa, len).is_none() {
            return Err(KvmError::OutOfBounds { gpa, len });
        }
        self.memory
            .read_slice(buf, GuestAddress(gpa))
            .map_err(|e| KvmError::Memory(e.to_string()))
    }

    /// Create vCPU `id`. The vCPU borrows the VM and cannot outlive it.
    ///
    /// # Errors
    ///
    /// [`KvmError::Kvm`] if `KVM_CREATE_VCPU` fails.
    pub fn create_vcpu(&self, id: u64) -> Result<KvmVcpu<'_>, KvmError> {
        let fd = self
            .vm
            .create_vcpu(id)
            .map_err(kvm_err("KVM_CREATE_VCPU"))?;
        Ok(KvmVcpu {
            fd,
            id,
            last_unhandled: None,
            _vm: PhantomData,
        })
    }
}

/// One KVM vCPU of a [`KvmVm`].
pub struct KvmVcpu<'vm> {
    fd: VcpuFd,
    /// vCPU index, which is also its local APIC ID.
    id: u64,
    last_unhandled: Option<String>,
    _vm: PhantomData<&'vm KvmVm>,
}

impl fmt::Debug for KvmVcpu<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KvmVcpu")
            .field("id", &self.id)
            .field("last_unhandled", &self.last_unhandled)
            .finish_non_exhaustive()
    }
}

impl KvmVcpu<'_> {
    /// Point the vCPU at real-mode address `cs:ip` with interrupts disabled.
    /// Other segment registers keep their reset values (selector 0, base 0).
    ///
    /// # Errors
    ///
    /// [`KvmError::Kvm`] if reading or writing registers fails.
    pub fn set_real_mode_entry(&mut self, cs: u16, ip: u16) -> Result<(), KvmError> {
        let mut sregs = self.fd.get_sregs().map_err(kvm_err("KVM_GET_SREGS"))?;
        sregs.cs.selector = cs;
        // Real-mode segment base is selector * 16; at most 0xF_FFF0, no overflow.
        #[allow(clippy::arithmetic_side_effects)]
        let base = u64::from(cs) * 16;
        sregs.cs.base = base;
        self.fd
            .set_sregs(&sregs)
            .map_err(kvm_err("KVM_SET_SREGS"))?;
        let regs = kvm_regs {
            rip: u64::from(ip),
            rflags: RFLAGS_RESERVED,
            ..kvm_regs::default()
        };
        self.fd.set_regs(&regs).map_err(kvm_err("KVM_SET_REGS"))
    }

    /// Why the most recent [`VmExit::Unhandled`] happened, in words.
    #[must_use]
    pub fn last_unhandled_detail(&self) -> Option<&str> {
        self.last_unhandled.as_deref()
    }

    /// CPUID from `KVM_GET_SUPPORTED_CPUID`, with this vCPU's APIC ID patched
    /// in (see [`apply_apic_id`]); then LAPIC LINT0 as `ExtINT` and LINT1 as
    /// NMI.
    fn set_cpuid_and_lint(&mut self, vm: &KvmVm) -> Result<(), KvmError> {
        let mut cpuid = vm
            .kvm
            .get_supported_cpuid(KVM_MAX_CPUID_ENTRIES)
            .map_err(kvm_err("KVM_GET_SUPPORTED_CPUID"))?;
        let apic_id =
            u32::try_from(self.id).map_err(|_| KvmError::Unsupported("vCPU index above 2^32"))?;
        apply_apic_id(cpuid.as_mut_slice(), apic_id);
        self.fd
            .set_cpuid2(&cpuid)
            .map_err(kvm_err("KVM_SET_CPUID2"))?;
        let mut lapic = self.fd.get_lapic().map_err(kvm_err("KVM_GET_LAPIC"))?;
        set_lvt_mode(&mut lapic.regs, APIC_LVT_LINT0, APIC_MODE_EXTINT);
        set_lvt_mode(&mut lapic.regs, APIC_LVT_LINT1, APIC_MODE_NMI);
        self.fd.set_lapic(&lapic).map_err(kvm_err("KVM_SET_LAPIC"))
    }

    /// Prepare an application processor (any vCPU but 0) for an SMP Linux
    /// guest: CPUID with its APIC ID, and LAPIC LINT0/LINT1. With KVM's
    /// in-kernel LAPIC the vCPU then waits for the guest's INIT/SIPI, which
    /// KVM handles; no registers are set here.
    ///
    /// # Errors
    ///
    /// [`KvmError::Kvm`] if an ioctl fails (e.g. no in-kernel LAPIC).
    pub fn set_ap_state(&mut self, vm: &KvmVm) -> Result<(), KvmError> {
        self.set_cpuid_and_lint(vm)
    }

    /// The vCPU index (and local APIC ID).
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Prepare this vCPU to enter a kernel loaded by [`KvmVm::load_linux`]:
    /// - CPUID from `KVM_GET_SUPPORTED_CPUID`, with this vCPU's APIC ID;
    /// - LAPIC LINT0 as `ExtINT` and LINT1 as NMI, so in-kernel PIC interrupts
    ///   reach the CPU;
    /// - long mode with the early identity map and the boot-protocol GDT;
    /// - RIP at the 64-bit entry, RSI at the zero page, interrupts off.
    ///
    /// # Errors
    ///
    /// [`KvmError::Kvm`] if an ioctl fails, including when
    /// [`KvmVm::enable_legacy_interrupts`] was not called (no in-kernel LAPIC).
    pub fn set_linux_entry(&mut self, vm: &KvmVm, entry: &LinuxEntry) -> Result<(), KvmError> {
        self.set_cpuid_and_lint(vm)?;

        let mut sregs = self.fd.get_sregs().map_err(kvm_err("KVM_GET_SREGS"))?;
        sregs.cs = flat_segment(boot::CODE_SELECTOR, SegmentKind::Code64);
        let data = flat_segment(boot::DATA_SELECTOR, SegmentKind::Data);
        sregs.ds = data;
        sregs.es = data;
        sregs.fs = data;
        sregs.gs = data;
        sregs.ss = data;
        sregs.tr = flat_segment(boot::TSS_SELECTOR, SegmentKind::Tss);
        sregs.gdt.base = boot::GDT_ADDR;
        sregs.gdt.limit = u16::try_from(boot::GDT_SIZE.saturating_sub(1)).unwrap_or(u16::MAX);
        // No IDT: the kernel installs its own before enabling interrupts.
        sregs.idt.base = 0;
        sregs.idt.limit = 0;
        sregs.cr3 = boot::PML4_ADDR;
        sregs.cr4 |= CR4_PAE;
        sregs.cr0 |= CR0_PE | CR0_PG;
        sregs.efer |= EFER_LME | EFER_LMA;
        self.fd
            .set_sregs(&sregs)
            .map_err(kvm_err("KVM_SET_SREGS"))?;

        let regs = kvm_regs {
            rip: entry.rip,
            rsi: boot::ZERO_PAGE_ADDR,
            rsp: boot::BOOT_STACK,
            rbp: boot::BOOT_STACK,
            rflags: RFLAGS_RESERVED,
            ..kvm_regs::default()
        };
        self.fd.set_regs(&regs).map_err(kvm_err("KVM_SET_REGS"))
    }
}

/// Where and how to enter a kernel prepared by [`KvmVm::load_linux`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxEntry {
    /// 64-bit entry point.
    pub rip: u64,
    /// Initramfs guest address and length, if one was loaded.
    pub initrd: Option<(u64, u64)>,
}

#[derive(Clone, Copy)]
enum SegmentKind {
    Code64,
    Data,
    Tss,
}

/// A flat (base 0, 4 GiB limit, 4 KiB granularity) segment matching the
/// descriptor at `selector` in [`boot::gdt`].
fn flat_segment(selector: u16, kind: SegmentKind) -> kvm_segment {
    // (type, S, L, D/B) per descriptor; see boot::gdt.
    let (type_, s, l, db) = match kind {
        SegmentKind::Code64 => (0xB, 1, 1, 0),
        SegmentKind::Data => (0x3, 1, 0, 1),
        SegmentKind::Tss => (0xB, 0, 0, 0),
    };
    kvm_segment {
        base: 0,
        limit: 0xFFFF_FFFF,
        selector,
        type_,
        present: 1,
        dpl: 0,
        db,
        s,
        l,
        g: 1,
        ..kvm_segment::default()
    }
}

/// Set the delivery mode of a local APIC LVT register in a `KVM_GET_LAPIC` image.
fn set_lvt_mode(regs: &mut [std::ffi::c_char], range: std::ops::Range<usize>, mode: u32) {
    let Some(reg) = regs.get_mut(range) else {
        return;
    };
    let mut bytes = [0u8; 4];
    for (b, r) in bytes.iter_mut().zip(reg.iter()) {
        *b = r.cast_unsigned();
    }
    let value = (u32::from_le_bytes(bytes) & !APIC_DELIVERY_MODE_MASK) | mode;
    for (r, b) in reg.iter_mut().zip(value.to_le_bytes()) {
        *r = b.cast_signed();
    }
}

/// Make CPUID report `apic_id` as this CPU's local APIC ID, as KVM's
/// in-kernel LAPIC does (ID = vCPU index), instead of the host CPU's:
/// - leaf 1 EBX\[31:24\] (initial APIC ID);
/// - leaves 0xB and 0x1F EDX (x2APIC ID), on every subleaf.
fn apply_apic_id(entries: &mut [kvm_cpuid_entry2], apic_id: u32) {
    for e in entries {
        match e.function {
            0x1 => e.ebx = (e.ebx & 0x00FF_FFFF) | ((apic_id & 0xFF) << 24),
            0xB | 0x1F => e.edx = apic_id,
            _ => {}
        }
    }
}

/// Devices reach guest RAM (DMA) through the VM's bounds-checked accessors,
/// and also with the guest's own permissions from the memory map, unlike a
/// host-side load: a device may not write a region the guest cannot write
/// (such as a read-only ROM), nor read one the guest cannot read.
impl GuestMemoryAccess for KvmVm {
    fn read(&self, gpa: u64, buf: &mut [u8]) -> Result<(), GuestAccessError> {
        let len = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        let err = GuestAccessError { gpa, len };
        if !self
            .map
            .region_for(gpa, len)
            .is_some_and(|r| r.permissions.read)
        {
            return Err(err);
        }
        self.read_guest(gpa, buf).map_err(|_| err)
    }

    fn write(&self, gpa: u64, data: &[u8]) -> Result<(), GuestAccessError> {
        let len = u64::try_from(data.len()).unwrap_or(u64::MAX);
        let err = GuestAccessError { gpa, len };
        if !self
            .map
            .region_for(gpa, len)
            .is_some_and(|r| r.permissions.write)
        {
            return Err(err);
        }
        self.write_guest(gpa, data).map_err(|_| err)
    }
}

/// An I/O APIC input driven with `KVM_IRQ_LINE`, from [`KvmVm::irq_line`].
#[derive(Debug)]
pub struct KvmIrqLine<'vm> {
    vm: &'vm VmFd,
    gsi: u32,
    failures: std::sync::atomic::AtomicU64,
}

impl KvmIrqLine<'_> {
    /// How many `KVM_IRQ_LINE` calls failed (expected: 0).
    #[must_use]
    pub fn failures(&self) -> u64 {
        self.failures.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl InterruptLine for KvmIrqLine<'_> {
    fn set_level(&self, asserted: bool) {
        if self.vm.set_irq_line(self.gsi, asserted).is_err() {
            self.failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

fn port_size(data: &[u8]) -> Option<AccessSize> {
    match data.len() {
        1 => Some(AccessSize::Byte),
        2 => Some(AccessSize::Word),
        4 => Some(AccessSize::Dword),
        _ => None,
    }
}

fn mmio_size(data: &[u8]) -> Option<AccessSize> {
    match data.len() {
        8 => Some(AccessSize::Qword),
        _ => port_size(data),
    }
}

/// Little-endian value of up to 8 bytes.
fn le_value(data: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = data.len().min(8);
    buf[..n].copy_from_slice(&data[..n]);
    u64::from_le_bytes(buf)
}

impl Vcpu for KvmVcpu<'_> {
    type Error = KvmError;

    fn run(&mut self, io: &mut dyn GuestIo) -> Result<VmExit, KvmError> {
        let invalid = |_| KvmError::Unsupported("KVM reported an impossible access size");
        // An application processor waits inside KVM_RUN for INIT and SIPI,
        // and KVM_RUN returns EAGAIN after each one changes its state: run
        // again. Each EAGAIN follows a guest event, so this cannot spin, and
        // a pending kick signal makes KVM_RUN return EINTR instead.
        let exit = loop {
            match self.fd.run() {
                Err(e) if e.errno() == libc::EAGAIN => {}
                other => break other,
            }
        };
        // `Err(detail)` marks an exit Aegis cannot map; the raw reason is read
        // after the borrow of the run area ends.
        let mapped: Result<VmExit, String> = match exit.map_err(kvm_err("KVM_RUN"))? {
            VcpuExit::IoOut(port, data) => match port_size(data) {
                Some(size) => {
                    let value = u32::try_from(le_value(data)).unwrap_or(u32::MAX);
                    io.write(port, size, value);
                    Ok(VmExit::Io(
                        IoAccess::new(port, size, Some(value)).map_err(invalid)?,
                    ))
                }
                None => Err(format!(
                    "{}-byte port write to {port:#x} (string I/O is unsupported)",
                    data.len()
                )),
            },
            VcpuExit::IoIn(port, data) => match port_size(data) {
                Some(size) => {
                    let bytes = io.read(port, size).to_le_bytes();
                    let n = data.len();
                    data.copy_from_slice(&bytes[..n]);
                    Ok(VmExit::Io(
                        IoAccess::new(port, size, None).map_err(invalid)?,
                    ))
                }
                None => Err(format!(
                    "{}-byte port read from {port:#x} (string I/O is unsupported)",
                    data.len()
                )),
            },
            VcpuExit::MmioRead(gpa, data) => match mmio_size(data) {
                Some(size) => {
                    let access = MmioAccess::new(gpa, size, None).map_err(invalid)?;
                    if let Some(value) = io.mmio_read(gpa, size) {
                        let bytes = value.to_le_bytes();
                        let n = data.len();
                        data.copy_from_slice(&bytes[..n]);
                        Ok(VmExit::MmioDevice(access))
                    } else {
                        // No device: the run stops, but never hand back stale bytes.
                        data.fill(0xFF);
                        Ok(VmExit::Mmio(access))
                    }
                }
                None => Err(format!("{}-byte MMIO read at {gpa:#x}", data.len())),
            },
            VcpuExit::MmioWrite(gpa, data) => match mmio_size(data) {
                Some(size) => {
                    let value = le_value(data);
                    let access = MmioAccess::new(gpa, size, Some(value)).map_err(invalid)?;
                    if io.mmio_write(gpa, size, value) {
                        Ok(VmExit::MmioDevice(access))
                    } else {
                        Ok(VmExit::Mmio(access))
                    }
                }
                None => Err(format!("{}-byte MMIO write at {gpa:#x}", data.len())),
            },
            VcpuExit::Hlt => Ok(VmExit::Hlt),
            VcpuExit::Shutdown => Ok(VmExit::Shutdown),
            VcpuExit::FailEntry(reason, cpu) => Err(format!(
                "VM entry failed: hardware reason {reason:#x} on cpu {cpu}"
            )),
            other => Err(format!("{other:?}")),
        };

        match mapped {
            Ok(exit) => {
                self.last_unhandled = None;
                Ok(exit)
            }
            Err(detail) => {
                let raw_reason = u64::from(self.fd.get_kvm_run().exit_reason);
                self.last_unhandled = Some(detail);
                Ok(VmExit::Unhandled { raw_reason })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_sizes_from_kvm_data_lengths() {
        for (len, want) in [
            (1, Some(AccessSize::Byte)),
            (2, Some(AccessSize::Word)),
            (4, Some(AccessSize::Dword)),
        ] {
            assert_eq!(port_size(&vec![0; len]), want);
            assert_eq!(mmio_size(&vec![0; len]), want);
        }
        assert_eq!(port_size(&[0; 8]), None, "no 8-byte port I/O");
        assert_eq!(mmio_size(&[0; 8]), Some(AccessSize::Qword));
        for len in [0, 3, 5, 6, 7, 16] {
            assert_eq!(port_size(&vec![0; len]), None, "{len}");
            assert_eq!(mmio_size(&vec![0; len]), None, "{len}");
        }
    }

    #[test]
    fn apic_id_is_patched_into_cpuid() {
        let entry = |function, index, ebx, edx| kvm_cpuid_entry2 {
            function,
            index,
            ebx,
            edx,
            ..kvm_cpuid_entry2::default()
        };
        let mut entries = [
            entry(0x0, 0, 0x756e_6547, 0),
            entry(0x1, 0, 0x1320_0800, 0), // host APIC ID 0x13 in EBX[31:24]
            entry(0xB, 0, 1, 0x13),
            entry(0xB, 1, 2, 0x13),
            entry(0x1F, 0, 1, 0x13),
        ];
        apply_apic_id(&mut entries, 3);
        assert_eq!(entries[0].ebx, 0x756e_6547, "other leaves untouched");
        assert_eq!(entries[1].ebx, 0x0320_0800, "only EBX[31:24] replaced");
        assert!(
            entries[2..].iter().all(|e| e.edx == 3),
            "x2APIC ID on every subleaf"
        );
        assert_eq!(entries[3].ebx, 2);
    }

    #[test]
    fn little_endian_values() {
        assert_eq!(le_value(&[0x41]), 0x41);
        assert_eq!(le_value(&[0xEF, 0xBE]), 0xBEEF);
        assert_eq!(le_value(&[1, 2, 3, 4, 5, 6, 7, 8]), 0x0807_0605_0403_0201);
    }
}
