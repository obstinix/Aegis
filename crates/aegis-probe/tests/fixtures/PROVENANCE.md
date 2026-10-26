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
