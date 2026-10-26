//! Aegis hosted backend: Linux KVM via rust-vmm (ADR-0004, roadmap M2).
//!
//! `KvmVm` owns a KVM VM and its guest memory, built from a validated
//! [`aegis_core::GuestMemoryMap`]. `KvmVcpu` implements [`aegis_core::Vcpu`],
//! so the backend-neutral [`aegis_core::run_vcpu`] drives it. (This crate's
//! own types are not linked here because they exist only on Linux, while
//! this crate-level text is rendered on every OS.)
//!
//! Memory safety of the one `unsafe` operation (registering guest memory with
//! KVM) rests on two type-level guarantees:
//! - `KvmVm` drops its `VmFd` before the guest memory mapping.
//! - `KvmVcpu<'vm>` borrows its `KvmVm`, so no vCPU outlives the memory.
//!
//! Limits in M2: real-mode entry only; string (`rep ins/outs`) port I/O is
//! reported as an unhandled exit; MMIO is reported but no device serves it;
//! memory can be read-only but not non-executable (KVM memory slots cannot
//! deny execution).
//!
//! On non-Linux targets this crate is empty.

#![cfg(target_os = "linux")]
#![deny(unsafe_code)]
#![warn(clippy::undocumented_unsafe_blocks)]

use std::fmt;
use std::marker::PhantomData;

use aegis_core::{AccessSize, GuestMemoryMap, IoAccess, MmioAccess, PortIo, Vcpu, VmExit};
use kvm_bindings::{KVM_MEM_READONLY, kvm_regs, kvm_userspace_memory_region};
use kvm_ioctls::{Cap, Kvm, VcpuExit, VcpuFd, VmFd};
use vm_memory::{Bytes, GuestAddress, GuestMemoryBackend, GuestMemoryMmap, GuestMemoryRegion};

/// RFLAGS bit 1 is reserved and must be set.
const RFLAGS_RESERVED: u64 = 0x2;

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
    _kvm: Kvm,
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
            _kvm: kvm,
        })
    }

    /// The guest memory map this VM was built from.
    #[must_use]
    pub fn memory_map(&self) -> &GuestMemoryMap {
        &self.map
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
            last_unhandled: None,
            _vm: PhantomData,
        })
    }
}

/// One KVM vCPU of a [`KvmVm`].
pub struct KvmVcpu<'vm> {
    fd: VcpuFd,
    last_unhandled: Option<String>,
    _vm: PhantomData<&'vm KvmVm>,
}

impl fmt::Debug for KvmVcpu<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KvmVcpu")
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

    fn run(&mut self, io: &mut dyn PortIo) -> Result<VmExit, KvmError> {
        let invalid = |_| KvmError::Unsupported("KVM reported an impossible access size");
        // `Err(detail)` marks an exit Aegis cannot map; the raw reason is read
        // after the borrow of the run area ends.
        let mapped: Result<VmExit, String> = match self.fd.run().map_err(kvm_err("KVM_RUN"))? {
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
                    // No MMIO devices: the run stops, but never hand back stale bytes.
                    data.fill(0xFF);
                    Ok(VmExit::Mmio(
                        MmioAccess::new(gpa, size, None).map_err(invalid)?,
                    ))
                }
                None => Err(format!("{}-byte MMIO read at {gpa:#x}", data.len())),
            },
            VcpuExit::MmioWrite(gpa, data) => match mmio_size(data) {
                Some(size) => Ok(VmExit::Mmio(
                    MmioAccess::new(gpa, size, Some(le_value(data))).map_err(invalid)?,
                )),
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
    fn little_endian_values() {
        assert_eq!(le_value(&[0x41]), 0x41);
        assert_eq!(le_value(&[0xEF, 0xBE]), 0xBEEF);
        assert_eq!(le_value(&[1, 2, 3, 4, 5, 6, 7, 8]), 0x0807_0605_0403_0201);
    }
}
