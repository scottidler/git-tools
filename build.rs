// build.rs
use std::env;
use std::fs::{File, read_to_string};
use std::io::Write;
use std::path::Path;
use std::process::Command;

fn git_describe_value() -> String {
    // First, check if GIT_DESCRIBE env var is set and use it if so
    if let Ok(value) = env::var("GIT_DESCRIBE") {
        return value;
    }

    // Fallback to using git command
    Command::new("git")
        .args(&["describe", "--tags", "--always"])
        .output()
        .ok()
        .and_then(|output| if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            None
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn main() {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let dest_path = Path::new(&out_dir).join("git_describe.rs");
    let current_version = git_describe_value();

    // Compare with existing version, if any, to determine if we need to update
    let old_version = read_to_string(&dest_path).unwrap_or_default();
    if old_version.contains(&current_version) {
        println!("Version unchanged, skipping update.");
        return;
    }

    let mut f = File::create(&dest_path).expect("Could not create file");
    writeln!(f, "pub const GIT_DESCRIBE: &str = \"{}\";", current_version).expect("Could not write to file");

    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-env-changed=GIT_DESCRIBE");
}

