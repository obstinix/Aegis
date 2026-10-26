//! CPUID decoding.
//!
//! [`CpuidSnapshot`] holds the raw register values of the leaves Aegis
//! inspects. Decoding works on stored values, so captured snapshots from
//! other machines decode exactly like a live read.
//!
//! References: Intel SDM Vol. 2A, CPUID instruction (leaf 01h ECX bit 5 = VMX,
//! bit 31 = hypervisor present); AMD APM Vol. 3, CPUID `Fn8000_0001` ECX bit 2 =
//! SVM, `Fn8000_000A` EDX bit 0 = nested paging.
//!
//! CPUID cannot report Intel EPT support: that lives in the
//! `IA32_VMX_PROCBASED_CTLS2` MSR, which only ring-0 code can read.

/// Leaf 01h ECX bit 5: VMX.
const LEAF1_ECX_VMX: u32 = 0x0000_0020;
/// Leaf 01h ECX bit 31: running under a hypervisor.
const LEAF1_ECX_HYPERVISOR: u32 = 0x8000_0000;
/// Leaf `8000_0001h` ECX bit 2: SVM.
const EXT1_ECX_SVM: u32 = 0x0000_0004;
/// Leaf `8000_000Ah` EDX bit 0: nested paging (NPT).
const SVM_EDX_NPT: u32 = 0x0000_0001;

const EXT_BASE: u32 = 0x8000_0000;
const EXT_FEATURES: u32 = 0x8000_0001;
const EXT_BRAND_LAST: u32 = 0x8000_0004;
const EXT_SVM: u32 = 0x8000_000A;
const HYPERVISOR_BASE: u32 = 0x4000_0000;

/// The four registers returned by one CPUID leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct CpuidRegs {
    /// EAX
    pub eax: u32,
    /// EBX
    pub ebx: u32,
    /// ECX
    pub ecx: u32,
    /// EDX
    pub edx: u32,
}

impl CpuidRegs {
    /// EBX, EDX, ECX as 12 little-endian bytes: the vendor-string layout of leaf 0.
    fn ebx_edx_ecx_bytes(self) -> [u8; 12] {
        let mut out = [0u8; 12];
        fill_le(&mut out, &[self.ebx, self.edx, self.ecx]);
        out
    }

    /// EBX, ECX, EDX as 12 little-endian bytes: the vendor-string layout of leaf `4000_0000h`.
    fn ebx_ecx_edx_bytes(self) -> [u8; 12] {
        let mut out = [0u8; 12];
        fill_le(&mut out, &[self.ebx, self.ecx, self.edx]);
        out
    }
}

/// Write `regs` into `dst` as consecutive little-endian `u32`s.
fn fill_le(dst: &mut [u8], regs: &[u32]) {
    for (chunk, reg) in dst.chunks_exact_mut(4).zip(regs) {
        chunk.copy_from_slice(&reg.to_le_bytes());
    }
}

/// CPU vendor, from the leaf-0 vendor string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuVendor {
    /// `GenuineIntel`
    Intel,
    /// `AuthenticAMD`
    Amd,
    /// Anything else (raw vendor string kept).
    Other([u8; 12]),
}

/// Raw values of the CPUID leaves Aegis inspects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct CpuidSnapshot {
    /// Leaf 0: max basic leaf (EAX) and vendor string.
    pub leaf_0: CpuidRegs,
    /// Leaf 1: feature flags. Zeroed if the max basic leaf is 0.
    pub leaf_1: CpuidRegs,
    /// Leaf `8000_0000h`: max extended leaf.
    pub ext_0: CpuidRegs,
    /// Leaf `8000_0001h`, if supported.
    pub ext_1: Option<CpuidRegs>,
    /// Leaves `8000_0002h`..=`8000_0004h` (brand string), if supported.
    pub brand: Option<[CpuidRegs; 3]>,
    /// Leaf `8000_000Ah` (SVM features), read only when SVM is reported.
    pub svm_features: Option<CpuidRegs>,
    /// Leaf `4000_0000h` (hypervisor vendor), read only when the hypervisor bit is set.
    pub hypervisor: Option<CpuidRegs>,
}

