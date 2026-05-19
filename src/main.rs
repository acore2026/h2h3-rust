use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use futures_util::stream::{FuturesUnordered, StreamExt};
use h2::client::SendRequest;
use h2::server::SendResponse;
use http::{Request, Response};
use mio::net::UdpSocket;
use mio::{Events, Interest, Poll, Token};
use quiche::h3::NameValue;
use rcgen::{generate_simple_self_signed, CertifiedKey};
use serde_json::value::RawValue;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{ErrorKind, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_rustls::rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName,
};
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const MAX_DATAGRAM_SIZE: usize = 1350;
const READ_BUF_SIZE: usize = 65535;
const H3_SOCKET: Token = Token(0);
const TOKIO_WORKER_THREADS: usize = 2;
const WORKLOAD_MODEL: &str = "serial_per_user";
const RUN_ORDER: &str = "interleaved_by_repeat";
const RAW_QUIC_ALPN: &[&[u8]] = &[b"quic-overload/1"];
const OVERLOAD_ERROR_CODE: u64 = 0x503;
const RAW_STREAM_CHUNK_SIZE: usize = 4096;
const SIMULATED_PROCESSING_DELAY: Duration = Duration::from_millis(2);

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "overload") {
        args.remove(0);
        let cfg = OverloadConfig::from_args(args)?;
        run_overload_cli(cfg)
    } else if args.first().is_some_and(|arg| arg == "h2load-rate") {
        args.remove(0);
        let cfg = H2LoadRateConfig::from_args(args)?;
        run_h2load_rate_cli(cfg).await
    } else if args.first().is_some_and(|arg| arg == "rust-pps") {
        args.remove(0);
        let cfg = RustPpsConfig::from_args(args)?;
        run_rust_pps_cli(cfg).await
    } else if args.first().is_some_and(|arg| arg == "rust-pps-nginx") {
        args.remove(0);
        let cfg = NginxPpsConfig::from_args(args)?;
        run_nginx_pps_cli(cfg).await
    } else {
        let cfg = Config::from_args(args)?;
        run_cli(cfg).await
    }
}

include!("cli.rs");
include!("h2.rs");
include!("h3.rs");
include!("rust_pps.rs");
include!("nginx_pps.rs");
include!("h2load_rate.rs");
include!("overload.rs");
include!("netem_csv.rs");

#[cfg(test)]
mod tests {
    include!("tests.rs");
}
