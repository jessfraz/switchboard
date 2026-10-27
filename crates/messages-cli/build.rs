use std::{env, ffi::OsString, path::PathBuf, process::Command};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=native/contacts.m");
    println!("cargo:rerun-if-changed=native/files.c");
    if env::var("CARGO_CFG_TARGET_FAMILY")? != "unix" {
        return Ok(());
    }
    let target = env::var("TARGET")?;
    let host = env::var("HOST")?;
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is missing")?);
    let compiler = target_tool("CC", &target, &host).unwrap_or_else(|| "cc".into());
    let archiver = target_tool("AR", &target, &host).unwrap_or_else(|| "ar".into());
    let macos = env::var("CARGO_CFG_TARGET_OS")? == "macos";
    let mut target_flags = Vec::new();
    if macos {
        // cargo-dist can build Intel artifacts on Apple Silicon runners. A host
        // cc invocation would otherwise silently archive ARM objects.
        let arch = match env::var("CARGO_CFG_TARGET_ARCH")?.as_str() {
            "aarch64" => "arm64",
            "x86_64" => "x86_64",
            _ => return Err("unsupported macOS target architecture".into()),
        };
        target_flags.extend([OsString::from("-arch"), OsString::from(arch)]);
        println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
        let deployment = env::var("MACOSX_DEPLOYMENT_TARGET")
            .unwrap_or_else(|_| if arch == "arm64" { "11.0" } else { "10.12" }.into());
        target_flags.push(format!("-mmacosx-version-min={deployment}").into());
    }
    let files_object = output.join("messages_files.o");
    if !Command::new(&compiler)
        .args(&target_flags)
        .args(["-c", "native/files.c", "-o"])
        .arg(&files_object)
        .status()?
        .success()
    {
        return Err("failed to compile Unix attachment helper".into());
    }
    let mut objects = vec![files_object];
    if macos {
        let object = output.join("messages_contacts.o");
        if !Command::new(&compiler)
            .args(&target_flags)
            .args(["-c", "-fobjc-arc", "-fblocks", "native/contacts.m", "-o"])
            .arg(&object)
            .status()?
            .success()
        {
            return Err("failed to compile macOS Contacts helper".into());
        }
        objects.push(object);
    }
    let archive = output.join("libmessages_native.a");
    if !Command::new(archiver)
        .arg("crs")
        .arg(&archive)
        .args(objects)
        .status()?
        .success()
    {
        return Err("failed to archive native Messages helpers".into());
    }
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=messages_native");
    if macos {
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=Contacts");
    }
    Ok(())
}

// Follow the target-specific tool convention used by cc-rs/cargo-zigbuild.
// Values are executable paths, never shell commands or evaluated text.
fn target_tool(name: &str, target: &str, host: &str) -> Option<OsString> {
    let prefix = if target == host { "HOST" } else { "TARGET" };
    let names = [
        format!("{name}_{target}"),
        format!("{name}_{}", target.replace('-', "_")),
        format!("{prefix}_{name}"),
        name.into(),
    ];
    for name in &names {
        println!("cargo:rerun-if-env-changed={name}");
    }
    names.iter().find_map(env::var_os)
}
