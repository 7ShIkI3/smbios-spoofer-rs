use std::env;
use std::path::PathBuf;
use std::process::Command;

fn find_msvc_lib_exe() -> Option<PathBuf> {
    if let Ok(p) = env::var("LIB_EXE") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let msvc_root = PathBuf::from(
        r"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC",
    );
    if let Ok(entries) = std::fs::read_dir(&msvc_root) {
        let mut versions: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|x| x.path())).collect();
        versions.sort();
        if let Some(latest) = versions.last() {
            let candidate = latest.join(r"bin\HostX64\x64\lib.exe");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    Some(PathBuf::from("lib.exe"))
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let def_path = manifest_dir.join("ntoskrnl.def");
    let lib_path = out_dir.join("ntoskrnl.lib");

    println!("cargo:rerun-if-changed=ntoskrnl.def");

    if let Some(lib_exe) = find_msvc_lib_exe() {
        let status = Command::new(lib_exe)
            .arg("/NOLOGO")
            .arg(format!("/DEF:{}", def_path.display()))
            .arg(format!("/OUT:{}", lib_path.display()))
            .arg("/MACHINE:X64")
            .status()
            .expect("Failed to execute lib.exe to build ntoskrnl.lib");
        assert!(status.success(), "lib.exe failed to generate ntoskrnl.lib");
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=dylib=ntoskrnl");

    // Configure MSVC link.exe to emit a relocatable 64-bit Windows Kernel WDM Driver (.sys)
    println!("cargo:rustc-link-arg-bins=/SUBSYSTEM:NATIVE,10.0");
    println!("cargo:rustc-link-arg-bins=/DRIVER");
    println!("cargo:rustc-link-arg-bins=/FIXED:NO");
    println!("cargo:rustc-link-arg-bins=/DYNAMICBASE");
    println!("cargo:rustc-link-arg-bins=/HIGHENTROPYVA");
    println!("cargo:rustc-link-arg-bins=/NXCOMPAT");
    println!("cargo:rustc-link-arg-bins=/ENTRY:DriverEntry");
    println!("cargo:rustc-link-arg-bins=/NODEFAULTLIB");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
    println!("cargo:rustc-link-arg-bins=/INTEGRITYCHECK");
    println!("cargo:rustc-link-arg-bins=/BREPRO");
}
