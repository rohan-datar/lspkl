use lspkl_project::{
    cli::PklCli,
    dto::{PackageDependency, PackageUri},
    package_manager::{PackageError, PackageManager},
};
use sha2::{Digest, Sha256};
use std::{fs, io::Write};

#[test]
fn cached_package_metadata_must_match_the_requested_checksum() {
    let temp = tempfile::tempdir().unwrap();
    let identity = "package://example.test/a@1.0.0";
    let metadata = format!(
        "{{\"dependencies\":{{}},\"name\":\"a\",\"packageZipUrl\":\"https://example.test/a.zip\",\"version\":\"1.0.0\",\"packageUri\":\"{identity}\"}}"
    );
    let checksum = format!("{:x}", Sha256::digest(metadata.as_bytes()));
    let uri = PackageUri::create(&format!("{identity}::sha256:{checksum}")).unwrap();
    let metadata_file = temp.path().join(&uri.relative_metadata_files()[0]);
    fs::create_dir_all(metadata_file.parent().unwrap()).unwrap();
    fs::write(&metadata_file, metadata).unwrap();
    let zip_file = temp.path().join(&uri.relative_zip_files()[0]);
    let mut zip = zip::ZipWriter::new(fs::File::create(zip_file).unwrap());
    zip.start_file("main.pkl", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"value = 1\n").unwrap();
    zip.finish().unwrap();
    let dependency = PackageDependency {
        uri,
        checksums: None,
    };
    let manager = PackageManager::new(PklCli::default());
    assert!(
        manager
            .get_library_roots(&dependency, temp.path())
            .unwrap()
            .is_some()
    );
    fs::write(&metadata_file, "{}").unwrap();
    assert!(matches!(
        manager.get_library_roots(&dependency, temp.path()),
        Err(PackageError::ChecksumMismatch { .. })
    ));
}
