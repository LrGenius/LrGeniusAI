//! Links clang's compiler-rt on macOS.
//!
//! ONNX Runtime's CoreML provider (prebuilt into `ort-sys`) uses
//! Objective-C `@available(...)` checks, which the compiler lowers to calls to
//! `___isPlatformVersionAtLeast`. That symbol lives in clang's compiler-rt
//! (`libclang_rt.osx.a`), which a clang-driven link normally adds on its own —
//! but rustc links with `-nodefaultlibs`, and `ort-sys` does not add it, so any
//! binary that pulls in `ort` with the `coreml` feature fails with
//! "Undefined symbols: ___isPlatformVersionAtLeast" (first seen with ort
//! 2.0.0-rc.14 / ONNX Runtime 1.31).
//!
//! A `rustc-link-lib` from a library crate reaches every binary and test that
//! depends on it, so doing this here covers `lrg-server` and all test targets.
//!
//! The archive is linked `-bundle`. A bundled static library is copied into
//! the rlib, where it sits *before* the `ort-sys` rlib that needs it; the
//! linker then never pulls the member out, and targets that link little else
//! (the `bench_image` example) still fail with the undefined symbol. Unbundled,
//! `-lclang_rt.osx` goes on the final link line after every rlib.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let resource_dir = Command::new("clang")
        .arg("--print-resource-dir")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let Some(resource_dir) = resource_dir else {
        println!(
            "cargo:warning=`clang --print-resource-dir` failed; not linking clang_rt.osx. \
             Linking may fail with an undefined `___isPlatformVersionAtLeast`."
        );
        return;
    };

    let lib_dir = format!("{resource_dir}/lib/darwin");
    if !std::path::Path::new(&format!("{lib_dir}/libclang_rt.osx.a")).exists() {
        println!(
            "cargo:warning=libclang_rt.osx.a not found in {lib_dir}; not linking it. \
             Linking may fail with an undefined `___isPlatformVersionAtLeast`."
        );
        return;
    }

    println!("cargo:rustc-link-search=native={lib_dir}");
    println!("cargo:rustc-link-lib=static:-bundle=clang_rt.osx");
}
