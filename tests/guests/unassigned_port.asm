; unassigned_port: real-mode test guest for the M2 unassigned-port policy.
;
; Load at guest-physical 0x1000; enter in real mode with CS=0, IP=0x1000.
; Writes to unassigned port 0x80, reads unassigned port 0x404, echoes the
; byte it read to the UART at 0x3F8, writes 'k', and halts.
;
; With the M2 policy (writes dropped, reads return all ones, both logged,
; guest continues), the expected exits are:
;   out 0x80 (0x12), in 0x404, out 0x3F8 (0xFF), out 0x3F8 ('k'), HLT
; and the UART output is the two bytes 0xFF 'k'.
;
; Assemble: nasm -f bin -o unassigned_port.bin unassigned_port.asm

bits 16
org 0x1000

start:
    mov al, 0x12
    out 0x80, al            ; unassigned: dropped
    mov dx, 0x404
    in al, dx               ; unassigned: expect 0xFF
    mov dx, 0x3f8
    out dx, al              ; echo the value read
    mov al, 'k'
    out dx, al
    hlt
