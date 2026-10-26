; mmio_unmapped: real-mode test guest for the M2 unhandled-MMIO policy.
;
; Load at guest-physical 0x1000; enter in real mode with CS=0, IP=0x1000.
; The test maps guest RAM only below 0x80000. The guest writes 0x42 to
; linear address 0x90000, which has neither RAM nor a device, so KVM
; reports an MMIO write there. No MMIO devices exist in M2, so the VM must
; stop as failed, with the access in its diagnostics.
;
; Expected exits: one MMIO write of 1 byte, value 0x42, at 0x90000.
;
; Assemble: nasm -f bin -o mmio_unmapped.bin mmio_unmapped.asm

bits 16
org 0x1000

start:
    mov ax, 0x9000
    mov es, ax
    mov byte [es:0], 0x42   ; linear 0x90000
    hlt                     ; not reached: the run stops at the MMIO exit
