use std::path::Path;
use std::process::Command;

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git").args(arguments).current_dir(root).output()
        .expect("read qualification source identity with git");
    assert!(output.status.success(), "git {:?} failed: {}", arguments, String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("git source identity is UTF-8").trim().to_owned()
}

fn main() {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory");
    let root = Path::new(&manifest).join("../..").canonicalize().expect("owning checkout");
    let revision = git(&root, &["rev-parse", "HEAD"]);
    let changes = git(&root, &["status", "--porcelain=v1", "--untracked-files=all"]);
    assert!(changes.is_empty(), "qualification requires committed source; observed changes:\n{changes}");
    println!("cargo:rustc-env=QUALIFICATION_SOURCE_REVISION={revision}");
    for name in ["HEAD", "refs/heads/main", "packed-refs"] {
        let path = git(&root, &["rev-parse", "--git-path", name]);
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    for path in ["run.rs", "pipeline.rs", "support", "cases", "provider", "pricing", "js", "Cargo.toml", "Cargo.lock"] {
        println!("cargo:rerun-if-changed={path}");
    }
}
