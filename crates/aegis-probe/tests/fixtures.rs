//! `aegis doctor` evaluation against observations captured on the development
//! workstation (see `tests/fixtures/PROVENANCE.md`).
//!
//! Each assertion compares the report with a fact observed independently of
//! the code under test: raw `/proc` and `/sys` text, or WMI values recorded
//! before any Aegis code existed.

use std::path::{Path, PathBuf};

use aegis_probe::kvm::DevKvm;
use aegis_probe::linux::LinuxObservations;
use aegis_probe::report::{HostOs, Observations, Report, Tri, Verdict};

const BRAND: &str = "Intel(R) Core(TM) Ultra 9 275HX";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn load(name: &str) -> Observations {
    let text = std::fs::read_to_string(fixture(name).join("observations.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// WSL2 observations: CPUID and `/dev/kvm` from the capture, Linux sources
/// parsed from the copied file tree.
fn wsl2() -> (Observations, LinuxObservations) {
    let captured = load("wsl2-ultra9-275hx");
    let parsed = LinuxObservations::collect(&fixture("wsl2-ultra9-275hx"));
    (
        Observations {
            linux: Some(parsed.clone()),
            ..captured
        },
        parsed,
    )
}

#[test]
fn wsl2_file_tree_parses_to_what_the_live_probe_saw() {
    let captured = load("wsl2-ultra9-275hx");
    let (_, parsed) = wsl2();
    assert_eq!(captured.linux.as_ref(), Some(&parsed));
}

#[test]
fn wsl2_cpu_and_host_match_proc_cpuinfo() {
    let (obs, linux) = wsl2();
    let cpuinfo = linux.cpuinfo.as_ref().unwrap();
    let r = Report::evaluate(&obs);

    assert_eq!(r.host.os, HostOs::Linux);
    assert!(r.host.wsl);
    assert_eq!(
        r.host.kernel_release.as_deref(),
        Some("6.18.33.2-microsoft-standard-WSL2")
    );

    // CPUID-derived fields agree with the kernel's own view in /proc/cpuinfo.
    assert_eq!(r.cpu.vendor.as_deref(), cpuinfo.vendor_id.as_deref());
    assert_eq!(r.cpu.vendor.as_deref(), Some("GenuineIntel"));
    assert_eq!(r.cpu.brand.as_deref(), cpuinfo.model_name.as_deref());
    assert_eq!(r.cpu.brand.as_deref(), Some(BRAND));
    assert!(cpuinfo.flags.contains("vmx"));
    assert_eq!(r.cpu.vmx, Tri::Yes);
    assert!(!cpuinfo.flags.contains("svm"));
    assert_eq!(r.cpu.svm, Tri::No);
    assert!(cpuinfo.flags.contains("hypervisor"));
    assert_eq!(r.host.hypervisor_present, Tri::Yes);
    // WSL2 is a Hyper-V guest.
    assert_eq!(r.host.hypervisor_vendor.as_deref(), Some("Microsoft Hv"));
}

#[test]
fn wsl2_virtualization_features_match_vmx_flags_and_module_params() {
    let (obs, linux) = wsl2();
    let r = Report::evaluate(&obs);
    let vmx_flags = linux.cpuinfo.unwrap().vmx_flags.unwrap();

    assert!(vmx_flags.contains("ept") && vmx_flags.contains("unrestricted_guest"));
    assert_eq!(r.virtualization.second_level_translation, Tri::Yes);
    assert_eq!(r.virtualization.unrestricted_guest, Tri::Yes);
    // Hyper-V does not expose APICv or posted interrupts to the WSL2 VM.
    assert!(
        !vmx_flags
            .iter()
            .any(|f| f.contains("apicv") || f.contains("posted"))
    );

    let p = |k: &str| r.kvm.module_params.get(k).map(String::as_str);
    assert_eq!(p("kvm_intel.nested"), Some("Y"));
    assert_eq!(p("kvm_intel.ept"), Some("Y"));
    assert_eq!(p("kvm_intel.unrestricted_guest"), Some("Y"));
    assert_eq!(p("kvm_intel.enable_apicv"), Some("N"));
    assert!(r.notes.iter().any(|n| n.contains("enable_apicv=N")));
}

#[test]
fn wsl2_dev_kvm_permission_denied_blocks_both_kvm_paths_with_a_fix() {
    let (obs, _) = wsl2();
    assert_eq!(obs.dev_kvm, Some(DevKvm::PermissionDenied));
    let r = Report::evaluate(&obs);

    for verdict in [&r.verdicts.hosted_kvm, &r.verdicts.nested_vmx_testing] {
        let Verdict::Blocked { reason, fix } = verdict else {
            panic!("expected Blocked, got {verdict:?}");
        };
        assert!(reason.contains("permission denied"), "{reason}");
        assert!(
            fix.as_deref()
                .is_some_and(|f| f.contains("usermod -aG kvm"))
        );
    }
}

#[test]
fn wsl2_vulnerabilities_are_passed_through_verbatim() {
    let (obs, _) = wsl2();
    let r = Report::evaluate(&obs);
    assert_eq!(
        r.vulnerabilities.get("retbleed").map(String::as_str),
        Some("Mitigation: Enhanced IBRS")
    );
    assert_eq!(
        r.vulnerabilities.get("meltdown").map(String::as_str),
        Some("Not affected")
    );
    assert!(r.vulnerabilities.len() >= 19);
}

#[test]
fn windows_matches_wmi_facts() {
    let obs = load("windows-ultra9-275hx");
    assert_eq!(obs.host_os, HostOs::Windows);
    let r = Report::evaluate(&obs);

    // WMI HypervisorPresent = True.
    assert_eq!(r.host.hypervisor_present, Tri::Yes);
    assert_eq!(r.host.hypervisor_vendor.as_deref(), Some("Microsoft Hv"));
    // WMI VMMonitorModeExtensions = False: Hyper-V hides VMX from the root partition.
    assert_eq!(r.cpu.vmx, Tri::No);
    // WMI Win32_Processor.Name.
    assert_eq!(r.cpu.brand.as_deref(), Some(BRAND));
    assert_eq!(r.cpu.vendor.as_deref(), Some("GenuineIntel"));

    // EPT cannot be read from user mode on Windows; the report must not guess.
    assert_eq!(r.virtualization.second_level_translation, Tri::Unknown);
    assert!(r.virtualization.vmx_features.is_none());

    assert!(matches!(r.verdicts.hosted_kvm, Verdict::Unsupported { .. }));
    assert!(matches!(
        r.verdicts.nested_vmx_testing,
        Verdict::Unsupported { .. }
    ));
    assert!(
        r.notes
            .iter()
            .any(|n| n.contains("VMX is not visible to this OS"))
    );
    assert!(r.vulnerabilities.is_empty());
}

#[test]
fn both_fixtures_come_from_the_same_physical_cpu() {
    let (wsl, _) = wsl2();
    let win = load("windows-ultra9-275hx");
    let (wsl_cpuid, win_cpuid) = (wsl.cpuid.unwrap(), win.cpuid.unwrap());
    assert_eq!(wsl_cpuid.vendor_bytes(), win_cpuid.vendor_bytes());
    assert_eq!(wsl_cpuid.brand_bytes(), win_cpuid.brand_bytes());
    // Same family/model/stepping signature.
    assert_eq!(wsl_cpuid.leaf_1.eax, win_cpuid.leaf_1.eax);
}

#[test]
fn json_report_keeps_the_documented_shape() {
    let (obs, _) = wsl2();
    let json = serde_json::to_value(Report::evaluate(&obs)).unwrap();
    assert_eq!(json["schema"], "aegis.doctor/1");
    assert_eq!(json["host"]["os"], "linux");
    assert_eq!(json["cpu"]["vmx"], "yes");
    assert_eq!(json["kvm"]["dev_kvm"]["state"], "permission_denied");
    assert_eq!(json["verdicts"]["hosted_kvm"]["status"], "blocked");
    assert!(json["verdicts"]["hosted_kvm"]["fix"].is_string());
    assert_eq!(json["virtualization"]["second_level_translation"], "yes");
}

/// Values from raw `ioctl(2)` calls made with Python `fcntl` on the same host,
/// independently of `kvm-ioctls` and Aegis (see PROVENANCE.md).
const RAW_IOCTL_EXTENSIONS: [(&str, bool); 5] = [
    ("ext_cpuid", true),
    ("immediate_exit", true),
    ("irqchip", true),
    ("pit2", true),
    ("user_memory", true),
];

#[test]
fn wsl2_with_kvm_access_matches_raw_ioctls() {
    let obs = load("wsl2-ultra9-275hx-kvm");
    let Some(DevKvm::Opened {
        api_version,
        extensions,
    }) = &obs.dev_kvm
    else {
        panic!("expected an opened /dev/kvm, got {:?}", obs.dev_kvm);
    };
    // KVM_GET_API_VERSION via raw ioctl returned 12.
    assert_eq!(*api_version, 12);
    let expected: std::collections::BTreeMap<String, bool> = RAW_IOCTL_EXTENSIONS
        .iter()
        .map(|(k, v)| ((*k).to_owned(), *v))
        .collect();
    assert_eq!(extensions, &expected);
}

#[test]
fn wsl2_with_kvm_access_is_ready_for_both_kvm_paths() {
    let obs = load("wsl2-ultra9-275hx-kvm");
    let r = Report::evaluate(&obs);
    assert!(
        matches!(r.verdicts.hosted_kvm, Verdict::Ready { .. }),
        "{:?}",
        r.verdicts.hosted_kvm
    );
    let Verdict::Ready { detail } = &r.verdicts.nested_vmx_testing else {
        panic!("expected Ready, got {:?}", r.verdicts.nested_vmx_testing);
    };
    // Readiness is a capability claim only; no nested guest has run yet.
    assert!(detail.contains("not yet exercised"), "{detail}");
    // Same machine and session as the permission-denied capture, except for the group change.
    let (before, _) = wsl2();
    let (now, then) = (obs.cpuid.unwrap(), before.cpuid.unwrap());
    // Not the whole snapshot: leaf 1 EBX[31:24] is the initial APIC ID of the
    // logical CPU that ran CPUID, and the two captures ran on different cores
    // (0x13 and 0x0f). Compare the fields that identify the CPU and its features.
    assert_eq!(now.vendor_bytes(), then.vendor_bytes());
    assert_eq!(now.brand_bytes(), then.brand_bytes());
    assert_eq!(now.leaf_1.eax, then.leaf_1.eax);
    assert_eq!(
        (now.leaf_1.ecx, now.leaf_1.edx),
        (then.leaf_1.ecx, then.leaf_1.edx)
    );
    assert_eq!(now.hypervisor, then.hypervisor);
    assert_eq!(
        obs.linux.as_ref().map(|l| &l.kvm_intel),
        before.linux.as_ref().map(|l| &l.kvm_intel)
    );
    assert_eq!(before.dev_kvm, Some(DevKvm::PermissionDenied));
}

#[test]
fn json_report_for_opened_kvm_keeps_the_documented_shape() {
    let json = serde_json::to_value(Report::evaluate(&load("wsl2-ultra9-275hx-kvm"))).unwrap();
    assert_eq!(json["kvm"]["dev_kvm"]["state"], "opened");
    assert_eq!(json["kvm"]["dev_kvm"]["api_version"], 12);
    assert_eq!(json["kvm"]["dev_kvm"]["extensions"]["irqchip"], true);
    assert_eq!(json["verdicts"]["hosted_kvm"]["status"], "ready");
    assert_eq!(json["verdicts"]["nested_vmx_testing"]["status"], "ready");
}
