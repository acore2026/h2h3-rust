#[derive(Debug, Clone)]
struct H2LoadRateConfig {
    protocols: Vec<Protocol>,
    payload_sizes: Vec<usize>,
    target_pps: Vec<usize>,
    duration: Duration,
    clients: usize,
    streams: usize,
    threads: usize,
    repeats: usize,
    h2load: String,
    output: PathBuf,
    log_dir: PathBuf,
}

impl H2LoadRateConfig {
    fn from_args<I>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = String>,
    {
        let output = PathBuf::from("reports/h2load_rate_rust/results.csv");
        let mut cfg = Self {
            protocols: vec![Protocol::H2, Protocol::H3],
            payload_sizes: vec![1024],
            target_pps: vec![1000, 3000, 6000, 10000],
            duration: Duration::from_secs(10),
            clients: 1,
            streams: 100,
            threads: 4,
            repeats: 1,
            h2load: "h2load".to_string(),
            log_dir: PathBuf::from("reports/h2load_rate_rust/logs"),
            output,
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
                "-clients" | "--clients" => {
                    cfg.clients = parse_positive_usize(&next_arg(&mut args, &arg)?, "clients")?
                }
                "-streams" | "--streams" => {
                    cfg.streams = parse_positive_usize(&next_arg(&mut args, &arg)?, "streams")?
                }
                "-threads" | "--threads" => {
                    cfg.threads = parse_positive_usize(&next_arg(&mut args, &arg)?, "threads")?
                }
                "-repeat" | "--repeat" | "-repeats" | "--repeats" => {
                    cfg.repeats = parse_positive_usize(&next_arg(&mut args, &arg)?, "repeats")?
                }
                "-h2load" | "--h2load" => cfg.h2load = next_arg(&mut args, &arg)?,
                "-output" | "--output" => cfg.output = PathBuf::from(next_arg(&mut args, &arg)?),
                "-log-dir" | "--log-dir" => {
                    cfg.log_dir = PathBuf::from(next_arg(&mut args, &arg)?)
                }
                "-h" | "--help" => {
                    print_h2load_rate_help();
                    std::process::exit(0);
                }
                _ => return Err(anyhow!("unknown h2load-rate argument {arg}")),
            }
        }

        Ok(cfg)
    }
}

fn print_h2load_rate_help() {
    println!(
        "transportbench-rust h2load-rate\n\n\
         Starts the Rust server and drives it with h2load at target request rates.\n\n\
         Flags:\n\
           -protocols h2,h3\n\
           -payload-sizes 1024\n\
           -pps 1000,3000,6000,10000\n\
           -duration 10\n\
           -clients 1\n\
           -streams 100\n\
           -threads 4\n\
           -repeat 1\n\
           -h2load h2load\n\
           -output ./reports/h2load_rate_rust/results.csv\n\
           -log-dir ./reports/h2load_rate_rust/logs"
    );
}

#[derive(Debug, Clone)]
struct H2LoadCapabilities {
    h3: bool,
    rps: bool,
    log_file: bool,
    alpn_list: bool,
}

struct H2LoadRateRow {
    protocol: Protocol,
    payload_size: usize,
    target_pps: usize,
    effective_target_pps: usize,
    repeat_index: usize,
    duration: Duration,
    requests_total: usize,
    requests_success: usize,
    requests_failed: usize,
    actual_req_s: f64,
    latency: LatencySummary,
    server_resources: ResourceDelta,
    server_rss_bytes: i64,
    client_cpu_percent: f64,
    client_mem_mib: f64,
    h2load_status: i32,
}

async fn run_h2load_rate_cli(cfg: H2LoadRateConfig) -> Result<()> {
    fs::create_dir_all(&cfg.log_dir)?;
    if let Some(parent) = cfg.output.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let caps = detect_h2load_capabilities(&cfg.h2load)?;
    if !caps.log_file {
        return Err(anyhow!("{} does not support --log-file", cfg.h2load));
    }
    if cfg.protocols.contains(&Protocol::H3) && !caps.h3 {
        return Err(anyhow!(
            "{} does not support --h3; provide a HTTP/3-enabled h2load with -h2load",
            cfg.h2load
        ));
    }

    let mut rows = Vec::new();
    for repeat_index in 1..=cfg.repeats {
        for payload_size in &cfg.payload_sizes {
            let payload_path = cfg
                .log_dir
                .join(format!("payload-{}-{}.bin", payload_size, repeat_index));
            fs::write(&payload_path, vec![0x42; *payload_size])?;
            for pps in &cfg.target_pps {
                for protocol in &cfg.protocols {
                    if *protocol == Protocol::H3 && cfg.clients != 1 {
                        return Err(anyhow!(
                            "Rust h3 h2load-rate currently supports -clients 1 because the simple quiche server accepts one client connection"
                        ));
                    }
                    rows.push(
                        run_h2load_rate_trial(
                            &cfg,
                            &caps,
                            *protocol,
                            *payload_size,
                            *pps,
                            repeat_index,
                            &payload_path,
                        )
                        .await?,
                    );
                }
            }
        }
    }
    write_h2load_rate_csv(&cfg.output, &rows)?;
    Ok(())
}

