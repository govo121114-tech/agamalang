//! Code generator - walking AST and emitting x86-64 machine code.
//! Returns raw code bytes and symbol information for the PE writer.

use crate::ast::*;
use std::collections::HashMap;

/// Request to import a function
#[derive(Clone)]
pub struct ImportRequest {
    pub name: String,
    pub dll: String,
}

pub struct CompiledUnit {
    pub code: Vec<u8>,
    pub entry_point: u32,
    pub strings: Vec<u8>,
    pub string_relocs: Vec<(u32, u32)>, // (code_offset, string_index)
    pub imports: Vec<ImportRequest>,     // function names to import
    pub import_relocs: Vec<(u32, String)>, // (code_offset, function_name)
}

/// Compute the size of a type for layout (4 for int/char/bool, 8 for pointers)
fn type_size(ty: &Type, layouts: &HashMap<String, StructLayout>) -> i32 {
    match ty {
        Type::Int | Type::Char | Type::Bool => 4,
        Type::UpInt | Type::UnInt | Type::Fixed => 8,
        Type::Ptr(_) => 8,
        Type::Array(_) => 8,
        Type::Void => 0,
        Type::StaticArray(elem, n) => type_size(elem, layouts) * (*n as i32),
        Type::Named(name) => layouts.get(name).map(|l| l.size).unwrap_or(4),
    }
}

/// Get actual struct size from layout, or default to 4
fn struct_size(name: &str, layouts: &HashMap<String, StructLayout>) -> i32 {
    layouts.get(name).map(|l| l.size).unwrap_or(4)
}

/// Get field offset for a struct
fn field_offset(name: &str, field: &str, layouts: &HashMap<String, StructLayout>) -> Option<i32> {
    layouts.get(name)
        .and_then(|l| l.fields.iter().find(|f| f.name == field))
        .map(|f| f.offset)
}

fn field_size(name: &str, field: &str, layouts: &HashMap<String, StructLayout>) -> Option<i32> {
    layouts.get(name)
        .and_then(|l| l.fields.iter().find(|f| f.name == field))
        .map(|f| f.size)
}

/// Get field size from a Member expression for correct-width store
fn get_member_field_size(
    var_types: &HashMap<String, Type>,
    layouts: &HashMap<String, StructLayout>,
    object: &Expr,
    member: &str,
) -> i32 {
    if let Some(sname) = resolve_struct_name(object, var_types, layouts) {
        return field_size(sname, member, layouts).unwrap_or(8);
    }
    8
}

