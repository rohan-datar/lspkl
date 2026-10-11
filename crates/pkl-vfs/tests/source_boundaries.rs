use pkl_vfs::FileManager;
use std::{fs, io::Write};

#[test]
fn directories_and_symlinks_are_not_empty_archive_modules() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("modules.zip");
    let mut archive = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    archive.add_directory("dir/", options).unwrap();
    archive
        .add_symlink("link.pkl", "empty.pkl", options)
        .unwrap();
    archive.start_file("empty.pkl", options).unwrap();
    archive.finish().unwrap();
    let root =
        fluent_uri::Uri::parse(format!("jar:{}!/", pkl_vfs::path_to_uri(&path).unwrap())).unwrap();
    let files = FileManager::new();
    for member in ["dir/", "link.pkl"] {
        let uri = files.resolve_uri(&root, member).unwrap();
        assert!(
            files.get_or_load(&uri).is_err(),
            "{member} must not be parsed as a module"
        );
    }
    let empty = files.resolve_uri(&root, "empty.pkl").unwrap();
    assert_eq!(files.get_or_load(&empty).unwrap().contents(), "");
}

#[test]
fn bracketed_filenames_are_loadable_and_enumerated_as_valid_uris() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("literal[{.pkl");
    fs::File::create(&path)
        .unwrap()
        .write_all(b"value = 7\n")
        .unwrap();
    let files = FileManager::new();
    let root = pkl_vfs::path_to_uri(temp.path()).unwrap();
    let listed = files.list_files(&root).unwrap();
    assert_eq!(listed, [pkl_vfs::path_to_uri(&path).unwrap()]);
    let direct = files.get_modulepath_file(&path).unwrap();
    assert_eq!(direct.url, listed[0]);
    assert_eq!(direct.contents(), "value = 7\n");
}
