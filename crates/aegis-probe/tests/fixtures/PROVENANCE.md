# Fixture provenance

Captured 2026-10-26 on the development workstation (Intel Core Ultra 9 275HX, Windows 11 with Hyper-V/VBS active). The files are byte-exact (`.gitattributes` marks them `-text`).

## `wsl2-ultra9-275hx/`

WSL2 Ubuntu 24.04, kernel `6.18.33.2-microsoft-standard-WSL2`, user `obsti`, not a member of `kvm`.

| File | Source |
|---|---|
| `proc/cpuinfo` | first processor block of `/proc/cpuinfo` (`awk 'BEGIN{RS=""} NR==1'`) |
| `proc/sys/kernel/osrelease` | copied verbatim |
| `sys/module/kvm_intel/parameters/{nested,ept,unrestricted_guest,enable_apicv}` | copied verbatim |
| `sys/devices/system/cpu/vulnerabilities/*` | copied verbatim |
| `observations.json` | `aegis doctor --raw`, run in that environment. The tests take `cpuid` and `dev_kvm` from it, and parse the file tree for everything else. |

At capture time `/dev/kvm` was `crw-rw---- root kvm`.

## `wsl2-ultra9-275hx-kvm/`

The same WSL2 environment, captured later on 2026-10-26 after the owner added `obsti` to `kvm` (`getent group kvm` → `kvm:x:993:obsti`; `/etc/group` modified 04:57:04 UTC).

| File | Source |
|---|---|
| `observations.json` | `aegis doctor --raw` in that environment |

Independent cross-check used by the tests: raw ioctls with Python's `fcntl`, without `kvm-ioctls` or any Aegis code. Request numbers come from the kernel's KVM API: `KVM_GET_API_VERSION` = `_IO(0xAE, 0x00)`, `KVM_CHECK_EXTENSION` = `_IO(0xAE, 0x03)`.

```
fd = os.open("/dev/kvm", os.O_RDWR | os.O_CLOEXEC)
fcntl.ioctl(fd, 0xAE00, 0)      -> 12
fcntl.ioctl(fd, 0xAE03, 0)      -> 1   # KVM_CAP_IRQCHIP
fcntl.ioctl(fd, 0xAE03, 3)      -> 1   # KVM_CAP_USER_MEMORY
fcntl.ioctl(fd, 0xAE03, 7)      -> 1   # KVM_CAP_EXT_CPUID
fcntl.ioctl(fd, 0xAE03, 33)     -> 1   # KVM_CAP_PIT2
fcntl.ioctl(fd, 0xAE03, 136)    -> 1   # KVM_CAP_IMMEDIATE_EXIT
```

The two WSL2 captures ran on different logical CPUs, so leaf 1 EBX (which includes the initial APIC ID) differs between them. That's expected.

## `windows-ultra9-275hx/`

Windows 11 root partition under Hyper-V.

| File | Source |
|---|---|
| `observations.json` | `aegis doctor --raw` on the Windows host |

Independent facts used by the tests, recorded in Phase 1 (`research/hypervisor-landscape.md` §7) from WMI before any Aegis code existed:
- `Win32_ComputerSystem.HypervisorPresent = True`
- `Win32_Processor.VMMonitorModeExtensions = False`
- `Win32_Processor.SecondLevelAddressTranslationExtensions = False`
- `Win32_Processor.Name = Intel(R) Core(TM) Ultra 9 275HX`
