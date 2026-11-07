use std::{
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Unlike the single-shot origin, accepts concurrent GETs and counts each one.
/// Requests are held until release, so an epoch change can expose duplicate fetches.
pub struct CountedHttps {
    pub uri: String,
    requests: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl CountedHttps {
    pub fn start(ca_path: &Path) -> Self {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let ca = params.self_signed(&ca_key).unwrap();
        std::fs::write(ca_path, ca.pem()).unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
            .unwrap()
            .signed_by(&key, &ca, &ca_key)
            .unwrap();
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(
                    rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()),
                ),
            )
            .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (count, gate, stopping) = (requests.clone(), released.clone(), stop.clone());
        let thread = thread::spawn(move || {
            let mut workers = Vec::new();
            while !stopping.load(Ordering::SeqCst) {
                let (socket, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                let (config, count, gate, stopping) = (
                    config.clone(),
                    count.clone(),
                    gate.clone(),
                    stopping.clone(),
                );
                workers.push(thread::spawn(move || {
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let connection = rustls::ServerConnection::new(config).unwrap();
                    let mut stream = rustls::StreamOwned::new(connection, socket);
                    let mut request = Vec::new();
                    let mut byte = [0];
                    while !request.ends_with(b"\r\n\r\n") {
                        if stream.read(&mut byte).unwrap_or(0) == 0 {
                            return;
                        }
                        request.push(byte[0]);
                    }
                    assert!(request.starts_with(b"GET /library.pkl "));
                    count.fetch_add(1, Ordering::SeqCst);
                    let deadline = Instant::now() + Duration::from_secs(15);
                    while !gate.load(Ordering::SeqCst) && !stopping.load(Ordering::SeqCst) {
                        assert!(Instant::now() < deadline, "release counted HTTPS origin");
                        thread::sleep(Duration::from_millis(2));
                    }
                    let body = "value: Int = 42\n";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                }));
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            uri: format!("https://localhost:{port}/library.pkl"),
            requests,
            released,
            stop,
            thread: Some(thread),
        }
    }
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
    pub fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
    }
}
impl Drop for CountedHttps {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.release();
        self.thread.take().unwrap().join().unwrap();
    }
}
