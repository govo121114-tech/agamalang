; Minimal PE executable for x86-64 Windows
; Assemble: nasm -f bin -o test.exe test_pe.asm
;
; Uses NASM flat binary mode with ORG at image base.
; $$ = ORG value = IMAGE_BASE
; $  = current virtual address

BITS 64
ORG 0x140000000

; ============================================================
; First, compute all sizes as EQU constants (before they're used)
; ============================================================

; These will be filled in later with the actual values
; We define them as forward references

; ============================================================
; DOS Header (64 bytes)
; ============================================================
    db 'M', 'Z'                     ; e_magic
    dw 0x0090                       ; e_cblp
    dw 0x0003                       ; e_cp
    dw 0x0000                       ; e_crlc
    dw 0x0004                       ; e_cparhdr
    dw 0x0000                       ; e_minalloc
    dw 0xFFFF                       ; e_maxalloc
    dw 0x0000                       ; e_ss
    dw 0x0100                       ; e_sp
    dw 0x0000                       ; e_csum
    dw 0x0000                       ; e_ip
    dw 0x0000                       ; e_cs
    dw 0x0040                       ; e_lfarlc (offset to PE signature)
    times 36 db 0                   ; e_res to end of DOS header

; ============================================================
; PE Signature
; ============================================================
    db 'P', 'E', 0, 0

; ============================================================
; COFF Header (20 bytes)
; ============================================================
    dw 0x8664                       ; Machine: AMD64
    dw 0x0002                       ; NumberOfSections
    dd 0x00000000                   ; TimeDateStamp
    dd 0x00000000                   ; PointerToSymbolTable
    dd 0x00000000                   ; NumberOfSymbols
    dw 0x0070                       ; SizeOfOptionalHeader (112 bytes)
    dw 0x002F                       ; Characteristics

; ============================================================
; Optional Header PE32+ (112 bytes)
; ============================================================
    dw 0x020B                       ; Magic (PE32+)
    db 0x0E                         ; MajorLinkerVersion
    db 0x00                         ; MinorLinkerVersion

; We'll fill these sizes at the end of the file using a fixup approach
; For now, placeholder values
    dd 0x00000000                   ; SizeOfCode (placeholder)
    dd 0x00000000                   ; SizeOfInitializedData (placeholder)
    dd 0x00000000                   ; SizeOfUninitializedData

; AddressOfEntryPoint (RVA) - placeholder
    dd 0x00000000                   ; AddressOfEntryPoint (placeholder)
    dd 0x00001000                   ; BaseOfCode (RVA)

    ; PE32+ specific fields
    dq 0x140000000                 ; ImageBase
    dd 0x00001000                   ; SectionAlignment
    dd 0x00000200                   ; FileAlignment
    dw 0x0006                       ; MajorOperatingSystemVersion
    dw 0x0000                       ; MinorOperatingSystemVersion
    dw 0x0000                       ; MajorImageVersion
    dw 0x0000                       ; MinorImageVersion
    dw 0x0006                       ; MajorSubsystemVersion
    dw 0x0000                       ; MinorSubsystemVersion
    dd 0x00000000                   ; Win32VersionValue
    dd 0x00000000                   ; SizeOfImage (placeholder)
    dd 0x00000000                   ; SizeOfHeaders (placeholder)
    dd 0x00000000                   ; CheckSum
    dw 0x0003                       ; Subsystem (CONSOLE)
    dw 0x0100                       ; DllCharacteristics
    dq 0x00100000                   ; SizeOfStackReserve
    dq 0x00001000                   ; SizeOfStackCommit
    dq 0x00100000                   ; SizeOfHeapReserve
    dq 0x00001000                   ; SizeOfHeapCommit
    dd 0x00000000                   ; LoaderFlags
    dd 0x00000010                   ; NumberOfRvaAndSizes

; ============================================================
; Data Directories (16 entries, 8 bytes each)
; ============================================================
    ; Export directory (empty)
    dq 0
    ; Import directory (placeholder)
    dd 0x00000000                   ; Import RVA (placeholder)
    dd 0x00000000                   ; Import Size (placeholder)
    ; 14 remaining entries
    times 14 dq 0

; ============================================================
; Section Headers (2 sections, 40 bytes each)
; ============================================================
; .text section header
    db '.', 't', 'e', 'x', 't', 0, 0, 0
    dd 0x00000000                   ; VirtualSize (placeholder)
    dd code_section_vaddr           ; VirtualAddress
    dd 0x00000000                   ; SizeOfRawData (placeholder)
    dd 0x00000000                   ; PointerToRawData (placeholder)
    dd 0x00000000                   ; PointerToRelocations
    dd 0x00000000                   ; PointerToLinenumbers
    dw 0x0000                       ; NumberOfRelocations
    dw 0x0000                       ; NumberOfLinenumbers
    dd 0x60000020                   ; Characteristics (CODE | EXECUTE | READ)

; .data section header
    db '.', 'd', 'a', 't', 'a', 0, 0, 0
    dd 0x00000000                   ; VirtualSize (placeholder)
    dd data_section_vaddr           ; VirtualAddress
    dd 0x00000000                   ; SizeOfRawData (placeholder)
    dd 0x00000000                   ; PointerToRawData (placeholder)
    dd 0x00000000                   ; PointerToRelocations
    dd 0x00000000                   ; PointerToLinenumbers
    dw 0x0000                       ; NumberOfRelocations
    dw 0x0000                       ; NumberOfLinenumbers
    dd 0xC0000040                   ; Characteristics (INITIALIZED_DATA | READ | WRITE)

; End of headers
headers_end:

; ============================================================
; Now the sections start. We align to section alignment.
; ============================================================

; ============================================================
; .text Section (code)
; ============================================================
code_section_start:

; We need to align to file alignment (0x200) from the start
; headers_size = code_section_start - $$
; But we need to fill it in the headers. Let's align here.
times ($$ + 0x200) - $ db 0x90   ; Align to 0x200 boundary (after headers)

code_section_vaddr equ code_section_start - $$
code_section_raw_offset equ $ - $$

; ====== Entry Point ======
entry_point:
    ; === Call GetStdHandle(STD_OUTPUT_HANDLE = -11) ===
    mov  ecx, 0xFFFFFFF5            ; STD_OUTPUT_HANDLE = -11
    sub  rsp, 0x28                  ; Allocate shadow space + alignment
    call [rel getstdhandle_ptr]
    add  rsp, 0x28

    ; === Call WriteFile(hConsole, str, len, &written, NULL) ===
    mov  rcx, rax                   ; hConsoleOutput
    lea  rdx, [rel hello_str]       ; lpBuffer
    mov  r8d, hello_str_len         ; nNumberOfCharsToWrite
    sub  rsp, 0x38                  ; 32 shadow + 8 written + 8 align
    lea  r9, [rsp+0x30]            ; lpNumberOfCharsWritten
    mov  qword [rsp+0x28], 0       ; lpReserved = NULL
    call [rel writefile_ptr]
    add  rsp, 0x38

    ; === Call ExitProcess(0) ===
    xor  ecx, ecx                   ; exit code = 0
    call [rel exitprocess_ptr]

; Hello world string
hello_str: db 'Hello from AgamaLang!', 0x0D, 0x0A
hello_str_len equ $ - hello_str

; Import address pointers (filled by PE loader)
align 8
getstdhandle_ptr:  dq import_GetStdHandle - $$
writefile_ptr:     dq import_WriteFile - $$
exitprocess_ptr:   dq import_ExitProcess - $$

code_section_end:

; ============================================================
; .data Section (imports)
; ============================================================
data_section_start:

; Align to file alignment
times ($$ + 0x400) - $ db 0        ; .data starts at 0x400 file offset
data_section_vaddr equ data_section_start - $$
data_section_raw_offset equ $ - $$

; Import Directory Table
import_directory:
    ; Import descriptor for kernel32.dll
    dd import_ilt - $$               ; ImportLookupTable RVA
    dd 0x00000000                   ; TimeDateStamp
    dd 0x00000000                   ; ForwarderChain
    dd kernel32_name - $$           ; Name RVA
    dd import_iat - $$              ; ImportAddressTable RVA
    ; Null terminator
    times 5 dd 0

import_directory_end:

; Import Lookup Table (ILT)
import_ilt:
    dq IMAGE_BASE + import_GetStdHandle_hintname - $$
    dq IMAGE_BASE + import_WriteFile_hintname - $$
    dq IMAGE_BASE + import_ExitProcess_hintname - $$
    dq 0                            ; null terminator

; Import Address Table (IAT) - same as ILT initially
import_iat:
    dq IMAGE_BASE + import_GetStdHandle_hintname - $$
    dq IMAGE_BASE + import_WriteFile_hintname - $$
    dq IMAGE_BASE + import_ExitProcess_hintname - $$
    dq 0                            ; null terminator

; Hint/Name Table
import_GetStdHandle_hintname:
    dw 0                            ; Hint
    db 'GetStdHandle', 0
    align 2, db 0

import_WriteFile_hintname:
    dw 0                            ; Hint
    db 'WriteFile', 0
    align 2, db 0

import_ExitProcess_hintname:
    dw 0                            ; Hint
    db 'ExitProcess', 0
    align 2, db 0

; DLL Name
kernel32_name:
    db 'kernel32.dll', 0

data_section_end:

; ============================================================
; Now we need to go back and patch the PE header placeholders
; We'll do this at assembly time using "dd target - $$" style
; But since NASM only goes forward, we use a trick:
; We define a second file that patches the first one.
; 
; ALTERNATIVELY: We use the `-f bin` output and patch with a tool.
; For now, let's just output a map file and manually patch.
; ============================================================

; Let's use the post-processing approach - write the values as comments
; and have a tool patch them.

; The actual patch values:
; headers_size   = 0x200 (actually 0x1C0 + padding)
; code_size      = code_section_end - code_section_start
; data_size      = data_section_end - data_section_start
; entry_rva      = entry_point - IMAGE_BASE
; import_rva     = import_directory - IMAGE_BASE
; import_size    = import_directory_end - import_directory
; image_size     = data_section_end - $$ (aligned to 0x1000)

; We'll build a patching script to fix the PE header
