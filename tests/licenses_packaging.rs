//! Verify that `cargo package` ships everything the `gitoxide` build script
//! compiles. `build.rs` pulls shared modules out of `gitoxide-core` with
//! `#[path]`, so they live outside this package's own `src/` tree and are
//! only packaged if the `include` list in `Cargo.toml` names them. A crate
//! that builds from the workspace but not from its published tarball fails
//! for the first time at release, so this asks cargo which files it would
//! package and checks that every `#[path]` target is among them.

use std::collections::BTreeSet;
use std::process::Command;

#[test]
fn every_build_script_path_module_is_packaged() {
    let build_rs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs")).expect("read build.rs");
    let path_modules: BTreeSet<&str> = build_rs
        .lines()
        .filter_map(|line| line.trim().strip_prefix("#[path = \"")?.strip_suffix("\"]"))
        .collect();
    assert!(
        !path_modules.is_empty(),
        "expected build.rs to declare `#[path = \"...\"]` modules, but the scan found none"
    );

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["package", "--package", "gitoxide", "--list", "--allow-dirty"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run `cargo package --list`");
    assert!(
        output.status.success(),
        "`cargo package --list` failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let packaged: BTreeSet<&str> = std::str::from_utf8(&output.stdout)
        .expect("file list is UTF-8")
        .lines()
        .collect();

    let missing: Vec<&str> = path_modules
        .iter()
        .copied()
        .filter(|module| !packaged.contains(module))
        .collect();
    assert!(
        missing.is_empty(),
        "build.rs `#[path]` modules that `cargo package` would not ship \
         (extend `include` in Cargo.toml): {missing:?}"
    );
}
