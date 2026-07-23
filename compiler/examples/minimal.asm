; Minimal working PE for x86-64 Windows
; nasm -f win64 -o minimal.obj minimal.asm
; link /subsystem:console /entry:main /out:minimal.exe minimal.obj /defaultlib:kernel32.lib
;
; But since we don't have a linker, use this approach:
; nasm -f bin -o minimal.exe minimal.asm
;
; This PE is hand-crafted with correct headers.

BITS 64
ORG 0x140000000

; ============ DOS HEADER ============
    db 'M', 'Z'
    times 58 db 0
    dd 0x40  ; e_lfanew

; ============ PE SIGNATURE ============
    db 'P', 'E', 0, 0

; ============ COFF HEADER ============
    dw 0x8664                    ; Machine: AMD64
    dw 1                         ; NumberOfSections
    dd 0                         ; TimeDateStamp
    dd 0                         ; PointerToSymbolTable
    dd 0                         ; NumberOfSymbols
    dw 0x00F0                    ; SizeOfOptionalHeader (240)
    dw 0x002F                    ; Characteristics

; ============ OPTIONAL HEADER (PE32+, 240 bytes) ============
    dw 0x020B                    ; Magic
    db 0x0E, 0x00                ; LinkerVersion
    dd 0x200                     ; SizeOfCode
    dd 0x200                     ; SizeOfInitData
    dd 0                         ; SizeOfUninitData
    dd main_rva                  ; AddressOfEntryPoint
    dd section..text.vstart      ; BaseOfCode
    dq 0x140000000               ; ImageBase
    dd 0x1000                    ; SectionAlignment
    dd 0x200                     ; FileAlignment
    dw 6, 0                      ; OS v6.0
    dw 0, 0                      ; ImageVer
    dw 6, 0                      ; Subsys v6.0
    dd 0                         ; Win32Version
    dd image_size                ; SizeOfImage
    dd headers_size              ; SizeOfHeaders
    dd 0                         ; CheckSum
    dw 3                         ; Subsystem: CONSOLE
    dw 0x0100                    ; DllCharacteristics
    dq 0x100000                  ; StackReserve
    dq 0x1000                    ; StackCommit
    dq 0x100000                  ; HeapReserve
    dq 0x1000                    ; HeapCommit
    dd 0                         ; LoaderFlags
    dd 16                        ; NumberOfRvaAndSizes

; Data directories
    ; Export
    dq 0
    ; Import
    dd import_dir - IMAGE_BASE   ; Import RVA
    dd import_dir_end - import_dir ; Import Size
    ; 14 remaining
    times 14 dq 0

headers_size equ $ - IMAGE_BASE

; ============ SECTION: .text ============
section .text vstart=0x1000 followsto=.data
main_rva equ main - IMAGE_BASE

main:
    ; ExitProcess(0)
    xor ecx, ecx
    call [rel ExitProcess_ptr]
    ret

; Import thunks
ExitProcess_ptr: dq ExitProcess_iat - IMAGE_BASE

section .data vstart=0x2000
; Import directory
import_dir:
    dd import_ilt - IMAGE_BASE    ; ILT
    dd 0                         ; TimeDateStamp
    dd 0                         ; ForwarderChain
    dd kernel32_name - IMAGE_BASE ; Name RVA
    dd import_iat - IMAGE_BASE   ; IAT
    times 5 dd 0                 ; null terminator
import_dir_end:

import_ilt:
    dq ExitProcess_hintname - IMAGE_BASE
    dq 0 ; null
import_iat:
    dq ExitProcess_hintname - IMAGE_BASE
    dq 0 ; null

ExitProcess_hintname:
    dw 0
    db 'ExitProcess', 0
    align 2, db 0

kernel32_name:
    db 'kernel32.dll', 0

image_size equ $ - IMAGE_BASE
