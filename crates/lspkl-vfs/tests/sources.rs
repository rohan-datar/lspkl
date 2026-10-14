use fluent_uri::Uri;
use lspkl_vfs::{SourceLoader, StandardSourceLoader, effective_uri, to_editor_uri};
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
    let fm = StandardSourceLoader::new();
    let first = fm.resolve_uri(&a, "dir/main.pkl").unwrap();
    let second = fm.resolve_uri(&b, "dir/main.pkl").unwrap();
    assert_eq!(
        fm.load(&first, &lspkl_vfs::default_cache())
            .unwrap()
            .contents(),
        "value = 1"
    );
    assert_eq!(
        fm.load(&second, &lspkl_vfs::default_cache())
            .unwrap()
            .contents(),
        "value = 2"
    );
    let other = fm.resolve_uri(&first, "../other.pkl").unwrap();
    assert_eq!(
        fm.load(&other, &lspkl_vfs::default_cache())
            .unwrap()
            .contents(),
        "other = 2"
    );
    assert!(fm.resolve_uri(&first, "../../escape.pkl").is_err());
    assert!(fm.resolve_uri(&first, "%2e%2e/%2e%2e/escape.pkl").is_err());
    let folder = fm.resolve_uri(&a, "dir/").unwrap();
    assert_eq!(fm.list_files(&folder).unwrap().len(), 2);
    assert_eq!(fm.list_files(&a).unwrap().len(), 5);
    let unicode = fm.resolve_uri(&a, "space%20%2B%20%C3%BC.pkl").unwrap();
    assert_eq!(
        fm.load(&unicode, &lspkl_vfs::default_cache())
            .unwrap()
            .contents(),
        "unicode = true"
    );
    assert!(!dir.path().join("escape.pkl").exists());
    fs::write(dir.path().join("broken.zip"), "not a zip").unwrap();
    let broken = uri(&format!(
        "jar:{}!/a.pkl",
        url::Url::from_file_path(dir.path().join("broken.zip")).unwrap()
    ));
    assert!(fm.load(&broken, &lspkl_vfs::default_cache()).is_err());
}
#[test]
fn stdlib_missing_module_is_an_error() {
    let fm = StandardSourceLoader::new();
    assert!(
        fm.load(&uri("pkl:missing"), &lspkl_vfs::default_cache())
            .is_err()
    );
}
#[test]
fn editor_uri_roundtrips() {
    for physical in [
        "file:///tmp/a%20b.pkl",
        "pkl:json",
        "https://example.com/a%20b+c.pkl?q=one%20two&x=+",
        "jar:file:///tmp/a%20b.jar!/dir/a%20b.pkl",
    ] {
        let physical = effective_uri(&uri(physical)).unwrap();
        let editor = to_editor_uri(&physical).unwrap();
        assert_eq!(effective_uri(&editor).unwrap(), physical);
    }
    assert_eq!(
        to_editor_uri(&uri("https://example.com/a.pkl"))
            .unwrap()
            .as_str(),
        "lspkl://https/https%3A%2F%2Fexample.com%2Fa.pkl"
    );
    assert!(effective_uri(&uri("lspkl://unknown/a")).is_err());
    assert!(effective_uri(&uri("lspkl://https/http%3A%2F%2Fexample.com")).is_err());
}
#[cfg(unix)]
#[test]
fn filesystem_listing_recurses_without_following_symlink_cycles() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("nested/deep")).unwrap();
    fs::write(dir.path().join("a.pkl"), "a = 1").unwrap();
    fs::write(dir.path().join("nested/deep/b.pkl"), "b = 2").unwrap();
    std::os::unix::fs::symlink(dir.path(), dir.path().join("nested/cycle")).unwrap();
    let root = uri(url::Url::from_directory_path(dir.path()).unwrap().as_str());
    assert_eq!(
        StandardSourceLoader::new().list_files(&root).unwrap().len(),
        2
    );
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
                // macOS/Windows can inherit the listener's nonblocking mode.
                // rustls::StreamOwned requires blocking I/O with these timeouts.
                socket.set_nonblocking(false).unwrap();
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
    fn loader(&self, timeout: Duration) -> StandardSourceLoader {
        StandardSourceLoader::with_trusted_certificates(vec![self.ca.clone()], timeout).unwrap()
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
    let fm = service.loader(Duration::from_secs(2));
    let first = uri(&format!("{}/dir/main.pkl?q=1", service.base));
    let second = uri(&format!("{}/dir/main.pkl?q=2", service.base));
    // A query load must not contact even a reachable, trusted server.
    assert!(fm.load(&first, cache.path()).is_err());
    let first_source = fm.fetch(&first, cache.path()).unwrap();
    let second_source = fm.fetch(&second, cache.path()).unwrap();
    assert!(first_source.contents().contains("q=1"));
    assert!(second_source.contents().contains("q=2"));
    let relative = fm.resolve_uri(&first, "../other.pkl").unwrap();
    assert_eq!(relative.as_str(), format!("{}/other.pkl", service.base));
    assert!(
        fm.fetch(&relative, cache.path())
            .unwrap()
            .contents()
            .contains("other.pkl")
    );
    let untrusted = StandardSourceLoader::new();
    assert!(
        untrusted
            .fetch(&first, &cache.path().join("untrusted"))
            .is_err()
    );
    for path in ["/error", "/redirect"] {
        assert!(
            fm.fetch(&uri(&format!("{}{path}", service.base)), cache.path())
                .is_err()
        );
    }
    let slow = service.loader(Duration::from_millis(40));
    assert!(
        slow.fetch(&uri(&format!("{}/slow", service.base)), cache.path())
            .is_err()
    );
    drop(service);
    let offline = StandardSourceLoader::new();
    assert_eq!(
        offline.load(&first, cache.path()).unwrap().contents(),
        first_source.contents()
    );
    assert_eq!(
        offline.load(&second, cache.path()).unwrap().contents(),
        second_source.contents()
    );
    assert!(
        offline
            .load(&uri("http://localhost/a.pkl"), cache.path())
            .is_err()
    );
    assert_eq!(fs::read_dir(cache.path().join("https")).unwrap().count(), 3);
}
