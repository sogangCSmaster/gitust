use std::{env, fs, io, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let assets = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?)
        .join("../../packages/design-system/dist/assets")
        .canonicalize()?;
    println!("cargo:rerun-if-changed={}", assets.display());
    let mut files = fs::read_dir(&assets)?.collect::<Result<Vec<_>, _>>()?;
    files.sort_by_key(|file| file.file_name());
    for required in ["index.js", "index.css"] {
        if !files.iter().any(|file| file.file_name() == required) {
            return Err(io::Error::other(format!("Missing UI entry asset: {required}")).into());
        }
    }
    let mut source = String::from("static EMBEDDED_ASSETS: &[EmbeddedAsset] = &[\n");
    for file in files {
        if !file.file_type()?.is_file() {
            return Err(
                io::Error::other("UI assets must be regular files in a flat directory").into(),
            );
        }
        let path = file.path();
        let name = file
            .file_name()
            .into_string()
            .map_err(|_| io::Error::other("UI asset names must be UTF-8"))?;
        let content_type = match path.extension().and_then(|ext| ext.to_str()) {
            Some("js") => "application/javascript; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            _ => return Err(io::Error::other(format!("Unsupported UI asset: {name}")).into()),
        };
        source.push_str(&format!(
            "EmbeddedAsset {{ name: {name:?}, content_type: {content_type:?}, bytes: include_bytes!({path:?}) }},\n",
        ));
    }
    source.push_str("];\n");
    fs::write(
        PathBuf::from(env::var("OUT_DIR")?).join("webui_assets.rs"),
        source,
    )?;
    Ok(())
}
