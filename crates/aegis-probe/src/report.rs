//! The `aegis doctor` report.
//!
//! [`Observations`] is everything collected from the host. [`Report::evaluate`]
//! is a pure function from observations to the report, including verdicts on
//! whether each Aegis backend can run here. Fixture tests drive the exact
//! code path the live command uses.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cpuid::{CpuVendor, CpuidSnapshot};
use crate::kvm::{DevKvm, EXPECTED_API_VERSION, REQUIRED_EXTENSIONS};
use crate::linux::{LinuxObservations, param_bool};

/// Identifier and version of the JSON report schema. Bump the number on any
/// incompatible change.
pub const SCHEMA: &str = "aegis.doctor/1";

/// The host operating system family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostOs {
    /// Linux, including WSL2.
    Linux,
    /// Windows.
    Windows,
    /// Anything else.
    Other,
}

impl HostOs {
    /// The OS this binary was built for.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// Raw facts collected from the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observations {
    /// OS family.
    pub host_os: HostOs,
    /// CPUID values, if this is an x86-64 build.
    pub cpuid: Option<CpuidSnapshot>,
    /// Linux sources, on Linux hosts.
    pub linux: Option<LinuxObservations>,
    /// `/dev/kvm` state, on Linux hosts.
    pub dev_kvm: Option<DevKvm>,
}

impl Observations {
    /// Collect from the live host.
    #[must_use]
    pub fn collect() -> Self {
        let host_os = HostOs::current();
        #[cfg(target_arch = "x86_64")]
        let cpuid = Some(CpuidSnapshot::read());
        #[cfg(not(target_arch = "x86_64"))]
        let cpuid = None;
        #[cfg(target_os = "linux")]
        let (linux, dev_kvm) = (
            Some(LinuxObservations::collect(std::path::Path::new("/"))),
            Some(DevKvm::probe()),
        );
        #[cfg(not(target_os = "linux"))]
        let (linux, dev_kvm) = (None, None);
        Self {
            host_os,
            cpuid,
            linux,
            dev_kvm,
        }
    }
}

/// Yes, no, or cannot tell from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tri {
    /// Supported.
    Yes,
    /// Not supported.
    No,
    /// Not determinable from this vantage point.
    Unknown,
}

impl From<Option<bool>> for Tri {
    fn from(v: Option<bool>) -> Self {
        match v {
            Some(true) => Self::Yes,
            Some(false) => Self::No,
            None => Self::Unknown,
        }
    }
}

/// Whether an Aegis capability can be used on this host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Verdict {
    /// Usable now.
    Ready {
        /// What was verified.
        detail: String,
    },
    /// Possible on this host, but something must change first.
    Blocked {
        /// What stops it.
        reason: String,
        /// What the owner can do about it, if known.
        fix: Option<String>,
    },
    /// Not possible on this host.
    Unsupported {
        /// Why.
        reason: String,
    },
    /// Not enough information.
    Unknown {
        /// What is missing.
        reason: String,
    },
}

/// Host facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostReport {
    /// OS family.
    pub os: HostOs,
    /// Linux kernel release, if Linux.
    pub kernel_release: Option<String>,
    /// Running under WSL2.
    pub wsl: bool,
    /// CPUID reports a hypervisor below this OS.
    pub hypervisor_present: Tri,
    /// That hypervisor's CPUID vendor string, e.g. `Microsoft Hv`.
    pub hypervisor_vendor: Option<String>,
}

/// CPU facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CpuReport {
    /// Vendor string, e.g. `GenuineIntel`.
    pub vendor: Option<String>,
    /// Brand string.
    pub brand: Option<String>,
    /// VMX visible to this OS through CPUID.
    pub vmx: Tri,
    /// SVM visible to this OS through CPUID.
    pub svm: Tri,
}

/// Virtualization feature facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VirtualizationReport {
    /// VMX capabilities from the kernel (`vmx flags` in `/proc/cpuinfo`), sorted.
    pub vmx_features: Option<Vec<String>>,
    /// Second-level address translation: EPT (Intel) or NPT (AMD).
    pub second_level_translation: Tri,
    /// Unrestricted guest (Intel), from `vmx flags`.
    pub unrestricted_guest: Tri,
}

