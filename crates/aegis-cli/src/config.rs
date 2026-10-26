//! `vm.toml`: the configuration for `aegis run` (ADR-0009).
//!
//! ```toml
//! memory_mib = 256                      # 16..=1024
//! kernel = "bzImage"                    # relative to this file's directory
//! initrd = "initramfs.cpio"             # optional
//! cmdline = "console=ttyS0 reboot=t"    # optional; default below
//! vcpus = 2                             # optional; 1..=8, default 1
//! virtio_console = true                 # optional; default false
//! virtio_console_stdin = true           # optional; default false; needs virtio_console
//! ```
//!
//! With `virtio_console_stdin`, bytes read from `aegis run`'s stdin are
//! offered to the guest on the virtio-console (`/dev/hvc0`). Piping a file
//! or a command into `aegis run` gives a guest deterministic input.
//!
//! Parsing is strict: unknown keys are rejected, the file is size-capped, and
//! values are range-checked before anything touches KVM.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Largest accepted config file.
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
/// Accepted guest memory range.
pub(crate) const MEMORY_MIB: std::ops::RangeInclusive<u64> = 16..=1024;
/// Accepted vCPU counts (ACPI MADT limit in `aegis-kvm`).
pub(crate) const VCPUS: std::ops::RangeInclusive<u8> = 1..=8;

/// Default kernel command line:
/// - console on the 16550 at 0x3F8;
/// - restart by triple fault, so the VM stops with a KVM shutdown exit;
/// - reboot immediately on panic;
/// - no PCI, modules or i8042 probing, since none of those devices exist.
pub(crate) const DEFAULT_CMDLINE: &str = "console=ttyS0 earlyprintk=serial,ttyS0,115200 reboot=t panic=-1 \
     pci=off nomodules io_delay=none i8042.noaux i8042.nomux i8042.nopnp i8042.dumbkbd";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    memory_mib: u64,
    kernel: PathBuf,
    initrd: Option<PathBuf>,
    cmdline: Option<String>,
    vcpus: Option<u8>,
    virtio_console: Option<bool>,
    virtio_console_stdin: Option<bool>,
}

/// A validated `vm.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VmConfig {
    pub(crate) memory_mib: u64,
    pub(crate) kernel: PathBuf,
    pub(crate) initrd: Option<PathBuf>,
    pub(crate) cmdline: String,
    pub(crate) vcpus: u8,
    pub(crate) virtio_console: bool,
    /// Forward stdin to the virtio-console.
    pub(crate) virtio_console_stdin: bool,
}

