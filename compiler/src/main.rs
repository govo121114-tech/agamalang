// AgamaLang Compiler
// A mid-level compiled programming language targeting x86-64 Windows

mod token;
mod lexer;
mod ast;
mod parser;
mod codegen;
mod pe;

use std::env;
use std::fs;
use std::path::Path;
use lexer::Lexer;
use parser::Parser;
use codegen::compile;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: agamalang <source.aga> [output.exe]");
        eprintln!("");
        eprintln!("Compiles an AgamaLang source file to a Windows executable.");
        return;
    }

    let source_path = &args[1];
    let output_path = if args.len() > 2 {
        args[2].clone()
    } else {
        let stem = Path::new(source_path).file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        format!("{}.exe", stem)
    };

    // Read source file
    let source = match fs::read_to_string(source_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading '{}': {}", source_path, e);
            return;
        }
    };

    // Phase 1: Lexing
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize();

    // Phase 2: Parsing
    let mut parser = Parser::new(tokens);
    let program = parser.parse();

    // Phase 3: Code generation
    let compiled = compile(&program);

    // Phase 4: PE output
    let mut file = match fs::File::create(&output_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error creating '{}': {}", output_path, e);
            return;
        }
    };

    match pe::write_pe(&mut file, &compiled) {
        Ok(_) => {
            let metadata = fs::metadata(&output_path).unwrap();
            println!("✓ Compiled '{}' -> '{}' ({} bytes)", source_path, output_path, metadata.len());
        }
        Err(e) => {
            eprintln!("Error writing PE: {}", e);
        }
    }
}
