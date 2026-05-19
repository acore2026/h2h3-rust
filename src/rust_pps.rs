#[derive(Debug, Clone)]
struct RustPpsConfig {
    protocols: Vec<Protocol>,
    payload_sizes: Vec<usize>,
    target_pps: Vec<usize>,
    duration: Duration,
    max_inflight: usize,
    repeats: usize,
    output: PathBuf,
}

impl RustPpsConfig {
    fn from_args<I>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = String>,
    {
        let mut cfg = Self {
            protocols: vec![Protocol::H2, Protocol::H3],
            payload_sizes: vec![1024],
            target_pps: vec![1000, 3000, 6000, 10000],
            duration: Duration::from_secs(10),
            max_inflight: 100,
            repeats: 1,
            output: PathBuf::from("reports/rust_pps_results/results.csv"),
        };

        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-protocols" | "--protocols" => {
                    cfg.protocols = parse_protocols(&next_arg(&mut args, &arg)?)?
                }
                "-payload-sizes" | "--payload-sizes" => {
                    cfg.payload_sizes =
                        parse_positive_usizes(&next_arg(&mut args, &arg)?, "payload-sizes")?
                }
                "-pps" | "--pps" | "-target-pps" | "--target-pps" => {
                    cfg.target_pps =
                        parse_positive_usizes(&next_arg(&mut args, &arg)?, "target-pps")?
                }
                "-duration" | "--duration" => {
                    cfg.duration = Duration::from_secs(parse_positive_usize(
                        &next_arg(&mut args, &arg)?,
                        "duration",
                    )? as u64)
                }
                "-max-inflight" | "--max-inflight" | "-concurrency" | "--concurrency" => {
                    cfg.max_inflight =
                        parse_positive_usize(&next_arg(&mut args, &arg)?, "max-inflight")?
                }
                "-repeat" | "--repeat" | "-repeats" | "--repeats" => {
                    cfg.repeats = parse_positive_usize(&next_arg(&mut args, &arg)?, "repeats")?
                }
                "-output" | "--output" => cfg.output = PathBuf::from(next_arg(&mut args, &arg)?),
                "-h" | "--help" => {
                    print_rust_pps_help();
                    std::process::exit(0);
                }
                _ => return Err(anyhow!("unknown rust-pps argument {arg}")),
            }
        }

        Ok(cfg)
    }
}

fn print_rust_pps_help() {
    println!(
        "transportbench-rust rust-pps\n\n\
         Uses the Rust client and Rust server with fixed target request rates.\n\n\
         Flags:\n\
           -protocols h2,h3\n\
           -payload-sizes 1024\n\
           -pps 1000,3000,6000,10000\n\
           -duration 10\n\
           -max-inflight 100\n\
           -repeat 1\n\
           -output ./reports/rust_pps_results/results.csv"
    );
}

struct RustPpsRow {
    protocol: Protocol,
    payload_size: usize,
    target_pps: usize,
    repeat_index: usize,
    duration: Duration,
    scheduled_requests: usize,
    sent_requests: usize,
    completed_requests: usize,
    successes: usize,
    errors: usize,
    latency: LatencySummary,
    resources: ResourceDelta,
    rss_bytes_after: i64,
}

async fn run_rust_pps_cli(cfg: RustPpsConfig) -> Result<()> {
    if let Some(parent) = cfg.output.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut rows = Vec::new();
    for repeat_index in 1..=cfg.repeats {
        for payload_size in &cfg.payload_sizes {
            for target_pps in &cfg.target_pps {
                for protocol in &cfg.protocols {
                    let payload = Bytes::from(vec![0x42; *payload_size]);
                    let row = match protocol {
                        Protocol::H2 => {
                            run_h2_rust_pps_trial(
                                *protocol,
                                payload,
                                *payload_size,
                                *target_pps,
                                repeat_index,
                                cfg.duration,
                                cfg.max_inflight,
                            )
                            .await?
                        }
                        Protocol::H3 => run_h3_rust_pps_trial(
                            *protocol,
                            payload,
                            *payload_size,
                            *target_pps,
                            repeat_index,
                            cfg.duration,
                            cfg.max_inflight,
                        )?,
                    };
                    rows.push(row);
                }
            }
        }
    }
    write_rust_pps_csv(&cfg.output, &rows)?;
    Ok(())
}

