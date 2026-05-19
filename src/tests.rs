
use super::*;

#[test]
fn parses_config() {
        let cfg = Config::from_args([
            "-protocols".to_string(),
            "h2,h2-inline,h3".to_string(),
            "-users".to_string(),
            "1,4".to_string(),
            "-messages".to_string(),
            "5".to_string(),
            "-repeat".to_string(),
            "2".to_string(),
        ])
        .unwrap();
        assert_eq!(cfg.protocols, vec![Protocol::H2, Protocol::H3]);
        assert_eq!(cfg.users, vec![1, 4]);
        assert_eq!(cfg.messages, vec![5]);
        assert_eq!(cfg.repeats, 2);
    }

    #[test]
    fn percentile_interpolates() {
        let values = vec![
            Duration::from_millis(1),
            Duration::from_millis(5),
            Duration::from_millis(10),
            Duration::from_millis(20),
        ];
        assert_eq!(percentile(&values, 0.5), Duration::from_micros(7500));
    }

    #[test]
    fn netem_profile_requires_flag() {
        let err =
            Config::from_args(["-failures".to_string(), "delay100ms".to_string()]).unwrap_err();
        assert!(err.to_string().contains("requires -netem"));
    }

    #[test]
    fn csv_records_benchmark_controls() {
        let path = std::env::temp_dir().join(format!(
            "transportbench-rust-test-{}.csv",
            std::process::id()
        ));
        let row = ResultRow {
            scenario: Scenario {
                protocol: Protocol::H2,
                users: 2,
                messages_per_user: 3,
                failure: FailureProfile::none(),
            },
            repeat_index: 2,
            payload_count: 4,
            payload_bytes_avg: 5,
            duration: Duration::from_millis(10),
            throughput: 600.0,
            errors: 0,
            latency: LatencySummary {
                mean: Duration::from_millis(1),
                p50: Duration::from_millis(1),
                p95: Duration::from_millis(2),
                p99: Duration::from_millis(3),
                min: Duration::from_micros(500),
                max: Duration::from_millis(4),
            },
            resources: ResourceDelta {
                cpu_user: Duration::from_millis(5),
                cpu_system: Duration::from_millis(6),
                rss_bytes_delta: 7,
                maxrss_kb_after: 8,
            },
        };
        write_csv(path.to_str(), &[row]).unwrap();
        let csv = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);

        let mut lines = csv.lines();
        let header = lines.next().unwrap();
        assert!(header.contains("runtime_worker_threads"));
        assert!(header.contains("workload_model"));
        assert!(header.contains("client_scheduler"));
        assert!(header.contains("server_scheduler"));
        assert!(header.contains("run_order"));
        let data = lines.next().unwrap();
        assert!(data.contains(
            ",2,serial_per_user,tokio_user_tasks,in_connection_futures,interleaved_by_repeat,"
        ));
        assert!(data.starts_with("h2,2,3,6,2,"));
    }

    #[test]
    fn parses_overload_config() {
        let cfg = OverloadConfig::from_args([
            "-requests".to_string(),
            "20".to_string(),
            "-concurrency".to_string(),
            "4,8".to_string(),
            "-payload-sizes".to_string(),
            "1024,2048".to_string(),
            "-thresholds".to_string(),
            "2".to_string(),
            "-reject-percent".to_string(),
            "5".to_string(),
            "-modes".to_string(),
            "reset,app503".to_string(),
            "-repeat".to_string(),
            "2".to_string(),
            "-output-dir".to_string(),
            "/tmp/quic-overload".to_string(),
        ])
        .unwrap();
        assert_eq!(cfg.total_requests, 20);
        assert_eq!(cfg.concurrency, vec![4, 8]);
        assert_eq!(cfg.payload_sizes, vec![1024, 2048]);
        assert_eq!(cfg.thresholds, vec![2]);
        assert_eq!(cfg.reject_percent, 5.0);
        assert_eq!(cfg.modes, vec![OverloadMode::Reset, OverloadMode::App503]);
        assert_eq!(cfg.repeats, 2);
        assert_eq!(cfg.output_dir, PathBuf::from("/tmp/quic-overload"));
    }

    #[test]
    fn overload_csv_records_new_metrics() {
        let path = std::env::temp_dir().join(format!(
            "transportbench-rust-overload-test-{}.csv",
            std::process::id()
        ));
        let row = OverloadResultRow {
            scenario: OverloadScenario {
                mode: OverloadMode::Reset,
                total_requests: 10,
                concurrency: 4,
                payload_size: 1024,
                threshold: 2,
                reject_percent: 5.0,
            },
            repeat_index: 1,
            duration: Duration::from_millis(10),
            throughput: 1000.0,
            successes: 2,
            rejected: 0,
            reset_streams: 8,
            errors: 0,
            latency: LatencySummary {
                mean: Duration::from_millis(1),
                p50: Duration::from_millis(1),
                p95: Duration::from_millis(2),
                p99: Duration::from_millis(3),
                min: Duration::from_micros(500),
                max: Duration::from_millis(4),
            },
            rejection_latency: LatencySummary {
                mean: Duration::from_millis(1),
                p50: Duration::from_millis(1),
                p95: Duration::from_millis(2),
                p99: Duration::from_millis(3),
                min: Duration::from_micros(500),
                max: Duration::from_millis(4),
            },
            server: OverloadServerStats {
                bytes_read: 2048,
                handling_time: Duration::from_millis(5),
                accepted_requests: 2,
                successful_requests: 2,
                app_rejections: 0,
                reset_rejections: 8,
                peak_inflight: 2,
            },
            resources: ResourceDelta {
                cpu_user: Duration::from_millis(1),
                cpu_system: Duration::from_millis(2),
                rss_bytes_delta: 3,
                maxrss_kb_after: 4,
            },
        };
        write_overload_csv(&path, &[row]).unwrap();
        let csv = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);

        let header = csv.lines().next().unwrap();
        assert!(header.contains("server_bytes_read"));
        assert!(header.contains("server_handling_time_ms"));
        assert!(header.contains("reset_streams"));
        assert!(header.contains("reject_percent"));
        assert!(header.contains("cpu_percent"));
        assert!(csv.contains("reset,10,4,1024,2,5.000,1,"));
    }

    #[test]
    fn overload_claim_requires_lower_reset_costs() {
        let app = test_overload_row(OverloadMode::App503, 10_000, 20, 3.0);
        let reset = test_overload_row(OverloadMode::Reset, 1_000, 5, 1.0);
        assert!(overload_claim_supported(&[&app, &reset]));

        let weak_reset = test_overload_row(OverloadMode::Reset, 20_000, 5, 1.0);
        assert!(!overload_claim_supported(&[&app, &weak_reset]));
    }

    fn test_overload_row(
        mode: OverloadMode,
        bytes_read: usize,
        handling_ms: u64,
        rejection_ms: f64,
    ) -> OverloadResultRow {
        OverloadResultRow {
            scenario: OverloadScenario {
                mode,
                total_requests: 10,
                concurrency: 4,
                payload_size: 1024,
                threshold: 2,
                reject_percent: 0.0,
            },
            repeat_index: 1,
            duration: Duration::from_millis(10),
            throughput: 1000.0,
            successes: 0,
            rejected: 0,
            reset_streams: 0,
            errors: 0,
            latency: LatencySummary::default(),
            rejection_latency: LatencySummary {
                mean: Duration::from_nanos((rejection_ms * 1_000_000.0) as u64),
                ..LatencySummary::default()
            },
            server: OverloadServerStats {
                bytes_read,
                handling_time: Duration::from_millis(handling_ms),
                accepted_requests: 1,
                ..OverloadServerStats::default()
            },
            resources: ResourceDelta {
                cpu_user: Duration::ZERO,
                cpu_system: Duration::ZERO,
                rss_bytes_delta: 0,
                maxrss_kb_after: 0,
            },
        }
    }
