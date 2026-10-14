use lspkl_vfs::{SourceLoader, StandardSourceLoader};
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
    let root = fluent_uri::Uri::parse(format!("jar:{}!/", lspkl_vfs::path_to_uri(&path).unwrap()))
        .unwrap();
    let files = StandardSourceLoader::new();
    for member in ["dir/", "link.pkl"] {
        let uri = files.resolve_uri(&root, member).unwrap();
        assert!(
            files.load(&uri, temp.path()).is_err(),
            "{member} must not be parsed as a module"
        );
    }
    let empty = files.resolve_uri(&root, "empty.pkl").unwrap();
    assert_eq!(files.load(&empty, temp.path()).unwrap().contents(), "");
}

#[test]
fn bracketed_filenames_are_loadable_and_enumerated_as_valid_uris() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("literal[{.pkl");
    fs::File::create(&path)
        .unwrap()
        .write_all(b"value = 7\n")
        .unwrap();
    let files = StandardSourceLoader::new();
    let root = lspkl_vfs::path_to_uri(temp.path()).unwrap();
    let listed = files.list_files(&root).unwrap();
    assert_eq!(listed, [lspkl_vfs::path_to_uri(&path).unwrap()]);
    let direct = files.get_modulepath_file(&path, temp.path()).unwrap();
    assert_eq!(direct.url, listed[0]);
    assert_eq!(direct.contents(), "value = 7\n");
}

#[test]
fn relative_source_references_encode_literal_space_and_unicode() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nested/space ü.pkl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "value = 7\n").unwrap();
    let base = lspkl_vfs::path_to_uri(&temp.path().join("main.pkl")).unwrap();
    let files = StandardSourceLoader::new();
    let literal = files.resolve_uri(&base, "nested/space ü.pkl").unwrap();
    let escaped = files
        .resolve_uri(&base, "nested/space%20%C3%BC.pkl")
        .unwrap();
    assert_eq!(literal, escaped);
    assert_eq!(literal, lspkl_vfs::path_to_uri(&path).unwrap());
    assert_eq!(
        files.load(&literal, temp.path()).unwrap().contents(),
        "value = 7\n"
    );
}

#[test]
fn file_references_preserve_windows_drive_roots() {
    let files = StandardSourceLoader::new();
    let base = fluent_uri::Uri::parse("file:///C:/project/main.pkl".to_owned()).unwrap();
    for reference in ["/shared.pkl", "../../../shared.pkl"] {
        assert_eq!(
            files.resolve_uri(&base, reference).unwrap().as_str(),
            "file:///C:/shared.pkl"
        );
    }
    assert_eq!(
        files.resolve_uri(&base, "/D:/shared.pkl").unwrap().as_str(),
        "file:///D:/shared.pkl"
    );
    assert_eq!(
        files
            .resolve_uri(&base, "//server/shared.pkl")
            .unwrap()
            .as_str(),
        "file://server/shared.pkl"
    );
}

#[test]
fn colon_filenames_are_not_windows_drive_segments() {
    let files = StandardSourceLoader::new();
    let base = fluent_uri::Uri::parse("file:///C:source.pkl".to_owned()).unwrap();
    assert_eq!(
        files.resolve_uri(&base, "sibling.pkl").unwrap().as_str(),
        "file:///sibling.pkl"
    );
    let base = fluent_uri::Uri::parse("file:///C:/project/main.pkl".to_owned()).unwrap();
    assert_eq!(
        files.resolve_uri(&base, "/D:target.pkl").unwrap().as_str(),
        "file:///C:/D:target.pkl"
    );
}

#[test]
fn file_references_replace_base_fragments_without_losing_reference_fragments() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("sibling.pkl");
    fs::write(&path, "value = 7\n").unwrap();
    let base = fluent_uri::Uri::parse(format!(
        "{}#view",
        lspkl_vfs::path_to_uri(&temp.path().join("main.pkl")).unwrap()
    ))
    .unwrap();
    let files = StandardSourceLoader::new();
    let sibling = files.resolve_uri(&base, "./sibling.pkl").unwrap();
    assert_eq!(sibling, lspkl_vfs::path_to_uri(&path).unwrap());
    assert_eq!(
        files.load(&sibling, temp.path()).unwrap().contents(),
        "value = 7\n"
    );
    assert_eq!(
        files
            .resolve_uri(&base, "./sibling.pkl#new")
            .unwrap()
            .as_str(),
        format!("{sibling}#new")
    );
    assert_eq!(
        files.resolve_uri(&base, "#new").unwrap().as_str(),
        base.as_str().replace("#view", "#new")
    );
}
