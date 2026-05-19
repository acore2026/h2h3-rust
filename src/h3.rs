fn run_h3_scenario(scenario: &Scenario, corpus: PayloadCorpus) -> Result<ResultRow> {
    let cert_files = CertFiles::new()?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let server = H3Server::start(&cert_files, Arc::clone(&shutdown))?;
    let mut client = H3Client::connect(server.addr, scenario.users)?;

    for _ in 0..8 {
        client.request(corpus.payloads[0].clone(), 0)?;
    }

    let total = scenario.users * scenario.messages_per_user;
    let before = capture_resources();
    let started = Instant::now();
    let mut latencies = Vec::with_capacity(total);
    let mut errors = 0usize;

    let mut completed = 0usize;
    let mut user_next = vec![0usize; scenario.users];
    let mut user_inflight = vec![false; scenario.users];
    let mut last_progress = Instant::now();
    while completed < total {
        for user in 0..scenario.users {
            if user_inflight[user] || user_next[user] >= scenario.messages_per_user {
                continue;
            }
            let payload_index =
                (user * scenario.messages_per_user + user_next[user]) % corpus.payloads.len();
            let payload = corpus.payloads[payload_index].clone();
            match client.send_request(payload, user) {
                Ok(true) => {
                    user_inflight[user] = true;
                    last_progress = Instant::now();
                }
                Ok(false) => break,
                Err(_) => {
                    let remaining = scenario.messages_per_user - user_next[user];
                    errors += remaining;
                    completed += remaining;
                    user_next[user] = scenario.messages_per_user;
                    user_inflight[user] = false;
                }
            }
        }
        let events = client.drive()?;
        for event in events {
            if event.user < scenario.users {
                user_inflight[event.user] = false;
                user_next[event.user] += 1;
            }
            completed += 1;
            last_progress = Instant::now();
            if event.ok {
                latencies.push(event.latency);
            } else {
                errors += 1;
            }
        }
        if last_progress.elapsed() > Duration::from_secs(30) {
            errors += total - completed;
            break;
        }
    }

    let duration = started.elapsed();
    let after = capture_resources();
    shutdown.store(true, AtomicOrdering::SeqCst);
    let _ = server.handle.join();

    Ok(ResultRow {
        scenario: scenario.clone(),
        repeat_index: 1,
        payload_count: corpus.payloads.len(),
        payload_bytes_avg: corpus.avg_bytes,
        duration,
        throughput: total as f64 / duration.as_secs_f64(),
        errors,
        latency: summarize_latencies(&latencies),
        resources: diff_resources(before, after),
    })
}

struct CertFiles {
    base: PathBuf,
    cert_path: PathBuf,
    key_path: PathBuf,
}

impl CertFiles {
    fn new() -> Result<Self> {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["localhost".to_string()])?;
        let base = std::env::temp_dir().join(format!("transportbench-rust-{}", std::process::id()));
        fs::create_dir_all(&base)?;
        let cert_path = base.join("cert.pem");
        let key_path = base.join("key.pem");
        fs::write(&cert_path, cert.pem())?;
        fs::write(&key_path, signing_key.serialize_pem())?;
        Ok(Self {
            base,
            cert_path,
            key_path,
        })
    }
}

impl Drop for CertFiles {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

struct H3Server {
    addr: SocketAddr,
    handle: thread::JoinHandle<()>,
}

struct H3ServerClient {
    conn: quiche::Connection,
    h3: Option<quiche::h3::Connection>,
    peer: SocketAddr,
    finished: HashSet<u64>,
    pending_responses: HashSet<u64>,
}

impl H3Server {
    fn start(cert_files: &CertFiles, shutdown: Arc<AtomicBool>) -> Result<Self> {
        Self::start_on("127.0.0.1:0".parse()?, cert_files, shutdown)
    }