/// KVM facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KvmReport {
    /// `kvm_intel.*` / `kvm_amd.*` module parameters.
    pub module_params: BTreeMap<String, String>,
    /// `/dev/kvm` state, on Linux.
    pub dev_kvm: Option<DevKvm>,
}

/// Verdicts for each Aegis execution path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdicts {
    /// Hosted KVM backend (roadmap track H).
    pub hosted_kvm: Verdict,
    /// Running the bare-metal VMX core as a nested guest under QEMU/KVM (track B tests).
    pub nested_vmx_testing: Verdict,
}

/// The full `aegis doctor` report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Always [`SCHEMA`].
    pub schema: &'static str,
    /// Host facts.
    pub host: HostReport,
    /// CPU facts.
    pub cpu: CpuReport,
    /// Virtualization feature facts.
    pub virtualization: VirtualizationReport,
    /// KVM facts.
    pub kvm: KvmReport,
    /// CPU vulnerability status as reported by the host kernel. Aegis adds
    /// no mitigations of its own.
    pub vulnerabilities: BTreeMap<String, String>,
    /// What can run here.
    pub verdicts: Verdicts,
    /// Explanations for anything surprising or not checked.
    pub notes: Vec<String>,
}

fn ascii_field(bytes: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(bytes)
        .ok()?
        .trim_matches(char::from(0))
        .trim();
    (!s.is_empty()).then(|| s.to_owned())
}

const KVM_GROUP_FIX: &str = "add this user to the group that owns /dev/kvm (usually `kvm`): \
    `sudo usermod -aG kvm $USER`, then start a new login session (on WSL2: `wsl --shutdown`)";

impl Report {
    /// Build the report from observations. Pure: no I/O.
    #[must_use]
    pub fn evaluate(obs: &Observations) -> Self {
        let mut notes = Vec::new();
        let linux = obs.linux.as_ref();
        let cpuinfo = linux.and_then(|l| l.cpuinfo.as_ref());
        let cpuid = obs.cpuid.as_ref();

        let host = HostReport {
            os: obs.host_os,
            kernel_release: linux.and_then(|l| l.kernel_release.clone()),
            wsl: linux.is_some_and(LinuxObservations::is_wsl),
            hypervisor_present: cpuid.map(CpuidSnapshot::hypervisor_present).into(),
            hypervisor_vendor: cpuid
                .and_then(CpuidSnapshot::hypervisor_vendor_bytes)
                .and_then(|b| ascii_field(&b)),
        };

        let cpu = CpuReport {
            vendor: cpuid
                .and_then(|c| ascii_field(&c.vendor_bytes()))
                .or_else(|| cpuinfo.and_then(|i| i.vendor_id.clone())),
            brand: cpuid
                .and_then(CpuidSnapshot::brand_bytes)
                .and_then(|b| ascii_field(&b))
                .or_else(|| cpuinfo.and_then(|i| i.model_name.clone())),
            vmx: cpuid.map(CpuidSnapshot::vmx).into(),
            svm: cpuid.map(CpuidSnapshot::svm).into(),
        };

        let vmx_flags = cpuinfo.and_then(|i| i.vmx_flags.as_ref());
        let is_amd = cpuid.is_some_and(|c| c.vendor() == CpuVendor::Amd);
        let second_level_translation = if let Some(flags) = vmx_flags {
            Tri::from(Some(flags.contains("ept")))
        } else if is_amd {
            cpuid.and_then(CpuidSnapshot::npt).into()
        } else {
            Tri::Unknown
        };
        if second_level_translation == Tri::Unknown && cpu.vmx != Tri::No {
            notes.push(
                "EPT support is reported in the IA32_VMX_PROCBASED_CTLS2 MSR, which only ring-0 \
                 code can read; this host exposes no kernel-provided copy (Linux `vmx flags`)."
                    .to_owned(),
            );
        }
        let virtualization = VirtualizationReport {
            vmx_features: vmx_flags.map(|f| f.iter().cloned().collect()),
            second_level_translation,
            unrestricted_guest: vmx_flags.map(|f| f.contains("unrestricted_guest")).into(),
        };

        let mut module_params = BTreeMap::new();
        if let Some(l) = linux {
            for (prefix, params) in [("kvm_intel", &l.kvm_intel), ("kvm_amd", &l.kvm_amd)] {
                for (k, v) in params {
                    module_params.insert(format!("{prefix}.{k}"), v.clone());
                }
            }
        }

        let hosted_kvm = hosted_kvm_verdict(obs.host_os, obs.dev_kvm.as_ref());
        let nested_vmx_testing = nested_verdict(obs, &hosted_kvm);

        if obs.host_os == HostOs::Windows {
            if host.hypervisor_present == Tri::Yes && cpu.vmx == Tri::No {
                notes.push(format!(
                    "VMX is not visible to this OS: it runs under a hypervisor ({}) that does not \
                     expose VMX to it. Bare-metal Aegis cannot run here directly; use the WSL2 \
                     nested path.",
                    host.hypervisor_vendor
                        .as_deref()
                        .unwrap_or("unknown vendor")
                ));
            }
            notes.push(
                "Not checked: Windows Hypervisor Platform availability (roadmap W1).".to_owned(),
            );
            notes.push("CPU vulnerability status is not collected on Windows.".to_owned());
        }
        if linux
            .and_then(|l| l.kvm_intel.get("enable_apicv"))
            .and_then(|v| param_bool(v))
            == Some(false)
        {
            notes.push(
                "APICv is disabled for KVM here (kvm_intel.enable_apicv=N). Aegis does not depend \
                 on it (ADR-0005)."
                    .to_owned(),
            );
        }

        Self {
            schema: SCHEMA,
            host,
            cpu,
            virtualization,
            kvm: KvmReport {
                module_params,
                dev_kvm: obs.dev_kvm.clone(),
            },
            vulnerabilities: linux.map(|l| l.vulnerabilities.clone()).unwrap_or_default(),
            verdicts: Verdicts {
                hosted_kvm,
                nested_vmx_testing,
            },
            notes,
        }
    }
}

