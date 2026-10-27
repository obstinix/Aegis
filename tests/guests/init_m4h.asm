; init_m4h: Linux /init for the M4h SMP + virtio-console test (x86-64,
; static, no libc).
;
; 1. open("/dev/kmsg") and log START, so the UART console shows /init ran.
; 2. mount("proc", "/proc", "proc").
; 3. open("/dev/hvc0", O_WRONLY | O_NOCTTY), the virtio-console's port 0, and
;    write HVC_MARKER to it. Each write goes through the transmit virtqueue.
; 4. Wait 200 ms, then copy /proc/interrupts to hvc0, then HVC_END. The host
;    test reads the virtio interrupt count (one column per CPU) from what hvc0
;    carried. The wait is there because nothing orders the used-buffer
;    interrupt for step 3's write before this read: it may be delivered on the
;    other CPU, whose vCPU thread the host may not have scheduled yet. One run
;    in 22 read a count of 0 without the wait. A lost interrupt still reads 0
;    however long the wait.
; 5. Log DONE to /dev/kmsg, then reboot(LINUX_REBOOT_CMD_RESTART), which is a
;    triple fault with `reboot=t`, as in init_marker.asm.
;
; If hvc0 cannot be opened, log HVC_FAIL to /dev/kmsg and reboot. A failed
; /proc mount or open only loses the interrupt table, which the host test
; reports as missing.
;
; Build: nasm -f elf64 -o init.o init_m4h.asm && ld -static -nostdlib -o init init.o

bits 64
default rel

%define SYS_read    0
%define SYS_write   1
%define SYS_open    2
%define SYS_nanosleep 35
%define SYS_exit    60
%define SYS_mount   165
%define SYS_reboot  169
%define O_RDONLY    0
%define O_WRONLY    1
%define O_NOCTTY    0x100
%define BUF_SIZE    4096
%define LINUX_REBOOT_MAGIC1     0xfee1dead
%define LINUX_REBOOT_MAGIC2     672274793
%define LINUX_REBOOT_CMD_RESTART 0x01234567

; write(fd_reg, label, label_len)
%macro WRITE 2
    mov eax, SYS_write
    mov rdi, %1
    lea rsi, [%2]
    mov edx, %2_len
    syscall
%endmacro

section .text
global _start
_start:
    mov eax, SYS_open
    lea rdi, [kmsg_path]
    mov esi, O_WRONLY
    xor edx, edx
    syscall
    mov r12, rax                ; kmsg fd (a negative errno makes writes fail harmlessly)
    WRITE r12, start

    mov eax, SYS_mount
    lea rdi, [proc_name]
    lea rsi, [proc_path]
    lea rdx, [proc_name]
    xor r10d, r10d
    xor r8d, r8d
    syscall

    mov eax, SYS_open
    lea rdi, [hvc_path]
    mov esi, O_WRONLY | O_NOCTTY
    xor edx, edx
    syscall
    test rax, rax
    js .hvc_failed
    mov r13, rax                ; hvc0 fd
    WRITE r13, hvc_marker

    mov eax, SYS_nanosleep      ; see step 4 above
    lea rdi, [settle]
    xor esi, esi
    syscall

    mov eax, SYS_open
    lea rdi, [interrupts_path]
    mov esi, O_RDONLY
    xor edx, edx
    syscall
    mov r14, rax                ; /proc/interrupts fd (negative: the read fails, loop ends)
.copy:
    mov eax, SYS_read
    mov rdi, r14
    lea rsi, [buf]
    mov edx, BUF_SIZE
    syscall
    test rax, rax
    jle .copied
    mov rdx, rax
    mov eax, SYS_write
    mov rdi, r13
    lea rsi, [buf]
    syscall
    jmp .copy
.copied:
    WRITE r13, hvc_end
    WRITE r12, done
    jmp .reboot

.hvc_failed:
    WRITE r12, hvc_fail

.reboot:
    mov eax, SYS_reboot
    mov edi, LINUX_REBOOT_MAGIC1
    mov esi, LINUX_REBOOT_MAGIC2
    mov edx, LINUX_REBOOT_CMD_RESTART
    xor r10d, r10d
    syscall

    mov eax, SYS_exit
    mov edi, 1
    syscall

section .rodata
align 8
settle:          dq 0, 200000000    ; struct timespec: 0 s, 200 ms
kmsg_path:       db "/dev/kmsg", 0
hvc_path:        db "/dev/hvc0", 0
proc_name:       db "proc", 0
proc_path:       db "/proc", 0
interrupts_path: db "/proc/interrupts", 0
start:           db "AEGIS_M4H_INIT_START", 10
start_len        equ $ - start
done:            db "AEGIS_M4H_INIT_DONE", 10
done_len         equ $ - done
hvc_fail:        db "AEGIS_M4H_HVC_FAIL", 10
hvc_fail_len     equ $ - hvc_fail
hvc_marker:      db "AEGIS_M4H_HVC_OK", 10
hvc_marker_len   equ $ - hvc_marker
hvc_end:         db "AEGIS_M4H_HVC_END", 10
hvc_end_len      equ $ - hvc_end

section .bss
buf:             resb BUF_SIZE
