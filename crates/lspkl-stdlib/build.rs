use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};

// Pin the source revision and both official archive formats when upgrading.
const VERSION: &str = "0.33.0";
const SOURCE_URL: &str =
    "https://codeload.github.com/apple/pkl/zip/5ee1f296e3503abd8ac8e9b27d197b2f779ea408";
const SOURCE_PREFIX: &str = "pkl-5ee1f296e3503abd8ac8e9b27d197b2f779ea408/stdlib/";
const SOURCE_SHA256: &str = "52431e90a999872c53dfe6083d6cded5672f7194730bfee6bf1ab19d0461669e";
const STDLIB_SHA256: &str = "62cf7d26371cc3c01a107457d312976d1857da47e9b271ae6f6a38253b103bc3";

#[derive(Clone, Copy)]
enum ArchiveLayout {
    Source,
    Stdlib,
}

impl ArchiveLayout {
    fn module_prefix(self) -> &'static str {
        match self {
            Self::Source => SOURCE_PREFIX,
            Self::Stdlib => "org/pkl/stdlib/",
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PKL_STDLIB_URL");
    println!("cargo:rerun-if-env-changed=PKL_STDLIB_ARCHIVE");
    println!("cargo:rerun-if-env-changed=SSL_CERT_FILE");
    println!("cargo:rerun-if-env-changed=SSL_CERT_DIR");
    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR is missing")?);
    let (bytes, layout) = load_archive(&out_dir)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let module_dir = out_dir.join("stdlib");
    fs::create_dir_all(&module_dir)?;
    let mut modules = BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(name) = entry
            .name()
            .strip_prefix(layout.module_prefix())
            .and_then(|name| name.strip_suffix(".pkl"))
        else {
            continue;
        };
        // This is documentation-package metadata, not an importable stdlib module.
        if name == "doc-package-info" {
            continue;
        }
        if entry.is_dir()
            || entry.is_symlink()
            || name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(format!("unexpected stdlib archive member: {}", entry.name()).into());
        }
        let name = name.to_owned();
        let mut source = String::with_capacity(entry.size() as usize);
        entry.read_to_string(&mut source)?;
        fs::write(module_dir.join(format!("{name}.pkl")), source)?;
        modules.insert(name);
    }
    if !modules.contains("base") {
        return Err("the stdlib artifact is missing base.pkl".into());
    }

    let mut generated = format!(
        "/// The Pkl version of the embedded sources.\npub const VERSION: &str = {VERSION:?};\n\
         /// Importable modules from the pinned standard-library artifact.\npub const MODULE_NAMES: &[&str] = &[\n"
    );
    for name in &modules {
        writeln!(generated, "    {name:?},")?;
    }
    generated.push_str(
        "];\n\n/// Look up a module by its `pkl:` name, without the URI scheme.\n\
         pub fn source(name: &str) -> Option<&'static str> {\n    match name {\n",
    );
    for name in &modules {
        if name == "base" {
            generated.push_str("        \"base\" => Some(BASE),\n");
        } else {
            writeln!(
                generated,
                "        {name:?} => Some(include_str!(concat!(env!(\"OUT_DIR\"), \"/stdlib/{name}.pkl\"))),"
            )?;
        }
    }
    generated.push_str("        _ => None,\n    }\n}\n");
    fs::write(out_dir.join("stdlib.rs"), generated)?;
    Ok(())
}

fn load_archive(out_dir: &Path) -> Result<(Vec<u8>, ArchiveLayout), Box<dyn Error>> {
    if let Some(path) = std::env::var_os("PKL_STDLIB_ARCHIVE") {
        let path = Path::new(&path);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = fs::read(path).map_err(|error| {
            format!("could not read stdlib archive {}: {error}", path.display())
        })?;
        let layout = verify_archive(&bytes)?;
        return Ok((bytes, layout));
    }
    let path = out_dir.join(format!("pkl-stdlib-{VERSION}.zip"));
    match fs::read(&path) {
        Ok(bytes) => {
            let layout = verify_archive(&bytes).map_err(|error| {
                format!(
                    "cached stdlib at {}: {error}; remove the archive and rebuild",
                    path.display()
                )
            })?;
            Ok((bytes, layout))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let url = match std::env::var("PKL_STDLIB_URL") {
                Ok(url) => url,
                Err(std::env::VarError::NotPresent) => SOURCE_URL.to_owned(),
                Err(error) => return Err(error.into()),
            };
            eprintln!("Downloading Pkl {VERSION} stdlib from {url}");
            let mut response = reqwest::blocking::Client::builder()
                .https_only(true)
                .timeout(Duration::from_secs(60))
                .build()?
                .get(&url)
                .send()
                .map_err(|error| format!("could not download {url}: {error:?}; a fresh stdlib build requires network access"))?
                .error_for_status()?;
            let mut bytes = Vec::with_capacity(response.content_length().unwrap_or(0) as usize);
            response.copy_to(&mut bytes)?;
            let layout = verify_archive(&bytes)?;
            let mut temporary = tempfile::NamedTempFile::new_in(out_dir)?;
            temporary.write_all(&bytes)?;
            temporary.persist(&path)?;
            Ok((bytes, layout))
        }
        Err(error) => Err(error.into()),
    }
}

fn verify_archive(bytes: &[u8]) -> Result<ArchiveLayout, Box<dyn Error>> {
    let checksum = format!("{:x}", Sha256::digest(bytes));
    match checksum.as_str() {
        SOURCE_SHA256 => Ok(ArchiveLayout::Source),
        STDLIB_SHA256 => Ok(ArchiveLayout::Stdlib),
        _ => Err(format!(
            "stdlib checksum mismatch: expected source archive {SOURCE_SHA256} or stdlib artifact {STDLIB_SHA256}, got {checksum}"
        )
        .into()),
    }
}
