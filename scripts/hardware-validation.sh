#!/usr/bin/env bash
# Aegis hardware validation (roadmap M5h).
#
# Run this ON the machine being validated (the ThinkPad X1 Carbon, PyxisOS
# Track A), from the repository root:
#
#   scripts/hardware-validation.sh --kernel /path/to/bzImage
#
# It records the environment, runs the existing test suites in order, and
# writes everything to target/hardware-validation/<host>-<UTC time>/:
#   environment.txt   what the machine is and what it exposes
#   doctor.txt/.json  `aegis doctor` output
#   <step>.log        full output of each step
#   summary.txt       one line per step: PASS, FAIL or SKIP (with reason)
#   results.md        a filled-in copy of the results template from
#                     docs/hardware-validation.md
#
# Steps are REQUIRED (the M5h criterion: the M3 tests, with the M2 KVM guest
# tests they build on) or EXTENDED (later milestones; informative). A step
# whose prerequisites are missing is reported as SKIP with the reason, never
# as a pass. Nothing is installed, nothing runs as root, and nothing outside
# the repository is written.
#
# Exit status: 0 if every REQUIRED step passed, 1 otherwise, 2 for usage
# errors.

set -u

usage() {
    cat <<'EOF'
usage: scripts/hardware-validation.sh [--kernel <bzImage>] [--out <dir>] [--skip-extended]

  --kernel <bzImage>  guest kernel for the Linux boot tests (sets
                      AEGIS_TEST_KERNEL). Default: $AEGIS_TEST_KERNEL.
  --out <dir>         results directory (default:
                      target/hardware-validation/<host>-<UTC time>)
  --skip-extended     run only the REQUIRED (M5h) steps
EOF
}

kernel="${AEGIS_TEST_KERNEL:-}"
out=""
extended=1
while [ $# -gt 0 ]; do
    case "$1" in
        --kernel) kernel="${2:-}"; shift 2 || { usage; exit 2; } ;;
        --out) out="${2:-}"; shift 2 || { usage; exit 2; } ;;
        --skip-extended) extended=0; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if [ ! -f Cargo.toml ] || [ ! -d crates/aegis-kvm ]; then
    echo "run this from the root of the Aegis repository" >&2
    exit 2
fi

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
host="$(hostname 2>/dev/null || echo unknown-host)"
out="${out:-target/hardware-validation/${host}-${stamp}}"
mkdir -p "$out" || { echo "cannot create $out" >&2; exit 2; }
summary="$out/summary.txt"
: > "$summary"
required_failed=0

note() { printf '%s\n' "$*" | tee -a "$summary"; }

# record <name> <level> <result> <detail>
record() {
    printf '%-28s %-9s %-5s %s\n' "$1" "$2" "$3" "$4" | tee -a "$summary"
    if [ "$2" = REQUIRED ] && [ "$3" != PASS ]; then
        required_failed=1
    fi
}

# run_step <name> <level> <command...>: run, log, and record PASS or FAIL.
run_step() {
    local name="$1" level="$2"
    shift 2
    local log="$out/$name.log" started ended status
    started=$(date +%s)
    {
        echo "\$ $*"
        echo "started: $(date -u +%FT%TZ)"
    } > "$log"
    "$@" >> "$log" 2>&1
    status=$?
    ended=$(date +%s)
    echo "exit status: $status" >> "$log"
    if [ "$status" -eq 0 ]; then
        record "$name" "$level" PASS "$((ended - started)) s, $log"
    else
        record "$name" "$level" FAIL "exit $status after $((ended - started)) s, see $log"
    fi
}

# --- environment -----------------------------------------------------------