fn hosted_kvm_verdict(os: HostOs, dev_kvm: Option<&DevKvm>) -> Verdict {
    if os != HostOs::Linux {
        return Verdict::Unsupported {
            reason: "the hosted KVM backend requires a Linux host (on this workstation: WSL2)"
                .to_owned(),
        };
    }
    match dev_kvm {
        None => Verdict::Unknown {
            reason: "/dev/kvm was not probed".to_owned(),
        },
        Some(DevKvm::Absent) => Verdict::Unsupported {
            reason:
                "/dev/kvm does not exist (KVM module not loaded, or no hardware virtualization)"
                    .to_owned(),
        },
        Some(DevKvm::PermissionDenied) => Verdict::Blocked {
            reason: "/dev/kvm exists but this user cannot open it: permission denied".to_owned(),
            fix: Some(KVM_GROUP_FIX.to_owned()),
        },
        Some(DevKvm::OpenFailed { error }) => Verdict::Blocked {
            reason: format!("opening /dev/kvm failed: {error}"),
            fix: None,
        },
        Some(DevKvm::Opened {
            api_version,
            extensions,
        }) => {
            let missing: Vec<&str> = REQUIRED_EXTENSIONS
                .iter()
                .copied()
                .filter(|name| extensions.get(*name) != Some(&true))
                .collect();
            if *api_version != EXPECTED_API_VERSION {
                Verdict::Blocked {
                    reason: format!(
                        "KVM API version {api_version}, expected {EXPECTED_API_VERSION}"
                    ),
                    fix: None,
                }
            } else if !missing.is_empty() {
                Verdict::Blocked {
                    reason: format!("KVM lacks required extensions: {}", missing.join(", ")),
                    fix: None,
                }
            } else {
                Verdict::Ready {
                    detail: format!(
                        "/dev/kvm opened; API version {api_version}; required extensions present"
                    ),
                }
            }
        }
    }
}

