#[derive(Debug, Clone)]
struct OverloadConfig {
    total_requests: usize,
    concurrency: Vec<usize>,
    payload_sizes: Vec<usize>,
    thresholds: Vec<usize>,
    reject_percent: f64,
    modes: Vec<OverloadMode>,
    repeats: usize,
    output_dir: PathBuf,
}

impl OverloadConfig {
    fn from_args<I>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = String>,
    {
        let mut cfg = Self {
            total_requests: 1000,
            concurrency: vec![50, 100, 200, 500],
            payload_sizes: vec![1024, 16 * 1024, 64 * 1024, 256 * 1024],
            thresholds: vec![20, 50],
            reject_percent: 0.0,
            modes: vec![OverloadMode::App503, OverloadMode::Reset],
            repeats: 1,
            output_dir: PathBuf::from("reports/overload_results"),
        };

        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-requests" | "--requests" => {
                    cfg.total_requests =
                        parse_positive_usize(&next_arg(&mut args, &arg)?, "requests")?
                }
                "-concurrency" | "--concurrency" => {
                    cfg.concurrency =
                        parse_positive_usizes(&next_arg(&mut args, &arg)?, "concurrency")?
                }
                "-payload-sizes" | "--payload-sizes" => {
                    cfg.payload_sizes =
                        parse_positive_usizes(&next_arg(&mut args, &arg)?, "payload-sizes")?
                }
                "-thresholds" | "--thresholds" => {
                    cfg.thresholds =
                        parse_positive_usizes(&next_arg(&mut args, &arg)?, "thresholds")?
                }
                "-reject-percent" | "--reject-percent" => {
                    cfg.reject_percent = parse_percent(&next_arg(&mut args, &arg)?)?
                }
                "-modes" | "--modes" => {
                    cfg.modes = parse_overload_modes(&next_arg(&mut args, &arg)?)?
                }
                "-repeat" | "--repeat" | "-repeats" | "--repeats" => {
                    cfg.repeats = parse_positive_usize(&next_arg(&mut args, &arg)?, "repeats")?
                }
                "-output-dir" | "--output-dir" => {
                    cfg.output_dir = PathBuf::from(next_arg(&mut args, &arg)?)
                }
                "-h" | "--help" => {
                    print_overload_help();
                    std::process::exit(0);
                }
                _ => return Err(anyhow!("unknown overload argument {arg}")),
            }
        }

        Ok(cfg)
    }
}

fn print_overload_help() {
    println!(
        "transportbench-rust overload\n\n\
         Flags:\n\
           -requests 1000\n\
           -concurrency 50,100,200,500\n\
           -payload-sizes 1024,16384,65536,262144\n\
           -thresholds 20,50\n\
           -reject-percent 0\n\
           -modes app503,reset\n\
           -repeat 1\n\
           -output-dir ./reports/overload_results"
    );
}

fn parse_percent(value: &str) -> Result<f64> {
    let n = value
        .parse::<f64>()
        .with_context(|| format!("percent contains non-number value {value:?}"))?;
    if !(0.0..=100.0).contains(&n) {
        return Err(anyhow!("percent must be between 0 and 100"));
    }
    Ok(n)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum OverloadMode {
    App503,
    Reset,
}

impl OverloadMode {
    fn as_str(self) -> &'static str {
        match self {
            OverloadMode::App503 => "app503",
            OverloadMode::Reset => "reset",
        }
    }
}

