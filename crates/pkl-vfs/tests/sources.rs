use fluent_uri::Uri;
use pkl_vfs::{FileManager, Origin, effective_uri, to_editor_uri};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
fn uri(value: &str) -> Uri<String> {
    Uri::parse(value.to_owned()).unwrap()
}
fn archive(path: &std::path::Path, value: &str) -> Uri<String> {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, text) in [
        ("dir/main.pkl", value),
        ("other.pkl", "other = 2"),
        ("dir/deep/child.pkl", "child = 3"),
        ("../escape.pkl", "bad = true"),
        ("dir2/no.pkl", "no = true"),
        ("space + ü.pkl", "unicode = true"),
    ] {
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    uri(&format!(
        "jar:{}!/",
        url::Url::from_file_path(path).unwrap()
    ))
}
#[test]
fn archives_load_resolve_list_and_keep_distinct_identities() {
    let dir = tempfile::tempdir().unwrap();
    let a = archive(&dir.path().join("first archive.zip"), "value = 1");
    let b = archive(&dir.path().join("second.jar"), "value = 2");
    let fm = FileManager::new();
    let first = fm.resolve_uri(&a, "dir/main.pkl").unwrap();
    let second = fm.resolve_uri(&b, "dir/main.pkl").unwrap();
    assert_eq!(fm.get_or_load(&first).unwrap().contents(), "value = 1");
    assert_eq!(fm.get_or_load(&second).unwrap().contents(), "value = 2");
    assert_eq!(fm.get_or_load(&first).unwrap().origin, Origin::Jar);
    assert!(fm.parse(&first).unwrap().node_count() > 0);
    let other = fm.resolve_uri(&first, "../other.pkl").unwrap();
    assert_eq!(fm.get_or_load(&other).unwrap().contents(), "other = 2");
    assert!(fm.resolve_uri(&first, "../../escape.pkl").is_err());
    assert!(fm.resolve_uri(&first, "%2e%2e/%2e%2e/escape.pkl").is_err());
    let folder = fm.resolve_uri(&a, "dir/").unwrap();
    assert_eq!(fm.list_files(&folder).unwrap().len(), 2);
    assert_eq!(fm.list_files(&a).unwrap().len(), 5);
    let unicode = fm.resolve_uri(&a, "space%20%2B%20%C3%BC.pkl").unwrap();
    assert_eq!(
        fm.get_or_load(&unicode).unwrap().contents(),
        "unicode = true"
    );
    assert!(!dir.path().join("escape.pkl").exists());
    fs::write(dir.path().join("broken.zip"), "not a zip").unwrap();
    let broken = uri(&format!(
        "jar:{}!/a.pkl",
        url::Url::from_file_path(dir.path().join("broken.zip")).unwrap()
    ));
    assert!(fm.get_or_load(&broken).is_err());
}
#[test]
fn stdlib_all_modules_are_embedded_parseable_and_roundtrip() {
    let fm = FileManager::new();
    assert_eq!(pkl_vfs::stdlib::MODULE_NAMES.len(), 23);
    for name in pkl_vfs::stdlib::MODULE_NAMES {
        let physical = uri(&format!("pkl:{name}"));
        let editor = to_editor_uri(&physical).unwrap();
        assert_eq!(editor.as_str(), format!("pkl-lsp://stdlib/{name}.pkl"));
        assert_eq!(effective_uri(&editor).unwrap(), physical);
        let file = fm.get_or_load(&editor).unwrap();
        assert_eq!(file.origin, Origin::Stdlib);
        assert!(file.contents().contains("module pkl."));
        assert!(fm.parse(&physical).unwrap().node_count() > 0);
    }
    assert_eq!(
        fm.get_or_load(&uri("pkl:base")).unwrap().contents(),
        include_str!("../../pkl-types/tests/fixtures/base.pkl")
    );
    assert!(fm.get_or_load(&uri("pkl:missing")).is_err());
}
#[test]
fn editor_uri_roundtrips_and_registry_uses_one_identity() {
    for physical in [
        "file:///tmp/a%20b.pkl",
        "pkl:json",
        "https://example.com/a%20b+c.pkl?q=one%20two&x=+",
        "jar:file:///tmp/a%20b.jar!/dir/a%20b.pkl",
    ] {
        let physical = effective_uri(&uri(physical)).unwrap();
        let editor = to_editor_uri(&physical).unwrap();
        assert_eq!(effective_uri(&editor).unwrap(), physical);
        let fm = FileManager::new();
        let file = fm.open(editor.clone(), "x = 1".into(), Some(1));
        assert_eq!(file.url, physical);
        assert!(Arc::ptr_eq(&file, &fm.get(&physical).unwrap()));
        fm.update(&editor, "x = 2".into(), Some(2)).unwrap();
        assert_eq!(fm.get(&physical).unwrap().contents(), "x = 2");
        assert!(Arc::ptr_eq(
            &fm.parse(&editor).unwrap(),
            &fm.parse(&physical).unwrap()
        ));
        assert_eq!(fm.len(), 1);
        fm.close(&editor);
        assert!(fm.is_empty());
    }
    assert_eq!(
        to_editor_uri(&uri("https://example.com/a.pkl"))
            .unwrap()
            .as_str(),
        "pkl-lsp://https/https%3A%2F%2Fexample.com%2Fa.pkl"
    );
    assert!(effective_uri(&uri("pkl-lsp://unknown/a")).is_err());
    assert!(effective_uri(&uri("pkl-lsp://https/http%3A%2F%2Fexample.com")).is_err());
}
#[test]
fn filesystem_listing_is_recursive_and_component_safe() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("nested/deep")).unwrap();
    fs::write(dir.path().join("a.pkl"), "a = 1").unwrap();
    fs::write(dir.path().join("nested/deep/b.pkl"), "b = 2").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path(), dir.path().join("nested/cycle")).unwrap();
    let root = uri(url::Url::from_directory_path(dir.path()).unwrap().as_str());
    assert_eq!(FileManager::new().list_files(&root).unwrap().len(), 2);
}