impl CpuidSnapshot {
    /// Read the current CPU's CPUID values.
    ///
    /// Inside a VM these are the values the hypervisor chooses to expose,
    /// which is exactly what Aegis needs to know.
    #[cfg(target_arch = "x86_64")]
    #[must_use]
    pub fn read() -> Self {
        let read = |leaf: u32| {
            // `__cpuid` is a safe function on the pinned toolchain (verified to
            // compile under forbid(unsafe_code) on 1.97.0); CPUID always exists on x86-64.
            let r = core::arch::x86_64::__cpuid(leaf);
            CpuidRegs {
                eax: r.eax,
                ebx: r.ebx,
                ecx: r.ecx,
                edx: r.edx,
            }
        };

        let leaf_0 = read(0);
        let leaf_1 = if leaf_0.eax >= 1 {
            read(1)
        } else {
            CpuidRegs::default()
        };
        let ext_0 = read(EXT_BASE);
        let max_ext = ext_0.eax;
        // A valid max-extended-leaf value is itself in the 8000_xxxxh range.
        let has_ext = |leaf: u32| (max_ext & 0xFFFF_0000) == EXT_BASE && max_ext >= leaf;

        let ext_1 = has_ext(EXT_FEATURES).then(|| read(EXT_FEATURES));
        let brand = has_ext(EXT_BRAND_LAST)
            .then(|| [read(0x8000_0002), read(0x8000_0003), read(EXT_BRAND_LAST)]);
        let svm_reported = ext_1.is_some_and(|r| r.ecx & EXT1_ECX_SVM != 0);
        let svm_features = (svm_reported && has_ext(EXT_SVM)).then(|| read(EXT_SVM));
        let hypervisor = (leaf_1.ecx & LEAF1_ECX_HYPERVISOR != 0).then(|| read(HYPERVISOR_BASE));

        Self {
            leaf_0,
            leaf_1,
            ext_0,
            ext_1,
            brand,
            svm_features,
            hypervisor,
        }
    }

    /// The 12-byte vendor string, e.g. `GenuineIntel`.
    #[must_use]
    pub fn vendor_bytes(&self) -> [u8; 12] {
        self.leaf_0.ebx_edx_ecx_bytes()
    }

    /// The CPU vendor.
    #[must_use]
    pub fn vendor(&self) -> CpuVendor {
        match &self.vendor_bytes() {
            b"GenuineIntel" => CpuVendor::Intel,
            b"AuthenticAMD" => CpuVendor::Amd,
            other => CpuVendor::Other(*other),
        }
    }

    /// The 48-byte processor brand string, if the CPU reports one.
    /// May contain leading spaces and trailing NULs.
    #[must_use]
    pub fn brand_bytes(&self) -> Option<[u8; 48]> {
        let leaves = self.brand?;
        let mut out = [0u8; 48];
        for (dst, leaf) in out.chunks_exact_mut(16).zip(leaves) {
            fill_le(dst, &[leaf.eax, leaf.ebx, leaf.ecx, leaf.edx]);
        }
        Some(out)
    }

    /// Whether VMX (Intel VT-x) is visible through CPUID.
    #[must_use]
    pub fn vmx(&self) -> bool {
        self.leaf_1.ecx & LEAF1_ECX_VMX != 0
    }

    /// Whether SVM (AMD-V) is visible through CPUID.
    #[must_use]
    pub fn svm(&self) -> bool {
        self.ext_1.is_some_and(|r| r.ecx & EXT1_ECX_SVM != 0)
    }

    /// Nested paging (AMD NPT) support, if SVM is reported and leaf `8000_000Ah` was read.
    #[must_use]
    pub fn npt(&self) -> Option<bool> {
        self.svm_features.map(|r| r.edx & SVM_EDX_NPT != 0)
    }

    /// Whether the CPU reports running under a hypervisor.
    #[must_use]
    pub fn hypervisor_present(&self) -> bool {
        self.leaf_1.ecx & LEAF1_ECX_HYPERVISOR != 0
    }

    /// The hypervisor's 12-byte vendor string, e.g. `Microsoft Hv`, if present.
    #[must_use]
    pub fn hypervisor_vendor_bytes(&self) -> Option<[u8; 12]> {
        self.hypervisor.map(CpuidRegs::ebx_ecx_edx_bytes)
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)] // fixed small offsets into 16-byte test buffers
mod tests {
    use super::*;

