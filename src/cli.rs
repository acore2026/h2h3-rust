#[derive(Debug, Clone)]
struct Config {
    payloads: String,
    protocols: Vec<Protocol>,
    users: Vec<usize>,
    messages: Vec<usize>,
    failures: Vec<FailureProfile>,
    repeats: usize,
    netem: bool,
    output: Option<String>,
}

impl Config {
    fn from_args<I>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = String>,
    {
        let mut cfg = Self {
            payloads: "../../SBI_PAYLOADS.json".to_string(),
            protocols: vec![Protocol::H2, Protocol::H3],
            users: vec![1, 8, 32, 128],
            messages: vec![100, 1000],
            failures: vec![FailureProfile::none()],
            repeats: 1,
            netem: false,
            output: None,
        };

        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-payloads" | "--payloads" => cfg.payloads = next_arg(&mut args, &arg)?,
                "-protocols" | "--protocols" => {
                    cfg.protocols = parse_protocols(&next_arg(&mut args, &arg)?)?
                }
                "-users" | "--users" => {
                    cfg.users = parse_positive_usizes(&next_arg(&mut args, &arg)?, "users")?
                }
                "-messages" | "--messages" => {
                    cfg.messages = parse_positive_usizes(&next_arg(&mut args, &arg)?, "messages")?
                }
                "-failures" | "--failures" => {
                    cfg.failures = parse_failures(&next_arg(&mut args, &arg)?)?
                }
                "-repeat" | "--repeat" | "-repeats" | "--repeats" => {
                    cfg.repeats = parse_positive_usize(&next_arg(&mut args, &arg)?, "repeats")?
                }
                "-output" | "--output" => cfg.output = Some(next_arg(&mut args, &arg)?),
                "-netem" | "--netem" => cfg.netem = true,
                "-h" | "--help" => {
                    print_help();
                    std::process::exit(0);
                }
                _ => return Err(anyhow!("unknown argument {arg}")),
            }
        }

        for failure in &cfg.failures {
            if !failure.netem_args.is_empty() && !cfg.netem {
                return Err(anyhow!(
                    "failure profile {:?} requires -netem",
                    failure.name
                ));
            }
        }

        Ok(cfg)
    }
}

fn next_arg<I>(args: &mut I, flag: &str) -> Result<String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| anyhow!("{flag} requires a value"))
}

fn print_help() {
    println!(
        "transportbench-rust\n\n\
         Subcommands:\n\
           overload   raw QUIC overload-handling experiment\n\n\
           h2load-rate   Rust server with h2load target-PPS client\n\n\
           rust-pps   Rust client/server fixed-PPS benchmark\n\n\
           rust-pps-nginx   Rust client fixed-PPS benchmark against NGINX\n\n\
         Flags:\n\
           -payloads ../../SBI_PAYLOADS.json\n\
           -protocols h2,h3\n\
           -users 1,8,32,128\n\
           -messages 100,1000\n\
           -failures none,loss0.001,loss0.01,loss0.1,loss1,delay100ms,delay1s,delay100ms_loss1\n\
           -repeat 1\n\
           -netem\n\
           -output reports/transportbench_rust.csv"
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    H2,
    H3,
}

impl Protocol {
    fn as_str(self) -> &'static str {
        match self {
            Protocol::H2 => "h2",
            Protocol::H3 => "h3-quiche",
        }
    }

    fn client_scheduler(self) -> &'static str {
        match self {
            Protocol::H2 => "tokio_user_tasks",
            Protocol::H3 => "quiche_readiness_loop",
        }
    }

    fn server_scheduler(self) -> &'static str {
        match self {
            Protocol::H2 => "in_connection_futures",
            Protocol::H3 => "quiche_readiness_loop",
        }
    }
}

fn parse_protocols(value: &str) -> Result<Vec<Protocol>> {
    let mut out = Vec::new();
    for part in split_csv(value) {
        let protocol = match part.as_str() {
            "h2" | "h2-inline" => Protocol::H2,
            "h3" | "h3-quiche" => Protocol::H3,
            _ => return Err(anyhow!("unknown protocol {part:?}")),
        };
        if !out.contains(&protocol) {
            out.push(protocol);
        }
    }
    if out.is_empty() {
        return Err(anyhow!("at least one protocol is required"));
    }
    Ok(out)
}

