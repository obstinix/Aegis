//! Linux host sources: `/proc/cpuinfo`, the kernel release, KVM module
//! parameters, and CPU vulnerability status.
//!
//! Everything is read relative to a root directory, `/` on a live system or
//! a captured fixture tree in tests. Missing or unreadable files are recorded
//! as absent, never treated as errors: a probe reports what it can see.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// `kvm_intel` parameters Aegis reports.
pub const KVM_INTEL_PARAMS: &[&str] = &["nested", "ept", "unrestricted_guest", "enable_apicv"];
/// `kvm_amd` parameters Aegis reports.
pub const KVM_AMD_PARAMS: &[&str] = &["nested", "npt", "avic"];

/// The fields Aegis uses from the first processor block of `/proc/cpuinfo`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CpuInfo {
    /// `vendor_id`, e.g. `GenuineIntel`.
    pub vendor_id: Option<String>,
    /// `model name`.
    pub model_name: Option<String>,
    /// `flags`.
    pub flags: BTreeSet<String>,
    /// `vmx flags`: VMX capabilities as the kernel read them from the VMX
    /// capability MSRs. `None` if the line is absent (non-Intel, or VMX not exposed).
    pub vmx_flags: Option<BTreeSet<String>>,
}

/// Parse the first processor block of `/proc/cpuinfo` text.
#[must_use]
pub fn parse_cpuinfo(text: &str) -> CpuInfo {
    let mut info = CpuInfo::default();
    for line in text.lines().take_while(|l| !l.trim().is_empty()) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let words = || {
            value
                .split_whitespace()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
        };
        match key.trim() {
            "vendor_id" => info.vendor_id = Some(value.to_owned()),
            "model name" => info.model_name = Some(value.to_owned()),
            "flags" => info.flags = words(),
            "vmx flags" => info.vmx_flags = Some(words()),
            _ => {}
        }
    }
    info
}

/// Everything collected from Linux sources.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LinuxObservations {
    /// `/proc/sys/kernel/osrelease`.
    pub kernel_release: Option<String>,
    /// First processor block of `/proc/cpuinfo`.
    pub cpuinfo: Option<CpuInfo>,
    /// Present `kvm_intel` parameters from [`KVM_INTEL_PARAMS`].
    pub kvm_intel: BTreeMap<String, String>,
    /// Present `kvm_amd` parameters from [`KVM_AMD_PARAMS`].
    pub kvm_amd: BTreeMap<String, String>,
    /// `/sys/devices/system/cpu/vulnerabilities/*`: name to status line.
    pub vulnerabilities: BTreeMap<String, String>,
}

impl LinuxObservations {
    /// Read all sources below `root` (`/` for the live system).
    #[must_use]
    pub fn collect(root: &Path) -> Self {
        let read = |rel: &str| {
            fs::read_to_string(root.join(rel))
                .ok()
                .map(|s| s.trim().to_owned())
        };
        let params = |module: &str, names: &[&str]| -> BTreeMap<String, String> {
            names
                .iter()
                .filter_map(|name| {
                    read(&format!("sys/module/{module}/parameters/{name}"))
                        .map(|v| ((*name).to_owned(), v))
                })
                .collect()
        };

        Self {
            kernel_release: read("proc/sys/kernel/osrelease"),
            cpuinfo: fs::read_to_string(root.join("proc/cpuinfo"))
                .ok()
                .map(|t| parse_cpuinfo(&t)),
            kvm_intel: params("kvm_intel", KVM_INTEL_PARAMS),
            kvm_amd: params("kvm_amd", KVM_AMD_PARAMS),
            vulnerabilities: read_dir_values(&root.join("sys/devices/system/cpu/vulnerabilities")),
        }
    }

    /// Whether the kernel release identifies Windows Subsystem for Linux.
    #[must_use]
    pub fn is_wsl(&self) -> bool {
        self.kernel_release
            .as_deref()
            .is_some_and(|r| r.to_ascii_lowercase().contains("microsoft"))
    }
}

/// Every regular file in `dir` as name to trimmed contents. Unreadable entries are skipped.
fn read_dir_values(dir: &Path) -> BTreeMap<String, String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let value = fs::read_to_string(e.path()).ok()?;
            Some((name, value.trim().to_owned()))
        })
        .collect()
}

/// Interpret a kernel boolean module parameter (`Y`/`N`, `1`/`0`).
#[must_use]
pub fn param_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "Y" | "y" | "1" => Some(true),
        "N" | "n" | "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: Test CPU\n\
        flags\t\t: fpu vmx hypervisor\nvmx flags\t: ept vpid\n\nprocessor\t: 1\nvendor_id\t: Other\n";

    #[test]
    fn parses_only_the_first_processor_block() {
        let info = parse_cpuinfo(SAMPLE);
        assert_eq!(info.vendor_id.as_deref(), Some("GenuineIntel"));
        assert_eq!(info.model_name.as_deref(), Some("Test CPU"));
        assert!(info.flags.contains("vmx") && info.flags.contains("hypervisor"));
        let vmx = info.vmx_flags.unwrap();
        assert!(vmx.contains("ept") && vmx.contains("vpid"));
    }

    #[test]
    fn missing_vmx_flags_line_is_none_not_empty() {
        let info = parse_cpuinfo("vendor_id\t: AuthenticAMD\nflags\t: svm\n");
        assert_eq!(info.vmx_flags, None);
        assert!(info.flags.contains("svm"));
    }

    #[test]
    fn handles_crlf_and_garbage_lines() {
        let info = parse_cpuinfo("vendor_id\t: GenuineIntel\r\nno colon here\r\nflags : a b\r\n");
        assert_eq!(info.vendor_id.as_deref(), Some("GenuineIntel"));
        assert_eq!(info.flags.len(), 2);
    }

    #[test]
    fn module_param_booleans() {
        assert_eq!(param_bool("Y\n"), Some(true));
        assert_eq!(param_bool("1"), Some(true));
        assert_eq!(param_bool("N"), Some(false));
        assert_eq!(param_bool("0"), Some(false));
        assert_eq!(param_bool("maybe"), None);
    }

    #[test]
    fn collect_on_missing_root_is_empty_not_an_error() {
        let obs = LinuxObservations::collect(Path::new("this/path/does/not/exist"));
        assert_eq!(obs, LinuxObservations::default());
        assert!(!obs.is_wsl());
    }
}
