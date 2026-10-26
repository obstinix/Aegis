; hello16: real-mode test guest for roadmap M2.
;
; Load at guest-physical 0x1000; enter in real mode with CS=0, IP=0x1000.
; Writes MESSAGE to the 16550 transmit register at port 0x3F8, one OUT per
; byte, then halts.
;
; Expected exits, in order: one 1-byte port write to 0x3F8 per byte of
; MESSAGE (excluding the terminating NUL), then HLT. No other exits.
;
; Assemble: nasm -f bin -o hello16.bin hello16.asm

bits 16
org 0x1000

start:
    xor ax, ax
    mov ds, ax              ; MESSAGE is addressed through DS:SI
    cld
    mov dx, 0x3f8
    mov si, message
.next:
    lodsb
    test al, al
    jz .done
    out dx, al
    jmp .next
.done:
    hlt
    jmp .done               ; not reached in M2: HLT ends the run

message: db "Aegis M2: hello from a KVM guest", 10, 0