/// Resolve the struct name from an expression, looking up through var_types for
/// Identifiers (including pointer-to-struct) and through struct field types for Member chains.
fn resolve_struct_name<'a>(expr: &Expr, var_types: &'a HashMap<String, Type>, layouts: &'a HashMap<String, StructLayout>) -> Option<&'a str> {
    match expr {
        Expr::Identifier(name) => {
            match var_types.get(name) {
                Some(Type::Named(n)) => return Some(n.as_str()),
                Some(Type::Ptr(inner)) => {
                    if let Type::Named(n) = inner.as_ref() {
                        return Some(n.as_str());
                    }
                }
                _ => {}
            }
            None
        }
        Expr::Member { object, member } => {
            let parent = resolve_struct_name(object, var_types, layouts)?;
            let layout = layouts.get(parent)?;
            let field = layout.fields.iter().find(|f| f.name == *member)?;
            if let Type::Named(ref n) = field.field_type {
                Some(n.as_str())
            } else {
                None
            }
        }
        Expr::Index { object, .. } => {
            if let Expr::Identifier(oname) = object.as_ref() {
                if let Some(Type::Ptr(inner)) = var_types.get(oname) {
                    if let Type::Named(n) = inner.as_ref() {
                        return Some(n.as_str());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Get the element size for a pointer/array expression (1 for *char, struct_size for *Struct, 4 for *int, etc.)
fn get_ptr_elem_size(expr: &Expr, var_types: &HashMap<String, Type>, layouts: &HashMap<String, StructLayout>) -> i32 {
    if let Expr::Identifier(name) = expr {
        match var_types.get(name) {
            Some(Type::Ptr(inner)) => match inner.as_ref() {
                Type::Char => 1,
                Type::Named(n) => struct_size(n, layouts),
                _ => 4,
            },
            Some(Type::Array(inner)) | Some(Type::StaticArray(inner, _)) => type_size(inner, layouts),
            _ => 4,
        }
    } else { 4 }
}

/// Compile a program and return the compiled unit
pub fn compile(program: &Program) -> CompiledUnit {
    let mut gen = Generator::new();
        // Build struct layouts
        for sd in &program.structs {
            let mut fields = Vec::new();
            let mut offset: i32 = 0;
            for (fname, ftype) in &sd.fields {
                let fsz = type_size(&ftype, &gen.struct_layouts);
                fields.push(StructField {
                    name: fname.clone(),
                    offset,
                    size: fsz,
                    field_type: ftype.clone(),
                });
                offset += fsz;
            }
            gen.struct_layouts.insert(sd.name.clone(), StructLayout {
                size: offset,
                fields,
            });
        }
    for imp in &program.imports {
        if imp == "math" {
            gen.has_math = true;
        }
        if imp == "gdi" {
            gen.has_gdi = true;
        }
    }
    if gen.has_gdi {
        gen.register_gdi_imports();
    }
    gen.compile_program(program);
    gen.finalize()
}

#[derive(Clone)]
struct StructField {
    name: String,
    offset: i32,
    size: i32,
    field_type: Type,
}

#[derive(Clone)]
struct StructLayout {
    size: i32,
    fields: Vec<StructField>,
}

struct Generator {
    asm: Vec<u8>,
    functions: HashMap<String, u32>,
    struct_layouts: HashMap<String, StructLayout>,
    vars: HashMap<String, i32>,
    var_types: HashMap<String, Type>,
    var_struct_fields: HashMap<String, Vec<(i32, Type)>>, // for StructInit vars
    stack_size: u32,
    strings: Vec<u8>,
    string_map: HashMap<String, u32>,
    string_relocs: Vec<(u32, u32)>,
    imports: Vec<ImportRequest>,
    import_relocs: Vec<(u32, String)>,
    labels: HashMap<u32, u32>,
    label_fixups: Vec<(u32, u32, bool)>, // (offset, label_id, is_32bit)
    label_counter: u32,
    current_fn: String,
    loop_break_labels: Vec<u32>,  // stack of break-target labels
    loop_continue_labels: Vec<u32>, // stack of continue-target labels
    has_math: bool,
    has_gdi: bool,
}

impl Generator {
    fn new() -> Self {
        Generator {
            asm: Vec::new(),
            functions: HashMap::new(),
            struct_layouts: HashMap::new(),
            vars: HashMap::new(),
            var_types: HashMap::new(),
            var_struct_fields: HashMap::new(),
            stack_size: 0,
            strings: Vec::new(),
            string_map: HashMap::new(),
            string_relocs: Vec::new(),
            imports: Vec::new(),
            import_relocs: Vec::new(),
            labels: HashMap::new(),
            label_fixups: Vec::new(),
            label_counter: 0,
            current_fn: String::new(),
            loop_break_labels: Vec::new(),
            loop_continue_labels: Vec::new(),
            has_math: false,
            has_gdi: false,
        }
    }

    fn new_label(&mut self) -> u32 {
        let l = self.label_counter;
        self.label_counter += 1;
        l
    }

    fn put_label(&mut self, label: u32) {
        self.labels.insert(label, self.asm.len() as u32);
    }

    fn add_label_fixup(&mut self, offset: u32, label: u32, is_32bit: bool) {
        self.label_fixups.push((offset, label, is_32bit));
    }

    fn resolve_labels(&mut self) {
        let fixups = std::mem::take(&mut self.label_fixups);
        for (offset, label, is_32bit) in fixups {
            if let Some(&target) = self.labels.get(&label) {
                let current = if is_32bit { offset + 4 } else { offset + 1 };
                let rel = target as i64 - current as i64;
                if is_32bit {
                    self.asm[offset as usize..offset as usize + 4].copy_from_slice(&(rel as i32).to_le_bytes());
                } else {
                    let rel_byte = rel as i8;
                    self.asm[offset as usize] = rel_byte as u8;
                }
            }
        }
    }

    fn intern_string(&mut self, s: &str) -> u32 {
        if let Some(&off) = self.string_map.get(s) { return off; }
        let off = self.strings.len() as u32;
        self.strings.extend_from_slice(s.as_bytes());
        self.strings.push(0);
        self.string_map.insert(s.to_string(), off);
        off
    }

    fn u8(&mut self, v: u8) { self.asm.push(v); }
    fn u16(&mut self, v: u16) { self.asm.extend_from_slice(&v.to_le_bytes()); }
    fn u32(&mut self, v: u32) { self.asm.extend_from_slice(&v.to_le_bytes()); }
    fn u64(&mut self, v: u64) { self.asm.extend_from_slice(&v.to_le_bytes()); }
    fn i32(&mut self, v: i32) { self.asm.extend_from_slice(&v.to_le_bytes()); }

    fn rex(&mut self, w: bool, r: bool, x: bool, b: bool) {
        let mut v: u8 = 0x40;
        if w { v |= 0x08; } if r { v |= 0x04; }
        if x { v |= 0x02; } if b { v |= 0x01; }
        if v != 0x40 { self.u8(v); }
    }

    fn modrm(&mut self, mod_: u8, reg: u8, rm: u8) {
        self.u8((mod_ << 6) | ((reg & 7) << 3) | (rm & 7));
    }

    fn encode_addr(&mut self, reg: u8, base: u8, off: i32) {
        // x86-64: when ModRM.rm = 4 (RSP/SPL), a SIB byte MUST follow
        // SIB 0x24 = scale=00, index=100 (none), base=100 (RSP) = [RSP + disp]
        let needs_sib = (base & 7) == 4;

        if off == 0 && (base & 7) != 5 {
            self.modrm(0, reg, base & 7);
            if needs_sib { self.u8(0x24); }
        } else if off >= -128 && off <= 127 {
            self.modrm(1, reg, base & 7);
            if needs_sib { self.u8(0x24); }
            self.u8(off as u8);
        } else {
            self.modrm(2, reg, base & 7);
            if needs_sib { self.u8(0x24); }
            self.i32(off);
        }
    }

    fn mov_r64_imm32(&mut self, r: u8, imm: u32) {
        if r >= 8 { self.rex(false, false, false, true); }
        self.u8(0xB8 + (r & 7));
        self.u32(imm);
    }

    fn mov_r64_imm64(&mut self, r: u8, imm: u64) {
        self.rex(true, false, false, r >= 8);
        self.u8(0xB8 + (r & 7));
        self.u64(imm);
    }

    fn mov_r64_m64(&mut self, dst: u8, base: u8, off: i32) {
        self.rex(true, dst >= 8, false, base >= 8);
        self.u8(0x8B);
        self.encode_addr(dst, base, off);
    }

    fn mov_r64_m32(&mut self, dst: u8, base: u8, off: i32) {
        // Sign-extend 32-bit load (movsxd): `int` is signed, comparisons must be correct
        self.rex(true, dst >= 8, false, base >= 8);
        self.u8(0x63);
        self.encode_addr(dst, base, off);
    }

    fn mov_m64_r64(&mut self, base: u8, off: i32, src: u8) {
        self.rex(true, src >= 8, false, base >= 8);
        self.u8(0x89);
        self.encode_addr(src, base, off);
    }

    fn mov_m32_r64(&mut self, base: u8, off: i32, src: u8) {
        // 32-bit store: mov [base+off], src32 (no REX.W)
        self.rex(false, src >= 8, false, base >= 8);
        self.u8(0x89);
        self.encode_addr(src, base, off);
    }

    fn mov_m64_imm32(&mut self, base: u8, off: i32, imm: i32) {
        self.rex(true, false, false, base >= 8);
        self.u8(0xC7);
        self.encode_addr(0, base, off);
        self.i32(imm);
    }

    fn mov_r64_r64(&mut self, dst: u8, src: u8) {
        self.rex(true, src >= 8, false, dst >= 8);
        self.u8(0x89);
        self.modrm(3, src & 7, dst & 7);
    }

    fn movzx_r64(&mut self, dst: u8, base: u8, off: i32) {
        self.rex(true, dst >= 8, false, base >= 8);
        self.u8(0x0F); self.u8(0xB6);
        self.encode_addr(dst, base, off);
    }

    fn lea_r64(&mut self, dst: u8, base: u8, off: i32) {
        self.rex(true, dst >= 8, false, base >= 8);
        self.u8(0x8D);
        self.encode_addr(dst, base, off);
    }

    fn lea_r64_rip(&mut self, dst: u8) {
        // Emit lea reg, [rip+0] - displacement will be patched later
        self.rex(true, dst >= 8, false, false);
        self.u8(0x8D);
        self.modrm(0, dst & 7, 5); // mod=00, rm=101 = RIP-relative
        self.i32(0); // placeholder displacement
    }

    fn add_r64(&mut self, dst: u8, src: u8) {
        self.rex(true, src >= 8, false, dst >= 8);
        self.u8(0x01); self.modrm(3, src & 7, dst & 7);
    }

    fn sub_r64(&mut self, dst: u8, src: u8) {
        self.rex(true, src >= 8, false, dst >= 8);
        self.u8(0x29); self.modrm(3, src & 7, dst & 7);
    }

    fn sub_rm64_imm8(&mut self, rm: u8, imm: i8) {
        self.rex(true, false, false, rm >= 8);
        self.u8(0x83); self.modrm(3, 5, rm & 7);
        self.u8(imm as u8);
    }

    fn add_rm64_imm8(&mut self, rm: u8, imm: i8) {
        self.rex(true, false, false, rm >= 8);
        self.u8(0x83); self.modrm(3, 0, rm & 7);
        self.u8(imm as u8);
    }

    fn cmp_r64(&mut self, a: u8, b: u8) {
        self.rex(true, b >= 8, false, a >= 8);
        self.u8(0x39); self.modrm(3, b & 7, a & 7);
    }

    fn cmp_r64_imm32(&mut self, r: u8, imm: i32) {
        if imm >= -128 && imm <= 127 {
            self.rex(true, false, false, r >= 8);
            self.u8(0x83); self.modrm(3, 7, r & 7);
            self.u8(imm as u8);
        } else {
            self.rex(true, false, false, r >= 8);
            self.u8(0x81); self.modrm(3, 7, r & 7);
            self.i32(imm);
        }
    }

    fn xor_r64(&mut self, dst: u8, src: u8) {
        self.rex(true, src >= 8, false, dst >= 8);
        self.u8(0x31); self.modrm(3, src & 7, dst & 7);
    }

    fn imul_r64(&mut self, dst: u8, src: u8) {
        self.rex(true, dst >= 8, false, src >= 8);
        self.u8(0x0F); self.u8(0xAF);
        self.modrm(3, dst & 7, src & 7);
    }

    fn cqo(&mut self) { self.rex(true, false, false, false); self.u8(0x99); }

    fn idiv_r64(&mut self, d: u8) {
        self.rex(true, false, false, d >= 8);
        self.u8(0xF7); self.modrm(3, 7, d & 7);
    }

    fn neg_r64(&mut self) {
        self.rex(true, false, false, false);
        self.u8(0xF7); self.modrm(3, 3, 0);
    }

    fn push_r64(&mut self, r: u8) {
        if r >= 8 { self.rex(false, false, false, true); }
        self.u8(0x50 + (r & 7));
    }

    fn pop_r64(&mut self, r: u8) {
        if r >= 8 { self.rex(false, false, false, true); }
        self.u8(0x58 + (r & 7));
    }

    fn ret(&mut self) { self.u8(0xC3); }
    fn call_rel32(&mut self) { self.u8(0xE8); self.i32(0); } // placeholder
    fn jmp_rel32(&mut self) { self.u8(0xE9); self.i32(0); } // placeholder

    fn jcc_rel32(&mut self, cc: u8) {
        self.u8(0x0F); self.u8(cc); self.i32(0);
    }

    fn setcc_r8(&mut self, cc: u8, r: u8) {
        if r >= 8 { self.rex(false, false, false, true); }
        self.u8(0x0F); self.u8(0x90 + (cc & 0x0F));
        self.modrm(3, 0, r & 7);
    }

    /// movzx r32, r8 — zero-extend a byte register to 32-bit (which also zero-extends to 64-bit)
    fn movzx_r32_r8(&mut self, dst: u8, src: u8) {
        if dst >= 8 || src >= 8 { self.rex(false, dst >= 8, false, src >= 8); }
        self.u8(0x0F); self.u8(0xB6);
        self.modrm(3, src & 7, dst & 7);
    }

    fn and_r8_r8(&mut self, dst: u8, src: u8) {
        if dst >= 8 || src >= 8 { self.rex(false, src >= 8, false, dst >= 8); }
        self.u8(0x20); self.modrm(3, src & 7, dst & 7);
    }

    fn or_r8_r8(&mut self, dst: u8, src: u8) {
        if dst >= 8 || src >= 8 { self.rex(false, src >= 8, false, dst >= 8); }
        self.u8(0x08); self.modrm(3, src & 7, dst & 7);
    }

    /// call [rip+disp32] - for import calls
    fn call_rip_placeholder(&mut self) {
        self.u8(0xFF); self.u8(0x15); self.i32(0);
    }

    fn compile_program(&mut self, program: &Program) {
        // Collect function offsets
        for func in &program.functions {
            self.functions.insert(func.name.clone(), 0);
        }

        // Compile each function
        for func in &program.functions {
            self.compile_function(func);
        }

        // Resolve label fixups
        self.resolve_labels();
    }

    fn compile_function(&mut self, func: &Function) {
        let fn_offset = self.asm.len() as u32;
        *self.functions.get_mut(&func.name).unwrap() = fn_offset;
        self.current_fn = func.name.clone();
        self.vars.clear();
        self.var_types.clear();
        self.var_struct_fields.clear();
        self.stack_size = 8;
        self.loop_break_labels.clear();
        self.loop_continue_labels.clear();

        // Prologue
        self.push_r64(5);           // push rbp
        self.mov_r64_r64(5, 4);     // mov rbp, rsp

        // Store parameters
        let param_regs = [1, 2, 8, 9]; // RCX, RDX, R8, R9
        for (i, p) in func.params.iter().enumerate() {
            let psz = type_size(&p.param_type, &self.struct_layouts).max(4) as u32;
            self.stack_size += psz.max(8); // minimum 8-byte slot to avoid overlap
            let off = -(self.stack_size as i32);
            self.vars.insert(p.name.clone(), off);
            self.var_types.insert(p.name.clone(), p.param_type.clone());
            if i < 4 {
                if psz == 4 {
                    self.mov_m32_r64(5, off, param_regs[i]);
                } else {
                    self.mov_m64_r64(5, off, param_regs[i]);
                }
            }
        }

        // Allocate stack space for variables — compute from max needed
        // Must be multiple of 16 to keep RSP ≡ 0 (mod 16) after push rbp
        let max_var_stack: u32 = {
            let mut sz = 8u32;
            fn count_vardecls(stmts: &[Stmt], layouts: &HashMap<String, StructLayout>, sz: &mut u32) {
                for stmt in stmts {
                    match stmt {
                        Stmt::VariableDecl { var_type, .. } => {
                            let vs = match var_type {
                                Some(Type::Named(n)) => struct_size(n, layouts).max(4) as u32,
                            Some(ty) => type_size(ty, layouts).max(4) as u32,
                            None => 4,
                        };
                        *sz += vs.max(8);
                    }
                    Stmt::Block(stmts) => count_vardecls(stmts, layouts, sz),
                        Stmt::If { then_branch, else_branch, .. } => {
                            count_vardecls(then_branch, layouts, sz);
                            if let Some(stmts) = else_branch { count_vardecls(stmts, layouts, sz); }
                        }
                        Stmt::While { body, .. } => count_vardecls(body, layouts, sz),
                        Stmt::For { init, body, .. } => {
                            count_vardecls(std::slice::from_ref(init.as_ref()), layouts, sz);
                            count_vardecls(body, layouts, sz);
                        }
                        _ => {}
                    }
                }
            }
            count_vardecls(&func.body, &self.struct_layouts, &mut sz);
            // Use full sz to cover the last variable's complete extent
            let needed = sz;
            ((needed + 15) & !15).max(0x20)
        };
        if max_var_stack > 0 {
            self.sub_rm64_imm8(4, max_var_stack as i8);
        }

        // Body
        for stmt in &func.body { self.compile_stmt(stmt); }

        // Epilogue
        if func.name == "main" {
            self.xor_r64(1, 1); // RCX = 0
            self.emit_import_call("ExitProcess");
        }
        self.mov_r64_r64(4, 5);
        self.pop_r64(5);
        self.ret();
    }

    /// Compute address of an lvalue expression into a register
    fn compile_expr_addr(&mut self, expr: &Expr, reg: u8) {
        match expr {
            Expr::Identifier(name) => {
                if let Some(&off) = self.vars.get(name) {
                    if reg == 5 {
                        // Special case: lea reg, [rbp+off] can't use rbp as target
                        self.lea_r64(reg, 5, off);
                    } else {
                        self.lea_r64(reg, 5, off);
                    }
                }
            }
            Expr::Member { object, member } => {
                // If object is a pointer to struct, load the pointer value first
                let is_ptr = matches!(object.as_ref(), Expr::Identifier(oname)
                    if self.var_types.get(oname).map_or(false, |t| matches!(t, Type::Ptr(_))));
                if is_ptr {
                    self.compile_expr_to(object, reg);
                } else {
                    self.compile_expr_addr(object, reg);
                }
                if let Some(sname) = resolve_struct_name(object, &self.var_types, &self.struct_layouts) {
                    if let Some(foff) = field_offset(sname, member, &self.struct_layouts) {
                        if foff != 0 { self.add_rm64_imm8(reg, foff as i8); }
                    }
                }
            }
            Expr::Index { object, index } => {
                let is_ptr = matches!(object.as_ref(), Expr::Identifier(oname)
                    if self.var_types.get(oname).map_or(false, |t| matches!(t, Type::Ptr(_))));
                let elem_size = get_ptr_elem_size(object, &self.var_types, &self.struct_layouts);
                if is_ptr {
                    self.compile_expr_to(object, 0);
                } else {
                    self.compile_expr_addr(object, 0);
                }
                self.push_r64(0);
                self.compile_expr_to(index, 1);
                self.pop_r64(0);
                if elem_size != 1 {
                    self.mov_r64_imm32(2, elem_size as u32);
                    self.imul_r64(1, 2);
                }
                self.add_r64(0, 1);
                if reg != 0 { self.mov_r64_r64(reg, 0); }
            }
            Expr::Unary { op: UnOp::Deref, operand } => {
                self.compile_expr_to(operand, reg);
            }
            _ => {}
        }
    }

    fn compile_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Block(stmts) => { for s in stmts { self.compile_stmt(s); } }
            Stmt::VariableDecl { name, var_type, init } => {
                let var_sz = match var_type {
                    Some(Type::Named(n)) => struct_size(n, &self.struct_layouts).max(4),
                    Some(ty) => type_size(ty, &self.struct_layouts).max(4),
                    None => 4,
                };
                self.stack_size += (var_sz as u32).max(8); // minimum 8-byte slot
                let off = -(self.stack_size as i32);
                self.vars.insert(name.clone(), off);
                if let Some(ty) = var_type {
                    self.var_types.insert(name.clone(), ty.clone());
                }
                if let Some(e) = init {
                    // Handle struct init specially: store fields directly
                    if let Expr::StructInit { type_name: _, fields } = e {
                        for (fname, fval) in fields {
                            self.compile_expr_to(fval, 0);
                            if let Some(ty) = var_type {
                                if let Type::Named(struct_name) = ty {
                                    if let Some(fsz) = field_size(struct_name, fname, &self.struct_layouts) {
                                        if let Some(foff) = field_offset(struct_name, fname, &self.struct_layouts) {
                                            if fsz == 4 {
                                                self.mov_m32_r64(5, off + foff, 0);
                                            } else {
                                                self.mov_m64_r64(5, off + foff, 0);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else if let Expr::ArrayInit(elems) = e {
                        // Inline array init: store each element at the variable's stack offset
                        for (i, elem) in elems.iter().enumerate() {
                            self.compile_expr_to(elem, 0);
                            self.mov_m32_r64(5, off + i as i32 * 4, 0);
                        }
                    } else {
                        self.compile_expr_to(e, 0);
                        // For unint, negate the value (unint always stores negative)
                        if matches!(var_type, Some(Type::UnInt)) {
                            self.neg_r64();
                        }
                        // Use appropriate store width based on type
                        let is_4byte = match var_type {
                            Some(Type::Int | Type::Char | Type::Bool) => true,
                            _ => false,
                        };
                        if is_4byte {
                            self.mov_m32_r64(5, off, 0);
                        } else {
                            self.mov_m64_r64(5, off, 0);
                        }
                    }
                }
            }
            Stmt::Return { value } => {
                if let Some(e) = value {
                    self.compile_expr_to(e, 0);
                }
                if self.current_fn == "main" {
                    if value.is_some() {
                        self.mov_r64_r64(1, 0);
                    } else {
                        self.xor_r64(1, 1);
                    }
                    self.emit_import_call("ExitProcess");
                }
                self.mov_r64_r64(4, 5); self.pop_r64(5); self.ret();
            }
            Stmt::Expr(e) => { self.compile_expr_to(e, 0); }
            Stmt::If { condition, then_branch, else_branch } => {
                let else_lbl = self.new_label();
                let end_lbl = self.new_label();
                self.compile_expr_to(condition, 0);
                self.cmp_r64_imm32(0, 0);
                self.jcc_rel32(CC_E);
                self.add_label_fixup(self.asm.len() as u32 - 4, else_lbl, true);
                for s in then_branch { self.compile_stmt(s); }
                if else_branch.is_some() {
                    self.jmp_rel32();
                    self.add_label_fixup(self.asm.len() as u32 - 4, end_lbl, true);
                }
                self.put_label(else_lbl);
                if let Some(stmts) = else_branch { for s in stmts { self.compile_stmt(s); } }
                self.put_label(end_lbl);
            }
            Stmt::While { condition, body } => {
                let loop_lbl = self.new_label();
                let end_lbl = self.new_label();
                self.loop_continue_labels.push(loop_lbl);
                self.loop_break_labels.push(end_lbl);
                self.put_label(loop_lbl);
                self.compile_expr_to(condition, 0);
                self.cmp_r64_imm32(0, 0);
                self.jcc_rel32(CC_E);
                self.add_label_fixup(self.asm.len() as u32 - 4, end_lbl, true);
                for s in body { self.compile_stmt(s); }
                self.jmp_rel32();
                self.add_label_fixup(self.asm.len() as u32 - 4, loop_lbl, true);
                self.put_label(end_lbl);
                self.loop_continue_labels.pop();
                self.loop_break_labels.pop();
            }
            Stmt::For { init, condition, post, body } => {
                self.compile_stmt(init);
                let loop_lbl = self.new_label();
                let continue_lbl = self.new_label();
                let end_lbl = self.new_label();
                self.loop_continue_labels.push(continue_lbl);
                self.loop_break_labels.push(end_lbl);
                self.put_label(loop_lbl);
                if let Some(c) = condition {
                    self.compile_expr_to(c, 0);
                    self.cmp_r64_imm32(0, 0);
                    self.jcc_rel32(CC_E);
                    self.add_label_fixup(self.asm.len() as u32 - 4, end_lbl, true);
                }
                for s in body { self.compile_stmt(s); }
                self.put_label(continue_lbl);
                if let Some(p) = post { self.compile_expr_to(p, 0); }
                self.jmp_rel32();
                self.add_label_fixup(self.asm.len() as u32 - 4, loop_lbl, true);
                self.put_label(end_lbl);
                self.loop_continue_labels.pop();
                self.loop_break_labels.pop();
            }
            Stmt::Break => {
                if let Some(&end_lbl) = self.loop_break_labels.last() {
                    self.jmp_rel32();
                    self.add_label_fixup(self.asm.len() as u32 - 4, end_lbl, true);
                }
            }
            Stmt::Continue => {
                if let Some(&start_lbl) = self.loop_continue_labels.last() {
                    self.jmp_rel32();
                    self.add_label_fixup(self.asm.len() as u32 - 4, start_lbl, true);
                }
            }
        }
    }

    fn compile_expr_to(&mut self, expr: &Expr, reg: u8) {
        match expr {
            Expr::Integer(n) => {
                if *n >= 0 && *n <= 0xFFFF_FFFF {
                    self.mov_r64_imm32(reg, *n as u32);
                } else {
                    self.mov_r64_imm64(reg, *n as u64);
                }
            }
            Expr::Fixed(n) => {
                self.mov_r64_imm64(reg, *n as u64);
            }
            Expr::Bool(b) => { self.mov_r64_imm32(reg, if *b { 1 } else { 0 }); }
            Expr::Char(c) => { self.mov_r64_imm32(reg, *c as u32); }
            Expr::Null => { self.xor_r64(reg, reg); }
            Expr::String(s) => {
                let str_idx = self.intern_string(s);
                let offset = self.asm.len() as u32;
                self.lea_r64_rip(reg);
                self.string_relocs.push((offset, str_idx));
            }
            Expr::Identifier(name) => {
                if let Some(&off) = self.vars.get(name) {
                    let is_4byte = match self.var_types.get(name) {
                        Some(Type::Int | Type::Char | Type::Bool) => true,
                        _ => false,
                    };
                    if is_4byte {
                        self.mov_r64_m32(reg, 5, off);
                    } else {
                        self.mov_r64_m64(reg, 5, off);
                    }
                }
            }
            Expr::Binary { op, left, right } => {
                use crate::ast::BinOp::*;
                self.compile_expr_to(left, 0);
                self.push_r64(0);
                self.compile_expr_to(right, 0);
                self.mov_r64_r64(1, 0);
                self.pop_r64(0);

                match op {
                    Add => self.add_r64(0, 1),
                    Sub => self.sub_r64(0, 1),
                    Mul => self.imul_r64(0, 1),
                    Div => { self.cqo(); self.idiv_r64(1); }
                    Mod => { self.cqo(); self.idiv_r64(1); self.mov_r64_r64(0, 2); }
                    Equal | NotEqual | Less | Greater | LessEqual | GreaterEqual => {
                        self.cmp_r64(0, 1);
                        let cc = cc_from_binop(op);
                        self.setcc_r8(cc, 0);
                        self.movzx_r32_r8(0, 0); // zero-extend result (setcc only sets low byte)
                    }
                    And => {
                        self.cmp_r64_imm32(0, 0); self.setcc_r8(CC_NE, 0);
                        self.movzx_r32_r8(0, 0);
                        self.cmp_r64_imm32(1, 0); self.setcc_r8(CC_NE, 1);
                        self.movzx_r32_r8(1, 1);
                        self.and_r8_r8(0, 1);
                    }
                    Or => {
                        self.cmp_r64_imm32(0, 0); self.setcc_r8(CC_NE, 0);
                        self.movzx_r32_r8(0, 0);
                        self.cmp_r64_imm32(1, 0); self.setcc_r8(CC_NE, 1);
                        self.movzx_r32_r8(1, 1);
                        self.or_r8_r8(0, 1);
                    }
                }
                // Copy result to requested register if needed
                if reg != 0 && reg != 1 {
                    self.mov_r64_r64(reg, 0);
                }
            }
            Expr::Unary { op, operand } => {
                match op {
                    UnOp::Negate => { self.compile_expr_to(operand, 0); self.neg_r64(); }
                    UnOp::Not => {
                        self.compile_expr_to(operand, 0);
                        self.cmp_r64_imm32(0, 0);
                        self.setcc_r8(CC_E, 0);
                        self.movzx_r32_r8(0, 0);
                    }
                    UnOp::AddrOf => {
                        self.compile_expr_addr(operand, reg);
                    }
                    UnOp::Deref => {
                        self.compile_expr_to(operand, 0);
                        let elem_size = if let Expr::Identifier(oname) = operand.as_ref() {
                            if let Some(Type::Ptr(inner)) = self.var_types.get(oname) {
                                type_size(inner, &self.struct_layouts)
                            } else { 8 }
                        } else { 8 };
                        if elem_size == 1 {
                            self.movzx_r64(reg, 0, 0);
                        } else if elem_size == 4 {
                            self.mov_r64_m32(reg, 0, 0);
                        } else {
                            self.mov_r64_m64(reg, 0, 0);
                        }
                    }
                }
            }
            Expr::Call { callee, args } => {
                self.compile_call(callee, args);
                if reg != 0 { self.mov_r64_r64(reg, 0); }
            }
            Expr::Assign { target, value } => {
                self.compile_expr_to(value, 0);
                match target.as_ref() {
                    Expr::Identifier(name) => {
                        if let Some(&off) = self.vars.get(name) {
                            let is_4byte = match self.var_types.get(name) {
                                Some(Type::Int | Type::Char | Type::Bool) => true,
                                _ => false,
                            };
                            if is_4byte {
                                self.mov_m32_r64(5, off, 0);
                            } else {
                                self.mov_m64_r64(5, off, 0);
                            }
                        }
                    }
                    Expr::Member { object, member } => {
                        self.push_r64(0); // save value
                        // For pointer types, load pointer value; otherwise get struct address
                        let is_ptr = matches!(object.as_ref(), Expr::Identifier(oname)
                            if self.var_types.get(oname).map_or(false, |t| matches!(t, Type::Ptr(_))));
                        if is_ptr {
                            self.compile_expr_to(object, 1);
                        } else {
                            self.compile_expr_addr(object, 1);
                        }
                        let fsz = get_member_field_size(&self.var_types, &self.struct_layouts, object, member);
                        if let Some(sname) = resolve_struct_name(object, &self.var_types, &self.struct_layouts) {
                            if let Some(foff) = field_offset(sname, member, &self.struct_layouts) {
                                if foff != 0 { self.add_rm64_imm8(1, foff as i8); }
                            }
                        }
                        self.pop_r64(0); // restore value
                        if fsz == 4 {
                            self.mov_m32_r64(1, 0, 0);
                        } else {
                            self.mov_m64_r64(1, 0, 0);
                        }
                    }
                    Expr::Index { object, index } => {
                        self.push_r64(0);
                        self.compile_expr_addr(target, 1);
                        self.pop_r64(0);
                        let elem_size = get_ptr_elem_size(object, &self.var_types, &self.struct_layouts);
                        if elem_size == 1 {
                            self.u8(0x88); self.modrm(0, 0, 1);
                        } else if elem_size == 4 {
                            self.mov_m32_r64(1, 0, 0);
                        } else {
                            self.mov_m64_r64(1, 0, 0);
                        }
                    }
                    Expr::Unary { op: UnOp::Deref, operand } => {
                        self.push_r64(0);
                        self.compile_expr_addr(target, 1);
                        self.pop_r64(0);
                        let elem_size = if let Expr::Identifier(oname) = operand.as_ref() {
                            if let Some(Type::Ptr(inner)) = self.var_types.get(oname) {
                                type_size(inner, &self.struct_layouts)
                            } else { 8 }
                        } else { 8 };
                        if elem_size == 1 {
                            self.u8(0x88); self.modrm(0, 0, 1);
                        } else if elem_size == 4 {
                            self.mov_m32_r64(1, 0, 0);
                        } else {
                            self.mov_m64_r64(1, 0, 0);
                        }
                    }
                    _ => {
                        self.push_r64(0);
                        self.compile_expr_addr(target, 1);
                        self.pop_r64(0);
                        self.mov_m64_r64(1, 0, 0);
                    }
                }
            }
            Expr::Member { object, member } => {
                // For pointer types, load the pointer value first; otherwise get address
                let is_ptr = matches!(object.as_ref(), Expr::Identifier(oname)
                    if self.var_types.get(oname).map_or(false, |t| matches!(t, Type::Ptr(_))));
                if is_ptr {
                    self.compile_expr_to(object, reg);
                } else {
                    self.compile_expr_addr(object, reg);
                }
                if let Some(sname) = resolve_struct_name(object, &self.var_types, &self.struct_layouts) {
                    if let Some(foff) = field_offset(sname, member, &self.struct_layouts) {
                        let fsz = field_size(sname, member, &self.struct_layouts).unwrap_or(8);
                        if fsz == 4 {
                            self.mov_r64_m32(reg, reg, foff);
                        } else {
                            self.mov_r64_m64(reg, reg, foff);
                        }
                    }
                }
            }
            Expr::Index { object, index } => {
                let is_ptr = matches!(object.as_ref(), Expr::Identifier(oname)
                    if self.var_types.get(oname).map_or(false, |t| matches!(t, Type::Ptr(_))));
                let elem_size = get_ptr_elem_size(object, &self.var_types, &self.struct_layouts);
                if is_ptr {
                    self.compile_expr_to(object, 0);
                } else {
                    self.compile_expr_addr(object, 0);
                }
                self.push_r64(0);
                self.compile_expr_to(index, 1);
                self.pop_r64(0);
                if elem_size != 1 {
                    self.mov_r64_imm32(2, elem_size as u32);
                    self.imul_r64(1, 2);
                }
                self.add_r64(0, 1);
                if reg != 0 { self.mov_r64_r64(reg, 0); }
                if elem_size == 1 {
                    self.movzx_r64(reg, reg, 0);
                } else if elem_size == 4 {
                    self.mov_r64_m32(reg, reg, 0);
                } else {
                    self.mov_r64_m64(reg, reg, 0);
                }
            }
            Expr::ArrayInit(elems) => {
                // Allocate temp space on stack, store elements, return pointer
                let count = elems.len() as i32;
                let bytes = count * 4;
                if bytes > 0 {
                    if bytes <= 127 {
                        self.sub_rm64_imm8(4, bytes as i8);
                    } else {
                        self.mov_r64_imm32(1, bytes as u32);
                        self.sub_r64(4, 1);
                    }
                }
                self.mov_r64_r64(reg, 4);
                for (i, elem) in elems.iter().enumerate() {
                    self.compile_expr_to(elem, 0);
                    self.mov_m32_r64(4, i as i32 * 4, 0);
                }
            }
            Expr::StructInit { type_name, fields } => {
                // Allocate temp space on stack, store fields, return pointer
                let sz = struct_size(type_name, &self.struct_layouts);
                if sz > 0 {
                    self.sub_rm64_imm8(4, sz as i8);
                }
                self.mov_r64_r64(reg, 4);
                for (fname, fval) in fields {
                    if let Some(foff) = field_offset(type_name, fname, &self.struct_layouts) {
                        if let Some(fsz) = field_size(type_name, fname, &self.struct_layouts) {
                            self.compile_expr_to(fval, 0);
                            if fsz == 4 {
                                self.mov_m32_r64(4, foff, 0);
                            } else {
                                self.mov_m64_r64(4, foff, 0);
                            }
                        }
                    }
                }
            }
            Expr::SizeOf(ty) => {
                let sz = match ty {
                    Type::Named(name) => struct_size(name, &self.struct_layouts),
                    _ => type_size(ty, &self.struct_layouts),
                };
                self.mov_r64_imm32(reg, sz as u32);
            }
        }
    }

    fn compile_call(&mut self, callee: &str, args: &[Expr]) {
        if callee == "print" || callee == "println" {
            self.compile_print(callee, args);
            return;
        }
        if callee == "read_file" {
            self.compile_read_file(args);
            return;
        }
        if callee == "write_file" {
            self.compile_write_file(args);
            return;
        }
        if callee == "str_eq" { self.compile_str_eq(args); return; }
        if callee == "str_len" { self.compile_str_len(args); return; }
        if callee == "str_to_int" { self.compile_str_to_int(args); return; }
        if callee == "malloc" { self.compile_malloc(args); return; }
        if callee == "free" { self.compile_free(args); return; }
        if callee == "cmdline" { self.compile_cmdline(args); return; }

        // Math intrinsics (only if `eat math` used)
        if self.has_math {
            match callee {
                "abs" => { self.compile_abs(args); return; }
                "min" => { self.compile_min(args); return; }
                "max" => { self.compile_max(args); return; }
                "clamp" => { self.compile_clamp(args); return; }
                "pow" => { self.compile_pow(args); return; }
                "gcd" => { self.compile_gcd(args); return; }
                "rand" => { self.compile_rand(args); return; }
                "srand" => { self.compile_srand(args); return; }
                _ => {}
            }
        }

        // GDI intrinsics (only if `eat gdi` used)
        if self.has_gdi {
            match callee {
                "window_create" => { self.compile_gdi_window_create(args); return; }
                "get_dc" => { self.compile_gdi_get_dc(args); return; }
                "create_buffer" => { self.compile_gdi_create_buffer(args); return; }
                "delete_buffer" => { self.compile_gdi_delete_buffer(args); return; }
                "set_pixel" => { self.compile_gdi_set_pixel(args); return; }
                "bitblt" => { self.compile_gdi_bitblt(args); return; }
                "present" => { self.compile_gdi_present(args); return; }
                "poll_events" => { self.compile_gdi_poll_events(args); return; }
                "key_down" => { self.compile_gdi_key_down(args); return; }
                "sleep" => { self.compile_sleep(args); return; }
                _ => {}
            }
        }

        self.sub_rm64_imm8(4, 0x28);
        let arg_regs = [1, 2, 8, 9];
        for (i, arg) in args.iter().enumerate() {
            if i < 4 { self.compile_expr_to(arg, arg_regs[i]); }
        }
        if let Some(&target) = self.functions.get(callee) {
            let current = self.asm.len() as u32 + 5;
            let rel = target as i64 - current as i64;
            self.call_rel32(); // placeholder
            let entry = self.asm.len() as u32 - 4;
            // Patch directly since target is known
            self.asm[entry as usize..entry as usize + 4].copy_from_slice(&(rel as i32).to_le_bytes());
        }
        self.add_rm64_imm8(4, 0x28);
    }

    /// read_file(path) — reads entire file, returns pointer to heap-allocated content
    /// First 8 bytes BEFORE the returned pointer = file size
    /// i.e. size = *((int*)(ptr - 8)) or just *(ptr - 4) with 32-bit load
    /// Returns null (0) on failure
    ///
    /// Stack layout (0x50 bytes, aligned for Win64):
    ///   [RSP+0x00..0x1F] = shadow space for API calls (clobbered)
    ///   [RSP+0x20..0x2F] = 5th/6th arg slots for API calls
    ///   [RSP+0x30..0x37] = 7th arg slot for CreateFileA
    ///   [RSP+0x38..0x3F] = handle
    ///   [RSP+0x40..0x47] = size
    ///   [RSP+0x48..0x4F] = heap / buffer
    fn compile_read_file(&mut self, args: &[Expr]) {
        self.sub_rm64_imm8(4, 0x50);

        // RCX = path (eval to RAX, copy to RCX)
        if !args.is_empty() { self.compile_expr_to(&args[0], 0); self.mov_r64_r64(1, 0); }
        // CreateFileA(path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL)
        self.mov_r64_imm32(2, 0x80000000u32); // GENERIC_READ
        self.mov_r64_imm32(8, 1u32);          // FILE_SHARE_READ
        self.xor_r64(9, 9);                   // security = NULL
        self.mov_m64_imm32(4, 0x20, 3);       // OPEN_EXISTING at [RSP+0x20]
        self.mov_m64_imm32(4, 0x28, 0);       // flags at [RSP+0x28]
        self.mov_m64_imm32(4, 0x30, 0);       // template at [RSP+0x30]
        self.emit_import_call("CreateFileA");
        self.mov_m64_r64(4, 0x38, 0);         // [RSP+0x38] = handle (above shadow space)

        // GetFileSize(handle, NULL)
        self.mov_r64_m64(1, 4, 0x38);         // RCX = handle from [RSP+0x38]
        self.xor_r64(2, 2);
        self.emit_import_call("GetFileSize");
        self.mov_m64_r64(4, 0x40, 0);         // [RSP+0x40] = size

        // GetProcessHeap()
        self.emit_import_call("GetProcessHeap");
        self.mov_m64_r64(4, 0x48, 0);         // [RSP+0x48] = heap

        // HeapAlloc(heap, 8, size + 4) — 4 extra for size prefix
        self.mov_r64_m64(1, 4, 0x48);         // RCX = heap
        self.mov_r64_imm32(2, 8);             // EDX = HEAP_ZERO_MEMORY
        self.mov_r64_m64(8, 4, 0x40);         // R8 = size
        self.add_rm64_imm8(8, 4);             // R8 += 4
        self.emit_import_call("HeapAlloc");
        self.mov_m64_r64(4, 0x48, 0);         // [RSP+0x48] = buffer (overwrites heap, no longer needed)

        // Store size (64-bit) at buffer[0..7], content starts at buffer+4
        self.mov_r64_m64(8, 4, 0x40);         // R8 = size from [RSP+0x40]
        self.mov_m64_r64(0, 0, 8);            // [RAX+0] = R8 (buffer[0] = size, RAX still buffer from HeapAlloc)

        // ReadFile(handle, buffer+4, size, &bytesRead, overlapped=NULL)
        // Win64: 5th arg (overlapped) at [RSP+0x20], bytesRead stored at [RSP+0x28]
        self.mov_r64_m64(1, 4, 0x38);         // RCX = handle
        self.mov_r64_m64(2, 4, 0x48);         // RDX = buffer
        self.add_rm64_imm8(2, 4);             // RDX = buffer + 4
        self.mov_r64_m64(8, 4, 0x40);         // R8 = size
        self.lea_r64(9, 4, 0x28);             // R9 = &bytesRead at [RSP+0x28]
        self.mov_m64_imm32(4, 0x20, 0);       // [RSP+0x20] = overlapped = NULL (5th arg)
        self.emit_import_call("ReadFile");

        // CloseHandle(handle) — handle at [RSP+0x38] preserved (above shadow + arg slots)
        self.mov_r64_m64(1, 4, 0x38);
        self.emit_import_call("CloseHandle");

        // Return buffer+4 (pointer to content) in RAX
        self.mov_r64_m64(0, 4, 0x48);         // buffer address
        self.add_rm64_imm8(0, 4);             // +4 → skip size prefix
        self.add_rm64_imm8(4, 0x50);          // restore RSP
    }

    /// write_file(path, data, len) — writes len bytes from data pointer to file
    /// Returns 0 on success
    ///
    /// Stack layout (0x48 bytes):
    ///   [RSP+0x00..0x1F] = shadow space for API calls (clobbered)
    ///   [RSP+0x20..0x2F] = 5th/6th arg slots
    ///   [RSP+0x30..0x37] = handle (saved above shadow + arg slots)
    ///   [RSP+0x38..0x3F] = written count (for WriteFile)
    ///   [RSP+0x40..0x47] = unused
    fn compile_write_file(&mut self, args: &[Expr]) {
        if args.len() < 3 { self.xor_r64(0, 0); return; }
        self.sub_rm64_imm8(4, 0x48);

        // RCX = path (eval to RAX first, then copy to RCX)
        self.compile_expr_to(&args[0], 0);
        self.mov_r64_r64(1, 0);
        // CreateFileA(path, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL)
        self.mov_r64_imm32(2, 0x40000000u32); // GENERIC_WRITE
        self.xor_r64(8, 8);                   // no share
        self.xor_r64(9, 9);                   // security = NULL
        self.mov_m64_imm32(4, 0x20, 2);       // CREATE_ALWAYS at [RSP+0x20]
        self.mov_m64_imm32(4, 0x28, 0);       // flags at [RSP+0x28]
        self.mov_m64_imm32(4, 0x30, 0);       // template at [RSP+0x30]
        self.emit_import_call("CreateFileA");
        self.mov_m64_r64(4, 0x30, 0);         // [RSP+0x30] = handle (above shadow, reusing template slot)

        // WriteFile(handle, data, len, &written, NULL)
        self.mov_r64_m64(1, 4, 0x30);         // RCX = handle from [RSP+0x30]
        // Evaluate data (arg[1]) and len (arg[2]) into correct regs
        self.compile_expr_to(&args[1], 0);
        self.mov_r64_r64(2, 0);               // RDX = data pointer
        self.compile_expr_to(&args[2], 0);
        self.mov_r64_r64(8, 0);               // R8 = length
        self.lea_r64(9, 4, 0x38);             // R9 = &written at [RSP+0x38]
        self.mov_m64_imm32(4, 0x20, 0);       // [RSP+0x20] = overlapped = NULL (5th arg)
        self.emit_import_call("WriteFile");

        // CloseHandle(handle) — handle at [RSP+0x30] preserved
        self.mov_r64_m64(1, 4, 0x30);
        self.emit_import_call("CloseHandle");

        self.xor_r64(0, 0);
        self.add_rm64_imm8(4, 0x48);
    }

    /// str_eq(str1, str2): compare two null-terminated strings by content
    /// Returns 1 if equal, 0 if not.
    fn compile_str_eq(&mut self, args: &[Expr]) {
        if args.len() < 2 { self.xor_r64(0, 0); return; }
        self.compile_expr_to(&args[0], 0);  self.mov_r64_r64(8, 0); // R8 = str1
        self.compile_expr_to(&args[1], 0);  self.mov_r64_r64(9, 0); // R9 = str2

        let loop_lbl = self.new_label();
        let noteq_lbl = self.new_label();
        let foundeq_lbl = self.new_label();
        let end_lbl = self.new_label();
        self.put_label(loop_lbl);
        self.movzx_r64(2, 8, 0);  // RDX = byte [R8]
        self.u8(0x4D); self.u8(0x0F); self.u8(0xB6); self.u8(0x11); // movzx r10, byte [r9]
        self.cmp_r64(2, 10);
        self.jcc_rel32(CC_NE);
        self.add_label_fixup(self.asm.len() as u32 - 4, noteq_lbl, true);
        self.u8(0x48); self.u8(0x85); self.u8(0xD2);  // test rdx, rdx
        self.jcc_rel32(CC_E);
        self.add_label_fixup(self.asm.len() as u32 - 4, foundeq_lbl, true);
        self.u8(0x49); self.u8(0xFF); self.u8(0xC0);  // inc r8
        self.u8(0x49); self.u8(0xFF); self.u8(0xC1);  // inc r9
        self.jmp_rel32();
        self.add_label_fixup(self.asm.len() as u32 - 4, loop_lbl, true);

        self.put_label(noteq_lbl);
        self.xor_r64(0, 0);  // RAX = 0 (not equal)
        self.jmp_rel32();
        self.add_label_fixup(self.asm.len() as u32 - 4, end_lbl, true);

        self.put_label(foundeq_lbl);
        self.mov_r64_imm32(0, 1);  // RAX = 1 (equal)

        self.put_label(end_lbl);
    }

    /// str_len(str): return length of null-terminated string in RAX
    fn compile_str_len(&mut self, args: &[Expr]) {
        if args.is_empty() { self.xor_r64(0, 0); return; }
        self.compile_expr_to(&args[0], 0);  self.xor_r64(1, 1);
        let loop_lbl = self.new_label();
        let done_lbl = self.new_label();
        self.put_label(loop_lbl);
        self.u8(0x80); self.u8(0x3C); self.u8(0x08); self.u8(0x00);
        self.jcc_rel32(CC_E);
        self.add_label_fixup(self.asm.len() as u32 - 4, done_lbl, true);
        self.u8(0x48); self.u8(0xFF); self.u8(0xC1);
        self.jmp_rel32();
        self.add_label_fixup(self.asm.len() as u32 - 4, loop_lbl, true);
        self.put_label(done_lbl);
        self.mov_r64_r64(0, 1);
    }

    /// str_to_int(str): parse decimal integer from string, return in RAX
    fn compile_str_to_int(&mut self, args: &[Expr]) {
        if args.is_empty() { self.xor_r64(0, 0); return; }
        self.compile_expr_to(&args[0], 0);  self.mov_r64_r64(8, 0); // R8 = str
        self.xor_r64(0, 0);  // RAX = result

        let loop_lbl = self.new_label();
        let done_lbl = self.new_label();
        self.put_label(loop_lbl);
        self.movzx_r64(1, 8, 0);   // RCX = byte [R8]
        self.cmp_r64_imm32(1, 0);  // null terminator?
        self.jcc_rel32(CC_E);
        self.add_label_fixup(self.asm.len() as u32 - 4, done_lbl, true);
        self.cmp_r64_imm32(1, 48); // cmp rcx, '0'
        self.jcc_rel32(CC_L);
        self.add_label_fixup(self.asm.len() as u32 - 4, done_lbl, true);
        self.cmp_r64_imm32(1, 57); // cmp rcx, '9'
        self.jcc_rel32(CC_G);
        self.add_label_fixup(self.asm.len() as u32 - 4, done_lbl, true);
        // result = result * 10
        self.rex(true, false, false, false);
        self.u8(0x8D);
        self.modrm(0, 0, 4);
        self.u8(0x80);             // lea rax, [rax + rax*4]
        self.add_r64(0, 0);        // rax = rax + rax  (→ rax *= 10)
        // result = result + (digit - '0')
        self.sub_rm64_imm8(1, 48); // rcx -= '0'
        self.add_r64(0, 1);        // rax += rcx
        self.u8(0x49); self.u8(0xFF); self.u8(0xC0);  // inc r8
        self.jmp_rel32();
        self.add_label_fixup(self.asm.len() as u32 - 4, loop_lbl, true);

        self.put_label(done_lbl);
    }

    /// malloc(size): allocate zero-initialized memory from process heap
    fn compile_malloc(&mut self, args: &[Expr]) {
        if args.is_empty() { self.xor_r64(0, 0); return; }
        self.compile_expr_to(&args[0], 0);
        self.sub_rm64_imm8(4, 0x28);
        self.mov_m64_r64(4, 0x20, 0);   // save size at [RSP+0x20]
        self.emit_import_call("GetProcessHeap");
        self.mov_r64_r64(1, 0);         // RCX = heap
        self.mov_r64_imm32(2, 8);       // EDX = HEAP_ZERO_MEMORY
        self.mov_r64_m64(8, 4, 0x20);   // R8 = size (reload)
        self.emit_import_call("HeapAlloc");
        self.add_rm64_imm8(4, 0x28);
    }

    /// free(ptr): free memory allocated by malloc
    fn compile_free(&mut self, args: &[Expr]) {
        if args.is_empty() { return; }
        self.compile_expr_to(&args[0], 0);
        self.sub_rm64_imm8(4, 0x28);
        self.mov_m64_r64(4, 0x20, 0);   // save ptr at [RSP+0x20]
        self.emit_import_call("GetProcessHeap");
        self.mov_r64_r64(1, 0);         // RCX = heap
        self.xor_r64(2, 2);             // EDX = 0
        self.mov_r64_m64(8, 4, 0x20);   // R8 = ptr (reload)
        self.emit_import_call("HeapFree");
        self.add_rm64_imm8(4, 0x28);
    }

    fn compile_cmdline(&mut self, args: &[Expr]) {
        self.sub_rm64_imm8(4, 0x28);
        self.emit_import_call("GetCommandLineA");
        self.add_rm64_imm8(4, 0x28);
    }

    fn compile_print(&mut self, callee: &str, args: &[Expr]) {
        if args.is_empty() { return; }
        match &args[0] {
            Expr::Identifier(name) => {
                if let Some(Type::Ptr(inner)) = self.var_types.get(name) {
                    if matches!(inner.as_ref(), Type::Char) {
                        self.compile_expr_to(&args[0], 0);
                        self.emit_print_string_runtime(callee == "println");
                        return;
                    }
                }
                self.compile_expr_to(&args[0], 0);
                // Check if the variable is of type fixed, use fixed-point printing
                if matches!(self.var_types.get(name), Some(Type::Fixed)) {
                    self.emit_print_fixed(callee == "println");
                } else if matches!(self.var_types.get(name), Some(Type::UnInt)) {
                    self.emit_print_unint(callee == "println");
                } else {
                    self.emit_print_int(callee == "println");
                }
            },
            Expr::String(s) => {
                let s = format!("{}{}", s, if callee == "println" { "\r\n" } else { "" });
                self.emit_print_string(&s);
            },
            Expr::Integer(n) => {
                let s = format!("{}{}", n, if callee == "println" { "\r\n" } else { "" });
                self.emit_print_string(&s);
            },
            Expr::Fixed(n) => {
                let int_part = *n >> 16;
                let frac_raw = ((*n as u64) & 0xFFFF) * 100000 / 65536;
                let s = if int_part < 0 && frac_raw > 0 {
                    format!("-{}.{:05}{}", (-int_part), frac_raw, if callee == "println" { "\r\n" } else { "" })
                } else {
                    format!("{}.{:05}{}", int_part, frac_raw, if callee == "println" { "\r\n" } else { "" })
                };
                self.emit_print_string(&s);
            },
            Expr::Bool(b) => {
                let s = format!("{}{}", if *b { "true" } else { "false" }, if callee == "println" { "\r\n" } else { "" });
                self.emit_print_string(&s);
            },
            _ => {
                // Runtime: evaluate expression, convert int to string, print
                self.compile_expr_to(&args[0], 0);  // RAX = value (32-bit, zero-extended)
                self.emit_print_int(callee == "println");
            },
        }
    }

    /// Print a compile-time-known string (interned in .data rdata section)
    fn emit_print_string(&mut self, s: &str) {
        let str_idx = self.intern_string(s);
        let str_len = s.len() as i32;

        // Align stack to 16 bytes before call (after prologue RSP ≡ 8 mod 16, sub 0x28 → RSP ≡ 0)
        self.sub_rm64_imm8(4, 0x28);
        self.mov_r64_imm32(1, 0xFFFFFFF5u32);
        self.emit_import_call("GetStdHandle");
        self.add_rm64_imm8(4, 0x28);

        // WriteFile
        self.mov_r64_r64(1, 0); // RCX = handle
        let lea_off = self.asm.len() as u32;
        self.lea_r64_rip(2);    // RDX = string
        self.string_relocs.push((lea_off, str_idx));
        self.mov_r64_imm32(8, str_len as u32); // R8 = length
        // sub 0x38 aligns stack: RSP≡8 after prologue, 0x38≡8, 8+8≡0 mod 16 ✓
        self.sub_rm64_imm8(4, 0x38);
        self.lea_r64(9, 4, 0x30);              // R9 = &written (at [RSP+0x30])
        self.mov_m64_imm32(4, 0x20, 0);        // Overlapped = NULL (5th arg at [RSP+0x20])
        self.emit_import_call("WriteFile");
        self.add_rm64_imm8(4, 0x38);
    }

    /// Print a runtime integer value (already in RAX).
    /// Converts to decimal string on stack, then calls GetStdHandle + WriteFile.
    fn emit_print_int(&mut self, add_newline: bool) {
        // Save callee-saved registers we'll use
        self.push_r64(3);   // RBX
        self.push_r64(7);   // RDI

        // Allocate 48-byte buffer for decimal digits (max 20 for 64-bit int)
        self.sub_rm64_imm8(4, 48);

        // lea r8, [rsp+48] — end of buffer, we write digits backwards
        self.lea_r64(8, 4, 48);

        // === itoa loop ===
        let loop_lbl = self.new_label();
        self.put_label(loop_lbl);

        // xor rdx, rdx — clear high part for div
        self.xor_r64(2, 2);
        // mov ecx, 10 — divisor (zero-extends to RCX)
        self.mov_r64_imm32(1, 10);
        // div rcx — unsigned divide RDX:RAX by RCX → RAX=quotient, RDX=remainder
        self.rex(true, false, false, false);  // REX.W
        self.u8(0xF7); self.u8(0xF1);         // F7 F1 = div rcx
        // add dl, '0' — convert digit to ASCII
        self.u8(0x80); self.u8(0xC2); self.u8(0x30);  // 80 C2 30
        // dec r8 — move write pointer backwards
        self.u8(0x49); self.u8(0xFF); self.u8(0xC8);  // 49 FF C8 = dec r8
        // mov byte [r8], dl — store digit byte
        self.u8(0x41); self.u8(0x88); self.u8(0x10);  // 41 88 10 = mov [r8], dl

        // test rax, rax
        self.rex(true, false, false, false);  // REX.W
        self.u8(0x85); self.u8(0xC0);        // 85 C0 = test rax,rax
        // jnz loop
        self.jcc_rel32(CC_NE);
        self.add_label_fixup(self.asm.len() as u32 - 4, loop_lbl, true);

        // === After loop: R8 = pointer to first digit ===
        self.mov_r64_r64(3, 8);    // RBX = string pointer

        // Length = (RSP + 48) - RBX
        self.lea_r64(0, 4, 48);    // RAX = RSP + 48
        self.sub_r64(0, 3);        // RAX -= RBX
        self.mov_r64_r64(7, 0);    // RDI = length

        if add_newline {
            // Append \r\n at end of string
            self.mov_r64_r64(0, 3);   // RAX = RBX (string)
            self.add_r64(0, 7);       // RAX += RDI (→ after last digit)
            // mov byte [rax], 0x0D  (\r)
            self.u8(0xC6); self.u8(0x00); self.u8(0x0D);
            // mov byte [rax+1], 0x0A  (\n)
            self.u8(0xC6); self.u8(0x40); self.u8(0x01); self.u8(0x0A);
            // length += 2
            self.add_rm64_imm8(7, 2);
        }

        // === GetStdHandle(STD_OUTPUT_HANDLE) ===
        self.sub_rm64_imm8(4, 0x28);
        self.mov_r64_imm32(1, 0xFFFFFFF5u32);
        self.emit_import_call("GetStdHandle");
        self.add_rm64_imm8(4, 0x28);

        // === WriteFile(handle, string, length, &written, NULL) ===
        self.mov_r64_r64(1, 0);   // RCX = handle
        self.mov_r64_r64(2, 3);   // RDX = string (from RBX)
        self.mov_r64_r64(8, 7);   // R8 = length (from RDI)

        self.sub_rm64_imm8(4, 0x38);
        self.lea_r64(9, 4, 0x30);          // R9 = &written
        self.mov_m64_imm32(4, 0x20, 0);    // overlapped = NULL
        self.emit_import_call("WriteFile");
        self.add_rm64_imm8(4, 0x38);

        // === Restore ===
        self.add_rm64_imm8(4, 48);  // free buffer
        self.pop_r64(7);            // RDI
        self.pop_r64(3);            // RBX
    }

    /// Print a runtime string pointer (already in RAX). Computes length via strlen.
    fn emit_print_string_runtime(&mut self, add_newline: bool) {
        self.push_r64(3);   // RBX
        self.push_r64(7);   // RDI

        // RBX = string pointer, RDI = length (strlen)
        self.mov_r64_r64(3, 0);
        self.mov_r64_r64(7, 3);
        self.xor_r64(0, 0);
        self.mov_r64_imm32(1, 0xFFFFFFFFu32);
        self.u8(0xF2); self.u8(0xAE);
        self.rex(true, false, false, false); self.u8(0xF7); self.modrm(3, 2, 1);
        self.rex(true, false, false, false); self.u8(0xFF); self.modrm(3, 1, 1);
        self.mov_r64_r64(7, 1);

        // GetStdHandle(STD_OUTPUT_HANDLE)
        self.sub_rm64_imm8(4, 0x28);
        self.mov_r64_imm32(1, 0xFFFFFFF5u32);
        self.emit_import_call("GetStdHandle");
        self.add_rm64_imm8(4, 0x28);

        // WriteFile(handle, string, length, &written, NULL)
        self.mov_r64_r64(1, 0);   // RCX = handle
        self.mov_r64_r64(2, 3);   // RDX = string
        self.mov_r64_r64(8, 7);   // R8 = length
        self.sub_rm64_imm8(4, 0x38);
        self.lea_r64(9, 4, 0x30);
        self.mov_m64_imm32(4, 0x20, 0);
        self.emit_import_call("WriteFile");
        self.add_rm64_imm8(4, 0x38);

        if add_newline {
            // WriteFile(handle, "\r\n", 2, &written, NULL) — re-get handle
            self.sub_rm64_imm8(4, 0x28);
            self.mov_r64_imm32(1, 0xFFFFFFF5u32);
            self.emit_import_call("GetStdHandle");
            self.add_rm64_imm8(4, 0x28);
            self.sub_rm64_imm8(4, 0x38);
            self.mov_m64_imm32(4, 0x30, 0x0A0D);
            self.lea_r64(2, 4, 0x30);
            self.mov_r64_imm32(8, 2);
            self.mov_r64_r64(1, 0);
            self.lea_r64(9, 4, 0x28);
            self.mov_m64_imm32(4, 0x20, 0);
            self.emit_import_call("WriteFile");
            self.add_rm64_imm8(4, 0x38);
        }

        self.pop_r64(7);
        self.pop_r64(3);
    }

    /// Print a fixed-point Q16.16 value (already in RAX).
    fn emit_print_fixed(&mut self, add_newline: bool) {
        self.emit_print_int(add_newline);
    }

    /// Print an unsigned negative value (already in RAX, stored as negative i32).
    fn emit_print_unint(&mut self, add_newline: bool) {
        self.emit_print_int(add_newline);
    }

    // --- Math intrinsics ---

    fn compile_abs(&mut self, args: &[Expr]) {
        if !args.is_empty() { self.compile_expr_to(&args[0], 0); } else { self.xor_r64(0, 0); return; }
        // cdq (sign-extend eax → edx:eax)
        self.u8(0x99);
        // xor eax, edx; sub eax, edx  → absolute value
        self.asm.extend_from_slice(&[0x31, 0xD0]); // xor eax, edx
        self.asm.extend_from_slice(&[0x29, 0xD0]); // sub eax, edx
    }

    fn compile_min(&mut self, args: &[Expr]) {
        if args.len() < 2 { self.xor_r64(0, 0); return; }
        self.compile_expr_to(&args[0], 0);
        self.compile_expr_to(&args[1], 1);
        // cmp eax, ecx; cmovg eax, ecx
        self.asm.extend_from_slice(&[0x39, 0xC8]); // cmp eax, ecx
        self.asm.extend_from_slice(&[0x0F, 0x4F, 0xC1]); // cmovg eax, ecx
    }

    fn compile_max(&mut self, args: &[Expr]) {
        if args.len() < 2 { self.xor_r64(0, 0); return; }
        self.compile_expr_to(&args[0], 0);
        self.compile_expr_to(&args[1], 1);
        // cmp eax, ecx; cmovl eax, ecx
        self.asm.extend_from_slice(&[0x39, 0xC8]); // cmp eax, ecx
        self.asm.extend_from_slice(&[0x0F, 0x4C, 0xC1]); // cmovl eax, ecx
    }

    fn compile_clamp(&mut self, args: &[Expr]) {
        if args.len() < 3 { self.xor_r64(0, 0); return; }
        self.compile_expr_to(&args[0], 0);
        self.compile_expr_to(&args[1], 1);
        self.compile_expr_to(&args[2], 8);
        // cmp eax, ecx; cmovl eax, ecx (max with lo)
        self.asm.extend_from_slice(&[0x39, 0xC8]); // cmp eax, ecx
        self.asm.extend_from_slice(&[0x0F, 0x4C, 0xC1]); // cmovl eax, ecx
        // cmp eax, r8d; cmovg eax, r8d (min with hi)
        self.asm.extend_from_slice(&[0x44, 0x39, 0xC0]); // cmp eax, r8d
        self.asm.extend_from_slice(&[0x41, 0x0F, 0x4F, 0xC0]); // cmovg eax, r8d
    }

    fn compile_pow(&mut self, args: &[Expr]) {
        if args.len() < 2 { self.xor_r64(0, 0); return; }
        self.compile_expr_to(&args[0], 0); // base in RAX
        self.compile_expr_to(&args[1], 1); // exp in RCX
        // if exp <= 0, result = 1
        let done_lbl = self.new_label();
        // mov edx, 1 (result)
        self.asm.extend_from_slice(&[0xBA, 0x01, 0x00, 0x00, 0x00]);
        // test ecx, ecx; jle done
        self.u8(0x85); self.modrm(3, 1, 1); // test ecx, ecx
        self.jcc_rel32(CC_LE);
        self.add_label_fixup(self.asm.len() as u32 - 4, done_lbl, true);
        let loop_lbl = self.new_label();
        self.put_label(loop_lbl);
        // imul edx, eax (result *= base, 32-bit)
        self.asm.extend_from_slice(&[0x0F, 0xAF, 0xD0]); // imul edx, eax
        // dec ecx; jnz loop
        self.u8(0xFF); self.u8(0xC9); // dec ecx
        self.jcc_rel32(CC_NE);
        self.add_label_fixup(self.asm.len() as u32 - 4, loop_lbl, true);
        self.put_label(done_lbl);
        self.mov_r64_r64(0, 2); // result → RAX
    }

    fn compile_gcd(&mut self, args: &[Expr]) {
        if args.len() < 2 { self.xor_r64(0, 0); return; }
        self.compile_expr_to(&args[0], 0); // a in RAX
        self.compile_expr_to(&args[1], 1); // b in RCX
        let loop_lbl = self.new_label();
        let done_lbl = self.new_label();
        self.put_label(loop_lbl);
        // test ecx, ecx; je done
        self.u8(0x85); self.modrm(3, 1, 1); // test ecx, ecx
        self.jcc_rel32(CC_E);
        self.add_label_fixup(self.asm.len() as u32 - 4, done_lbl, true);
        // xor edx, edx; idiv ecx (edx = a % b)
        self.asm.extend_from_slice(&[0x31, 0xD2]); // xor edx, edx
        self.asm.extend_from_slice(&[0xF7, 0xF9]); // idiv ecx
        // mov eax, ecx (a = b); mov ecx, edx (b = remainder)
        self.asm.extend_from_slice(&[0x89, 0xC8]); // mov eax, ecx
        self.asm.extend_from_slice(&[0x89, 0xD1]); // mov ecx, edx
        // jmp loop
        self.jmp_rel32();
        self.add_label_fixup(self.asm.len() as u32 - 4, loop_lbl, true);
        self.put_label(done_lbl);
        // result in eax
    }

    fn compile_rand(&mut self, _args: &[Expr]) {
        // rand() → call msvcrt!rand, returns int in eax
        // RSP ≡ 0 mod 16 at this point (after prologue), need 0x30 (32 shadow + 16 alignment)
        self.sub_rm64_imm8(4, 0x30);
        let offset = self.asm.len() as u32;
        self.call_rip_placeholder();
        self.import_relocs.push((offset, "rand".to_string()));
        if !self.imports.iter().any(|i| i.name == "rand") {
            self.imports.push(ImportRequest {
                name: "rand".to_string(),
                dll: "msvcrt.dll".to_string(),
            });
        }
        self.add_rm64_imm8(4, 0x30);
    }

    fn compile_srand(&mut self, args: &[Expr]) {
        // srand(seed) → call msvcrt!srand
        if !args.is_empty() { self.compile_expr_to(&args[0], 0); } // seed to RAX
        self.sub_rm64_imm8(4, 0x30);
        if !args.is_empty() { self.mov_r64_r64(1, 0); } // RCX = seed
        let offset = self.asm.len() as u32;
        self.call_rip_placeholder();
        self.import_relocs.push((offset, "srand".to_string()));
        if !self.imports.iter().any(|i| i.name == "srand") {
            self.imports.push(ImportRequest {
                name: "srand".to_string(),
                dll: "msvcrt.dll".to_string(),
            });
        }
        self.add_rm64_imm8(4, 0x30);
    }

    // ── GDI intrinsics ──

    fn register_gdi_imports(&mut self) {
        for name in &["CreateWindowExA", "GetAsyncKeyState", "GetModuleHandleA",
            "PeekMessageA", "TranslateMessage", "DispatchMessageA",
            "ShowWindow", "GetDC", "ReleaseDC", "DestroyWindow"] {
            let dll = "user32.dll";
            if !self.imports.iter().any(|i| i.name == *name) {
                self.imports.push(ImportRequest { name: name.to_string(), dll: dll.to_string() });
            }
        }
        for name in &["SetPixel", "BitBlt", "CreateCompatibleDC", "CreateCompatibleBitmap",
            "SelectObject", "DeleteObject", "DeleteDC"] {
            let dll = "gdi32.dll";
            if !self.imports.iter().any(|i| i.name == *name) {
                self.imports.push(ImportRequest { name: name.to_string(), dll: dll.to_string() });
            }
        }
    }

    fn compile_gdi_window_create(&mut self, args: &[Expr]) {
        // window_create(title, w, h) → HWND via CreateWindowExA("STATIC", ...)
        let cls_idx = self.intern_string("STATIC");
        let title_str = match args.get(0) {
            Some(Expr::String(s)) => s.clone(),
            _ => "Game".to_string(),
        };
        let title_idx = self.intern_string(&title_str);

        self.sub_rm64_imm8(4, 0x28);
        self.xor_r64(1, 1);
        self.emit_dll_import_call("GetModuleHandleA", "kernel32.dll");
        self.add_rm64_imm8(4, 0x28);
        self.mov_r64_r64(3, 0);

        self.sub_rm64_imm8(4, 0x68);
        self.xor_r64(1, 1);
        let cls_off = self.asm.len() as u32;
        self.lea_r64_rip(2);
        self.string_relocs.push((cls_off, cls_idx));
        let title_off = self.asm.len() as u32;
        self.lea_r64_rip(8);
        self.string_relocs.push((title_off, title_idx));
        self.mov_r64_imm32(9, 0x10CF0000);
        self.mov_m64_imm32(4, 0x28, i32::MIN); // CW_USEDEFAULT = 0x80000000
        self.mov_m64_imm32(4, 0x30, i32::MIN);
        let w = match args.get(1) { Some(Expr::Integer(n)) => *n as i32, _ => 800 };
        let h = match args.get(2) { Some(Expr::Integer(n)) => *n as i32, _ => 600 };
        self.mov_m64_imm32(4, 0x38, w);
        self.mov_m64_imm32(4, 0x40, h);
        self.mov_m64_imm32(4, 0x48, 0);
        self.mov_m64_imm32(4, 0x50, 0);
        self.mov_m64_r64(4, 0x58, 3);
        self.mov_m64_imm32(4, 0x60, 0);
        self.emit_dll_import_call("CreateWindowExA", "user32.dll");
        self.add_rm64_imm8(4, 0x68);

        // ShowWindow(hwnd, SW_SHOWNORMAL)
        self.push_r64(3);
        self.mov_r64_r64(3, 0);
        self.sub_rm64_imm8(4, 0x28);
        self.mov_r64_r64(1, 3);
        self.mov_r64_imm32(2, 1);
        self.emit_dll_import_call("ShowWindow", "user32.dll");
        self.add_rm64_imm8(4, 0x28);
        self.mov_r64_r64(0, 3);
        self.pop_r64(3);
    }

    fn compile_gdi_get_dc(&mut self, args: &[Expr]) {
        self.sub_rm64_imm8(4, 0x28);
        if !args.is_empty() { self.compile_expr_to(&args[0], 1); }
        else { self.xor_r64(1, 1); }
        self.emit_dll_import_call("GetDC", "user32.dll");
        self.add_rm64_imm8(4, 0x28);
    }

    fn compile_gdi_create_buffer(&mut self, args: &[Expr]) {
        // create_buffer(desk_dc, w, h) → returns memory DC in RAX
        // Internally: CreateCompatibleDC(NULL) + CreateCompatibleBitmap(desk_dc, w, h) + SelectObject
        // Step 1: CreateCompatibleDC(NULL) → mem_dc
        self.sub_rm64_imm8(4, 0x28);
        self.xor_r64(1, 1);
        self.emit_dll_import_call("CreateCompatibleDC", "gdi32.dll");
        self.add_rm64_imm8(4, 0x28);
        self.push_r64(3); // save mem_dc in RBX
        self.mov_r64_r64(3, 0);

        // Step 2: CreateCompatibleBitmap(desk_dc, w, h) → bitmap
        self.sub_rm64_imm8(4, 0x28);
        if !args.is_empty() { self.compile_expr_to(&args[0], 1); } // RCX = desk_dc
        self.compile_expr_to(args.get(1).unwrap_or(&Expr::Integer(0)), 2); // RDX = w
        self.compile_expr_to(args.get(2).unwrap_or(&Expr::Integer(0)), 8); // R8 = h
        self.emit_dll_import_call("CreateCompatibleBitmap", "gdi32.dll");
        self.add_rm64_imm8(4, 0x28);

        // Step 3: SelectObject(mem_dc, bitmap)
        self.sub_rm64_imm8(4, 0x28);
        self.mov_r64_r64(1, 3); // RCX = mem_dc
        self.mov_r64_r64(2, 0); // RDX = bitmap handle
        self.emit_dll_import_call("SelectObject", "gdi32.dll");
        self.add_rm64_imm8(4, 0x28);

        // Return mem_dc in RAX
        self.mov_r64_r64(0, 3);
        self.pop_r64(3);
    }

    fn compile_gdi_delete_buffer(&mut self, args: &[Expr]) {
        self.sub_rm64_imm8(4, 0x28);
        if !args.is_empty() { self.compile_expr_to(&args[0], 1); }
        self.emit_dll_import_call("DeleteDC", "gdi32.dll");
        self.add_rm64_imm8(4, 0x28);
    }

    fn compile_gdi_set_pixel(&mut self, args: &[Expr]) {
        self.sub_rm64_imm8(4, 0x28);
        if args.len() > 0 { self.compile_expr_to(&args[0], 1); }
        if args.len() > 1 { self.compile_expr_to(&args[1], 2); }
        if args.len() > 2 { self.compile_expr_to(&args[2], 8); }
        if args.len() > 3 { self.compile_expr_to(&args[3], 9); }
        self.emit_dll_import_call("SetPixel", "gdi32.dll");
        self.add_rm64_imm8(4, 0x28);
    }

    fn compile_gdi_bitblt(&mut self, args: &[Expr]) {
        self.sub_rm64_imm8(4, 0x48);
        if args.len() > 0 { self.compile_expr_to(&args[0], 1); }
        if args.len() > 1 { self.compile_expr_to(&args[1], 2); }
        if args.len() > 2 { self.compile_expr_to(&args[2], 8); }
        if args.len() > 3 { self.compile_expr_to(&args[3], 9); }
        if args.len() > 4 { self.compile_expr_to(&args[4], 0); self.mov_m64_r64(4, 0x28, 0); }
        else { self.mov_m64_imm32(4, 0x28, 0); }
        if args.len() > 5 { self.compile_expr_to(&args[5], 0); self.mov_m64_r64(4, 0x30, 0); }
        else { self.mov_m64_imm32(4, 0x30, 0); }
        self.mov_m64_imm32(4, 0x38, 0);
        self.mov_m64_imm32(4, 0x40, 0);
        self.mov_m64_imm32(4, 0x48, 0x00CC0020);
        self.emit_dll_import_call("BitBlt", "gdi32.dll");
        self.add_rm64_imm8(4, 0x48);
    }

    fn compile_gdi_present(&mut self, args: &[Expr]) {
        // present(dst, src, w, h) → BitBlt(dst, 0, 0, w, h, src, 0, 0, SRCCOPY)
        self.sub_rm64_imm8(4, 0x48);
        if args.len() > 0 { self.compile_expr_to(&args[0], 1); } else { self.xor_r64(1, 1); } // RCX = dst
        self.xor_r64(2, 2); // RDX = dx = 0
        self.xor_r64(8, 8); // R8 = dy = 0
        if args.len() > 2 { self.compile_expr_to(&args[2], 9); } // R9 = w
        if args.len() > 3 { self.compile_expr_to(&args[3], 0); self.mov_m64_r64(4, 0x28, 0); } // h
        else { self.mov_m64_imm32(4, 0x28, 0); }
        if args.len() > 1 { self.compile_expr_to(&args[1], 0); self.mov_m64_r64(4, 0x30, 0); } // src
        else { self.mov_m64_imm32(4, 0x30, 0); }
        self.mov_m64_imm32(4, 0x38, 0); // sx = 0
        self.mov_m64_imm32(4, 0x40, 0); // sy = 0
        self.mov_m64_imm32(4, 0x48, 0x00CC0020); // SRCCOPY
        self.emit_dll_import_call("BitBlt", "gdi32.dll");
        self.add_rm64_imm8(4, 0x48);
    }

    fn compile_gdi_poll_events(&mut self, args: &[Expr]) {
        // allocate stack: 0x28 shadow + 0x40 for MSG (40 bytes rounded up)
        self.sub_rm64_imm8(4, 0x68);

        // PeekMessage loop — consume all pending messages
        let peek_loop = self.new_label();
        self.put_label(peek_loop);
        self.lea_r64(1, 4, 0x28);
        self.xor_r64(2, 2);
        self.xor_r64(8, 8);
        self.xor_r64(9, 9);
        self.mov_m64_imm32(4, 0x20, 1);
        self.emit_dll_import_call("PeekMessageA", "user32.dll");
        self.u8(0x85); self.u8(0xC0);
        let peek_done = self.new_label();
        self.jcc_rel32(CC_E);
        self.add_label_fixup(self.asm.len() as u32 - 4, peek_done, true);
        self.lea_r64(1, 4, 0x28);
        self.emit_dll_import_call("TranslateMessage", "user32.dll");
        self.lea_r64(1, 4, 0x28);
        self.emit_dll_import_call("DispatchMessageA", "user32.dll");
        self.jmp_rel32();
        self.add_label_fixup(self.asm.len() as u32 - 4, peek_loop, true);

        // All messages processed — check ESC key
        self.put_label(peek_done);
        self.mov_r64_imm32(1, 27);
        self.emit_dll_import_call("GetAsyncKeyState", "user32.dll");
        self.u8(0x66); self.u8(0x85); self.u8(0xC0);
        let done = self.new_label();
        self.jcc_rel32(CC_GE);
        self.add_label_fixup(self.asm.len() as u32 - 4, done, true);
        // ESC pressed → leave -1 in RAX
        self.mov_r64_imm32(0, -1i32 as u32);
        let skip = self.new_label();
        self.jmp_rel32();
        self.add_label_fixup(self.asm.len() as u32 - 4, skip, true);
        self.put_label(done);
        self.xor_r64(0, 0); // 0 in RAX
        self.put_label(skip);
        self.add_rm64_imm8(4, 0x68);
    }

    fn compile_gdi_key_down(&mut self, args: &[Expr]) {
        // key_down(vkey) → GetAsyncKeyState → returns 1 if high bit set
        self.sub_rm64_imm8(4, 0x28);
        if !args.is_empty() { self.compile_expr_to(&args[0], 1); }
        self.emit_dll_import_call("GetAsyncKeyState", "user32.dll");
        self.add_rm64_imm8(4, 0x28);
        self.u8(0x66); self.u8(0x85); self.u8(0xC0); // test ax, ax
        let not_down = self.new_label();
        self.jcc_rel32(CC_GE);
        self.add_label_fixup(self.asm.len() as u32 - 4, not_down, true);
        self.mov_r64_imm32(0, 1);
        let done = self.new_label();
        self.jmp_rel32();
        self.add_label_fixup(self.asm.len() as u32 - 4, done, true);
        self.put_label(not_down);
        self.xor_r64(0, 0);
        self.put_label(done);
    }

    fn compile_sleep(&mut self, args: &[Expr]) {
        self.sub_rm64_imm8(4, 0x28);
        if !args.is_empty() { self.compile_expr_to(&args[0], 1); }
        self.emit_import_call("Sleep");
        self.add_rm64_imm8(4, 0x28);
    }

    fn emit_import_call(&mut self, name: &str) {
        self.emit_dll_import_call(name, "kernel32.dll");
    }

    fn emit_dll_import_call(&mut self, name: &str, dll: &str) {
        let offset = self.asm.len() as u32;
        self.call_rip_placeholder();
        self.import_relocs.push((offset, name.to_string()));
        // Track import
        if !self.imports.iter().any(|i| i.name == name && i.dll == dll) {
            self.imports.push(ImportRequest {
                name: name.to_string(),
                dll: dll.to_string(),
            });
        }
    }

    fn finalize(&mut self) -> CompiledUnit {
        // Debug: print generated code
        #[cfg(debug_assertions)]
        {
            println!("; Generated code ({} bytes):", self.asm.len());
            for (i, &b) in self.asm.iter().enumerate() {
                if i > 0 && i % 16 == 0 { println!(); }
                print!(" {:02X}", b);
            }
            println!();
            println!("; Strings: {:?}", std::str::from_utf8(&self.strings).unwrap_or("(invalid utf8)"));
            println!("; String relocs: {:?}", self.string_relocs);
            println!("; Import relocs: {:?}", self.import_relocs);
        }
        CompiledUnit {
            code: std::mem::take(&mut self.asm),
            entry_point: self.functions.get("main").copied().unwrap_or(0),
            strings: std::mem::take(&mut self.strings),
            string_relocs: std::mem::take(&mut self.string_relocs),
            imports: std::mem::take(&mut self.imports),
            import_relocs: std::mem::take(&mut self.import_relocs),
        }
    }
}

// Condition codes
const CC_E: u8 = 0x84;
const CC_NE: u8 = 0x85;
const CC_L: u8 = 0x8C;
const CC_GE: u8 = 0x8D;
const CC_LE: u8 = 0x8E;
const CC_G: u8 = 0x8F;

fn cc_from_binop(op: &BinOp) -> u8 {
    use crate::ast::BinOp::*;
    match op {
        Equal => CC_E, NotEqual => CC_NE, Less => CC_L,
        Greater => CC_G, LessEqual => CC_LE, GreaterEqual => CC_GE,
        _ => CC_E,
    }
}
