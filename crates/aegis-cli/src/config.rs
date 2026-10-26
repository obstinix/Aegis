//! `vm.toml`: the configuration for `aegis run` (ADR-0009).
//!
//! ```toml
//! memory_mib = 256                      # 16..=1024
//! kernel = "bzImage"                    # relative to this file's directory
//! initrd = "initramfs.cpio"             # optional
//! cmdline = "console=ttyS0 reboot=t"    # optional; default below
//! ```
//!
//! Parsing is strict: unknown keys are rejected, the file is size-capped, and
//! values are range-checked before anything touches KVM.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Largest accepted config file.
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
/// Accepted guest memory range.
pub(crate) const MEMORY_MIB: std::ops::RangeInclusive<u64> = 16..=1024;

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
}

/// A validated `vm.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VmConfig {
    pub(crate) memory_mib: u64,
    pub(crate) kernel: PathBuf,
    pub(crate) initrd: Option<PathBuf>,
    pub(crate) cmdline: String,
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
    let cmdline = raw.cmdline.unwrap_or_else(|| DEFAULT_CMDLINE.to_owned());
    if cmdline.contains('\0') {
        return Err("cmdline contains a NUL byte".to_owned());
    }
    Ok(VmConfig {
        memory_mib: raw.memory_mib,
        kernel: base.join(raw.kernel),
        initrd: raw.initrd.map(|p| base.join(p)),
        cmdline,
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
        assert!(cfg.cmdline.contains("console=ttyS0") && cfg.cmdline.contains("reboot=t"));
        assert_eq!(cfg.initrd, None);
    }

    #[test]
    fn rejects_unknown_keys_missing_keys_and_bad_values() {
        let base = Path::new(".");
        assert!(
            parse("memory_mib = 64\nkernel = \"k\"\nvcpus = 2\n", base)
                .unwrap_err()
                .contains("vcpus")
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
