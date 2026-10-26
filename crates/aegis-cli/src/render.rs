//! Human-readable rendering of the `aegis doctor` report.

use std::fmt::Write as _;

use aegis_probe::kvm::DevKvm;
use aegis_probe::report::{HostOs, Report, Tri, Verdict};

fn tri(t: Tri) -> &'static str {
    match t {
        Tri::Yes => "yes",
        Tri::No => "no",
        Tri::Unknown => "unknown",
    }
}

fn opt(s: Option<&str>) -> &str {
    s.unwrap_or("unknown")
}

fn verdict(out: &mut String, name: &str, v: &Verdict) {
    // Writing to a String cannot fail.
    let _ = match v {
        Verdict::Ready { detail } => writeln!(out, "  {name:<20} READY        {detail}"),
        Verdict::Blocked { reason, fix } => {
            let _ = writeln!(out, "  {name:<20} BLOCKED      {reason}");
            match fix {
                Some(fix) => writeln!(out, "  {:<20}   fix:       {fix}", ""),
                None => Ok(()),
            }
        }
        Verdict::Unsupported { reason } => writeln!(out, "  {name:<20} UNSUPPORTED  {reason}"),
        Verdict::Unknown { reason } => writeln!(out, "  {name:<20} UNKNOWN      {reason}"),
    };
}

/// Render the report as plain text.
pub(crate) fn human(r: &Report) -> String {
    let mut out = String::new();
    let os = match r.host.os {
        HostOs::Linux if r.host.wsl => "Linux (WSL2)",
        HostOs::Linux => "Linux",
        HostOs::Windows => "Windows",
        HostOs::Other => "other",
    };
    let hv = match (
        r.host.hypervisor_present,
        r.host.hypervisor_vendor.as_deref(),
    ) {
        (Tri::Yes, Some(vendor)) => format!("yes ({vendor})"),
        (t, _) => tri(t).to_owned(),
    };
    let dev_kvm = match &r.kvm.dev_kvm {
        None => "not probed".to_owned(),
        Some(DevKvm::Absent) => "absent".to_owned(),
        Some(DevKvm::PermissionDenied) => "present, permission denied".to_owned(),
        Some(DevKvm::OpenFailed { error }) => format!("open failed: {error}"),
        Some(DevKvm::Opened { api_version, .. }) => format!("opened, API version {api_version}"),
    };

    let _ = writeln!(out, "aegis doctor ({})\n", r.schema);
    let _ = writeln!(out, "host");
    let _ = writeln!(out, "  os                   {os}");
    if let Some(k) = &r.host.kernel_release {
        let _ = writeln!(out, "  kernel               {k}");
    }
    let _ = writeln!(out, "  under a hypervisor   {hv}");
    let _ = writeln!(out, "\ncpu");
    let _ = writeln!(
        out,
        "  vendor               {}",
        opt(r.cpu.vendor.as_deref())
    );
    let _ = writeln!(
        out,
        "  brand                {}",
        opt(r.cpu.brand.as_deref())
    );
    let _ = writeln!(out, "  VMX (cpuid)          {}", tri(r.cpu.vmx));
    let _ = writeln!(out, "  SVM (cpuid)          {}", tri(r.cpu.svm));
    let _ = writeln!(
        out,
        "  EPT/NPT              {}",
        tri(r.virtualization.second_level_translation)
    );
    let _ = writeln!(
        out,
        "  unrestricted guest   {}",
        tri(r.virtualization.unrestricted_guest)
    );
    if let Some(f) = &r.virtualization.vmx_features {
        let _ = writeln!(out, "  vmx features         {}", f.join(" "));
    }
    let _ = writeln!(out, "\nkvm");
    let _ = writeln!(out, "  /dev/kvm             {dev_kvm}");
    if let Some(DevKvm::Opened { extensions, .. }) = &r.kvm.dev_kvm {
        for (name, present) in extensions {
            let _ = writeln!(
                out,
                "  ext {name:<16} {}",
                if *present { "yes" } else { "no" }
            );
        }
    }
    for (k, v) in &r.kvm.module_params {
        let _ = writeln!(out, "  {k:<30} {v}");
    }
    if !r.vulnerabilities.is_empty() {
        let _ = writeln!(out, "\ncpu vulnerabilities (host kernel)");
        for (k, v) in &r.vulnerabilities {
            let _ = writeln!(out, "  {k:<26} {v}");
        }
    }
    let _ = writeln!(out, "\nverdicts");
    verdict(&mut out, "hosted KVM", &r.verdicts.hosted_kvm);
    verdict(
        &mut out,
        "nested VMX testing",
        &r.verdicts.nested_vmx_testing,
    );
    if !r.notes.is_empty() {
        let _ = writeln!(out, "\nnotes");
        for n in &r.notes {
            let _ = writeln!(out, "  - {n}");
        }
    }
    out.truncate(out.trim_end().len());
    out
}
