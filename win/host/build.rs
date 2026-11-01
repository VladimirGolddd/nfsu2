// Links the game runtime (out/win/libnfsu2rt.a, made by win/build.sh) whole:
// its weak stand-ins (runtime/web/null_platform.c) must stay in, and nothing
// in this crate names most of its symbols.
fn main() {
    let dir = std::env::var("NFSU2_RT_LIB").unwrap_or_else(|_| "../../out/win".into());
    let dir = std::fs::canonicalize(&dir).unwrap_or_else(|_| panic!("NFSU2_RT_LIB: {dir} not found (run win/build.sh)"));
    println!("cargo:rerun-if-env-changed=NFSU2_RT_LIB");
    println!("cargo:rerun-if-changed={}", dir.join("libnfsu2rt.a").display());
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static:+whole-archive,-bundle=nfsu2rt");
    // winpthreads static: no libwinpthread-1.dll next to the program
    println!("cargo:rustc-link-lib=static=pthread");
    for l in ["bcrypt", "synchronization", "winmm"] { println!("cargo:rustc-link-lib={l}"); }
    // a console for the log when started from one; the window either way
    println!("cargo:rustc-link-arg-bins=-Wl,--stack,16777216");
}