fn nested_verdict(obs: &Observations, hosted: &Verdict) -> Verdict {
    if obs.host_os != HostOs::Linux {
        return Verdict::Unsupported {
            reason: "nested-VMX tests run under QEMU/KVM on Linux (on this workstation: WSL2)"
                .to_owned(),
        };
    }
    let nested = obs
        .linux
        .as_ref()
        .and_then(|l| l.kvm_intel.get("nested"))
        .and_then(|v| param_bool(v));
    match (nested, hosted) {
        (None, _) => Verdict::Unsupported {
            reason:
                "kvm_intel is not loaded or does not report `nested` (Intel VMX first, ADR-0005)"
                    .to_owned(),
        },
        (Some(false), _) => Verdict::Unsupported {
            reason: "kvm_intel.nested=N".to_owned(),
        },
        (Some(true), Verdict::Ready { .. }) => Verdict::Ready {
            detail: "kvm_intel.nested=Y and /dev/kvm usable (not yet exercised with a guest)"
                .to_owned(),
        },
        (Some(true), Verdict::Blocked { reason, fix }) => Verdict::Blocked {
            reason: format!("kvm_intel.nested=Y; blocked by hosted KVM: {reason}"),
            fix: fix.clone(),
        },
        (Some(true), Verdict::Unsupported { reason } | Verdict::Unknown { reason }) => {
            Verdict::Unknown {
                reason: format!("kvm_intel.nested=Y; hosted KVM unavailable: {reason}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux_obs(dev_kvm: DevKvm, nested: Option<&str>) -> Observations {
        let mut linux = LinuxObservations::default();
        if let Some(n) = nested {
            linux.kvm_intel.insert("nested".to_owned(), n.to_owned());
        }
        Observations {
            host_os: HostOs::Linux,
            cpuid: None,
            linux: Some(linux),
            dev_kvm: Some(dev_kvm),
        }
    }

    fn opened(api_version: i32, irqchip: bool) -> DevKvm {
        DevKvm::Opened {
            api_version,
            extensions: [
                ("irqchip".to_owned(), irqchip),
                ("user_memory".to_owned(), true),
            ]
            .into(),
        }
    }

    #[test]
    fn hosted_kvm_verdict_for_each_dev_kvm_state() {
        let v = |d| {
            Report::evaluate(&linux_obs(d, Some("Y")))
                .verdicts
                .hosted_kvm
        };
        assert!(matches!(v(DevKvm::Absent), Verdict::Unsupported { .. }));
        assert!(matches!(
            v(DevKvm::PermissionDenied),
            Verdict::Blocked { fix: Some(_), .. }
        ));
        assert!(matches!(
            v(DevKvm::OpenFailed {
                error: "EIO".into()
            }),
            Verdict::Blocked { fix: None, .. }
        ));
        assert!(matches!(v(opened(12, true)), Verdict::Ready { .. }));
        assert!(matches!(v(opened(11, true)), Verdict::Blocked { .. }));
        let Verdict::Blocked { reason, .. } = v(opened(12, false)) else {
            panic!("expected Blocked")
        };
        assert!(reason.contains("irqchip"));
    }

    #[test]
    fn nested_verdict_follows_param_and_hosted_state() {
        let v = |d, n| {
            Report::evaluate(&linux_obs(d, n))
                .verdicts
                .nested_vmx_testing
        };
        assert!(matches!(
            v(opened(12, true), Some("Y")),
            Verdict::Ready { .. }
        ));
        assert!(matches!(
            v(opened(12, true), Some("N")),
            Verdict::Unsupported { .. }
        ));
        assert!(matches!(
            v(opened(12, true), None),
            Verdict::Unsupported { .. }
        ));
        assert!(matches!(
            v(DevKvm::PermissionDenied, Some("Y")),
            Verdict::Blocked { fix: Some(_), .. }
        ));
        assert!(matches!(
            v(DevKvm::Absent, Some("1")),
            Verdict::Unknown { .. }
        ));
    }

    #[test]
    fn non_linux_hosts_are_unsupported_for_kvm_paths() {
        let obs = Observations {
            host_os: HostOs::Windows,
            cpuid: None,
            linux: None,
            dev_kvm: None,
        };
        let r = Report::evaluate(&obs);
        assert!(matches!(r.verdicts.hosted_kvm, Verdict::Unsupported { .. }));
        assert!(matches!(
            r.verdicts.nested_vmx_testing,
            Verdict::Unsupported { .. }
        ));
        assert_eq!(r.schema, SCHEMA);
    }

    #[test]
    fn ascii_field_trims_nuls_and_spaces() {
        assert_eq!(
            ascii_field(b"  Intel CPU\0\0"),
            Some("Intel CPU".to_owned())
        );
        assert_eq!(ascii_field(b"\0\0\0"), None);
        assert_eq!(ascii_field(&[0xFF, 0xFE]), None);
    }
}