fn parse_positive_usizes(value: &str, name: &str) -> Result<Vec<usize>> {
    let mut out = Vec::new();
    for part in split_csv(value) {
        let n = part
            .parse::<usize>()
            .with_context(|| format!("{name} contains non-integer value {part:?}"))?;
        if n == 0 {
            return Err(anyhow!("{name} values must be positive"));
        }
        out.push(n);
    }
    if out.is_empty() {
        return Err(anyhow!("at least one {name} value is required"));
    }
    Ok(out)
}

fn parse_positive_usize(value: &str, name: &str) -> Result<usize> {
    let n = value
        .parse::<usize>()
        .with_context(|| format!("{name} contains non-integer value {value:?}"))?;
    if n == 0 {
        return Err(anyhow!("{name} value must be positive"));
    }
    Ok(n)
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[derive(Debug, Clone)]
struct FailureProfile {
    name: String,
    netem_args: Vec<String>,
}

impl FailureProfile {
    fn none() -> Self {
        Self {
            name: "none".to_string(),
            netem_args: Vec::new(),
        }
    }
}

fn parse_failures(value: &str) -> Result<Vec<FailureProfile>> {
    let mut out = Vec::new();
    for part in split_csv(value) {
        let profile = match part.as_str() {
            "none" => FailureProfile::none(),
            "loss0.001" => FailureProfile {
                name: part,
                netem_args: vec!["loss".to_string(), "0.001%".to_string()],
            },
            "loss0.01" => FailureProfile {
                name: part,
                netem_args: vec!["loss".to_string(), "0.01%".to_string()],
            },
            "loss0.1" => FailureProfile {
                name: part,
                netem_args: vec!["loss".to_string(), "0.1%".to_string()],
            },
            "loss1" => FailureProfile {
                name: part,
                netem_args: vec!["loss".to_string(), "1%".to_string()],
            },
            "delay100ms" => FailureProfile {
                name: part,
                netem_args: vec!["delay".to_string(), "100ms".to_string()],
            },
            "delay1s" => FailureProfile {
                name: part,
                netem_args: vec!["delay".to_string(), "1s".to_string()],
            },
            "delay100ms_loss1" => FailureProfile {
                name: part,
                netem_args: vec![
                    "delay".to_string(),
                    "100ms".to_string(),
                    "loss".to_string(),
                    "1%".to_string(),
                ],
            },
            _ => return Err(anyhow!("unknown failure profile {part:?}")),
        };
        out.push(profile);
    }
    if out.is_empty() {
        return Err(anyhow!("at least one failure profile is required"));
    }
    Ok(out)
}

#[derive(Clone)]
struct PayloadCorpus {
    payloads: Arc<Vec<Bytes>>,
    avg_bytes: usize,
}

fn load_payloads(path: &str) -> Result<PayloadCorpus> {
    let data = fs::read(path).with_context(|| format!("read payload corpus {path}"))?;
    let raw: Vec<Box<RawValue>> = serde_json::from_slice(&data).context("parse payload corpus")?;
    if raw.is_empty() {
        return Err(anyhow!("payload corpus is empty"));
    }
    let mut payloads = Vec::with_capacity(raw.len());
    let mut total = 0usize;
    for value in raw {
        let bytes = Bytes::copy_from_slice(value.get().as_bytes());
        total += bytes.len();
        payloads.push(bytes);
    }
    Ok(PayloadCorpus {
        avg_bytes: total / payloads.len(),
        payloads: Arc::new(payloads),
    })
}

async fn run_cli(cfg: Config) -> Result<()> {
    let corpus = load_payloads(&cfg.payloads)?;
    let mut results = Vec::new();
    let netem = Netem { enabled: cfg.netem };

    for failure in &cfg.failures {
        netem.clear();
        netem.apply(failure)?;
        for users in &cfg.users {
            for messages in &cfg.messages {
                for repeat_index in 1..=cfg.repeats {
                    for protocol in &cfg.protocols {
                        let scenario = Scenario {
                            protocol: *protocol,
                            users: *users,
                            messages_per_user: *messages,
                            failure: failure.clone(),
                        };
                        let mut result = match protocol {
                            Protocol::H2 => run_h2_scenario(&scenario, corpus.clone()).await?,
                            Protocol::H3 => run_h3_scenario(&scenario, corpus.clone())?,
                        };
                        result.repeat_index = repeat_index;
                        results.push(result);
                    }
                }
            }
        }
    }
    netem.clear();
    write_csv(cfg.output.as_deref(), &results)?;
    Ok(())
}

#[derive(Clone)]
struct Scenario {
    protocol: Protocol,
    users: usize,
    messages_per_user: usize,
    failure: FailureProfile,
}

struct ResultRow {
    scenario: Scenario,
    repeat_index: usize,
    payload_count: usize,
    payload_bytes_avg: usize,
    duration: Duration,
    throughput: f64,
    errors: usize,
    latency: LatencySummary,
    resources: ResourceDelta,
}

#[derive(Default)]
struct LatencySummary {
    mean: Duration,
    p50: Duration,
    p95: Duration,
    p99: Duration,
    min: Duration,
    max: Duration,
}

fn summarize_latencies(values: &[Duration]) -> LatencySummary {
    if values.is_empty() {
        return LatencySummary::default();
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let total = sorted
        .iter()
        .fold(Duration::ZERO, |acc, value| acc + *value);
    LatencySummary {
        mean: total / sorted.len() as u32,
        p50: percentile(&sorted, 0.50),
        p95: percentile(&sorted, 0.95),
        p99: percentile(&sorted, 0.99),
        min: sorted[0],
        max: sorted[sorted.len() - 1],
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = p * (sorted.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    if lower == upper {
        return sorted[lower];
    }
    let weight = rank - lower as f64;
    let lo = sorted[lower].as_nanos() as f64;
    let hi = sorted[upper].as_nanos() as f64;
    Duration::from_nanos((lo + (hi - lo) * weight) as u64)
}

#[derive(Clone, Copy)]
struct ResourceSnapshot {
    utime_us: i64,
    stime_us: i64,
    rss_bytes: i64,
    maxrss_kb: i64,
}

struct ResourceDelta {
    cpu_user: Duration,
    cpu_system: Duration,
    rss_bytes_delta: i64,
    maxrss_kb_after: i64,
}

fn capture_resources() -> ResourceSnapshot {
    capture_rusage(libc::RUSAGE_SELF)
}

fn capture_child_resources() -> ResourceSnapshot {
    capture_rusage(libc::RUSAGE_CHILDREN)
}

fn capture_rusage(who: libc::c_int) -> ResourceSnapshot {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    unsafe {
        libc::getrusage(who, usage.as_mut_ptr());
        let usage = usage.assume_init();
        ResourceSnapshot {
            utime_us: usage.ru_utime.tv_sec * 1_000_000 + usage.ru_utime.tv_usec,
            stime_us: usage.ru_stime.tv_sec * 1_000_000 + usage.ru_stime.tv_usec,
            rss_bytes: if who == libc::RUSAGE_SELF {
                current_rss_bytes().unwrap_or(0)
            } else {
                0
            },
            maxrss_kb: usage.ru_maxrss,
        }
    }
}

fn diff_resources(before: ResourceSnapshot, after: ResourceSnapshot) -> ResourceDelta {
    ResourceDelta {
        cpu_user: Duration::from_micros((after.utime_us - before.utime_us).max(0) as u64),
        cpu_system: Duration::from_micros((after.stime_us - before.stime_us).max(0) as u64),
        rss_bytes_delta: after.rss_bytes - before.rss_bytes,
        maxrss_kb_after: after.maxrss_kb,
    }
}

fn current_rss_bytes() -> Option<i64> {
    let statm = fs::read_to_string("/proc/self/statm").ok()?;
    let rss_pages = statm.split_whitespace().nth(1)?.parse::<i64>().ok()?;
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    Some(rss_pages * page_size)
}