    fn start_on(
        bind_addr: SocketAddr,
        cert_files: &CertFiles,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        let socket = UdpSocket::bind(bind_addr)?;
        let addr = socket.local_addr()?;
        let cert_path = cert_files.cert_path.clone();
        let key_path = cert_files.key_path.clone();
        let handle = thread::spawn(move || {
            let _ = run_h3_server_loop(socket, cert_path, key_path, shutdown);
        });
        Ok(Self { addr, handle })
    }
}

fn run_h3_server_loop(
    mut socket: UdpSocket,
    cert_path: PathBuf,
    key_path: PathBuf,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let local_addr = socket.local_addr()?;
    let mut poll = Poll::new()?;
    poll.registry()
        .register(&mut socket, H3_SOCKET, Interest::READABLE)?;
    let mut events = Events::with_capacity(128);
    let mut config = quiche_config(false, 1024)?;
    config.load_cert_chain_from_pem_file(cert_path.to_str().unwrap())?;
    config.load_priv_key_from_pem_file(key_path.to_str().unwrap())?;
    let h3_config = quiche::h3::Config::new()?;
    let mut client: Option<H3ServerClient> = None;
    let mut buf = [0u8; READ_BUF_SIZE];
    let mut out = [0u8; MAX_DATAGRAM_SIZE];

    while !shutdown.load(AtomicOrdering::SeqCst) {
        let timeout = client
            .as_ref()
            .and_then(|client| client.conn.timeout())
            .or(Some(Duration::from_millis(10)));
        events.clear();
        poll.poll(&mut events, timeout)?;
        if events.is_empty() {
            if let Some(client) = client.as_mut() {
                client.conn.on_timeout();
            }
        } else {
            loop {
                let (len, from) = match socket.recv_from(&mut buf) {
                    Ok(v) => v,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e.into()),
                };
                if client.is_none() {
                    let hdr = match quiche::Header::from_slice(
                        &mut buf[..len],
                        quiche::MAX_CONN_ID_LEN,
                    ) {
                        Ok(h) => h,
                        Err(_) => continue,
                    };
                    if hdr.ty != quiche::Type::Initial {
                        continue;
                    }
                    let scid = if hdr.dcid.is_empty() {
                        quiche::ConnectionId::from_ref(&[0xba; 16])
                    } else {
                        hdr.dcid.clone()
                    };
                    let conn = quiche::accept(&scid, None, local_addr, from, &mut config)?;
                    client = Some(H3ServerClient {
                        conn,
                        h3: None,
                        peer: from,
                        finished: HashSet::new(),
                        pending_responses: HashSet::new(),
                    });
                }

                let Some(client) = client.as_mut() else {
                    continue;
                };
                client.peer = from;
                let recv_info = quiche::RecvInfo {
                    to: local_addr,
                    from,
                };
                if client.conn.recv(&mut buf[..len], recv_info).is_err() {
                    continue;
                }
                if client.conn.is_established() && client.h3.is_none() {
                    client.h3 = Some(quiche::h3::Connection::with_transport(
                        &mut client.conn,
                        &h3_config,
                    )?);
                }
                handle_h3_server_events(client)?;
            }
        }

        if let Some(client) = client.as_mut() {
            handle_h3_server_events(client)?;
            flush_quiche(&socket, &mut client.conn, &mut out)?;
        }
    }
    Ok(())
}

