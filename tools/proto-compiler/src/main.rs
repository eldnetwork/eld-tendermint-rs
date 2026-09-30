//! Generate Prost sources from `proto/tendermint`.
//!
//! prost-build settings follow tendermint-rs `tools/proto-compiler`: `bytes` for
//! ABCI byte fields, and extern paths for `google.protobuf.Timestamp` and
//! `Duration`. Service stubs are not generated.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("tools/proto-compiler lives two levels below the repo root");

    let proto_dir = repo_root.join("proto");
    let mut protos = Vec::new();
    collect_protos(&proto_dir.join("tendermint"), &mut protos);
    protos.sort();
    if protos.is_empty() {
        eprintln!(
            "no .proto files under {}",
            proto_dir.join("tendermint").display()
        );
        process::exit(1);
    }

    let out_dir = env::temp_dir().join(format!("eld-tendermint-proto-{}", process::id()));
    let _ = fs::remove_dir_all(&out_dir);
    fs::create_dir_all(&out_dir).unwrap_or_else(|err| {
        eprintln!("create {}: {err}", out_dir.display());
        process::exit(1);
    });

    let includes = [proto_dir.clone(), proto_dir.join("third_party")];
    let mut config = prost_build::Config::new();
    config.bytes([".tendermint.abci"]);
    config.extern_path(".google.protobuf.Timestamp", "::prost_types::Timestamp");
    config.extern_path(".google.protobuf.Duration", "::prost_types::Duration");
    config.out_dir(&out_dir);

    if let Err(err) = config.compile_protos(&protos, &includes) {
        eprintln!("{err}");
        process::exit(1);
    }

    let dest = repo_root.join("crates/proto/src/prost");
    fs::create_dir_all(&dest).unwrap_or_else(|err| {
        eprintln!("create {}: {err}", dest.display());
        process::exit(1);
    });
    clear_rs(&dest);
    copy_rs(&out_dir, &dest);
    let _ = fs::remove_dir_all(&out_dir);
}

fn collect_protos(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("read {}: {err}", dir.display());
            process::exit(1);
        }
    };
    for entry in entries {
        let path = entry
            .unwrap_or_else(|err| {
                eprintln!("{err}");
                process::exit(1);
            })
            .path();
        if path.is_dir() {
            collect_protos(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("proto") {
            out.push(path);
        }
    }
}

fn clear_rs(dir: &Path) {
    for entry in fs::read_dir(dir).unwrap_or_else(|err| {
        eprintln!("read {}: {err}", dir.display());
        process::exit(1);
    }) {
        let path = entry
            .unwrap_or_else(|err| {
                eprintln!("{err}");
                process::exit(1);
            })
            .path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            fs::remove_file(&path).unwrap_or_else(|err| {
                eprintln!("remove {}: {err}", path.display());
                process::exit(1);
            });
        }
    }
}

fn copy_rs(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap_or_else(|err| {
        eprintln!("read {}: {err}", from.display());
        process::exit(1);
    }) {
        let path = entry
            .unwrap_or_else(|err| {
                eprintln!("{err}");
                process::exit(1);
            })
            .path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            let dest = to.join(path.file_name().expect("file name"));
            fs::copy(&path, &dest).unwrap_or_else(|err| {
                eprintln!("copy {} -> {}: {err}", path.display(), dest.display());
                process::exit(1);
            });
            println!("wrote {}", dest.display());
        }
    }
}
