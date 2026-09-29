use std::{
    collections::{BTreeSet, HashMap},
    io::{self, BufRead, BufReader, Read, Write},
    net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_ACTIVE_CONNECTIONS: usize = 32;
const REMOTE_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

pub struct ProviderEgressProxy {
    address: SocketAddr,
    running: Arc<AtomicBool>,
    active: Arc<Mutex<HashMap<u64, TcpStream>>>,
    listener_thread: Option<JoinHandle<()>>,
}

impl ProviderEgressProxy {
    pub fn start<I, S>(allowed_hosts: I) -> io::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let allowed_hosts = allowed_hosts
            .into_iter()
            .map(|host| normalize_host(host.as_ref()))
            .collect::<io::Result<BTreeSet<_>>>()?;
        if allowed_hosts.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the provider egress policy has no allowed hosts",
            ));
        }

        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let running = Arc::new(AtomicBool::new(true));
        let active = Arc::new(Mutex::new(HashMap::new()));
        let connection_count = Arc::new(AtomicUsize::new(0));
        let thread_running = running.clone();
        let thread_active = active.clone();
        let thread_count = connection_count.clone();

        let listener_thread = thread::Builder::new()
            .name("magi-provider-egress".to_owned())
            .spawn(move || {
                while thread_running.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if thread_count.fetch_add(1, Ordering::AcqRel)
                                >= MAX_ACTIVE_CONNECTIONS
                            {
                                thread_count.fetch_sub(1, Ordering::AcqRel);
                                reject(stream, 503, "Proxy capacity reached");
                                continue;
                            }
                            let id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);
                            if let Ok(clone) = stream.try_clone() {
                                if let Ok(mut connections) = thread_active.lock() {
                                    connections.insert(id, clone);
                                }
                            }
                            let hosts = allowed_hosts.clone();
                            let active = thread_active.clone();
                            let count = thread_count.clone();
                            let _ = thread::Builder::new()
                                .name("magi-provider-tunnel".to_owned())
                                .spawn(move || {
                                    handle_connect(stream, &hosts);
                                    if let Ok(mut connections) = active.lock() {
                                        connections.remove(&id);
                                    }
                                    count.fetch_sub(1, Ordering::AcqRel);
                                });
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) if !thread_running.load(Ordering::Acquire) => break,
                        Err(_) => thread::sleep(Duration::from_millis(100)),
                    }
                }
            })?;

        Ok(Self {
            address,
            running,
            active,
            listener_thread: Some(listener_thread),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    pub fn proxy_url(&self) -> String {
        format!("http://{}:{}", self.address.ip(), self.address.port())
    }
}

impl Drop for ProviderEgressProxy {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Ok(connections) = self.active.lock() {
            for stream in connections.values() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
        if let Some(listener_thread) = self.listener_thread.take() {
            let _ = listener_thread.join();
        }
    }
}

fn handle_connect(mut client: TcpStream, allowed_hosts: &BTreeSet<String>) {
    if client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .is_err()
    {
        return;
    }
    let request = match read_connect_request(&mut client) {
        Ok(request) => request,
        Err(_) => {
            reject(client, 400, "Only a valid HTTPS CONNECT request is allowed");
            return;
        }
    };
    if !allowed_hosts.contains(&request.host) {
        reject(client, 403, "Provider destination is not allowed");
        return;
    }

    let addresses = match (request.host.as_str(), request.port).to_socket_addrs() {
        Ok(addresses) => addresses.collect::<Vec<_>>(),
        Err(_) => {
            reject(client, 502, "Provider destination could not be resolved");
            return;
        }
    };
    let mut upstream = None;
    for address in addresses {
        if !is_public_address(address.ip()) {
            continue;
        }
        match TcpStream::connect_timeout(&address, REMOTE_CONNECT_TIMEOUT) {
            Ok(stream) => {
                upstream = Some(stream);
                break;
            }
            Err(_) => continue,
        }
    }
    let Some(mut upstream) = upstream else {
        reject(client, 502, "Provider destination connection failed");
        return;
    };
    let _ = client.set_read_timeout(None);
    let _ = client.set_write_timeout(Some(Duration::from_secs(10)));
    if client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .is_err()
    {
        return;
    }
    let _ = client.set_write_timeout(None);
    let _ = client.set_read_timeout(None);
    let _ = upstream.set_read_timeout(None);
    let _ = upstream.set_write_timeout(None);
    let _ = tunnel(client, upstream);
}

struct ConnectRequest {
    host: String,
    port: u16,
}

fn read_connect_request(stream: &mut TcpStream) -> io::Result<ConnectRequest> {
    let mut reader = BufReader::new(stream);
    let mut request_line = Vec::with_capacity(128);
    reader.read_until(b'\n', &mut request_line)?;
    if request_line.len() > 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "request line too long"));
    }
    let request_line = std::str::from_utf8(&request_line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid request line"))?
        .trim_end_matches(['\r', '\n']);
    let mut parts = request_line.split_ascii_whitespace();
    if parts.next() != Some("CONNECT") {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "CONNECT required"));
    }
    let authority = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing authority"))?;
    if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid request version"));
    }
    let mut header_bytes = request_line.len() + 2;
    loop {
        let mut line = Vec::with_capacity(128);
        reader.read_until(b'\n', &mut line)?;
        header_bytes = header_bytes.saturating_add(line.len());
        if header_bytes > MAX_HEADER_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "headers too large"));
        }
        if line == b"\r\n" {
            break;
        }
        if line.is_empty() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "incomplete headers"));
        }
    }

    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing port"))?;
    let host = normalize_host(host)?;
    let port = port
        .parse::<u16>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid port"))?;
    if port != 443 || host.parse::<IpAddr>().is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "only named HTTPS destinations are allowed",
        ));
    }
    Ok(ConnectRequest { host, port })
}

fn normalize_host(host: &str) -> io::Result<String> {
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    if host.is_empty()
        || !host.is_ascii()
        || host.len() > 253
        || host.starts_with('.')
        || host.ends_with('.')
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid provider hostname",
        ));
    }
    Ok(host)
}

fn is_public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_multicast()
                || octets[0] == 0
                || octets[0] >= 240
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
                || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
                || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113))
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || segments[0] == 0x2001 && segments[1] == 0x0db8
                || ip.to_ipv4_mapped().is_some())
        }
    }
}

fn tunnel(client: TcpStream, upstream: TcpStream) -> io::Result<()> {
    let mut client_reader = client.try_clone()?;
    let mut upstream_writer = upstream.try_clone()?;
    let outbound = thread::Builder::new()
        .name("magi-provider-tunnel-up".to_owned())
        .spawn(move || {
            let result = io::copy(&mut client_reader, &mut upstream_writer);
            let _ = upstream_writer.shutdown(Shutdown::Write);
            result
        })?;
    let mut upstream_reader = upstream;
    let mut client_writer = client;
    let inbound = io::copy(&mut upstream_reader, &mut client_writer);
    let _ = client_writer.shutdown(Shutdown::Write);
    let outbound = outbound
        .join()
        .map_err(|_| io::Error::other("proxy tunnel worker stopped unexpectedly"))?;
    inbound.and(outbound)
}

fn reject(mut stream: TcpStream, status: u16, reason: &str) {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.shutdown(Shutdown::Both);
}

impl Read for ProviderEgressProxy {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "provider egress proxy is not a byte stream",
        ))
    }
}
