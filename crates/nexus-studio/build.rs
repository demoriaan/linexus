//! Embeds every file under `apps/nexus-studio/` into the binary.
//!
//! Writes `$OUT_DIR/assets.rs`: a table of `Asset { path, mime, bytes }`
//! sorted by path, with the bytes pulled in through `include_bytes!`. The
//! server therefore needs no list of frontend file names; whatever the
//! frontend ships is served. Hidden files (dot files) are skipped so editor
//! and tool droppings never reach the browser.

#[path = "src/mime.rs"]
mod mime;

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("set by cargo"));
    let root = manifest_dir.join("../../apps/nexus-studio");
    // Cargo scans a directory given here recursively for changes.
    println!("cargo:rerun-if-changed={}", root.display());

    let mut files = Vec::new();
    if root.is_dir() {
        collect(&root, &root, &mut files);
    } else {
        println!(
            "cargo:warning=frontend directory {} is missing; Studio will serve no UI",
            root.display()
        );
    }
    files.sort();

    let mut table = String::from("pub static ASSETS: &[Asset] = &[\n");
    for (rel, abs) in &files {
        writeln!(
            table,
            "    Asset {{ path: {rel:?}, mime: {:?}, bytes: include_bytes!({abs:?}) }},",
            mime::for_path(rel)
        )
        .expect("writing to a String cannot fail");
    }
    table.push_str("];\n");
    // One validator for the whole UI: any changed file changes it, so a
    // browser never mixes modules from two builds.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (rel, abs) in &files {
        let bytes = fs::read(abs).unwrap_or_default();
        for byte in rel.bytes().chain([0]).chain(bytes) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    writeln!(
        table,
        "pub const ASSETS_ETAG: &str = \"\\\"{}-{hash:016x}\\\"\";",
        env::var("CARGO_PKG_VERSION").unwrap_or_default()
    )
    .expect("writing to a String cannot fail");
    fs::write(out_dir.join("assets.rs"), table).expect("failed to write the asset table");
}

/// Appends `(path relative to root with '/' separators, absolute path)` for
/// every regular file below `dir`. Names that are not UTF-8 are skipped: a
/// URL could not name them unambiguously anyway.
fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, String)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            collect(root, &path, files);
        } else if meta.is_file() {
            let Some(rel) = relative_url_path(root, &path) else {
                continue;
            };
            let abs = path.canonicalize().unwrap_or(path);
            if let Some(abs) = abs.to_str() {
                files.push((rel, abs.to_string()));
            }
        }
    }
}

fn relative_url_path(root: &Path, path: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect();
    Some(parts?.join("/"))
}