fn parse_overload_modes(value: &str) -> Result<Vec<OverloadMode>> {
    let mut out = Vec::new();
    for part in split_csv(value) {
        let mode = match part.as_str() {
            "app503" => OverloadMode::App503,
            "reset" => OverloadMode::Reset,
            _ => return Err(anyhow!("unknown overload mode {part:?}")),
        };
        if !out.contains(&mode) {
            out.push(mode);
        }
    }
    if out.is_empty() {
        return Err(anyhow!("at least one overload mode is required"));
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct OverloadScenario {
    mode: OverloadMode,
    total_requests: usize,
    concurrency: usize,
    payload_size: usize,
    threshold: usize,
    reject_percent: f64,
}

struct OverloadResultRow {
    scenario: OverloadScenario,
    repeat_index: usize,
    duration: Duration,
    throughput: f64,
    successes: usize,
    rejected: usize,
    reset_streams: usize,
    errors: usize,
    latency: LatencySummary,
    rejection_latency: LatencySummary,
    server: OverloadServerStats,
    resources: ResourceDelta,
}

#[derive(Default, Debug, Clone)]
struct OverloadServerStats {
    bytes_read: usize,
    handling_time: Duration,
    accepted_requests: usize,
    successful_requests: usize,
    app_rejections: usize,
    reset_rejections: usize,
    peak_inflight: usize,
}

fn run_overload_cli(cfg: OverloadConfig) -> Result<()> {
    fs::create_dir_all(&cfg.output_dir)?;
    let mut rows_by_mode: BTreeMap<OverloadMode, Vec<OverloadResultRow>> = BTreeMap::new();

    for repeat_index in 1..=cfg.repeats {
        for payload_size in &cfg.payload_sizes {
            for concurrency in &cfg.concurrency {
                for threshold in &cfg.thresholds {
                    for mode in &cfg.modes {
                        let scenario = OverloadScenario {
                            mode: *mode,
                            total_requests: cfg.total_requests,
                            concurrency: *concurrency,
                            payload_size: *payload_size,
                            threshold: *threshold,
                            reject_percent: cfg.reject_percent,
                        };
                        let row = run_overload_scenario(&scenario, repeat_index)?;
                        rows_by_mode.entry(*mode).or_default().push(row);
                    }
                }
            }
        }
    }

    let app503_path = cfg.output_dir.join("results_app503.csv");
    let reset_path = cfg.output_dir.join("results_reset.csv");
    write_overload_csv(
        &app503_path,
        rows_by_mode
            .get(&OverloadMode::App503)
            .map_or(&[], Vec::as_slice),
    )?;
    write_overload_csv(
        &reset_path,
        rows_by_mode
            .get(&OverloadMode::Reset)
            .map_or(&[], Vec::as_slice),
    )?;

    let all_rows: Vec<&OverloadResultRow> =
        rows_by_mode.values().flat_map(|rows| rows.iter()).collect();
    write_summary_report(&cfg.output_dir.join("summary_report.md"), &all_rows)?;
    write_overload_charts(&cfg.output_dir, &all_rows)?;
    Ok(())
}

fn run_overload_scenario(
    scenario: &OverloadScenario,
    repeat_index: usize,
) -> Result<OverloadResultRow> {
    let cert_files = CertFiles::new()?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let server = RawQuicServer::start(&cert_files, scenario.clone(), Arc::clone(&shutdown))?;
    let mut client = RawQuicClient::connect(server.addr, scenario.concurrency + 128)?;

    let payload = Bytes::from(vec![0x42; scenario.payload_size]);
    let before = capture_resources();
    let started = Instant::now();
    let mut latencies = Vec::with_capacity(scenario.total_requests);
    let mut rejection_latencies = Vec::new();
    let mut successes = 0usize;
    let mut rejected = 0usize;
    let mut reset_streams = 0usize;
    let mut errors = 0usize;
    let mut sent = 0usize;
    let mut completed = 0usize;
    let mut last_progress = Instant::now();

    while completed < scenario.total_requests {
        while sent < scenario.total_requests && client.inflight() < scenario.concurrency {
            match client.open_request(payload.clone()) {
                Ok(true) => {
                    sent += 1;
                    last_progress = Instant::now();
                }
                Ok(false) => break,
                Err(_) => {
                    sent += 1;
                    completed += 1;
                    errors += 1;
                    last_progress = Instant::now();
                }
            }
        }

        let events = client.drive()?;
        if events.is_empty() && client.inflight() == 0 && sent >= scenario.total_requests {
            break;
        }
        for event in events {
            completed += 1;
            latencies.push(event.latency);
            last_progress = Instant::now();
            match event.outcome {
                RawClientOutcome::Success => successes += 1,
                RawClientOutcome::AppRejected => {
                    rejected += 1;
                    rejection_latencies.push(event.latency);
                }
                RawClientOutcome::Reset => {
                    reset_streams += 1;
                    rejection_latencies.push(event.latency);
                }
                RawClientOutcome::Error => errors += 1,
            }
        }

        if last_progress.elapsed() > Duration::from_secs(30) {
            errors += scenario.total_requests - completed;
            break;
        }
    }

    let duration = started.elapsed();
    let after = capture_resources();
    shutdown.store(true, AtomicOrdering::SeqCst);
    let _ = server.handle.join();
    let server_stats = server
        .stats
        .lock()
        .map(|stats| stats.clone())
        .unwrap_or_default();

    Ok(OverloadResultRow {
        scenario: scenario.clone(),
        repeat_index,
        duration,
        throughput: completed as f64 / duration.as_secs_f64(),
        successes,
        rejected,
        reset_streams,
        errors,
        latency: summarize_latencies(&latencies),
        rejection_latency: summarize_latencies(&rejection_latencies),
        server: server_stats,
        resources: diff_resources(before, after),
    })
}

struct RawQuicServer {
    addr: SocketAddr,
    handle: thread::JoinHandle<()>,
    stats: Arc<Mutex<OverloadServerStats>>,
}

struct RawServerClient {
    conn: quiche::Connection,
    peer: SocketAddr,
    streams: HashMap<u64, RawServerStream>,
    reset_streams: HashSet<u64>,
    pending_responses: HashMap<u64, PendingRawResponse>,
    processing_streams: HashMap<u64, Instant>,
    in_flight: usize,
    seen_requests: usize,
}

struct RawServerStream {
    started: Instant,
    bytes_read: usize,
    overloaded: bool,
}

struct PendingRawResponse {
    data: &'static [u8],
    written: usize,
}

impl RawQuicServer {
    fn start(
        cert_files: &CertFiles,
        scenario: OverloadScenario,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0".parse()?)?;
        let addr = socket.local_addr()?;
        let cert_path = cert_files.cert_path.clone();
        let key_path = cert_files.key_path.clone();
        let stats = Arc::new(Mutex::new(OverloadServerStats::default()));
        let thread_stats = Arc::clone(&stats);
        let handle = thread::spawn(move || {
            let _ = run_raw_server_loop(
                socket,
                cert_path,
                key_path,
                scenario,
                shutdown,
                thread_stats,
            );
        });
        Ok(Self {
            addr,
            handle,
            stats,
        })
    }
}

fn run_raw_server_loop(
    mut socket: UdpSocket,
    cert_path: PathBuf,
    key_path: PathBuf,
    scenario: OverloadScenario,
    shutdown: Arc<AtomicBool>,
    stats: Arc<Mutex<OverloadServerStats>>,
) -> Result<()> {
    let local_addr = socket.local_addr()?;
    let mut poll = Poll::new()?;
    poll.registry()
        .register(&mut socket, H3_SOCKET, Interest::READABLE)?;
    let mut events = Events::with_capacity(128);
    let mut config = raw_quiche_config(false, scenario.concurrency + 128)?;
    config.load_cert_chain_from_pem_file(cert_path.to_str().unwrap())?;
    config.load_priv_key_from_pem_file(key_path.to_str().unwrap())?;
    let mut client: Option<RawServerClient> = None;
    let mut buf = [0u8; READ_BUF_SIZE];
    let mut out = [0u8; MAX_DATAGRAM_SIZE];

    while !shutdown.load(AtomicOrdering::SeqCst) {
        let timeout = client
            .as_ref()
            .map(|client| {
                if client.processing_streams.is_empty() {
                    client.conn.timeout().unwrap_or(Duration::from_millis(10))
                } else {
                    client
                        .conn
                        .timeout()
                        .unwrap_or(Duration::from_millis(1))
                        .min(Duration::from_millis(1))
                }
            })
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
                        quiche::ConnectionId::from_ref(&[0xbb; 16])
                    } else {
                        hdr.dcid.clone()
                    };
                    let conn = quiche::accept(&scid, None, local_addr, from, &mut config)?;
                    client = Some(RawServerClient {
                        conn,
                        peer: from,
                        streams: HashMap::new(),
                        reset_streams: HashSet::new(),
                        pending_responses: HashMap::new(),
                        processing_streams: HashMap::new(),
                        in_flight: 0,
                        seen_requests: 0,
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
                handle_raw_server_events(client, &scenario, &stats)?;
            }
        }

        if let Some(client) = client.as_mut() {
            handle_raw_server_events(client, &scenario, &stats)?;
            flush_quiche(&socket, &mut client.conn, &mut out)?;
        }
    }
    Ok(())
}

fn handle_raw_server_events(
    client: &mut RawServerClient,
    scenario: &OverloadScenario,
    stats: &Arc<Mutex<OverloadServerStats>>,
) -> Result<()> {
    let writable: Vec<u64> = client.conn.writable().collect();
    for stream_id in writable {
        retry_raw_response(client, stream_id)?;
    }
    finish_due_raw_server_streams(client, stats)?;

    let readable: Vec<u64> = client.conn.readable().collect();
    let mut body_buf = [0u8; 8192];
    for stream_id in readable {
        if client.reset_streams.contains(&stream_id)
            || client.processing_streams.contains_key(&stream_id)
        {
            continue;
        }
        if !client.streams.contains_key(&stream_id) {
            let check_started = Instant::now();
            client.seen_requests += 1;
            let reject = if scenario.reject_percent > 0.0 {
                should_reject_by_percent(client.seen_requests, scenario.reject_percent)
            } else {
                client.in_flight >= scenario.threshold
            };
            if reject {
                match scenario.mode {
                    OverloadMode::App503 => {
                        let _ = client.conn.stream_shutdown(
                            stream_id,
                            quiche::Shutdown::Read,
                            OVERLOAD_ERROR_CODE,
                        );
                        update_server_stats(stats, |stats| {
                            stats.app_rejections += 1;
                            stats.handling_time += check_started.elapsed();
                        });
                        send_raw_response(client, stream_id, b"503 overloaded")?;
                        continue;
                    }
                    OverloadMode::Reset => {
                        let _ = client.conn.stream_shutdown(
                            stream_id,
                            quiche::Shutdown::Read,
                            OVERLOAD_ERROR_CODE,
                        );
                        let _ = client.conn.stream_shutdown(
                            stream_id,
                            quiche::Shutdown::Write,
                            OVERLOAD_ERROR_CODE,
                        );
                        client.reset_streams.insert(stream_id);
                        update_server_stats(stats, |stats| {
                            stats.reset_rejections += 1;
                            stats.handling_time += check_started.elapsed();
                        });
                        continue;
                    }
                }
            }

            client.in_flight += 1;
            client.streams.insert(
                stream_id,
                RawServerStream {
                    started: Instant::now(),
                    bytes_read: 0,
                    overloaded: false,
                },
            );
            let in_flight = client.in_flight;
            update_server_stats(stats, |stats| {
                stats.accepted_requests += 1;
                stats.peak_inflight = stats.peak_inflight.max(in_flight);
            });
        }

        loop {
            match client.conn.stream_recv(stream_id, &mut body_buf) {
                Ok((read, fin)) => {
                    if let Some(stream) = client.streams.get_mut(&stream_id) {
                        stream.bytes_read += read;
                    }
                    update_server_stats(stats, |stats| stats.bytes_read += read);
                    if fin {
                        client
                            .processing_streams
                            .insert(stream_id, Instant::now() + SIMULATED_PROCESSING_DELAY);
                        break;
                    }
                }
                Err(quiche::Error::Done) => break,
                Err(quiche::Error::StreamReset(_)) => {
                    if client.streams.remove(&stream_id).is_some() {
                        client.in_flight = client.in_flight.saturating_sub(1);
                    }
                    break;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
    finish_due_raw_server_streams(client, stats)?;
    Ok(())
}

fn should_reject_by_percent(request_index: usize, percent: f64) -> bool {
    let basis_points = (percent * 100.0).round() as usize;
    if basis_points == 0 {
        return false;
    }
    if basis_points >= 10_000 {
        return true;
    }
    request_index.saturating_mul(basis_points) % 10_000 < basis_points
}

fn finish_due_raw_server_streams(
    client: &mut RawServerClient,
    stats: &Arc<Mutex<OverloadServerStats>>,
) -> Result<()> {
    let now = Instant::now();
    let due: Vec<u64> = client
        .processing_streams
        .iter()
        .filter_map(|(stream_id, ready_at)| (*ready_at <= now).then_some(*stream_id))
        .collect();
    for stream_id in due {
        client.processing_streams.remove(&stream_id);
        finish_raw_server_stream(client, stream_id, stats)?;
    }
    Ok(())
}

fn finish_raw_server_stream(
    client: &mut RawServerClient,
    stream_id: u64,
    stats: &Arc<Mutex<OverloadServerStats>>,
) -> Result<()> {
    let Some(stream) = client.streams.remove(&stream_id) else {
        return Ok(());
    };
    simulate_request_processing(stream.bytes_read);
    let response = if stream.overloaded {
        b"503 overloaded".as_slice()
    } else {
        b"200 ok".as_slice()
    };
    client.in_flight = client.in_flight.saturating_sub(1);
    update_server_stats(stats, |stats| {
        stats.handling_time += stream.started.elapsed();
        if stream.overloaded {
            stats.app_rejections += 1;
        } else {
            stats.successful_requests += 1;
        }
    });
    send_raw_response(client, stream_id, response)
}

fn send_raw_response(
    client: &mut RawServerClient,
    stream_id: u64,
    response: &'static [u8],
) -> Result<()> {
    match client.conn.stream_send(stream_id, response, true) {
        Ok(written) if written == response.len() => Ok(()),
        Ok(written) => {
            client.pending_responses.insert(
                stream_id,
                PendingRawResponse {
                    data: response,
                    written,
                },
            );
            Ok(())
        }
        Err(quiche::Error::Done) => {
            client.pending_responses.insert(
                stream_id,
                PendingRawResponse {
                    data: response,
                    written: 0,
                },
            );
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn retry_raw_response(client: &mut RawServerClient, stream_id: u64) -> Result<()> {
    let Some(response) = client.pending_responses.get_mut(&stream_id) else {
        return Ok(());
    };
    while response.written < response.data.len() {
        match client
            .conn
            .stream_send(stream_id, &response.data[response.written..], true)
        {
            Ok(0) => break,
            Ok(written) => response.written += written,
            Err(quiche::Error::Done) => break,
            Err(e) => return Err(e.into()),
        }
    }
    if response.written == response.data.len() {
        client.pending_responses.remove(&stream_id);
    }
    Ok(())
}

fn update_server_stats<F>(stats: &Arc<Mutex<OverloadServerStats>>, update: F)
where
    F: FnOnce(&mut OverloadServerStats),
{
    if let Ok(mut stats) = stats.lock() {
        update(&mut stats);
    }
}

fn simulate_request_processing(bytes: usize) {
    let mut acc = bytes as u64 ^ 0x9e37_79b9;
    let rounds = (bytes / 128).max(1) + 128;
    for i in 0..rounds {
        acc = acc
            .wrapping_mul(1_664_525)
            .wrapping_add(i as u64 ^ 1_013_904_223);
    }
    std::hint::black_box(acc);
}

struct RawQuicClient {
    socket: UdpSocket,
    poll: Poll,
    events: Events,
    conn: quiche::Connection,
    next_stream_id: u64,
    pending: HashMap<u64, RawPendingRequest>,
    recv_buf: [u8; READ_BUF_SIZE],
    out_buf: [u8; MAX_DATAGRAM_SIZE],
}

struct RawPendingRequest {
    started: Instant,
    payload: Bytes,
    written: usize,
    stopped_sending: bool,
    response: Vec<u8>,
}

struct RawClientCompletion {
    latency: Duration,
    outcome: RawClientOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RawClientOutcome {
    Success,
    AppRejected,
    Reset,
    Error,
}

impl RawQuicClient {
    fn connect(server_addr: SocketAddr, max_streams: usize) -> Result<Self> {
        let mut socket = UdpSocket::bind("127.0.0.1:0".parse()?)?;
        let mut poll = Poll::new()?;
        poll.registry()
            .register(&mut socket, H3_SOCKET, Interest::READABLE)?;
        let mut events = Events::with_capacity(128);
        let local_addr = socket.local_addr()?;
        let mut config = raw_quiche_config(true, max_streams)?;
        let scid_bytes = [0xcc; quiche::MAX_CONN_ID_LEN];
        let scid = quiche::ConnectionId::from_ref(&scid_bytes);
        let mut conn = quiche::connect(
            Some("localhost"),
            &scid,
            local_addr,
            server_addr,
            &mut config,
        )?;
        let mut out_buf = [0u8; MAX_DATAGRAM_SIZE];
        flush_quiche(&socket, &mut conn, &mut out_buf)?;
        let mut recv_buf = [0u8; READ_BUF_SIZE];
        let deadline = Instant::now() + Duration::from_secs(5);
        while !conn.is_established() {
            if Instant::now() > deadline {
                return Err(anyhow!("raw QUIC handshake timed out"));
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
        Ok(Self {
            socket,
            poll,
            events,
            conn,
            next_stream_id: 0,
            pending: HashMap::new(),
            recv_buf,
            out_buf,
        })
    }

    fn inflight(&self) -> usize {
        self.pending.len()
    }

    fn open_request(&mut self, payload: Bytes) -> Result<bool> {
        let stream_id = self.next_stream_id;
        match self.write_request_chunk(stream_id, &payload, 0) {
            Ok(RawWriteProgress::Written(written)) => {
                self.next_stream_id += 4;
                self.pending.insert(
                    stream_id,
                    RawPendingRequest {
                        started: Instant::now(),
                        payload,
                        written,
                        stopped_sending: false,
                        response: Vec::new(),
                    },
                );
                flush_quiche(&self.socket, &mut self.conn, &mut self.out_buf)?;
                Ok(true)
            }
            Ok(RawWriteProgress::Blocked) => Ok(false),
            Ok(RawWriteProgress::Reset) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn write_request_chunk(
        &mut self,
        stream_id: u64,
        payload: &Bytes,
        written: usize,
    ) -> Result<RawWriteProgress> {
        if written >= payload.len() {
            return Ok(RawWriteProgress::Written(written));
        }
        let end = (written + RAW_STREAM_CHUNK_SIZE).min(payload.len());
        let fin = end == payload.len();
        match self
            .conn
            .stream_send(stream_id, &payload[written..end], fin)
        {
            Ok(0) => Ok(RawWriteProgress::Blocked),
            Ok(n) => Ok(RawWriteProgress::Written(written + n)),
            Err(quiche::Error::Done) | Err(quiche::Error::StreamLimit) => {
                Ok(RawWriteProgress::Blocked)
            }
            Err(quiche::Error::StreamStopped(_)) => Ok(RawWriteProgress::Reset),
            Err(e) => Err(e.into()),
        }
    }

    fn drive(&mut self) -> Result<Vec<RawClientCompletion>> {
        let mut completions = self.flush_pending_requests()?;
        flush_quiche(&self.socket, &mut self.conn, &mut self.out_buf)?;
        self.events.clear();
        self.poll.poll(&mut self.events, self.conn.timeout())?;
        if self.events.is_empty() {
            self.conn.on_timeout();
        } else {
            recv_quiche(&self.socket, &mut self.conn, &mut self.recv_buf)?;
        }
        completions.extend(self.read_responses()?);
        completions.extend(self.flush_pending_requests()?);
        flush_quiche(&self.socket, &mut self.conn, &mut self.out_buf)?;
        Ok(completions)
    }

    fn flush_pending_requests(&mut self) -> Result<Vec<RawClientCompletion>> {
        let mut completions = Vec::new();
        let stream_ids: Vec<u64> = self.pending.keys().copied().collect();
        for stream_id in stream_ids {
            let Some((payload, written, stopped_sending)) =
                self.pending.get(&stream_id).map(|request| {
                    (
                        request.payload.clone(),
                        request.written,
                        request.stopped_sending,
                    )
                })
            else {
                continue;
            };
            if stopped_sending {
                continue;
            }
            if written >= payload.len() {
                continue;
            }
            match self.write_request_chunk(stream_id, &payload, written) {
                Ok(RawWriteProgress::Written(new_written)) => {
                    if let Some(request) = self.pending.get_mut(&stream_id) {
                        request.written = new_written;
                    }
                }
                Ok(RawWriteProgress::Blocked) => {}
                Ok(RawWriteProgress::Reset) => {
                    if let Some(request) = self.pending.get_mut(&stream_id) {
                        request.stopped_sending = true;
                    }
                }
                Err(_) => {
                    if let Some(request) = self.pending.remove(&stream_id) {
                        completions.push(RawClientCompletion {
                            latency: request.started.elapsed(),
                            outcome: RawClientOutcome::Error,
                        });
                    }
                }
            }
        }
        Ok(completions)
    }

    fn read_responses(&mut self) -> Result<Vec<RawClientCompletion>> {
        let readable: Vec<u64> = self.conn.readable().collect();
        let mut completions = Vec::new();
        for stream_id in readable {
            loop {
                match self.conn.stream_recv(stream_id, &mut self.recv_buf) {
                    Ok((read, fin)) => {
                        if let Some(request) = self.pending.get_mut(&stream_id) {
                            request.response.extend_from_slice(&self.recv_buf[..read]);
                        }
                        if fin {
                            if let Some(request) = self.pending.remove(&stream_id) {
                                completions.push(RawClientCompletion {
                                    latency: request.started.elapsed(),
                                    outcome: classify_raw_response(&request.response),
                                });
                            }
                            break;
                        }
                    }
                    Err(quiche::Error::Done) => break,
                    Err(quiche::Error::StreamReset(_)) => {
                        if let Some(request) = self.pending.remove(&stream_id) {
                            completions.push(RawClientCompletion {
                                latency: request.started.elapsed(),
                                outcome: RawClientOutcome::Reset,
                            });
                        }
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(completions)
    }
}

enum RawWriteProgress {
    Written(usize),
    Blocked,
    Reset,
}

fn classify_raw_response(response: &[u8]) -> RawClientOutcome {
    if response.starts_with(b"200") {
        RawClientOutcome::Success
    } else if response.starts_with(b"503") || response.starts_with(b"429") {
        RawClientOutcome::AppRejected
    } else {
        RawClientOutcome::Error
    }
}

fn write_overload_csv(path: &PathBuf, rows: &[OverloadResultRow]) -> Result<()> {
    let mut writer = fs::File::create(path)?;
    writeln!(
        writer,
        "mode,total_requests,concurrency,payload_size_bytes,threshold,reject_percent,repeat_index,duration_ms,throughput_req_s,successes,rejected,reset_streams,errors,latency_mean_ms,latency_p50_ms,latency_p95_ms,latency_p99_ms,latency_min_ms,latency_max_ms,rejection_latency_mean_ms,rejection_latency_p95_ms,rejection_latency_p99_ms,server_bytes_read,server_handling_time_ms,server_avg_handling_time_ms,server_accepted_requests,server_successful_requests,server_app_rejections,server_reset_rejections,server_peak_inflight,cpu_user_ms,cpu_system_ms,cpu_total_ms,cpu_percent,rss_bytes_delta,maxrss_kb_after"
    )?;
    for row in rows {
        let server_handled =
            row.server.accepted_requests + row.server.app_rejections + row.server.reset_rejections;
        let server_avg = if server_handled == 0 {
            0.0
        } else {
            ms(row.server.handling_time) / server_handled as f64
        };
        let cpu_total_ms = ms(row.resources.cpu_user) + ms(row.resources.cpu_system);
        let cpu_percent = cpu_percent(
            row.resources.cpu_user,
            row.resources.cpu_system,
            row.duration,
        );
        writeln!(
            writer,
            "{},{},{},{},{},{:.3},{},{:.3},{:.2},{},{},{},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{},{:.3},{:.3},{},{},{},{},{},{:.3},{:.3},{:.3},{:.2},{},{}",
            row.scenario.mode.as_str(),
            row.scenario.total_requests,
            row.scenario.concurrency,
            row.scenario.payload_size,
            row.scenario.threshold,
            row.scenario.reject_percent,
            row.repeat_index,
            ms(row.duration),
            row.throughput,
            row.successes,
            row.rejected,
            row.reset_streams,
            row.errors,
            ms(row.latency.mean),
            ms(row.latency.p50),
            ms(row.latency.p95),
            ms(row.latency.p99),
            ms(row.latency.min),
            ms(row.latency.max),
            ms(row.rejection_latency.mean),
            ms(row.rejection_latency.p95),
            ms(row.rejection_latency.p99),
            row.server.bytes_read,
            ms(row.server.handling_time),
            server_avg,
            row.server.accepted_requests,
            row.server.successful_requests,
            row.server.app_rejections,
            row.server.reset_rejections,
            row.server.peak_inflight,
            ms(row.resources.cpu_user),
            ms(row.resources.cpu_system),
            cpu_total_ms,
            cpu_percent,
            row.resources.rss_bytes_delta,
            row.resources.maxrss_kb_after
        )?;
    }
    Ok(())
}

fn cpu_percent(cpu_user: Duration, cpu_system: Duration, wall: Duration) -> f64 {
    if wall.is_zero() {
        0.0
    } else {
        (cpu_user + cpu_system).as_secs_f64() / wall.as_secs_f64() * 100.0
    }
}

fn write_summary_report(path: &PathBuf, rows: &[&OverloadResultRow]) -> Result<()> {
    let verdict = overload_claim_supported(rows);
    let mut writer = fs::File::create(path)?;
    writeln!(writer, "# QUIC Overload-Handling Experiment")?;
    writeln!(writer)?;
    writeln!(
        writer,
        "Claim: QUIC stream-level rejection can reduce Control Plane overload processing."
    )?;
    writeln!(writer)?;
    writeln!(writer, "Result:")?;
    writeln!(
        writer,
        "- Supported if reset mode shows lower server bytes read, lower server processing time, and lower overload rejection latency."
    )?;
    writeln!(
        writer,
        "- Not fully supported if the client requires standard HTTP 429/503 semantics."
    )?;
    writeln!(
        writer,
        "- Additional mapping is needed from QUIC stream error code to retry/backoff behavior."
    )?;
    writeln!(writer)?;
    writeln!(
        writer,
        "Observed outcome: {}.",
        if verdict {
            "supported by the completed matched scenarios"
        } else {
            "not consistently supported by the completed matched scenarios"
        }
    )?;
    writeln!(writer)?;
    writeln!(
        writer,
        "| Payload bytes | Concurrency | Threshold | app503 bytes read | reset bytes read | app503 server ms | reset server ms | app503 CPU % | reset CPU % | app503 reject mean ms | reset reject mean ms |"
    )?;
    writeln!(
        writer,
        "| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    )?;
    for comparison in overload_comparisons(rows) {
        writeln!(
            writer,
            "| {} | {} | {} | {} | {} | {:.3} | {:.3} | {:.2} | {:.2} | {:.3} | {:.3} |",
            comparison.payload_size,
            comparison.concurrency,
            comparison.threshold,
            comparison.app503_bytes_read,
            comparison.reset_bytes_read,
            comparison.app503_server_ms,
            comparison.reset_server_ms,
            comparison.app503_cpu_percent,
            comparison.reset_cpu_percent,
            comparison.app503_reject_mean_ms,
            comparison.reset_reject_mean_ms
        )?;
    }
    Ok(())
}

struct OverloadComparison {
    payload_size: usize,
    concurrency: usize,
    threshold: usize,
    app503_bytes_read: usize,
    reset_bytes_read: usize,
    app503_server_ms: f64,
    reset_server_ms: f64,
    app503_cpu_percent: f64,
    reset_cpu_percent: f64,
    app503_reject_mean_ms: f64,
    reset_reject_mean_ms: f64,
}

fn overload_comparisons(rows: &[&OverloadResultRow]) -> Vec<OverloadComparison> {
    let mut app503 = BTreeMap::new();
    let mut reset = BTreeMap::new();
    for row in rows {
        let key = (
            row.scenario.payload_size,
            row.scenario.concurrency,
            row.scenario.threshold,
            (row.scenario.reject_percent * 1000.0).round() as usize,
            row.repeat_index,
        );
        match row.scenario.mode {
            OverloadMode::App503 => {
                app503.insert(key, *row);
            }
            OverloadMode::Reset => {
                reset.insert(key, *row);
            }
        }
    }

    let mut out = Vec::new();
    for (key, app_row) in app503 {
        let Some(reset_row) = reset.get(&key) else {
            continue;
        };
        out.push(OverloadComparison {
            payload_size: key.0,
            concurrency: key.1,
            threshold: key.2,
            app503_bytes_read: app_row.server.bytes_read,
            reset_bytes_read: reset_row.server.bytes_read,
            app503_server_ms: ms(app_row.server.handling_time),
            reset_server_ms: ms(reset_row.server.handling_time),
            app503_cpu_percent: cpu_percent(
                app_row.resources.cpu_user,
                app_row.resources.cpu_system,
                app_row.duration,
            ),
            reset_cpu_percent: cpu_percent(
                reset_row.resources.cpu_user,
                reset_row.resources.cpu_system,
                reset_row.duration,
            ),
            app503_reject_mean_ms: ms(app_row.rejection_latency.mean),
            reset_reject_mean_ms: ms(reset_row.rejection_latency.mean),
        });
    }
    out
}

fn overload_claim_supported(rows: &[&OverloadResultRow]) -> bool {
    let comparisons = overload_comparisons(rows);
    !comparisons.is_empty()
        && comparisons.iter().all(|comparison| {
            comparison.reset_bytes_read < comparison.app503_bytes_read
                && comparison.reset_server_ms < comparison.app503_server_ms
                && comparison.reset_reject_mean_ms <= comparison.app503_reject_mean_ms
        })
}

fn write_overload_charts(output_dir: &PathBuf, rows: &[&OverloadResultRow]) -> Result<()> {
    let csv = rows
        .iter()
        .map(|row| {
            format!(
                "{},{},{},{},{},{:.6},{}",
                row.scenario.mode.as_str(),
                row.scenario.payload_size,
                row.scenario.concurrency,
                if row.scenario.reject_percent > 0.0 {
                    format!("r{:.1}", row.scenario.reject_percent)
                } else {
                    format!("t{}", row.scenario.threshold)
                },
                row.repeat_index,
                ms(row.latency.p95),
                row.server.bytes_read
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let script = r#"
import csv
import os
import sys
from io import StringIO
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

out_dir = sys.argv[1]
rows = list(csv.DictReader(StringIO(sys.stdin.read()), fieldnames=[
    "mode", "payload", "concurrency", "threshold", "repeat", "p95", "bytes"
]))
groups = {}
for row in rows:
    key = f"{int(row['payload'])//1024}KB/c{row['concurrency']}/{row['threshold']}"
    groups.setdefault(key, {})[row["mode"]] = row
labels = list(groups.keys())

def plot(metric, ylabel, filename):
    app = [float(groups[label].get("app503", {}).get(metric, 0)) for label in labels]
    reset = [float(groups[label].get("reset", {}).get(metric, 0)) for label in labels]
    x = list(range(len(labels)))
    width = 0.42
    fig_w = max(10, len(labels) * 0.42)
    plt.figure(figsize=(fig_w, 5))
    plt.bar([v - width / 2 for v in x], app, width, label="app503")
    plt.bar([v + width / 2 for v in x], reset, width, label="reset")
    plt.ylabel(ylabel)
    plt.xticks(x, labels, rotation=60, ha="right")
    plt.legend()
    plt.tight_layout()
    plt.savefig(os.path.join(out_dir, filename), dpi=150)
    plt.close()

plot("p95", "Client p95 latency (ms)", "latency_comparison.png")
plot("bytes", "Server bytes read", "server_bytes_read_comparison.png")
"#;
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(output_dir)
        .env("MPLCONFIGDIR", "/tmp/matplotlib-transportbench")
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("start matplotlib chart generator")?;
    {
        let stdin = child.stdin.as_mut().context("open chart generator stdin")?;
        stdin.write_all(csv.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "chart generation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}
