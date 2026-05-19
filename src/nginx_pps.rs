#[derive(Debug, Clone)]
struct NginxPpsConfig {
    protocols: Vec<Protocol>,
    payload_sizes: Vec<usize>,
    target_pps: Vec<usize>,
    duration: Duration,
    max_inflight: usize,
    repeats: usize,
    target: TargetEndpoint,
    nginx_container: String,
    output: PathBuf,
}

impl NginxPpsConfig {
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
            target: TargetEndpoint::parse("https://localhost:8443/api")?,
            nginx_container: "quic-nginx".to_string(),
            output: PathBuf::from("reports/nginx_rust_client_results/results.csv"),
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
                "-target" | "--target" => {
                    cfg.target = TargetEndpoint::parse(&next_arg(&mut args, &arg)?)?
                }
                "-nginx-container" | "--nginx-container" => {
                    cfg.nginx_container = next_arg(&mut args, &arg)?
                }
                "-output" | "--output" => cfg.output = PathBuf::from(next_arg(&mut args, &arg)?),
                "-h" | "--help" => {
                    print_nginx_pps_help();
                    std::process::exit(0);
                }
                _ => return Err(anyhow!("unknown rust-pps-nginx argument {arg}")),
            }
        }

        Ok(cfg)
    }
}

fn print_nginx_pps_help() {
    println!(
        "transportbench-rust rust-pps-nginx\n\n\
         Uses the Rust fixed-PPS client against an external NGINX H2/H3 endpoint.\n\n\
         Flags:\n\
           -target https://localhost:8443/api\n\
           -nginx-container quic-nginx\n\
           -protocols h2,h3\n\
           -payload-sizes 1024\n\
           -pps 1000,3000,6000,10000\n\
           -duration 10\n\
           -max-inflight 100\n\
           -repeat 1\n\
           -output ./reports/nginx_rust_client_results/results.csv"
    );
}

#[derive(Debug, Clone)]
struct TargetEndpoint {
    uri: String,
    host: String,
    authority: String,
    path: String,
    addr: SocketAddr,
}

impl TargetEndpoint {
    fn parse(value: &str) -> Result<Self> {
        let rest = value
            .strip_prefix("https://")
            .ok_or_else(|| anyhow!("target must start with https://"))?;
        let (authority, path) = match rest.split_once('/') {
            Some((authority, path)) => (authority.to_string(), format!("/{path}")),
            None => (rest.to_string(), "/".to_string()),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (
                host.to_string(),
                port.parse::<u16>()
                    .with_context(|| format!("invalid target port in {value:?}"))?,
            ),
            None => (authority.clone(), 443),
        };
        let addr = format!("{host}:{port}")
            .to_socket_addrs()
            .with_context(|| format!("resolve target {host}:{port}"))?
            .next()
            .ok_or_else(|| anyhow!("target {host}:{port} did not resolve"))?;
        Ok(Self {
            uri: value.to_string(),
            host,
            authority,
            path,
            addr,
        })
    }
}

struct NginxPpsRow {
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
    client_resources: ResourceDelta,
    client_rss_bytes_after: i64,
    nginx_cpu_percent: f64,
    nginx_mem_mib: f64,
}

async fn run_nginx_pps_cli(cfg: NginxPpsConfig) -> Result<()> {
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
                            run_h2_nginx_pps_trial(
                                &cfg,
                                payload,
                                *payload_size,
                                *target_pps,
                                repeat_index,
                            )
                            .await?
                        }
                        Protocol::H3 => run_h3_nginx_pps_trial(
                            &cfg,
                            payload,
                            *payload_size,
                            *target_pps,
                            repeat_index,
                        )?,
                    };
                    rows.push(row);
                }
            }
        }
    }
    write_nginx_pps_csv(&cfg.output, &rows)?;
    Ok(())
}

async fn run_h2_nginx_pps_trial(
    cfg: &NginxPpsConfig,
    payload: Bytes,
    payload_size: usize,
    target_pps: usize,
    repeat_index: usize,
) -> Result<NginxPpsRow> {
    let stream = TcpStream::connect(cfg.target.addr).await?;
    stream.set_nodelay(true)?;
    let connector = TlsConnector::from(h2_insecure_client_config());
    let stream = connector
        .connect(ServerName::try_from(cfg.target.host.clone())?, stream)
        .await?;
    let (sender, connection) = h2::client::handshake(stream).await?;
    let conn_task = tokio::spawn(async move {
        let _ = connection.await;
    });
    let row = run_h2_fixed_pps_sender(
        Protocol::H2,
        sender,
        payload,
        payload_size,
        target_pps,
        repeat_index,
        cfg.duration,
        cfg.max_inflight,
        &cfg.target.uri,
        Some(&cfg.nginx_container),
    )
    .await?;
    conn_task.abort();
    let _ = conn_task.await;
    Ok(row)
}

