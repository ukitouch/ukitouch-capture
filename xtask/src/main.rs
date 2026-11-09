use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

fn main() {
    if let Err(e) = run() {
        eprintln!("xtask: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    match env::args().nth(1).as_deref() {
        Some("dist") => dist(),
        Some(task) => Err(format!("unknown task: {task}").into()),
        None => Err("usage: cargo xtask dist".into()),
    }
}

fn dist() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let name = package_name(root)?;
    let platform = env::consts::OS;
    let entry = format!("{name}{}", env::consts::EXE_SUFFIX);

    build(root, &name)?;

    let mut manifest: serde_json::Map<String, Value> =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json"))?)?;
    manifest.insert("entry".into(), json!(entry));
    manifest.insert("platforms".into(), json!([platform]));

    let text = serde_json::to_string_pretty(&manifest)?;
    let bin = fs::read(root.join("target/release").join(&entry))?;

    let dist = root.join("dist");
    fs::create_dir_all(&dist)?;
    fs::write(dist.join("manifest.json"), &text)?;

    let mut zip = zip::ZipWriter::new(File::create(dist.join(format!("{name}-{platform}.zip")))?);
    zip.start_file("manifest.json", file_options(0o644))?;
    zip.write_all(text.as_bytes())?;
    zip.start_file(&entry, file_options(0o755))?;
    zip.write_all(&bin)?;
    zip.finish()?;

    Ok(())
}

fn file_options(unix: u32) -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(unix)
}

/// The root package name and also the binary name.
fn package_name(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let manifest: toml::Value = toml::from_str(&fs::read_to_string(root.join("Cargo.toml"))?)?;
    Ok(manifest["package"]["name"].as_str().unwrap().to_string())
}

fn build(root: &Path, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--release", "--package", name])
        .current_dir(root)
        .status()?;
    if !status.success() {
        return Err("cargo build failed".into());
    }
    Ok(())
}
