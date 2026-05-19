struct Netem {
    enabled: bool,
}

impl Netem {
    fn apply(&self, failure: &FailureProfile) -> Result<()> {
        if !self.enabled || failure.netem_args.is_empty() {
            return Ok(());
        }
        let mut args = vec!["qdisc", "replace", "dev", "lo", "root", "netem"];
        for arg in &failure.netem_args {
            args.push(arg);
        }
        let output = Command::new("tc").args(args).output()?;
        if !output.status.success() {
            return Err(anyhow!(
                "apply netem {:?}: {}",
                failure.name,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn clear(&self) {
        if self.enabled {
            let _ = Command::new("tc")
                .args(["qdisc", "del", "dev", "lo", "root"])
                .output();
        }
    }
}

fn write_csv(path: Option<&str>, rows: &[ResultRow]) -> Result<()> {
    let mut writer: Box<dyn Write> = match path {
        Some(path) => Box::new(fs::File::create(path)?),
        None => Box::new(std::io::stdout()),
    };
    writeln!(
        writer,
        "protocol,users,messages_per_user,total_messages,repeat_index,runtime_worker_threads,workload_model,client_scheduler,server_scheduler,run_order,payload_count,payload_bytes_avg,failure_profile,duration_ms,throughput_msg_s,errors,latency_mean_ms,latency_p50_ms,latency_p95_ms,latency_p99_ms,latency_min_ms,latency_max_ms,cpu_user_ms,cpu_system_ms,rss_bytes_delta,maxrss_kb_after"
    )?;
    for row in rows {
        let total = row.scenario.users * row.scenario.messages_per_user;
        writeln!(
            writer,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{:.3},{:.2},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{},{}",
            row.scenario.protocol.as_str(),
            row.scenario.users,
            row.scenario.messages_per_user,
            total,
            row.repeat_index,
            TOKIO_WORKER_THREADS,
            WORKLOAD_MODEL,
            row.scenario.protocol.client_scheduler(),
            row.scenario.protocol.server_scheduler(),
            RUN_ORDER,
            row.payload_count,
            row.payload_bytes_avg,
            row.scenario.failure.name,
            ms(row.duration),
            row.throughput,
            row.errors,
            ms(row.latency.mean),
            ms(row.latency.p50),
            ms(row.latency.p95),
            ms(row.latency.p99),
            ms(row.latency.min),
            ms(row.latency.max),
            ms(row.resources.cpu_user),
            ms(row.resources.cpu_system),
            row.resources.rss_bytes_delta,
            row.resources.maxrss_kb_after
        )?;
    }
    Ok(())
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
