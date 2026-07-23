//! PE (Portable Executable) file writer.
//! Generates a proper x86-64 PE executable with .text, .data, and .idata sections.

use crate::codegen::CompiledUnit;
use std::io::Write;
use std::collections::HashMap;

const IMAGE_BASE: u64 = 0x400000; // Standard MSVC image base
const SA: u32 = 0x1000;  // Section alignment
const FA: u32 = 0x200;   // File alignment

fn align_up(v: u32, a: u32) -> u32 { (v + a - 1) & !(a - 1) }

/// Build import data for a single DLL (ILT + IAT + hint/name + DLL name).
/// Returns (data_bytes, iat_rva_start, rva_base) where rva_base is the RVA of data_bytes.
fn build_import_data(dll: &str, funcs: &[&str], data_rva: u32) -> (Vec<u8>, u32) {
    let dll_bytes = dll.as_bytes();
    let n = funcs.len() as u32;
    let ilt_sz = (n + 1) * 8;
    let iat_sz = ilt_sz;
    let mut hn_sz = 0u32;
    for f in funcs { hn_sz += 2 + f.len() as u32 + 1; }
    let total = ilt_sz + iat_sz + hn_sz + dll_bytes.len() as u32 + 1;
    let mut buf = vec![0u8; total as usize];

    let iat_rva = data_rva + ilt_sz;

    // ILT entries
    let mut hn_pos = ilt_sz + iat_sz;
    for (i, f) in funcs.iter().enumerate() {
        let i_u32 = i as u32;
        let hn_rva = data_rva + hn_pos;
        let ilt_p = (i_u32 * 8) as usize;
        let iat_p = (ilt_sz + i_u32 * 8) as usize;
        buf[ilt_p..ilt_p + 8].copy_from_slice(&(hn_rva as u64).to_le_bytes());
        buf[iat_p..iat_p + 8].copy_from_slice(&(hn_rva as u64).to_le_bytes());
        let hp = hn_pos as usize;
        buf[hp..hp + 2].copy_from_slice(&(i_u32 as u16).to_le_bytes());
        buf[hp + 2..hp + 2 + f.len()].copy_from_slice(f.as_bytes());
        hn_pos += 2 + f.len() as u32 + 1;
    }
    // DLL name
    let dp = (ilt_sz + iat_sz + hn_sz) as usize;
    buf[dp..dp + dll_bytes.len()].copy_from_slice(dll_bytes);

    (buf, iat_rva)
}