env_file="$out/environment.txt"
{
    echo "== date (UTC)"; date -u +%FT%TZ
    echo "== host"; uname -a
    echo "== os-release"; cat /etc/os-release 2>/dev/null || echo "(none)"
    echo "== CPU"; grep -m1 '^model name' /proc/cpuinfo 2>/dev/null || echo "(unknown)"
    echo "== CPU flags of interest"
    for f in vmx svm ept vpid unrestricted_guest x2apic hypervisor; do
        if grep -qw "$f" /proc/cpuinfo 2>/dev/null; then echo "$f: yes"; else echo "$f: no"; fi
    done
    echo "== vmx flags line"; grep -m1 '^vmx flags' /proc/cpuinfo 2>/dev/null || echo "(no vmx flags line)"
    echo "== KVM modules"; grep -E '^kvm' /proc/modules 2>/dev/null || echo "(no kvm modules listed; may be built in)"
    for p in nested ept unrestricted_guest enable_apicv; do
        f="/sys/module/kvm_intel/parameters/$p"
        [ -r "$f" ] && echo "kvm_intel.$p=$(cat "$f")"
    done
    echo "== /dev/kvm"; ls -l /dev/kvm 2>&1
    echo "== user and groups"; id
    echo "== tools"
    for t in rustup cargo rustc nasm ld qemu-system-x86_64 file; do
        printf '%s: ' "$t"
        if command -v "$t" >/dev/null 2>&1; then
            "$t" --version 2>/dev/null | head -1
        else
            echo "not found"
        fi
    done
    echo "== repository"; git rev-parse HEAD 2>/dev/null; git status --short 2>/dev/null | head -20
    echo "== guest kernel"; echo "${kernel:-(not given)}"
    [ -n "$kernel" ] && [ -f "$kernel" ] && file "$kernel" 2>/dev/null
} > "$env_file" 2>&1

note "Aegis hardware validation: $host, $(date -u +%FT%TZ)"
note "results: $out"
note "CPU: $(grep -m1 '^model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ //')"
note ""

# --- prerequisites ---------------------------------------------------------

kvm_ok=0
if [ -c /dev/kvm ] && [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
    kvm_ok=1
fi
have() { command -v "$1" >/dev/null 2>&1; }

# A bzImage carries "HdrS" at offset 0x202 (boot protocol header).
kernel_ok=0
if [ -n "$kernel" ] && [ -f "$kernel" ]; then
    if [ "$(dd if="$kernel" bs=1 skip=514 count=4 2>/dev/null)" = HdrS ]; then
        kernel_ok=1
    fi
fi
kernel_reason="no bzImage given (--kernel or AEGIS_TEST_KERNEL)"
if [ -n "$kernel" ] && [ ! -f "$kernel" ]; then
    kernel_reason="$kernel does not exist"
elif [ -n "$kernel" ] && [ "$kernel_ok" -eq 0 ]; then
    kernel_reason="$kernel is not a bzImage (no HdrS header; a UKI or vmlinux will not do)"
fi
[ "$kernel_ok" -eq 1 ] && export AEGIS_TEST_KERNEL="$kernel"

# --- steps -----------------------------------------------------------------

printf '%-28s %-9s %-5s %s\n' STEP LEVEL RESULT DETAIL | tee -a "$summary"

run_step doctor EXTENDED sh -c 'cargo run -q -p aegis-cli -- doctor > "$1/doctor.txt" 2>&1; status=$?; cargo run -q -p aegis-cli -- doctor --json > "$1/doctor.json" 2>/dev/null; exit $status' sh "$out"

run_step unit-tests REQUIRED cargo test --workspace

if [ "$kvm_ok" -eq 0 ]; then
    record m2-kvm-guests REQUIRED SKIP "/dev/kvm missing or not readable and writable by $(id -un)"
elif ! have nasm; then
    record m2-kvm-guests REQUIRED SKIP "nasm not found"
else
    run_step m2-kvm-guests REQUIRED cargo test -p aegis-kvm --test kvm_guests -- --include-ignored
fi

if [ "$kvm_ok" -eq 0 ]; then
    record m3-linux-boot REQUIRED SKIP "/dev/kvm missing or not readable and writable by $(id -un)"
elif ! have nasm || ! have ld; then
    record m3-linux-boot REQUIRED SKIP "nasm or ld not found"
elif [ "$kernel_ok" -eq 0 ]; then
    record m3-linux-boot REQUIRED SKIP "$kernel_reason"
else
    run_step m3-linux-boot REQUIRED cargo test -p aegis-cli --test boot_linux -- --include-ignored --exact boots_linux_to_the_init_marker_under_seccomp
fi

run_step m3-seccomp-selftest REQUIRED cargo test -p aegis-cli --test boot_linux -- --exact seccomp_selftest_is_killed_by_sigsys

if [ "$extended" -eq 1 ]; then
    for test in boots_two_vcpus_with_a_virtio_console guest_reads_host_stdin_from_hvc0 output_streams_while_the_guest_runs_and_input_answers_its_prompt; do
        case "$test" in
            boots_two_vcpus_with_a_virtio_console) name=m4h-smp-virtio ;;
            guest_reads_host_stdin_from_hvc0) name=m6h-console-input ;;
            *) name=streamed-output ;;
        esac
        if [ "$kvm_ok" -eq 0 ] || [ "$kernel_ok" -eq 0 ] || ! have nasm || ! have ld; then
            record "$name" EXTENDED SKIP "needs /dev/kvm, nasm, ld and a bzImage"
        else
            run_step "$name" EXTENDED cargo test -p aegis-cli --test boot_linux -- --include-ignored --exact "$test"
        fi
    done
    # Exit 77 is uefi-test's "skipped" (no KVM, QEMU or OVMF here).
    started=$(date +%s)
    cargo xtask uefi-test > "$out/b0-uefi-test.log" 2>&1
    status=$?
    case "$status" in
        0) record b0-uefi-test EXTENDED PASS "$(( $(date +%s) - started )) s, $out/b0-uefi-test.log" ;;
        77) record b0-uefi-test EXTENDED SKIP "$(grep -m1 SKIPPED "$out/b0-uefi-test.log" | sed 's/^xtask: //')" ;;
        *) record b0-uefi-test EXTENDED FAIL "exit $status, see $out/b0-uefi-test.log" ;;
    esac
    run_step repro-test EXTENDED cargo xtask repro-test