async fn run_h2_rust_pps_trial(
    protocol: Protocol,
    payload: Bytes,
    payload_size: usize,
    target_pps: usize,
    repeat_index: usize,
    duration: Duration,
    max_inflight: usize,
) -> Result<RustPpsRow> {
    let server = H2Server::start().await?;
    let stream = TcpStream::connect(server.addr).await?;
    stream.set_nodelay(true)?;
    let connector = TlsConnector::from(Arc::clone(&server.client_config));
    let stream = connector
        .connect(ServerName::try_from("localhost")?, stream)
        .await?;
    let (mut sender, connection) = h2::client::handshake(stream).await?;
    let conn_task = tokio::spawn(async move {
        let _ = connection.await;
    });

    for _ in 0..8 {
        sender = h2_post(sender, payload.clone()).await?;
    }

    let scheduled_requests = target_pps * duration.as_secs() as usize;
    let interval = Duration::from_secs_f64(1.0 / target_pps as f64);
    let before = capture_resources();
    let started = Instant::now();
    let mut next_send = tokio::time::Instant::now();
    let deadline = next_send + duration;
    let mut sent_requests = 0usize;
    let mut completed_requests = 0usize;
    let mut successes = 0usize;
    let mut errors = 0usize;
    let mut latencies = Vec::with_capacity(scheduled_requests);
    let mut inflight = FuturesUnordered::new();

    while sent_requests < scheduled_requests || !inflight.is_empty() {
        while sent_requests < scheduled_requests
            && tokio::time::Instant::now() >= next_send
            && inflight.len() < max_inflight
        {
            let request_sender = sender.clone();
            let request_payload = payload.clone();
            inflight.push(tokio::spawn(async move {
                let started = Instant::now();
                let ok = h2_post(request_sender, request_payload).await.is_ok();
                (started.elapsed(), ok)
            }));
            sent_requests += 1;
            next_send += interval;
        }

        if sent_requests >= scheduled_requests && inflight.is_empty() {
            break;
        }

        if !inflight.is_empty() {
            let sleep_until = if sent_requests < scheduled_requests && inflight.len() < max_inflight
            {
                next_send.min(deadline)
            } else {
                tokio::time::Instant::now() + Duration::from_millis(1)
            };
            tokio::select! {
                result = inflight.next() => {
                    if let Some(result) = result {
                        completed_requests += 1;
                        match result {
                            Ok((latency, true)) => {
                                successes += 1;
                                latencies.push(latency);
                            }
                            Ok((latency, false)) => {
                                errors += 1;
                                latencies.push(latency);
                            }
                            Err(_) => errors += 1,
                        }
                    }
                }
                _ = tokio::time::sleep_until(sleep_until), if sent_requests < scheduled_requests => {}
            }
        } else if sent_requests < scheduled_requests {
            tokio::time::sleep_until(next_send).await;
        }
    }

    let measured_duration = started.elapsed();
    let after = capture_resources();
    drop(sender);
    conn_task.abort();
    let _ = conn_task.await;
    server.task.abort();
    let _ = server.task.await;

    Ok(RustPpsRow {
        protocol,
        payload_size,
        target_pps,
        repeat_index,
        duration: measured_duration,
        scheduled_requests,
        sent_requests,
        completed_requests,
        successes,
        errors,
        latency: summarize_latencies(&latencies),
        resources: diff_resources(before, after),
        rss_bytes_after: after.rss_bytes,
    })
}

