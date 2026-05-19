async fn run_h2_scenario(scenario: &Scenario, corpus: PayloadCorpus) -> Result<ResultRow> {
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
        sender = h2_post(sender, corpus.payloads[0].clone()).await?;
    }

    let total = scenario.users * scenario.messages_per_user;
    let before = capture_resources();
    let started = Instant::now();
    let mut handles = Vec::with_capacity(scenario.users);
    for user in 0..scenario.users {
        let user_sender = sender.clone();
        let payloads = Arc::clone(&corpus.payloads);
        let per_user = scenario.messages_per_user;
        handles.push(tokio::spawn(async move {
            let mut latencies = Vec::with_capacity(per_user);
            let mut errors = 0usize;
            let mut user_sender = Some(user_sender);
            for i in 0..per_user {
                let payload = payloads[(user * per_user + i) % payloads.len()].clone();
                let started = Instant::now();
                let Some(sender) = user_sender.take() else {
                    errors += per_user - i;
                    break;
                };
                match h2_post(sender, payload).await {
                    Ok(sender) => user_sender = Some(sender),
                    Err(_) => {
                        errors += per_user - i;
                        latencies.push(started.elapsed());
                        break;
                    }
                }
                latencies.push(started.elapsed());
            }
            (latencies, errors)
        }));
    }

    let mut latencies = Vec::with_capacity(total);
    let mut errors = 0usize;
    for handle in handles {
        let (mut worker_latencies, worker_errors) = handle.await?;
        latencies.append(&mut worker_latencies);
        errors += worker_errors;
    }
    let duration = started.elapsed();
    let after = capture_resources();
    drop(sender);
    conn_task.abort();
    let _ = conn_task.await;
    server.task.abort();
    let _ = server.task.await;

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

async fn h2_post(sender: SendRequest<Bytes>, payload: Bytes) -> Result<SendRequest<Bytes>> {
    h2_post_uri(sender, payload, "/sbi").await
}

async fn h2_post_uri(
    sender: SendRequest<Bytes>,
    payload: Bytes,
    uri: &str,
) -> Result<SendRequest<Bytes>> {
    let mut sender = sender.ready().await?;
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("content-length", payload.len().to_string())
        .body(())?;
    let (response, mut body) = sender.send_request(request, false)?;
    body.send_data(payload, true)?;
    let response = response.await?;
    if !response.status().is_success() {
        return Err(anyhow!("unexpected h2 status {}", response.status()));
    }
    Ok(sender)
}

struct H2Server {
    addr: SocketAddr,
    task: JoinHandle<()>,
    client_config: Arc<ClientConfig>,
}

impl H2Server {
    async fn start() -> Result<Self> {
        Self::start_on("127.0.0.1:0".parse()?).await
    }

    async fn start_on(bind_addr: SocketAddr) -> Result<Self> {
        let (server_config, client_config) = h2_tls_configs()?;
        let acceptor = TlsAcceptor::from(Arc::clone(&server_config));
        let listener = TcpListener::bind(bind_addr).await?;
        let addr = listener.local_addr()?;
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let _ = socket.set_nodelay(true);
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(socket) = acceptor.accept(socket).await else {
                        return;
                    };
                    let _ = serve_h2_connection(socket).await;
                });
            }
        });
        Ok(Self {
            addr,
            task,
            client_config,
        })
    }
}

async fn serve_h2_connection<T>(socket: T) -> Result<()>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut connection = h2::server::handshake(socket).await?;
    let mut active = FuturesUnordered::new();

    loop {
        tokio::select! {
            request = connection.accept() => {
                match request {
                    Some(request) => {
                        let (request, respond) = request?;
                        active.push(handle_h2_request(request, respond));
                    }
                    None => break,
                }
            }
            Some(result) = active.next(), if !active.is_empty() => {
                let _ = result;
            }
        }
    }

    while let Some(result) = active.next().await {
        let _ = result;
    }
    Ok(())
}

fn h2_tls_configs() -> Result<(Arc<ServerConfig>, Arc<ClientConfig>)> {
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["localhost".to_string()])?;
    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));

    let mut server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der.clone()], key_der)?;
    server_config.alpn_protocols = vec![b"h2".to_vec()];

    let mut roots = RootCertStore::empty();
    roots.add(cert_der)?;
    let mut client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_config.alpn_protocols = vec![b"h2".to_vec()];

    Ok((Arc::new(server_config), Arc::new(client_config)))
}

fn h2_insecure_client_config() -> Arc<ClientConfig> {
    let provider = tokio_rustls::rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(tokio_rustls::rustls::crypto::aws_lc_rs::default_provider()));
    let mut client_config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCertificateVerification(provider)))
        .with_no_client_auth();
    client_config.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(client_config)
}

#[derive(Debug)]
struct NoCertificateVerification(Arc<tokio_rustls::rustls::crypto::CryptoProvider>);

impl tokio_rustls::rustls::client::danger::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> Result<tokio_rustls::rustls::client::danger::ServerCertVerified, tokio_rustls::rustls::Error>
    {
        Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        tokio_rustls::rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        tokio_rustls::rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        self.0
            .signature_verification_algorithms
            .supported_schemes()
    }
}

async fn handle_h2_request(
    mut request: Request<h2::RecvStream>,
    mut respond: SendResponse<Bytes>,
) -> Result<()> {
    while let Some(chunk) = request.body_mut().data().await {
        let chunk = chunk?;
        let _ = request
            .body_mut()
            .flow_control()
            .release_capacity(chunk.len());
    }
    let response = Response::builder().status(204).body(())?;
    respond.send_response(response, true)?;
    Ok(())
}