pub fn write_pe<W: Write>(w: &mut W, unit: &CompiledUnit) -> std::io::Result<()> {
    let code_sz = unit.code.len() as u32;
    let str_sz = unit.strings.len() as u32;

    // Layout
    let hdr_sz = align_up(0x200, FA);
    let text_va = align_up(hdr_sz, SA);
    let data_va = align_up(text_va + std::cmp::max(code_sz, 1), SA);
    let idata_va = align_up(data_va + std::cmp::max(str_sz, 1), SA);
    let text_fo = align_up(hdr_sz, FA);
    let data_fo = align_up(text_fo + code_sz, FA);
    let idata_fo = if str_sz == 0 { data_fo + FA } else { align_up(data_fo + str_sz, FA) };
    let text_fs = align_up(code_sz, FA);
    let data_fs = align_up(str_sz, FA);

    // Group imports by DLL
    let mut dll_map: HashMap<String, Vec<&str>> = HashMap::new();
    let mut func_order: Vec<String> = Vec::new();
    for imp in &unit.imports {
        if !dll_map.contains_key(&imp.dll) {
            dll_map.insert(imp.dll.clone(), Vec::new());
        }
        let list = dll_map.get_mut(&imp.dll).unwrap();
        list.push(&imp.name);
        func_order.push(imp.name.clone());
    }

    // Build import data:
    // 1. Contiguous array of IMAGE_IMPORT_DESCRIPTORs (20 bytes each), terminated by null
    // 2. Then for each DLL: ILT + IAT + hint/name entries + DLL name
    let ndlls = dll_map.len();
    let desc_array_sz = (ndlls + 1) as u32 * 20; // ndlls descriptors + 1 null terminator
    let mut descriptors = vec![0u8; desc_array_sz as usize];
    let mut imp_data = Vec::new();
    let mut iat_map: HashMap<String, u32> = HashMap::new();

    for (dll_idx, (dll, funcs)) in dll_map.iter().enumerate() {
        let n = funcs.len() as u32;
        let ilt_sz = (n + 1) * 8;
        let iat_sz = ilt_sz;
        let mut hn_sz = 0u32;
        for f in funcs.iter() { hn_sz += 2 + f.len() as u32 + 1; }
        let data_rva = idata_va + desc_array_sz + imp_data.len() as u32;
        let (data, iat_rva) = build_import_data(dll, funcs, data_rva);
        // Fill descriptor in the descriptors array
        let dp = dll_idx * 20;
        descriptors[dp..dp + 4].copy_from_slice(&(data_rva).to_le_bytes());      // ILT RVA = start of data
        let name_rva = data_rva + ilt_sz + iat_sz + hn_sz;
        descriptors[dp + 12..dp + 16].copy_from_slice(&(name_rva).to_le_bytes()); // Name RVA
        descriptors[dp + 16..dp + 20].copy_from_slice(&(iat_rva).to_le_bytes());  // IAT RVA
        // Map function names to IAT entries
        for (i, f) in funcs.iter().enumerate() {
            iat_map.insert(f.to_string(), iat_rva + i as u32 * 8);
        }
        imp_data.extend_from_slice(&data);
    }
    // Null descriptor already zeroed at end of descriptors array

    // Concatenate: descriptors + data
    let mut imp_data_full = Vec::new();
    imp_data_full.extend_from_slice(&descriptors);
    imp_data_full.extend_from_slice(&imp_data);
    let imp_data = imp_data_full;

    let imp_sz = imp_data.len() as u32;
    let idata_fs = align_up(imp_sz, FA);
    let img_sz = align_up(idata_va + imp_sz, SA);
    let entry_va = text_va + unit.entry_point;

    // Patch code: string relocs
    let mut code = unit.code.clone();
    for &(coff, str_idx) in &unit.string_relocs {
        let ie = text_va + coff + 7;
        let sr = data_va + str_idx;
        let rel = sr as i64 - ie as i64;
        code[coff as usize + 3..coff as usize + 7].copy_from_slice(&(rel as i32).to_le_bytes());
    }

    // Patch code: import relocs (IAT relative to RIP)
    for &(coff, ref fnm) in &unit.import_relocs {
        if let Some(&iat_va) = iat_map.get(fnm) {
            let ie = text_va + coff + 6;
            let rel = iat_va as i64 - ie as i64;
            code[coff as usize + 2..coff as usize + 6].copy_from_slice(&(rel as i32).to_le_bytes());
        }
    }

    let mut pe = Vec::new();

    // === DOS HEADER (64 bytes) ===
    pe.extend_from_slice(b"MZ");
    pe.resize(60, 0);
    pe.extend_from_slice(&0x40u32.to_le_bytes()); // e_lfanew

    // === PE SIGNATURE ===
    pe.extend_from_slice(b"PE\0\0");

    // === COFF HEADER (20 bytes) ===
    pe.extend_from_slice(&0x8664u16.to_le_bytes());   // Machine
    pe.extend_from_slice(&3u16.to_le_bytes());         // 3 sections
    pe.extend_from_slice(&0u32.to_le_bytes());         // TimeDateStamp
    pe.extend_from_slice(&0u32.to_le_bytes());         // Symbols
    pe.extend_from_slice(&0u32.to_le_bytes());
    pe.extend_from_slice(&240u16.to_le_bytes());      // SizeOfOptionalHeader = 240 (includes data dirs)
    pe.extend_from_slice(&0x002Fu16.to_le_bytes());   // Characteristics

    // === OPTIONAL HEADER PE32+ (fixed fields + data dirs = 240 bytes) ===
    pe.extend_from_slice(&0x020Bu16.to_le_bytes());   // Magic
    pe.extend_from_slice(&[0x0E, 0x00]);              // Linker
    pe.extend_from_slice(&text_fs.to_le_bytes());     // SizeOfCode
    pe.extend_from_slice(&(data_fs + idata_fs).to_le_bytes()); // SizeOfInitData
    pe.extend_from_slice(&0u32.to_le_bytes());         // SizeOfUninitData
    pe.extend_from_slice(&entry_va.to_le_bytes());     // Entry
    pe.extend_from_slice(&text_va.to_le_bytes());      // BaseOfCode
    pe.extend_from_slice(&IMAGE_BASE.to_le_bytes());   // ImageBase
    pe.extend_from_slice(&SA.to_le_bytes());           // SectAlign
    pe.extend_from_slice(&FA.to_le_bytes());           // FileAlign
    pe.extend_from_slice(&[6, 0, 0, 0]);              // OS v6.0
    pe.extend_from_slice(&[0, 0, 0, 0]);              // ImageVer
    pe.extend_from_slice(&[6, 0, 0, 0]);              // Subsys v6.0
    pe.extend_from_slice(&0u32.to_le_bytes());
    pe.extend_from_slice(&img_sz.to_le_bytes());      // SizeOfImage
    pe.extend_from_slice(&hdr_sz.to_le_bytes());      // SizeOfHeaders
    pe.extend_from_slice(&0u32.to_le_bytes());         // CheckSum
    pe.extend_from_slice(&3u16.to_le_bytes());         // Subsystem: CONSOLE
    pe.extend_from_slice(&0x0100u16.to_le_bytes());   // DllChars
    pe.extend_from_slice(&0x100000u64.to_le_bytes()); // StackReserve
    pe.extend_from_slice(&0x1000u64.to_le_bytes());   // StackCommit
    pe.extend_from_slice(&0x100000u64.to_le_bytes()); // HeapReserve
    pe.extend_from_slice(&0x1000u64.to_le_bytes());   // HeapCommit
    pe.extend_from_slice(&0u32.to_le_bytes());         // LoaderFlags
    pe.extend_from_slice(&16u32.to_le_bytes());        // NumberOfRvaAndSizes

    // 16 data directories (8 bytes each = 128 bytes)
    pe.extend_from_slice(&[0u8; 8]);                   // Export
    pe.extend_from_slice(&idata_va.to_le_bytes());     // Import RVA
    pe.extend_from_slice(&imp_sz.to_le_bytes());       // Import Size
    pe.extend_from_slice(&[0u8; 14 * 8]);              // Rest

    // === SECTION HEADERS (3 * 40 = 120 bytes) ===
    // .text
    pe.extend_from_slice(b".text\0\0\0");
    pe.extend_from_slice(&code_sz.to_le_bytes());
    pe.extend_from_slice(&text_va.to_le_bytes());
    pe.extend_from_slice(&text_fs.to_le_bytes());
    pe.extend_from_slice(&text_fo.to_le_bytes());
    pe.extend_from_slice(&[0u8; 12]);
    pe.extend_from_slice(&0x60000020u32.to_le_bytes());

    // .data section (always present, use min VSize of 1 if no strings)
    pe.extend_from_slice(b".data\0\0\0");
    pe.extend_from_slice(&std::cmp::max(str_sz, 1).to_le_bytes()); // VirtualSize at least 1
    pe.extend_from_slice(&data_va.to_le_bytes());
    pe.extend_from_slice(&data_fs.to_le_bytes());
    pe.extend_from_slice(&data_fo.to_le_bytes());
    pe.extend_from_slice(&[0u8; 12]);
    pe.extend_from_slice(&0xC0000040u32.to_le_bytes());

    // .idata
    pe.extend_from_slice(b".idata\0\0");
    pe.extend_from_slice(&imp_sz.to_le_bytes());
    pe.extend_from_slice(&idata_va.to_le_bytes());
    pe.extend_from_slice(&idata_fs.to_le_bytes());
    pe.extend_from_slice(&idata_fo.to_le_bytes());
    pe.extend_from_slice(&[0u8; 12]);
    pe.extend_from_slice(&0xC0000040u32.to_le_bytes());

    // Pad to hdr_sz
    pe.resize(hdr_sz as usize, 0);

    // .text data
    pe.resize(text_fo as usize, 0);
    pe.extend_from_slice(&code);
    pe.resize((text_fo + text_fs) as usize, 0);

    // .data data (strings)
    pe.resize(data_fo as usize, 0);
    pe.extend_from_slice(&unit.strings);
    pe.resize((data_fo + data_fs) as usize, 0);

    // .idata data (imports)
    pe.resize(idata_fo as usize, 0);
    pe.extend_from_slice(&imp_data);
    pe.resize((idata_fo + idata_fs) as usize, 0);

    w.write_all(&pe)
}


