; init_marker: minimal Linux /init for the M3 boot test (x86-64, static, no libc).
;
; 1. open("/dev/kmsg", O_WRONLY) and write MARKER to it. The kernel prints
;    /dev/kmsg messages on its console (console=ttyS0), and the 8250 console
;    driver polls the UART's line status rather than waiting for interrupts.
;    That works with Aegis's transmit-only UART, which raises no IRQs.
; 2. reboot(LINUX_REBOOT_CMD_RESTART). With `reboot=t` on the kernel command
;    line, the restart is a triple fault, which KVM reports as a shutdown exit.
; 3. If reboot returns, exit(1): the kernel then panics ("Attempted to kill
;    init"), and with `panic=-1` it restarts the same way.
;
; Build: nasm -f elf64 -o init.o init_marker.asm && ld -static -nostdlib -o init init.o

bits 64
default rel

%define SYS_write   1
%define SYS_open    2
%define SYS_exit    60
%define SYS_reboot  169
%define O_WRONLY    1
%define LINUX_REBOOT_MAGIC1     0xfee1dead
%define LINUX_REBOOT_MAGIC2     672274793
%define LINUX_REBOOT_CMD_RESTART 0x01234567

section .text
global _start
_start:
    mov eax, SYS_open
    lea rdi, [kmsg_path]
    mov esi, O_WRONLY
    xor edx, edx
    syscall
    mov r12, rax                ; fd (a negative errno makes write fail harmlessly)

    mov eax, SYS_write
    mov rdi, r12
    lea rsi, [marker]
    mov edx, marker_len
    syscall

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
kmsg_path:  db "/dev/kmsg", 0
marker:     db "AEGIS_M3_INIT_OK", 10
marker_len  equ $ - marker