struct TlsService {
    base: String,
    ca: reqwest::Certificate,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl TlsService {
    fn start() -> Self {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let ca = params.self_signed(&ca_key).unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
            .unwrap()
            .signed_by(&key, &ca, &ca_key)
            .unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                key.serialize_der(),
            )),
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                let (socket, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let connection = rustls::ServerConnection::new(Arc::new(config.clone())).unwrap();
                let mut stream = rustls::StreamOwned::new(connection, socket);
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                if request.is_empty() {
                    continue;
                }
                let request = String::from_utf8(request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap();
                let response = if path == "/error" {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_owned()
                } else if path == "/redirect" {
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/downgrade\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned()
                } else {
                    if path == "/slow" {
                        thread::sleep(Duration::from_millis(150));
                    }
                    let body = format!("path = \"{path}\"\n");
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        Self {
            base: format!("https://localhost:{port}"),
            ca: reqwest::Certificate::from_der(ca.der()).unwrap(),
            stop,
            thread: Some(handle),
        }
    }
    fn manager(&self, cache: &std::path::Path, timeout: Duration) -> FileManager {
        let fm = FileManager::with_trusted_certificates(vec![self.ca.clone()], timeout).unwrap();
        fm.set_cache_dir(cache.to_owned());
        fm
    }
}
impl Drop for TlsService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}
#[test]
fn https_verifies_tls_resolves_relative_and_persists_correct_cache_keys() {
    let cache = tempfile::tempdir().unwrap();
    let service = TlsService::start();
    let fm = service.manager(cache.path(), Duration::from_secs(2));
    let first = uri(&format!("{}/dir/main.pkl?q=1", service.base));
    let second = uri(&format!("{}/dir/main.pkl?q=2", service.base));
    assert!(fm.get_or_load(&first).unwrap().contents().contains("q=1"));
    assert!(fm.get_or_load(&second).unwrap().contents().contains("q=2"));
    let relative = fm.resolve_uri(&first, "../other.pkl").unwrap();
    assert_eq!(relative.as_str(), format!("{}/other.pkl", service.base));
    assert!(
        fm.get_or_load(&relative)
            .unwrap()
            .contents()
            .contains("other.pkl")
    );
    let untrusted = FileManager::new();
    untrusted.set_cache_dir(cache.path().join("untrusted"));
    assert!(untrusted.get_or_load(&first).is_err());
    for path in ["/error", "/redirect"] {
        assert!(
            fm.get_or_load(&uri(&format!("{}{path}", service.base)))
                .is_err()
        );
    }
    let slow = service.manager(&cache.path().join("slow"), Duration::from_millis(40));
    assert!(
        slow.get_or_load(&uri(&format!("{}/slow", service.base)))
            .is_err()
    );
    drop(service);
    let offline = FileManager::new();
    offline.set_cache_dir(cache.path().to_owned());
    assert_eq!(
        offline.get_or_load(&first).unwrap().contents(),
        fm.get(&first).unwrap().contents()
    );
    assert_eq!(
        offline.get_or_load(&second).unwrap().contents(),
        fm.get(&second).unwrap().contents()
    );
    assert!(offline.get_or_load(&uri("http://localhost/a.pkl")).is_err());
    assert_eq!(fs::read_dir(cache.path().join("https")).unwrap().count(), 3);
}