fi

# --- diagnostics -----------------------------------------------------------

# The kernel log helps with KVM failures; reading it may need privileges,
# which this script does not ask for.
dmesg 2>/dev/null | grep -iE 'kvm|vmx|virtualization' | tail -50 > "$out/dmesg-kvm.txt" || true
[ -s "$out/dmesg-kvm.txt" ] || echo "(dmesg not readable without privileges, or no KVM lines)" > "$out/dmesg-kvm.txt"

# --- results.md ------------------------------------------------------------

{
    echo "# Hardware validation results"
    echo
    echo "- Machine: $host"
    echo "- CPU: $(grep -m1 '^model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ //')"
    echo "- Kernel (host): $(uname -r)"
    if grep -qw hypervisor /proc/cpuinfo 2>/dev/null; then
        echo "- Hypervisor present (CPUID hypervisor flag): **yes**. This run was inside a VM (for example WSL2), so it is NOT a physical-hardware result."
    else
        echo "- Hypervisor present (CPUID hypervisor flag): no (bare metal)"
    fi
    echo "- Date (UTC): $(date -u +%FT%TZ)"
    echo "- Aegis commit: $(git rev-parse --short HEAD 2>/dev/null)"
    echo "- Guest kernel: ${kernel:-(none)}"
    echo "- Run by: scripts/hardware-validation.sh (see docs/hardware-validation.md)"
    echo
    echo '```text'
    cat "$summary"
    echo '```'
    echo
    if [ "$required_failed" -eq 0 ]; then
        echo "All REQUIRED steps passed."
    else
        echo "At least one REQUIRED step did not pass (FAIL or SKIP)."
    fi
} > "$out/results.md"

note ""
if [ "$required_failed" -eq 0 ]; then
    note "All REQUIRED steps passed. Results: $out"
    exit 0
fi
note "At least one REQUIRED step did not pass. Results and logs: $out"
exit 1
