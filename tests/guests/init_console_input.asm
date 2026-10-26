; init_console_input: Linux /init for the virtio-console input test (x86-64,
; static, no libc).
;
; 1. open("/dev/kmsg") and log START, so the UART console shows /init ran.
; 2. open("/dev/hvc0", O_RDWR | O_NOCTTY) and write READY.
; 3. Twice: read one line from hvc0 (the tty is in canonical mode, so read
;    returns at a newline), then write it back framed as AEGIS_RX[<line>].
;    The tty also echoes input by itself, but never with this framing, so
;    a framed line proves that userspace read the bytes.
; 4. Write END to hvc0, log DONE to /dev/kmsg, then restart by triple fault
;    (`reboot=t`), as in init_marker.asm.
;
; If hvc0 cannot be opened, a read fails, or no line arrives within
; WAIT_MS, log the failure to /dev/kmsg and restart. The run then ends
; with a diagnosis rather than hanging until the host test's deadline.
;
; Build: nasm -f elf64 -o init.o init_console_input.asm && ld -static -nostdlib -o init init.o

bits 64
default rel

%define SYS_read    0
%define SYS_write   1
%define SYS_open    2
%define SYS_poll    7
%define SYS_exit    60
%define SYS_reboot  169
%define POLLIN      1
%define WAIT_MS     10000
%define O_WRONLY    1
%define O_RDWR      2
%define O_NOCTTY    0x100
%define BUF_SIZE    256
%define LINES       2
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

    mov eax, SYS_open
    lea rdi, [hvc_path]
    mov esi, O_RDWR | O_NOCTTY
    xor edx, edx
    syscall
    test rax, rax
    js .hvc_failed
    mov r13, rax                ; hvc0 fd
    WRITE r13, ready

    mov r15d, LINES
.line:
    mov [pfd], r13d             ; struct pollfd { fd, events = POLLIN, revents }
    mov word [pfd + 4], POLLIN
    mov word [pfd + 6], 0
    mov eax, SYS_poll
    lea rdi, [pfd]
    mov esi, 1
    mov edx, WAIT_MS
    syscall
    test rax, rax
    jz .timed_out
    mov eax, SYS_read
    mov rdi, r13
    lea rsi, [buf]
    mov edx, BUF_SIZE
    syscall
    test rax, rax
    jle .read_failed
    mov r14, rax                ; bytes in this line
    WRITE r13, open_tag
    mov eax, SYS_write
    mov rdi, r13
    lea rsi, [buf]
    mov rdx, r14
    syscall
    WRITE r13, close_tag
    dec r15d
    jnz .line

    WRITE r13, end
    WRITE r12, done
    jmp .reboot

.hvc_failed:
    WRITE r12, hvc_fail
    jmp .reboot

.timed_out:
    WRITE r12, timeout
    jmp .reboot

.read_failed:
    WRITE r12, read_fail

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
kmsg_path:       db "/dev/kmsg", 0
hvc_path:        db "/dev/hvc0", 0
start:           db "AEGIS_INPUT_INIT_START", 10
start_len        equ $ - start
done:            db "AEGIS_INPUT_INIT_DONE", 10
done_len         equ $ - done
hvc_fail:        db "AEGIS_INPUT_HVC_FAIL", 10
hvc_fail_len     equ $ - hvc_fail
read_fail:       db "AEGIS_INPUT_READ_FAIL", 10
read_fail_len    equ $ - read_fail
timeout:         db "AEGIS_INPUT_TIMEOUT", 10
timeout_len      equ $ - timeout
ready:           db "AEGIS_RX_READY", 10
ready_len        equ $ - ready
open_tag:        db "AEGIS_RX["
open_tag_len     equ $ - open_tag
close_tag:       db "]", 10
close_tag_len    equ $ - close_tag
end:             db "AEGIS_RX_END", 10
end_len          equ $ - end

section .bss
pfd:             resb 8
buf:             resb BUF_SIZE
