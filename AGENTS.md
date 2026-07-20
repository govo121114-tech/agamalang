# AgamaLang — Agents Guide

Compiler for a mid-level C-like language (agamalang), written in Rust, generating x86-64 PE .exe for Windows directly (no assembler/linker).

## Build & run

```powershell
cargo build
cargo run -- examples\exit.aga          # → exit.exe
cargo run -- examples\hello.aga         # → hello.exe (uses print/println)
cargo run -- examples\file.aga out.exe  # custom output name
```

Output `.exe` lands in current directory (not examples/). No tests yet.

## Source architecture (7 files, single crate)

```
main.rs      — orchestrates phases: read → lex → parse → codegen → pe_write
token.rs     — TokenKind enum
lexer.rs     — Lexer::tokenize() → Vec<Token>
ast.rs       — Program → Function → Stmt / Expr
parser.rs    — Parser::parse() → Program, recursive-descent, Newline as statement separator
codegen.rs   — Generator → CompiledUnit (raw bytes + relocation metadata)
pe.rs        — write_pe() → .exe file (no external linker needed)
pedump/      — separate utility (cargo run -- <file>) for PE hex inspection
```

## Language surface (parsed & codegen'd)

- `fn main() { ... }` — entry point; **must be named `main`**, return type always void
- `let x: int = 42;` — variables stored at [rbp+offset], 4-byte slots
- `if (cond) { ... } else { ... }` — condition MUST be in parens
- `while (cond) { ... }`, `for (init; cond; post) { ... }` — all work
- `break;` / `continue;` — inside loops
- `return [expr];` — early return; in `main` automatically calls `ExitProcess`
- `print("str")` / `println("str")` — Win32 `GetStdHandle` + `WriteFile`
- `struct Name { field: type; ... }` — struct declaration (new in Phase 1)
- `let x: StructName = StructName { field: expr; field: expr; };` — struct init (new)
- `expr.field` — struct member access (read/write, new in Phase 1)
- `expr[index]` — array indexing (parsed + codegen'd)
- `[expr, expr, ...]` — array literals (new)
- `sizeof(type)` — compile-time size (new)
- Types: `int`, `char`, `bool`, `void`, `*T`, `T[]`, `T[N]`, struct names
- Escape sequences in strings: `\n`, `\r`, `\t`, `\\`, `\"`, `\0`
- Operators: `+`, `-`, `*`, `/`, `%`, `==`, `!=`, `<`, `>`, `<=`, `>=`, `&&`, `||`, `!`, `&`, `*`

## PE layout

- ImageBase = `0x400000` (MSVC default)
- 3 sections: `.text` (RVA=0x1000), `.data` (RVA=0x2000), `.idata` (RVA=0x3000)
- Import data built manually (no linker): ILT → IAT → hint/name entries → DLL name
- IAT is patched at runtime by Windows loader; initially contains hint/name RVAs
- All DLL calls use `call [rip+offset_to_IAT]` via `FF 15` encoding

## Register map (Win64 ABI)

| reg | code | purpose |
|-----|------|---------|
| RAX | 0    | return value / scratch |
| RCX | 1    | arg 1 / scratch |
| RDX | 2    | arg 2 / scratch |
| RBX | 3    | *(not used yet)* |
| RSP | 4    | stack pointer |
| RBP | 5    | frame pointer |
| RSI | 6    | *(not used)* |
| RDI | 7    | *(not used)* |
| R8  | 8    | arg 3 / scratch |
| R9  | 9    | arg 4 / scratch |

## Critical gotchas (hard-won)

1. **SIB byte mandatory for `[rsp+offset]`** — any x86-64 instruction with ModRM.rm=4 (RSP) MUST be followed by SIB byte `0x24`. `encode_addr()` in `codegen.rs` handles this; do not add special-case ModRM skipping for RSP.

2. **Lexer `read_string()` must NOT `advance()` past opening `"`** — `next_token()` already consumed the opening quote via its own `advance()`. Calling `advance()` again in `read_string()` skips the first character of the string content. Same applies to `read_char()` and `'`.

3. **PE import RVAs must be absolute** — ILT, IAT, Name, and all IAT/ILT entries in `.idata` section must contain **full RVAs** (`ida_va + offset`), not section-relative offsets. The loader interprets them as absolute addresses.

4. **16-byte stack alignment before CALL** — Win64 ABI requires RSP ≡ 0 (mod 16) before `call`. After `push rbp` (8 bytes), RSP ≡ 8, so allocate an extra 8-byte-odd amount (e.g., `sub rsp, 0x28` or `sub rsp, 0x38`) before calling any Win32 API.

5. **`call [ExitProcess]` at end of `main`** — the function epilogue for `main` emits `xor rcx, rcx; call [ExitProcess]; mov rsp, rbp; pop rbp; ret`. ExitProcess does not return, but `mov rsp, rbp; pop rbp; ret` is still emitted (dead code, but harmless).

6. **`.` (dot) is current directory** — `cargo run -- examples\hello.aga` writes `hello.exe` to the current directory (project root), NOT to `examples/`.

7. **`Member` codegen must use ADDRESS, not VALUE** — `expr.field` first computes the ADDRESS of `expr` via `compile_expr_addr()` (which emits `lea`), then loads/stores at `[addr + field_offset]`. Using `compile_expr_to()` would load the first bytes of the struct as a pointer and dereference it → crash.

8. **Field offsets computed sequentially from `type_size()`** — each field occupies `type_size` bytes (currently 4 for all scalar types). Struct layout is computed in `compile()` → `struct_layouts: HashMap<String, StructLayout>`. For runtime `sizeof()`, the codegen looks up `struct_size()` from these layouts.

## Expected output (smoke test)

```powershell
cargo run -- examples\exit.aga
.\exit.exe; echo $LASTEXITCODE
# → 0

cargo run -- examples\hello.aga
.\hello.exe
# → Hello from AgamaLang!
# → This is a mid-level language.
# → It compiles directly to machine code!
```

## Warnings (intentional)

Compiler emits ~10 warnings about unused fields/variables (`column`, `var_type`, `return_type`, `imports`, etc.). These are leftover from incomplete features (structs, enums, member access) and can be ignored for now.

## File layout

```
agamalang/
├── src/main.rs          # CLI entry
├── src/token.rs         # TokenKind enum
├── src/lexer.rs         # tokenize()
├── src/ast.rs           # AST nodes
├── src/parser.rs        # recursive-descent parser
├── src/codegen.rs       # x86-64 code generator
├── src/pe.rs            # PE .exe writer
├── examples/            # .aga test files
├── pedump/              # standalone PE dump tool
└── Cargo.toml           # no dependencies
```
