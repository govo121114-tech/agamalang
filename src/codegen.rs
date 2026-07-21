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
fn type_size(ty: &Type) -> i32 {
    match ty {
        Type::Int | Type::Char | Type::Bool => 4,
        Type::Ptr(_) => 8,
        Type::Array(_) => 8, // pointer to array (or array descriptor)
        Type::Void => 0,
        Type::StaticArray(elem, n) => type_size(elem) * (*n as i32),
        Type::Named(name) => 4, // placeholder; struct layout has actual size
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
    if let Expr::Identifier(obj_name) = object {
        if let Some(ty) = var_types.get(obj_name) {
            if let Type::Named(struct_name) = ty {
                return field_size(struct_name, member, layouts).unwrap_or(8);
            }
        }
    }
    8
}

/// Compile a program and return the compiled unit
pub fn compile(program: &Program) -> CompiledUnit {
    let mut gen = Generator::new();
    // Build struct layouts
    for sd in &program.structs {
        let mut fields = Vec::new();
        let mut offset: i32 = 0;
        for (fname, ftype) in &sd.fields {
            let fsz = type_size(&ftype);
            fields.push(StructField {
                name: fname.clone(),
                offset,
                size: fsz,
            });
            offset += fsz;
        }
        gen.struct_layouts.insert(sd.name.clone(), StructLayout {
            size: offset,
            fields,
        });
    }
    gen.compile_program(program);
    gen.finalize()
}

#[derive(Clone)]
struct StructField {
    name: String,
    offset: i32,
    size: i32,
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
        self.rex(false, dst >= 8, false, base >= 8); // 32-bit load, zero-extends
        self.u8(0x8B);
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
            let psz = type_size(&p.param_type).max(4) as u32;
            self.stack_size += psz.max(8); // minimum 8-byte slot to avoid overlap
            let off = -(self.stack_size as i32);
            self.vars.insert(p.name.clone(), off);
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
            for stmt in &func.body {
                if let Stmt::VariableDecl { var_type, .. } = stmt {
                    let vs = match var_type {
                        Some(Type::Named(n)) => struct_size(n, &self.struct_layouts).max(4) as u32,
                        Some(ty) => type_size(ty).max(4) as u32,
                        None => 4,
                    };
                    sz += vs.max(8); // minimum 8-byte slot to avoid 8-byte store overlap
                }
            }
            // sz-8 = actual bytes needed, align to 16, minimum 0x20 (32)
            let needed = sz.saturating_sub(8);
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
                self.compile_expr_addr(object, reg);
                if let Expr::Identifier(obj_name) = object.as_ref() {
                    if let Some(ty) = self.var_types.get(obj_name) {
                        if let Type::Named(struct_name) = ty {
                            if let Some(foff) = field_offset(struct_name, member, &self.struct_layouts) {
                                if foff != 0 {
                                    self.add_rm64_imm8(reg, foff as i8);
                                }
                            }
                        }
                    }
                }
            }
            Expr::Index { object, index } => {
                self.compile_expr_addr(object, reg);
                // Save address, compute index*4, add to address
                self.push_r64(reg);
                self.compile_expr_to(index, 0); // index in RAX
                self.mov_r64_imm32(1, 4);
                self.imul_r64(0, 1); // RAX = index * 4
                self.pop_r64(1); // R1 = base address
                self.add_r64(1, 0); // R1 = base + index*4
                if reg != 1 {
                    self.mov_r64_r64(reg, 1);
                }
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
                    Some(ty) => type_size(ty).max(4),
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
                            // Compute field offset from struct type
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
                    } else {
                        self.compile_expr_to(e, 0);
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
                if let Some(e) = value { self.compile_expr_to(e, 0); }
                if self.current_fn == "main" {
                    self.xor_r64(1, 1);
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
                let end_lbl = self.new_label();
                self.loop_continue_labels.push(loop_lbl);
                self.loop_break_labels.push(end_lbl);
                self.put_label(loop_lbl);
                if let Some(c) = condition {
                    self.compile_expr_to(c, 0);
                    self.cmp_r64_imm32(0, 0);
                    self.jcc_rel32(CC_E);
                    self.add_label_fixup(self.asm.len() as u32 - 4, end_lbl, true);
                }
                for s in body { self.compile_stmt(s); }
                if let Some(p) = post { self.compile_expr_to(p, 0); }
                // Continue jumps here (before post! — standard for loop semantics)
                self.put_label(*self.loop_continue_labels.last().unwrap());
                self.loop_continue_labels.pop();
                if let Some(p) = post { self.compile_expr_to(p, 0); }
                self.jmp_rel32();
                self.add_label_fixup(self.asm.len() as u32 - 4, loop_lbl, true);
                self.put_label(end_lbl);
                self.loop_break_labels.pop();
                // For loop's continue label was already popped above
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
                        self.mov_r64_m64(reg, 0, 0);
                    }
                }
            }
            Expr::Call { callee, args } => { self.compile_call(callee, args); }
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
                        self.compile_expr_addr(object, 1); // R1 = struct address
                        // Check field size for correct store width
                        let fsz = get_member_field_size(&self.var_types, &self.struct_layouts, object, member);
                        self.pop_r64(0); // restore value
                        if fsz == 4 {
                            self.mov_m32_r64(1, 0, 0);
                        } else {
                            self.mov_m64_r64(1, 0, 0);
                        }
                    }
                    _ => {
                        // General case: compile_expr_addr to get target address in R1, store RAX
                        self.push_r64(0); // save value
                        self.compile_expr_addr(target, 1); // R1 = target address
                        self.pop_r64(0); // restore value
                        self.mov_m64_r64(1, 0, 0);
                    }
                }
            }
            Expr::Member { object, member } => {
                // Get ADDRESS of object, then load field at offset
                self.compile_expr_addr(object, reg);
                if let Expr::Identifier(obj_name) = object.as_ref() {
                    if let Some(ty) = self.var_types.get(obj_name) {
                        if let Type::Named(struct_name) = ty {
                            if let Some(foff) = field_offset(struct_name, member, &self.struct_layouts) {
                                let fsz = field_size(struct_name, member, &self.struct_layouts).unwrap_or(8);
                                if fsz == 4 {
                                    self.mov_r64_m32(reg, reg, foff);
                                } else {
                                    self.mov_r64_m64(reg, reg, foff);
                                }
                            }
                        }
                    }
                }
            }
            Expr::Index { object, index } => {
                // Compute base address, index * 4, then load
                self.compile_expr_to(object, 0);
                self.push_r64(0);
                self.compile_expr_to(index, 1);
                self.pop_r64(0); // R0 = base, R1 = index
                self.mov_r64_imm32(2, 4);
                self.imul_r64(1, 2); // R1 = index * 4
                self.add_r64(0, 1); // R0 = base + index * 4
                self.mov_r64_m64(reg, 0, 0); // load value at [base + index*4]
            }
            Expr::ArrayInit(elems) => {
                // Allocate temp space on stack, store elements, return pointer
                let count = elems.len() as i32;
                let bytes = count * 4;
                if bytes > 0 {
                    self.sub_rm64_imm8(4, bytes as i8);
                }
                self.mov_r64_r64(reg, 4); // return pointer to start of array
                for (i, elem) in elems.iter().enumerate() {
                    self.compile_expr_to(elem, 0);
                    self.mov_m64_r64(4, i as i32 * 4, 0);
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
                        self.compile_expr_to(fval, 0);
                        self.mov_m64_r64(4, foff, 0);
                    }
                }
            }
            Expr::SizeOf(ty) => {
                let sz = match ty {
                    Type::Named(name) => struct_size(name, &self.struct_layouts),
                    _ => type_size(ty),
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

    fn compile_print(&mut self, callee: &str, args: &[Expr]) {
        if args.is_empty() { return; }
        match &args[0] {
            Expr::String(s) => {
                let s = format!("{}{}", s, if callee == "println" { "\r\n" } else { "" });
                self.emit_print_string(&s);
            },
            Expr::Integer(n) => {
                let s = format!("{}{}", n, if callee == "println" { "\r\n" } else { "" });
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

        // Allocate 32-byte buffer for decimal digits (max ~10 for 32-bit int)
        self.sub_rm64_imm8(4, 32);

        // lea r8, [rsp+32] — end of buffer, we write digits backwards
        self.lea_r64(8, 4, 32);

        // === itoa loop ===
        let loop_lbl = self.new_label();
        self.put_label(loop_lbl);

        // xor edx, edx — clear high part for div
        self.xor_r64(2, 2);
        // mov ecx, 10 — divisor
        self.mov_r64_imm32(1, 10);
        // div ecx — unsigned divide EDX:EAX by ECX → EAX=quotient, EDX=remainder
        self.u8(0xF7); self.u8(0xF1);        // F7 F1 = div ecx
        // add dl, '0' — convert digit to ASCII
        self.u8(0x80); self.u8(0xC2); self.u8(0x30);  // 80 C2 30
        // dec r8 — move write pointer backwards
        self.u8(0x49); self.u8(0xFF); self.u8(0xC8);  // 49 FF C8 = dec r8
        // mov byte [r8], dl — store digit byte
        self.u8(0x41); self.u8(0x88); self.u8(0x10);  // 41 88 10 = mov [r8], dl

        // test eax, eax
        self.u8(0x85); self.u8(0xC0);        // 85 C0 = test eax,eax
        // jnz loop
        self.jcc_rel32(CC_NE);
        self.add_label_fixup(self.asm.len() as u32 - 4, loop_lbl, true);

        // === After loop: R8 = pointer to first digit ===
        self.mov_r64_r64(3, 8);    // RBX = string pointer

        // Length = (RSP + 32) - RBX
        self.lea_r64(0, 4, 32);    // RAX = RSP + 32
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
        self.add_rm64_imm8(4, 32);  // free buffer
        self.pop_r64(7);            // RDI
        self.pop_r64(3);            // RBX
    }

    fn emit_import_call(&mut self, name: &str) {
        let offset = self.asm.len() as u32;
        self.call_rip_placeholder();
        self.import_relocs.push((offset, name.to_string()));
        // Track import
        if !self.imports.iter().any(|i| i.name == name) {
            self.imports.push(ImportRequest {
                name: name.to_string(),
                dll: "kernel32.dll".to_string(),
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