fn detect_h2load_capabilities(path: &str) -> Result<H2LoadCapabilities> {
    let output = Command::new(path)
        .arg("--help")
        .output()
        .with_context(|| format!("run {path} --help"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(H2LoadCapabilities {
        h3: text.contains("--h3"),
        rps: text.contains("--rps"),
        log_file: text.contains("--log-file"),
        alpn_list: text.contains("--alpn-list"),
    })
}

async fn run_h2load_rate_trial(
    cfg: &H2LoadRateConfig,
    caps: &H2LoadCapabilities,
    protocol: Protocol,
    payload_size: usize,
    target_pps: usize,
    repeat_index: usize,
    payload_path: &Path,
) -> Result<H2LoadRateRow> {
    match protocol {
        Protocol::H2 => {
            let server = H2Server::start_on("127.0.0.1:0".parse()?).await?;
            let row = run_h2load_against_addr(
                cfg,
                caps,
                protocol,
                server.addr,
                payload_size,
                target_pps,
                repeat_index,
                payload_path,
            )?;
            server.task.abort();
            let _ = server.task.await;
            Ok(row)
        }
        Protocol::H3 => {
            let cert_files = CertFiles::new()?;
            let shutdown = Arc::new(AtomicBool::new(false));
            let server =
                H3Server::start_on("127.0.0.1:0".parse()?, &cert_files, Arc::clone(&shutdown))?;
            let row = run_h2load_against_addr(
                cfg,
                caps,
                protocol,
                server.addr,
                payload_size,
                target_pps,
                repeat_index,
                payload_path,
            )?;
            shutdown.store(true, AtomicOrdering::SeqCst);
            let _ = server.handle.join();
            Ok(row)
        }
    }
}

fn run_h2load_against_addr(
    cfg: &H2LoadRateConfig,
    caps: &H2LoadCapabilities,
    protocol: Protocol,
    addr: SocketAddr,
    payload_size: usize,
    target_pps: usize,
    repeat_index: usize,
    payload_path: &Path,
) -> Result<H2LoadRateRow> {
    let protocol_name = h2load_protocol_name(protocol);
    let log_path = cfg.log_dir.join(format!(
        "{}-payload{}-pps{}-rep{}.log",
        protocol_name, payload_size, target_pps, repeat_index
    ));
    let stdout_path = cfg.log_dir.join(format!(
        "{}-payload{}-pps{}-rep{}.stdout",
        protocol_name, payload_size, target_pps, repeat_index
    ));
    let timing_path = cfg.log_dir.join(format!(
        "{}-payload{}-pps{}-rep{}.timing",
        protocol_name, payload_size, target_pps, repeat_index
    ));
    let url = format!("https://localhost:{}/sbi", addr.port());

    let mut args = Vec::new();
    match protocol {
        Protocol::H2 => {
            if caps.alpn_list {
                args.push("--alpn-list=h2".to_string());
            } else {
                args.push("--npn-list=h2".to_string());
            }
        }
        Protocol::H3 => args.push("--h3".to_string()),
    }
    args.extend([
        "-c".to_string(),
        cfg.clients.to_string(),
        "-m".to_string(),
        cfg.streams.to_string(),
        "-t".to_string(),
        cfg.threads.to_string(),
        "-H".to_string(),
        "content-type: application/octet-stream".to_string(),
        "-d".to_string(),
        payload_path.display().to_string(),
        "--log-file".to_string(),
        log_path.display().to_string(),
    ]);

    let effective_target_pps = if caps.rps {
        let per_client = target_pps.div_ceil(cfg.clients);
        args.extend([
            "-D".to_string(),
            format!("{}s", cfg.duration.as_secs()),
            "--rps".to_string(),
            per_client.to_string(),
        ]);
        per_client * cfg.clients
    } else {
        if cfg.clients != 1 {
            return Err(anyhow!(
                "{} lacks --rps; use -clients 1 or provide a newer h2load",
                cfg.h2load
            ));
        }
        write_timing_script(&timing_path, &url, target_pps, cfg.duration)?;
        args.extend([
            "-n".to_string(),
            (target_pps * cfg.duration.as_secs() as usize).to_string(),
            "--timing-script-file".to_string(),
            timing_path.display().to_string(),
        ]);
        target_pps
    };
    args.push(url);

    let docker_container = docker_h2load_container_name(&cfg.h2load, protocol, target_pps);
    let server_before = capture_resources();
    let child_before = capture_child_resources();
    let started = Instant::now();
    let mut command = Command::new(&cfg.h2load);
    command.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(name) = &docker_container {
        command.env("H2LOAD_DOCKER_NAME", name);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("run {}", cfg.h2load))?;
    let mut docker_samples = Vec::new();
    loop {
        if let Some(_status) = child.try_wait()? {
            break;
        }
        if let Some(name) = &docker_container {
            if let Some(sample) = sample_docker_container(name) {
                docker_samples.push(sample);
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
    let output = child.wait_with_output()?;
    let duration = started.elapsed();
    let server_after = capture_resources();
    let child_after = capture_child_resources();
    fs::write(&stdout_path, &output.stdout)?;
    if !output.stderr.is_empty() {
        let stderr_path = stdout_path.with_extension("stderr");
        fs::write(stderr_path, &output.stderr)?;
    }
    let status = output.status.code().unwrap_or(-1);
    let parsed = parse_h2load_log(&log_path)?;
    let requests_total = parsed.latencies.len();
    let measured_seconds = cfg.duration.as_secs_f64();
    let actual_req_s = if measured_seconds == 0.0 {
        0.0
    } else {
        requests_total as f64 / measured_seconds
    };
    let child_resources = diff_resources(child_before, child_after);
    let child_cpu_percent =
        cpu_percent(child_resources.cpu_user, child_resources.cpu_system, duration);
    let (client_cpu_percent, client_mem_mib) = if docker_samples.is_empty() {
        (
            child_cpu_percent,
            child_resources.maxrss_kb_after as f64 / 1024.0,
        )
    } else {
        let cpu = docker_samples
            .iter()
            .map(|sample| sample.cpu_percent)
            .sum::<f64>()
            / docker_samples.len() as f64;
        let mem = docker_samples
            .iter()
            .map(|sample| sample.mem_mib)
            .sum::<f64>()
            / docker_samples.len() as f64;
        (cpu, mem)
    };

    Ok(H2LoadRateRow {
        protocol,
        payload_size,
        target_pps,
        effective_target_pps,
        repeat_index,
        duration,
        requests_total,
        requests_success: parsed.success,
        requests_failed: parsed.failed,
        actual_req_s,
        latency: summarize_latencies(&parsed.latencies),
        server_resources: diff_resources(server_before, server_after),
        server_rss_bytes: server_after.rss_bytes,
        client_cpu_percent,
        client_mem_mib,
        h2load_status: status,
    })
}

fn write_timing_script(
    path: &Path,
    url: &str,
    target_pps: usize,
    duration: Duration,
) -> Result<()> {
    let total = target_pps * duration.as_secs() as usize;
    let interval_ms = 1000.0 / target_pps as f64;
    let mut file = fs::File::create(path)?;
    for i in 0..total {
        writeln!(file, "{:.6}\t{}", i as f64 * interval_ms, url)?;
    }
    Ok(())
}

#[derive(Clone)]
struct DockerStatsSample {
    cpu_percent: f64,
    mem_mib: f64,
}

struct DockerStatsSampler {
    stop: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<DockerStatsSample>>>,
    handle: thread::JoinHandle<()>,
}

fn start_docker_stats_sampler(name: &str) -> DockerStatsSampler {
    let stop = Arc::new(AtomicBool::new(false));
    let samples = Arc::new(Mutex::new(Vec::new()));
    let thread_stop = Arc::clone(&stop);
    let thread_samples = Arc::clone(&samples);
    let name = name.to_string();
    let handle = thread::spawn(move || {
        while !thread_stop.load(AtomicOrdering::SeqCst) {
            if let Some(sample) = sample_docker_container(&name) {
                if let Ok(mut samples) = thread_samples.lock() {
                    samples.push(sample);
                }
            }
            thread::sleep(Duration::from_millis(250));
        }
    });
    DockerStatsSampler {
        stop,
        samples,
        handle,
    }
}

fn finish_docker_stats_sampler(sampler: DockerStatsSampler) -> (f64, f64) {
    sampler.stop.store(true, AtomicOrdering::SeqCst);
    let _ = sampler.handle.join();
    let samples = sampler.samples.lock().map(|samples| samples.clone());
    let Ok(samples) = samples else {
        return (0.0, 0.0);
    };
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let cpu = samples
        .iter()
        .map(|sample| sample.cpu_percent)
        .sum::<f64>()
        / samples.len() as f64;
    let mem = samples.iter().map(|sample| sample.mem_mib).sum::<f64>() / samples.len() as f64;
    (cpu, mem)
}

fn docker_h2load_container_name(
    h2load_path: &str,
    protocol: Protocol,
    target_pps: usize,
) -> Option<String> {
    h2load_path.contains("h2load-docker").then(|| {
        format!(
            "rust-h2load-{}-{}-{}",
            std::process::id(),
            h2load_protocol_name(protocol),
            target_pps
        )
    })
}

fn sample_docker_container(name: &str) -> Option<DockerStatsSample> {
    let output = Command::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--format",
            "{{.CPUPerc}},{{.MemUsage}}",
            name,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?;
    let mut parts = line.splitn(2, ',');
    let cpu = parts.next()?.trim().trim_end_matches('%').parse().ok()?;
    let mem = parse_docker_mem_mib(parts.next()?.trim())?;
    Some(DockerStatsSample {
        cpu_percent: cpu,
        mem_mib: mem,
    })
}

fn parse_docker_mem_mib(value: &str) -> Option<f64> {
    let left = value.split('/').next()?.trim();
    let mut number = String::new();
    let mut unit = String::new();
    for ch in left.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            number.push(ch);
        } else if !ch.is_whitespace() {
            unit.push(ch);
        }
    }
    let value = number.parse::<f64>().ok()?;
    let factor = match unit.as_str() {
        "B" => 1.0 / 1024.0 / 1024.0,
        "kB" | "KB" | "KiB" => 1.0 / 1024.0,
        "MB" | "MiB" => 1.0,
        "GB" | "GiB" => 1024.0,
        _ => 1.0,
    };
    Some(value * factor)
}

struct ParsedH2LoadLog {
    latencies: Vec<Duration>,
    success: usize,
    failed: usize,
}

fn parse_h2load_log(path: &Path) -> Result<ParsedH2LoadLog> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut latencies = Vec::new();
    let mut success = 0usize;
    let mut failed = 0usize;
    for line in text.lines() {
        let mut parts = line.split('\t');
        let _started = parts.next();
        let status = parts.next().and_then(|value| value.parse::<i32>().ok());
        let elapsed_us = parts.next().and_then(|value| value.parse::<u64>().ok());
        if let Some(elapsed_us) = elapsed_us {
            latencies.push(Duration::from_micros(elapsed_us));
        }
        match status {
            Some(200..=299) => success += 1,
            Some(_) => failed += 1,
            None => {}
        }
    }
    Ok(ParsedH2LoadLog {
        latencies,
        success,
        failed,
    })
}

fn h2load_protocol_name(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::H2 => "h2",
        Protocol::H3 => "h3",
    }
}

fn write_h2load_rate_csv(path: &Path, rows: &[H2LoadRateRow]) -> Result<()> {
    let mut writer = fs::File::create(path)?;
    writeln!(
        writer,
        "protocol,profile,payload,target_pps,effective_target_pps,repeat_index,duration_ms,actual_req_s,requests_total,requests_success,requests_failed,mean_ms,p95_ms,p99_ms,server_cpu_percent,server_mib,client_cpu_percent,client_mib,h2load_status"
    )?;
    for row in rows {
        let server_cpu = cpu_percent(
            row.server_resources.cpu_user,
            row.server_resources.cpu_system,
            row.duration,
        );
        writeln!(
            writer,
            "{},clean,{},{},{},{},{:.3},{:.1},{},{},{},{:.3},{:.3},{:.3},{:.2},{:.2},{:.2},{:.2},{}",
            h2load_protocol_name(row.protocol),
            payload_label(row.payload_size),
            row.target_pps,
            row.effective_target_pps,
            row.repeat_index,
            ms(row.duration),
            row.actual_req_s,
            row.requests_total,
            row.requests_success,
            row.requests_failed,
            ms(row.latency.mean),
            ms(row.latency.p95),
            ms(row.latency.p99),
            server_cpu,
            row.server_rss_bytes as f64 / 1024.0 / 1024.0,
            row.client_cpu_percent,
            row.client_mem_mib,
            row.h2load_status
        )?;
    }
    Ok(())
}

fn payload_label(bytes: usize) -> String {
    if bytes >= 1024 && bytes % 1024 == 0 {
        format!("{}k", bytes / 1024)
    } else {
        format!("{}b", bytes)
    }
}
