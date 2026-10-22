// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let flag = args.next().unwrap_or_else(|| {
        eprintln!("usage: panelet-rune-unit-compiler-poc --output <path.pnu>");
        std::process::exit(2);
    });

    if flag != "--output" {
        eprintln!("usage: panelet-rune-unit-compiler-poc --output <path.pnu>");
        std::process::exit(2);
    }

    let output = PathBuf::from(args.next().unwrap_or_else(|| {
        eprintln!("usage: panelet-rune-unit-compiler-poc --output <path.pnu>");
        std::process::exit(2);
    }));

    if args.next().is_some() {
        eprintln!("usage: panelet-rune-unit-compiler-poc --output <path.pnu>");
        std::process::exit(2);
    }

    let package = panelet_rune_unit_compiler_poc::fixture_package().unwrap_or_else(|error| {
        eprintln!("failed to compile fixture: {error}");
        std::process::exit(1);
    });

    std::fs::write(&output, &package).unwrap_or_else(|error| {
        eprintln!("failed to write {}: {error}", output.display());
        std::process::exit(1);
    });

    println!("wrote {} bytes to {}", package.len(), output.display());
}