/// Read and validate `path`.
pub(crate) fn load(path: &Path) -> Result<VmConfig, String> {
    let shown = path.display();
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot read {shown}: {e}"))?;
    if meta.len() > MAX_CONFIG_BYTES {
        return Err(format!("{shown} is larger than {MAX_CONFIG_BYTES} bytes"));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {shown}: {e}"))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    parse(&text, base).map_err(|e| format!("{shown}: {e}"))
}

/// Validate config `text`; relative paths are resolved against `base`.
pub(crate) fn parse(text: &str, base: &Path) -> Result<VmConfig, String> {
    let raw: RawConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    if !MEMORY_MIB.contains(&raw.memory_mib) {
        return Err(format!(
            "memory_mib = {} is outside {}..={}",
            raw.memory_mib,
            MEMORY_MIB.start(),
            MEMORY_MIB.end()
        ));
    }
    let vcpus = raw.vcpus.unwrap_or(1);
    if !VCPUS.contains(&vcpus) {
        return Err(format!(
            "vcpus = {vcpus} is outside {}..={}",
            VCPUS.start(),
            VCPUS.end()
        ));
    }
    let cmdline = raw.cmdline.unwrap_or_else(|| DEFAULT_CMDLINE.to_owned());
    if cmdline.contains('\0') {
        return Err("cmdline contains a NUL byte".to_owned());
    }
    let virtio_console = raw.virtio_console.unwrap_or(false);
    let virtio_console_stdin = raw.virtio_console_stdin.unwrap_or(false);
    if virtio_console_stdin && !virtio_console {
        return Err("virtio_console_stdin = true needs virtio_console = true".to_owned());
    }
    Ok(VmConfig {
        memory_mib: raw.memory_mib,
        kernel: base.join(raw.kernel),
        initrd: raw.initrd.map(|p| base.join(p)),
        cmdline,
        vcpus,
        virtio_console,
        virtio_console_stdin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_config_and_resolves_paths() {
        let cfg = parse(
            "memory_mib = 256\nkernel = \"bzImage\"\ninitrd = \"/abs/initrd\"\ncmdline = \"console=ttyS0\"\n",
            Path::new("/vms/a"),
        )
        .unwrap();
        assert_eq!(cfg.memory_mib, 256);
        assert_eq!(cfg.kernel, Path::new("/vms/a").join("bzImage"));
        assert_eq!(cfg.initrd.as_deref(), Some(Path::new("/abs/initrd")));
        assert_eq!(cfg.cmdline, "console=ttyS0");
    }

    #[test]
    fn defaults_the_command_line() {
        let cfg = parse("memory_mib = 64\nkernel = \"k\"\n", Path::new(".")).unwrap();
        assert_eq!(cfg.cmdline, DEFAULT_CMDLINE);
        assert_eq!((cfg.vcpus, cfg.virtio_console), (1, false), "M3 defaults");
        assert!(cfg.cmdline.contains("console=ttyS0") && cfg.cmdline.contains("reboot=t"));
        assert_eq!(cfg.initrd, None);
    }

    #[test]
    fn parses_vcpus_and_virtio_console() {
        let base = Path::new(".");
        let with =
            |extra: &str| parse(&format!("memory_mib = 64\nkernel = \"k\"\n{extra}\n"), base);
        let cfg = with("vcpus = 4\nvirtio_console = true").unwrap();
        assert_eq!((cfg.vcpus, cfg.virtio_console), (4, true));
        for bad in ["0", "9", "255"] {
            let err = with(&format!("vcpus = {bad}")).unwrap_err();
            assert!(err.contains("vcpus"), "{err}");
        }
        assert!(with("vcpus = 300").is_err(), "not a u8");
        assert!(with("vcpus = -1").is_err());
        assert!(with("virtio_console = \"yes\"").is_err(), "not a bool");
        assert!(
            !cfg.virtio_console_stdin,
            "stdin is not forwarded by default"
        );
    }

    #[test]
    fn stdin_forwarding_needs_the_virtio_console() {
        let base = Path::new(".");
        let with =
            |extra: &str| parse(&format!("memory_mib = 64\nkernel = \"k\"\n{extra}\n"), base);
        let cfg = with("virtio_console = true\nvirtio_console_stdin = true").unwrap();
        assert!(cfg.virtio_console && cfg.virtio_console_stdin);
        let err = with("virtio_console_stdin = true").unwrap_err();
        assert!(err.contains("needs virtio_console = true"), "{err}");
        assert!(with("virtio_console = false\nvirtio_console_stdin = true").is_err());
        assert!(
            with("virtio_console = true\nvirtio_console_stdin = 1").is_err(),
            "not a bool"
        );
    }

    #[test]
    fn rejects_unknown_keys_missing_keys_and_bad_values() {
        let base = Path::new(".");
        assert!(
            parse("memory_mib = 64\nkernel = \"k\"\nballoon = true\n", base)
                .unwrap_err()
                .contains("balloon")
        );
        assert!(
            parse("kernel = \"k\"\n", base).is_err(),
            "memory_mib is required"
        );
        assert!(
            parse("memory_mib = 64\n", base).is_err(),
            "kernel is required"
        );
        assert!(
            parse("memory_mib = 8\nkernel = \"k\"\n", base)
                .unwrap_err()
                .contains("outside")
        );
        assert!(
            parse("memory_mib = 2048\nkernel = \"k\"\n", base)
                .unwrap_err()
                .contains("outside")
        );
        assert!(parse("memory_mib = -1\nkernel = \"k\"\n", base).is_err());
        assert!(parse("memory_mib = \"64\"\nkernel = \"k\"\n", base).is_err());
        assert!(
            parse(
                "memory_mib = 64\nkernel = \"k\"\ncmdline = \"a\\u0000b\"\n",
                base
            )
            .unwrap_err()
            .contains("NUL")
        );
        assert!(parse("not toml at all [", base).is_err());
    }

    #[test]
    fn load_rejects_missing_and_oversized_files() {
        assert!(
            load(Path::new("definitely/missing/vm.toml"))
                .unwrap_err()
                .contains("cannot read")
        );
        let big = std::env::temp_dir().join(format!("aegis-big-{}.toml", std::process::id()));
        std::fs::write(&big, vec![b'#'; 70 * 1024]).unwrap();
        let err = load(&big).unwrap_err();
        let _ = std::fs::remove_file(&big);
        assert!(err.contains("larger than"), "{err}");
    }
}
