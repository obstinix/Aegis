//! State of `/dev/kvm`.
//!
//! The live probe opens `/dev/kvm` read-write, exactly as a VMM would, so
//! "permission denied" is detected for real rather than inferred from file
//! modes. Only when that succeeds does it issue `KVM_GET_API_VERSION` and
//! `KVM_CHECK_EXTENSION` through `kvm-ioctls`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The KVM API version every supported kernel reports (`KVM_API_VERSION`).
pub const EXPECTED_API_VERSION: i32 = 12;

/// Extensions the hosted backend needs for milestones M2/M3 (ADR-0008).
pub const REQUIRED_EXTENSIONS: &[&str] = &["irqchip", "user_memory"];

/// What happened when Aegis tried to use `/dev/kvm`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DevKvm {
    /// `/dev/kvm` does not exist.
    Absent,
    /// `/dev/kvm` exists but this user may not open it.
    PermissionDenied,
    /// Opening or querying `/dev/kvm` failed for another reason.
    OpenFailed {
        /// The OS error.
        error: String,
    },
    /// Opened and queried successfully.
    Opened {
        /// Result of `KVM_GET_API_VERSION`.
        api_version: i32,
        /// Result of `KVM_CHECK_EXTENSION` for each queried capability.
        extensions: BTreeMap<String, bool>,
    },
}

impl DevKvm {
    /// Probe the live `/dev/kvm`.
    ///
    /// Status: only the `PermissionDenied` path has run live (WSL2 on the
    /// development workstation, whose user is not in the `kvm` group). The
    /// `Opened` path, including the `kvm-ioctls` calls, is implemented but
    /// has not run yet. How each state is *reported* is covered by tests in
    /// [`crate::report`].
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn probe() -> Self {
        use std::fs::OpenOptions;
        use std::io::ErrorKind;

        use kvm_ioctls::{Cap, Kvm};

        match OpenOptions::new().read(true).write(true).open("/dev/kvm") {
            Err(e) if e.kind() == ErrorKind::NotFound => Self::Absent,
            Err(e) if e.kind() == ErrorKind::PermissionDenied => Self::PermissionDenied,
            Err(e) => Self::OpenFailed {
                error: e.to_string(),
            },
            Ok(file) => {
                drop(file);
                match Kvm::new() {
                    Err(e) => Self::OpenFailed {
                        error: e.to_string(),
                    },
                    Ok(kvm) => {
                        let caps = [
                            ("irqchip", Cap::Irqchip),
                            ("user_memory", Cap::UserMemory),
                            ("pit2", Cap::Pit2),
                            ("ext_cpuid", Cap::ExtCpuid),
                            ("immediate_exit", Cap::ImmediateExit),
                        ];
                        Self::Opened {
                            api_version: kvm.get_api_version(),
                            extensions: caps
                                .into_iter()
                                .map(|(name, cap)| (name.to_owned(), kvm.check_extension(cap)))
                                .collect(),
                        }
                    }
                }
            }
        }
    }
}