fn handle_h3_server_events(client: &mut H3ServerClient) -> Result<()> {
    if client.h3.is_some() {
        let writable: Vec<u64> = client.conn.writable().collect();
        for stream_id in writable {
            retry_h3_response(client, stream_id)?;
        }
    }
    let Some(h3) = client.h3.as_mut() else {
        return Ok(());
    };
    let mut body_buf = [0u8; 8192];
    loop {
        match h3.poll(&mut client.conn) {
            Ok((_stream_id, quiche::h3::Event::Headers { .. })) => {}
            Ok((stream_id, quiche::h3::Event::Data)) => loop {
                match h3.recv_body(&mut client.conn, stream_id, &mut body_buf) {
                    Ok(_) => {}
                    Err(quiche::h3::Error::Done) => break,
                    Err(e) => return Err(e.into()),
                }
            },
            Ok((stream_id, quiche::h3::Event::Finished)) => {
                if client.finished.insert(stream_id) {
                    let headers = vec![quiche::h3::Header::new(b":status", b"204")];
                    match h3.send_response(&mut client.conn, stream_id, &headers, true) {
                        Ok(_) => {}
                        Err(quiche::h3::Error::StreamBlocked) => {
                            client.pending_responses.insert(stream_id);
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            Ok((_stream_id, quiche::h3::Event::Reset(_))) => {}
            Ok((_stream_id, quiche::h3::Event::PriorityUpdate)) => {}
            Ok((_stream_id, quiche::h3::Event::GoAway)) => {}
            Err(quiche::h3::Error::Done) => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn retry_h3_response(client: &mut H3ServerClient, stream_id: u64) -> Result<()> {
    if !client.pending_responses.contains(&stream_id) {
        return Ok(());
    }
    let Some(h3) = client.h3.as_mut() else {
        return Ok(());
    };
    let headers = vec![quiche::h3::Header::new(b":status", b"204")];
    match h3.send_response(&mut client.conn, stream_id, &headers, true) {
        Ok(_) => {
            client.pending_responses.remove(&stream_id);
            Ok(())
        }
        Err(quiche::h3::Error::StreamBlocked) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

struct H3Client {
    socket: UdpSocket,
    poll: Poll,
    events: Events,
    conn: quiche::Connection,
    h3: quiche::h3::Connection,
    starts: HashMap<u64, Instant>,
    stream_users: HashMap<u64, usize>,
    statuses: HashMap<u64, u16>,
    pending_bodies: HashMap<u64, PendingBody>,
    recv_buf: [u8; READ_BUF_SIZE],
    out_buf: [u8; MAX_DATAGRAM_SIZE],
    authority: String,
    path: String,
}

struct PendingBody {
    data: Bytes,
    written: usize,
}

struct H3Completion {
    user: usize,
    latency: Duration,
    ok: bool,
}

impl H3Client {
    fn connect(server_addr: SocketAddr, users: usize) -> Result<Self> {
        Self::connect_with_request_target(server_addr, users, "localhost", "localhost", "/sbi")
    }

    fn connect_with_request_target(
        server_addr: SocketAddr,
        users: usize,
        sni_host: &str,
        authority: &str,
        path: &str,
    ) -> Result<Self> {
        let mut socket = UdpSocket::bind("127.0.0.1:0".parse()?)?;
        let mut poll = Poll::new()?;
        poll.registry()
            .register(&mut socket, H3_SOCKET, Interest::READABLE)?;
        let mut events = Events::with_capacity(128);
        let local_addr = socket.local_addr()?;
        let mut config = quiche_config(true, users + 128)?;
        let scid_bytes = [0xca; quiche::MAX_CONN_ID_LEN];
        let scid = quiche::ConnectionId::from_ref(&scid_bytes);
        let mut conn = quiche::connect(
            Some(sni_host),
            &scid,
            local_addr,
            server_addr,
            &mut config,
        )?;
        let mut out_buf = [0u8; MAX_DATAGRAM_SIZE];
        flush_quiche(&socket, &mut conn, &mut out_buf)?;
        let h3_config = quiche::h3::Config::new()?;
        let mut recv_buf = [0u8; READ_BUF_SIZE];
        let deadline = Instant::now() + Duration::from_secs(5);
        while !conn.is_established() {
            if Instant::now() > deadline {
                return Err(anyhow!("h3 handshake timed out"));
            }
            events.clear();
            poll.poll(&mut events, conn.timeout())?;
            if events.is_empty() {
                conn.on_timeout();
            } else {
                recv_quiche(&socket, &mut conn, &mut recv_buf)?;
            }
            flush_quiche(&socket, &mut conn, &mut out_buf)?;
        }
        let h3 = quiche::h3::Connection::with_transport(&mut conn, &h3_config)?;
        Ok(Self {
            socket,
            poll,
            events,
            conn,
            h3,
            starts: HashMap::new(),
            stream_users: HashMap::new(),
            statuses: HashMap::new(),
            pending_bodies: HashMap::new(),
            recv_buf,
            out_buf,
            authority: authority.to_string(),
            path: path.to_string(),
        })
    }

    fn request(&mut self, payload: Bytes, user: usize) -> Result<()> {
        while !self.send_request(payload.clone(), user)? {
            let _ = self.drive()?;
        }
        while self.inflight() > 0 {
            let _ = self.drive()?;
        }
        Ok(())
    }

    fn inflight(&self) -> usize {
        self.starts.len()
    }

    fn send_request(&mut self, payload: Bytes, user: usize) -> Result<bool> {
        let content_length = payload.len().to_string();
        let headers = vec![
            quiche::h3::Header::new(b":method", b"POST"),
            quiche::h3::Header::new(b":scheme", b"https"),
            quiche::h3::Header::new(b":authority", self.authority.as_bytes()),
            quiche::h3::Header::new(b":path", self.path.as_bytes()),
            quiche::h3::Header::new(b"content-type", b"application/json"),
            quiche::h3::Header::new(b"content-length", content_length.as_bytes()),
        ];
        let stream_id = match self.h3.send_request(&mut self.conn, &headers, false) {
            Ok(stream_id) => stream_id,
            Err(quiche::h3::Error::Done) | Err(quiche::h3::Error::StreamBlocked) => {
                flush_quiche(&self.socket, &mut self.conn, &mut self.out_buf)?;
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        self.starts.insert(stream_id, Instant::now());
        self.stream_users.insert(stream_id, user);
        match self.h3.send_body(&mut self.conn, stream_id, &payload, true) {
            Ok(written) if written == payload.len() => {}
            Ok(written) => {
                self.pending_bodies.insert(
                    stream_id,
                    PendingBody {
                        data: payload,
                        written,
                    },
                );
            }
            Err(quiche::h3::Error::Done) => {
                self.pending_bodies.insert(
                    stream_id,
                    PendingBody {
                        data: payload,
                        written: 0,
                    },
                );
            }
            Err(quiche::h3::Error::StreamBlocked) => {
                self.pending_bodies.insert(
                    stream_id,
                    PendingBody {
                        data: payload,
                        written: 0,
                    },
                );
            }
            Err(e) => return Err(e.into()),
        }
        flush_quiche(&self.socket, &mut self.conn, &mut self.out_buf)?;
        Ok(true)
    }

    fn drive(&mut self) -> Result<Vec<H3Completion>> {
        self.drive_with_max_timeout(self.conn.timeout())
    }

    fn drive_with_max_timeout(
        &mut self,
        max_timeout: Option<Duration>,
    ) -> Result<Vec<H3Completion>> {
        self.flush_pending_bodies()?;
        flush_quiche(&self.socket, &mut self.conn, &mut self.out_buf)?;
        self.events.clear();
        let timeout = min_optional_duration(self.conn.timeout(), max_timeout);
        self.poll.poll(&mut self.events, timeout)?;
        if self.events.is_empty() {
            self.conn.on_timeout();
        } else {
            recv_quiche(&self.socket, &mut self.conn, &mut self.recv_buf)?;
        }
        let mut completions = Vec::new();
        loop {
            match self.h3.poll(&mut self.conn) {
                Ok((stream_id, quiche::h3::Event::Headers { list, .. })) => {
                    self.statuses
                        .insert(stream_id, status_from_headers(&list).unwrap_or(0));
                }
                Ok((stream_id, quiche::h3::Event::Data)) => {
                    while self
                        .h3
                        .recv_body(&mut self.conn, stream_id, &mut self.recv_buf)
                        .is_ok()
                    {}
                }
                Ok((stream_id, quiche::h3::Event::Finished)) => {
                    if let Some(started) = self.starts.remove(&stream_id) {
                        self.pending_bodies.remove(&stream_id);
                        let user = self.stream_users.remove(&stream_id).unwrap_or(usize::MAX);
                        let status = self.statuses.remove(&stream_id).unwrap_or(0);
                        completions.push(H3Completion {
                            user,
                            latency: started.elapsed(),
                            ok: (200..300).contains(&status),
                        });
                    }
                }
                Ok((_stream_id, quiche::h3::Event::Reset(_))) => {}
                Ok((_stream_id, quiche::h3::Event::PriorityUpdate)) => {}
                Ok((_stream_id, quiche::h3::Event::GoAway)) => {}
                Err(quiche::h3::Error::Done) => break,
                Err(e) => return Err(e.into()),
            }
        }
        self.flush_pending_bodies()?;
        flush_quiche(&self.socket, &mut self.conn, &mut self.out_buf)?;
        Ok(completions)
    }

    fn flush_pending_bodies(&mut self) -> Result<()> {
        let stream_ids: Vec<u64> = self.pending_bodies.keys().copied().collect();
        let mut completed = Vec::new();
        for stream_id in stream_ids {
            let Some(body) = self.pending_bodies.get_mut(&stream_id) else {
                continue;
            };
            while body.written < body.data.len() {
                match self
                    .h3
                    .send_body(&mut self.conn, stream_id, &body.data[body.written..], true)
                {
                    Ok(0) => break,
                    Ok(written) => body.written += written,
                    Err(quiche::h3::Error::Done) => break,
                    Err(e) => return Err(e.into()),
                }
            }
            if body.written == body.data.len() {
                completed.push(stream_id);
            }
        }
        for stream_id in completed {
            self.pending_bodies.remove(&stream_id);
        }
        flush_quiche(&self.socket, &mut self.conn, &mut self.out_buf)?;
        Ok(())
    }
}

fn status_from_headers(headers: &[quiche::h3::Header]) -> Option<u16> {
    headers
        .iter()
        .find(|header| header.name() == b":status")
        .and_then(|header| std::str::from_utf8(header.value()).ok())
        .and_then(|status| status.parse::<u16>().ok())
}

fn quiche_config(client: bool, max_streams: usize) -> Result<quiche::Config> {
    quiche_config_with_protos(client, max_streams, quiche::h3::APPLICATION_PROTOCOL)
}

fn raw_quiche_config(client: bool, max_streams: usize) -> Result<quiche::Config> {
    quiche_config_with_protos(client, max_streams, RAW_QUIC_ALPN)
}

fn quiche_config_with_protos(
    client: bool,
    max_streams: usize,
    application_protos: &[&[u8]],
) -> Result<quiche::Config> {
    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
    if client {
        config.verify_peer(false);
    }
    config.set_application_protos(application_protos)?;
    config.set_max_idle_timeout(5000);
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_initial_max_data(64_000_000);
    config.set_initial_max_stream_data_bidi_local(4_000_000);
    config.set_initial_max_stream_data_bidi_remote(4_000_000);
    config.set_initial_max_stream_data_uni(4_000_000);
    config.set_initial_max_streams_bidi(max_streams as u64);
    config.set_initial_max_streams_uni(max_streams as u64);
    config.set_disable_active_migration(true);
    Ok(config)
}

fn recv_quiche(socket: &UdpSocket, conn: &mut quiche::Connection, buf: &mut [u8]) -> Result<()> {
    let local_addr = socket.local_addr()?;
    loop {
        let (len, from) = match socket.recv_from(buf) {
            Ok(v) => v,
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => return Err(e.into()),
        };
        let recv_info = quiche::RecvInfo {
            to: local_addr,
            from,
        };
        let _ = conn.recv(&mut buf[..len], recv_info);
    }
    Ok(())
}

fn flush_quiche(socket: &UdpSocket, conn: &mut quiche::Connection, out: &mut [u8]) -> Result<()> {
    loop {
        let (write, send_info) = match conn.send(out) {
            Ok(v) => v,
            Err(quiche::Error::Done) => break,
            Err(e) => return Err(e.into()),
        };
        match socket.send_to(&out[..write], send_info.to) {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn min_optional_duration(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}