    /// Leaf 0 as read on the development workstation (Intel Core Ultra 9 275HX):
    /// max leaf 0x23, "Genu" "ineI" "ntel".
    const INTEL_LEAF_0: CpuidRegs = CpuidRegs {
        eax: 0x23,
        ebx: 0x756e_6547,
        ecx: 0x6c65_746e,
        edx: 0x4965_6e69,
    };

    fn regs_from(bytes: &[u8; 16]) -> CpuidRegs {
        let word =
            |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        CpuidRegs {
            eax: word(0),
            ebx: word(4),
            ecx: word(8),
            edx: word(12),
        }
    }

    #[test]
    fn decodes_intel_vendor() {
        let s = CpuidSnapshot {
            leaf_0: INTEL_LEAF_0,
            ..CpuidSnapshot::default()
        };
        assert_eq!(&s.vendor_bytes(), b"GenuineIntel");
        assert_eq!(s.vendor(), CpuVendor::Intel);
    }

    #[test]
    fn decodes_amd_and_unknown_vendor() {
        // "Auth" "enti" "cAMD" in EBX, EDX, ECX.
        let amd = CpuidRegs {
            eax: 0x10,
            ebx: 0x6874_7541,
            ecx: 0x444d_4163,
            edx: 0x6974_6e65,
        };
        let s = CpuidSnapshot {
            leaf_0: amd,
            ..CpuidSnapshot::default()
        };
        assert_eq!(s.vendor(), CpuVendor::Amd);
        let zero = CpuidSnapshot::default();
        assert_eq!(zero.vendor(), CpuVendor::Other([0; 12]));
    }

    #[test]
    fn decodes_feature_bits() {
        let mut s = CpuidSnapshot::default();
        assert!(!s.vmx() && !s.svm() && !s.hypervisor_present());
        assert_eq!(s.npt(), None);
        s.leaf_1.ecx = LEAF1_ECX_VMX | LEAF1_ECX_HYPERVISOR;
        assert!(s.vmx() && s.hypervisor_present());
        s.ext_1 = Some(CpuidRegs {
            ecx: EXT1_ECX_SVM,
            ..CpuidRegs::default()
        });
        assert!(s.svm());
        s.svm_features = Some(CpuidRegs {
            edx: SVM_EDX_NPT,
            ..CpuidRegs::default()
        });
        assert_eq!(s.npt(), Some(true));
        s.svm_features = Some(CpuidRegs::default());
        assert_eq!(s.npt(), Some(false));
    }

    #[test]
    fn decodes_brand_string_in_register_order() {
        let text = *b"Intel(R) Core(TM) Ultra 9 275HX\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
        let mut chunks = [[0u8; 16]; 3];
        for (dst, src) in chunks.iter_mut().zip(text.chunks_exact(16)) {
            dst.copy_from_slice(src);
        }
        let s = CpuidSnapshot {
            brand: Some(chunks.map(|c| regs_from(&c))),
            ..CpuidSnapshot::default()
        };
        assert_eq!(s.brand_bytes(), Some(text));
        assert_eq!(CpuidSnapshot::default().brand_bytes(), None);
    }

    #[test]
    fn decodes_hypervisor_vendor_in_ebx_ecx_edx_order() {
        // "Microsoft Hv": EBX "Micr", ECX "osof", EDX "t Hv".
        let hv = regs_from(b"\0\0\0\0Microsoft Hv");
        assert_eq!(hv.ebx_ecx_edx_bytes(), *b"Microsoft Hv");
        let s = CpuidSnapshot {
            hypervisor: Some(hv),
            ..CpuidSnapshot::default()
        };
        assert_eq!(s.hypervisor_vendor_bytes(), Some(*b"Microsoft Hv"));
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn live_read_is_self_consistent() {
        let s = CpuidSnapshot::read();
        assert!(s.leaf_0.eax >= 1, "every x86-64 CPU supports leaf 1");
        assert_eq!(s.hypervisor.is_some(), s.hypervisor_present());
        if s.svm_features.is_some() {
            assert!(s.svm());
        }
    }
}