async fn run_h2_fixed_pps_sender(
    protocol: Protocol,
    sender: SendRequest<Bytes>,
    payload: Bytes,
    payload_size: usize,
    target_pps: usize,
    repeat_index: usize,
    duration: Duration,
    max_inflight: usize,
    uri: &str,
    nginx_container: Option<&str>,
) -> Result<NginxPpsRow> {
    let scheduled_requests = target_pps * duration.as_secs() as usize;
    let interval = Duration::from_secs_f64(1.0 / target_pps as f64);
    let sampler = nginx_container.map(start_docker_stats_sampler);
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
            let request_uri = uri.to_string();
            inflight.push(tokio::spawn(async move {
                let started = Instant::now();
                let ok = h2_post_uri(request_sender, request_payload, &request_uri)
                    .await
                    .is_ok();
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
    let (nginx_cpu_percent, nginx_mem_mib) =
        sampler.map(finish_docker_stats_sampler).unwrap_or((0.0, 0.0));

    Ok(NginxPpsRow {
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
        client_resources: diff_resources(before, after),
        client_rss_bytes_after: after.rss_bytes,
        nginx_cpu_percent,
        nginx_mem_mib,
    })
}

fn run_h3_nginx_pps_trial(
    cfg: &NginxPpsConfig,
    payload: Bytes,
    payload_size: usize,
    target_pps: usize,
    repeat_index: usize,
) -> Result<NginxPpsRow> {
    let client = H3Client::connect_with_request_target(
        cfg.target.addr,
        cfg.max_inflight + 128,
        &cfg.target.host,
        &cfg.target.authority,
        &cfg.target.path,
    )?;
    run_h3_fixed_pps_client(
        client,
        payload,
        payload_size,
        target_pps,
        repeat_index,
        cfg.duration,
        cfg.max_inflight,
        Some(&cfg.nginx_container),
    )
}

fn run_h3_fixed_pps_client(
    mut client: H3Client,
    payload: Bytes,
    payload_size: usize,
    target_pps: usize,
    repeat_index: usize,
    duration: Duration,
    max_inflight: usize,
    nginx_container: Option<&str>,
) -> Result<NginxPpsRow> {
    let scheduled_requests = target_pps * duration.as_secs() as usize;
    let interval = Duration::from_secs_f64(1.0 / target_pps as f64);
    let sampler = nginx_container.map(start_docker_stats_sampler);
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
            } else {
                errors += 1;
            }
            latencies.push(event.latency);
        }

        if last_progress.elapsed() > Duration::from_secs(30) {
            errors += scheduled_requests.saturating_sub(completed_requests);
            break;
        }
    }
    let measured_duration = started.elapsed();
    let after = capture_resources();
    let (nginx_cpu_percent, nginx_mem_mib) =
        sampler.map(finish_docker_stats_sampler).unwrap_or((0.0, 0.0));

    Ok(NginxPpsRow {
        protocol: Protocol::H3,
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
        client_resources: diff_resources(before, after),
        client_rss_bytes_after: after.rss_bytes,
        nginx_cpu_percent,
        nginx_mem_mib,
    })
}

fn write_nginx_pps_csv(path: &Path, rows: &[NginxPpsRow]) -> Result<()> {
    let mut writer = fs::File::create(path)?;
    writeln!(
        writer,
        "protocol,profile,payload,target_pps,repeat_index,duration_ms,actual_req_s,scheduled_requests,sent_requests,completed_requests,successes,errors,mean_ms,p95_ms,p99_ms,nginx_cpu_percent,nginx_mib,client_cpu_percent,client_mib"
    )?;
    for row in rows {
        let actual_req_s = row.sent_requests as f64 / row.duration.as_secs_f64();
        writeln!(
            writer,
            "{},clean,{},{},{},{:.3},{:.1},{},{},{},{},{},{:.3},{:.3},{:.3},{:.2},{:.2},{:.2},{:.2}",
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
            row.nginx_cpu_percent,
            row.nginx_mem_mib,
            cpu_percent(
                row.client_resources.cpu_user,
                row.client_resources.cpu_system,
                row.duration,
            ),
            row.client_rss_bytes_after as f64 / 1024.0 / 1024.0,
        )?;
    }
    Ok(())
}