fn run_h3_rust_pps_trial(
    protocol: Protocol,
    payload: Bytes,
    payload_size: usize,
    target_pps: usize,
    repeat_index: usize,
    duration: Duration,
    max_inflight: usize,
) -> Result<RustPpsRow> {
    let cert_files = CertFiles::new()?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let server = H3Server::start(&cert_files, Arc::clone(&shutdown))?;
    let mut client = H3Client::connect(server.addr, max_inflight + 128)?;

    for _ in 0..8 {
        client.request(payload.clone(), 0)?;
    }

    let scheduled_requests = target_pps * duration.as_secs() as usize;
    let interval = Duration::from_secs_f64(1.0 / target_pps as f64);
    let before = capture_resources();
    let started = Instant::now();
    let deadline = started + duration;
    let mut next_send = started;
    let mut sent_requests = 0usize;
    let mut completed_requests = 0usize;
    let mut successes = 0usize;
    let mut errors = 0usize;
    let mut latencies = Vec::with_capacity(scheduled_requests);
    let mut last_progress = Instant::now();

    while sent_requests < scheduled_requests || client.inflight() > 0 {
        while sent_requests < scheduled_requests
            && Instant::now() >= next_send
            && client.inflight() < max_inflight
        {
            match client.send_request(payload.clone(), sent_requests % max_inflight) {
                Ok(true) => {
                    sent_requests += 1;
                    next_send += interval;
                    last_progress = Instant::now();
                }
                Ok(false) => break,
                Err(_) => {
                    sent_requests += 1;
                    errors += 1;
                    next_send += interval;
                    last_progress = Instant::now();
                }
            }
        }

        let poll_timeout = if sent_requests < scheduled_requests && client.inflight() < max_inflight
        {
            Some(next_send.min(deadline).saturating_duration_since(Instant::now()))
        } else {
            Some(Duration::from_millis(1))
        };
        let events = client.drive_with_max_timeout(poll_timeout)?;
        for event in events {
            completed_requests += 1;
            last_progress = Instant::now();
            if event.ok {
                successes += 1;
                latencies.push(event.latency);
            } else {
                errors += 1;
                latencies.push(event.latency);
            }
        }

        if last_progress.elapsed() > Duration::from_secs(30) {
            errors += scheduled_requests.saturating_sub(completed_requests);
            break;
        }
    }

    let measured_duration = started.elapsed();
    let after = capture_resources();
    shutdown.store(true, AtomicOrdering::SeqCst);
    let _ = server.handle.join();

    Ok(RustPpsRow {
        protocol,
        payload_size,
        target_pps,
        repeat_index,
        duration: measured_duration,
        scheduled_requests,
        sent_requests,
        completed_requests,
        successes,
        errors,
        latency: summarize_latencies(&latencies),
        resources: diff_resources(before, after),
        rss_bytes_after: after.rss_bytes,
    })
}

fn write_rust_pps_csv(path: &Path, rows: &[RustPpsRow]) -> Result<()> {
    let mut writer = fs::File::create(path)?;
    writeln!(
        writer,
        "protocol,profile,payload,target_pps,repeat_index,duration_ms,actual_req_s,scheduled_requests,sent_requests,completed_requests,successes,errors,mean_ms,p95_ms,p99_ms,process_cpu_percent,process_mib,maxrss_mib"
    )?;
    for row in rows {
        let actual_req_s = row.sent_requests as f64 / row.duration.as_secs_f64();
        writeln!(
            writer,
            "{},clean,{},{},{},{:.3},{:.1},{},{},{},{},{},{:.3},{:.3},{:.3},{:.2},{:.2},{:.2}",
            h2load_protocol_name(row.protocol),
            payload_label(row.payload_size),
            row.target_pps,
            row.repeat_index,
            ms(row.duration),
            actual_req_s,
            row.scheduled_requests,
            row.sent_requests,
            row.completed_requests,
            row.successes,
            row.errors,
            ms(row.latency.mean),
            ms(row.latency.p95),
            ms(row.latency.p99),
            cpu_percent(row.resources.cpu_user, row.resources.cpu_system, row.duration),
            row.rss_bytes_after as f64 / 1024.0 / 1024.0,
            row.resources.maxrss_kb_after as f64 / 1024.0,
        )?;
    }
    Ok(())
}
