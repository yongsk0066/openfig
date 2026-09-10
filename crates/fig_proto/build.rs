use std::io::Result;
use std::path::PathBuf;

const PROTOC_REQUIRED: &str = "protoc >= 3.12 required: brew install protobuf";

enum Version {
    V1([u32; 3]),
    V2([u32; 2]),
}

/// Try to find the version of protoc installed on the system.
fn protoc_version() -> Option<Version> {
    let output = std::process::Command::new("protoc").arg("--version").output().ok()?;
    let version = String::from_utf8(output.stdout).ok()?;
    eprintln!("protoc version: {version:?}");

    let version = version.trim();
    eprintln!("version: {version:?}");
    let version = version.split(' ').next_back().expect("No version");
    let version = version.split('.').collect::<Vec<_>>();
    let version = version
        .iter()
        .map(|s| s.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;
    match version.len() {
        3 => Some(Version::V1([version[0], version[1], version[2]])),
        2 => Some(Version::V2([version[0], version[1]])),
        _ => None,
    }
}

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=build.rs");

    let proto_files = std::fs::read_dir("../../proto")?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "proto"))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();

    for file in &proto_files {
        println!("cargo:rerun-if-changed={}", file.display());
    }

    // --experimental_allow_proto3_optional is supported only on version of protoc >= 3.12
    // if the system protoc is missing or too old, we must panic
    let version = match protoc_version() {
        Some(Version::V1([major, minor, _]) | Version::V2([major, minor])) => (major, minor),
        None => panic!("{PROTOC_REQUIRED}"),
    };

    assert!(version >= (3, 12), "{PROTOC_REQUIRED}");

    let mut config = prost_build::Config::new();

    config.protoc_arg("--experimental_allow_proto3_optional");

    #[cfg(feature = "arbitrary")]
    config.type_attribute(
        ".",
        "#[cfg_attr(feature = \"arbitrary\", derive(arbitrary::Arbitrary))]",
    );

    config.extern_path(".fig_common.Empty", "()");

    prost_reflect_build::Builder::new()
        .file_descriptor_set_path(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("file_descriptor_set.bin"))
        .descriptor_pool("crate::DESCRIPTOR_POOL")
        .compile_protos_with_config(config, &proto_files, &["../../proto"])?;

    Ok(())
}
