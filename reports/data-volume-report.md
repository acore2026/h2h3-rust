B. Data Volume In Figure 5, we present the data volume evaluation results of a set of experiments under the 5G scenario, where the payload size of each request and response is 128 bytes. We chose this scenario for analysis because it gives a relatively larger network latency, meanwhile the edge cloud execution environment is more controllable. The figure shows that HTTP1.1 has a larger data volume per request than HTTP3 in all cases. This is primarily because TCP has a larger header size than UDP and QUIC, resulting in HTTP1.1 having a larger packet size than HTTP3 with the same payload amount. Comparing the data volume per request to the actual HTTP packet sizes in the figure, we can observe that the data volume per request with HTTP1.1 is much closer to its actual packet size compared to HTTP3. This implies that the overhead of HTTP1.1 is mainly due to the large header size in each protocol, while the overhead in HTTP3 is mainly introduced by the stream control frames and ACK packets as discussed earlier. It is worth noting that in the high-frequency case with a sampling rate of d = 5ms, the data volume of HTTP1.1 is much higher than in other cases. This is because of the large number of TCP session establishments in this case, which is also evident in Figure 4, and the handshake packets causing a lot of network overhead. In general, considering the cost of deploying the controller in public cloud, HTTP3 is a more economical choice than HTTP1.1.


The passage is arguing that **HTTP/3 produces less network data per request than HTTP/1.1 in the tested 5G edge-cloud scenario**, especially when requests are sent very frequently. The main reason is that, for small 128-byte request/response payloads, **protocol overhead becomes a large fraction of the total traffic**.

### Core analysis

The experiment uses a small payload size: **128 bytes per request and response**. With such small messages, the payload itself is tiny, so headers, acknowledgements, handshakes, and control frames dominate the total data volume.

The authors observe that **HTTP/1.1 has a larger data volume per request than HTTP/3 in all tested cases**. Their explanation is that HTTP/1.1 runs over TCP, while HTTP/3 runs over QUIC over UDP. A basic TCP header is larger than a UDP header, so each HTTP/1.1 packet can carry more transport-layer overhead than a comparable UDP packet. Therefore, when the same 128-byte payload is sent, HTTP/1.1 may consume more bytes on the wire.

However, the explanation is slightly simplified. It is not only because “TCP has a larger header than UDP.” HTTP/3 also has QUIC headers, encryption metadata, stream frames, and ACK frames. So HTTP/3 is not automatically smaller at the packet level. The more important point is that **HTTP/3/QUIC can manage streams and connections more efficiently**, especially when many small requests are being sent.

### Why HTTP/1.1 is worse in the high-frequency case

The most important observation is the high-frequency case where the sampling interval is:

> d = 5 ms

That means requests are generated very frequently. In that case, the passage says HTTP/1.1 produces much more data than in other cases because of many **TCP session establishments**.

This matters because opening a new TCP connection is expensive. A TCP connection requires handshake packets before application data can be exchanged. If TLS is also involved, there may be additional handshake traffic. So if the system repeatedly creates new TCP sessions instead of reusing an existing connection, the total network volume increases significantly.

In other words:

**At low request frequency**, the overhead per request is moderate.

**At very high request frequency**, HTTP/1.1 suffers because repeated TCP connection setup adds many extra packets.

**HTTP/3 avoids some of this cost because QUIC is designed to reduce connection setup overhead and handle multiple streams efficiently over one connection.**

### Meaning of “data volume per request vs actual HTTP packet size”

The authors also compare the measured data volume per request with the actual HTTP packet sizes.

They say HTTP/1.1’s data volume per request is closer to its actual packet size. This suggests that most HTTP/1.1 overhead comes directly from the packet/header size itself.

For HTTP/3, the measured data volume per request is less directly explained by the basic HTTP packet size. That implies that extra QUIC-related traffic contributes to the total volume, such as:

* stream control frames,
* ACK packets,
* connection-management frames,
* QUIC transport overhead.

So the passage distinguishes two kinds of overhead:

**HTTP/1.1 overhead:** mainly larger TCP/IP/protocol headers and connection setup.

**HTTP/3 overhead:** less from large packet headers, more from QUIC stream control and acknowledgement mechanisms.

### Why HTTP/3 is called more economical

The conclusion is that HTTP/3 is more economical because it transfers less data per request. In public cloud deployments, data transfer can affect cost, especially outbound traffic or high-volume edge-cloud communication. If HTTP/3 reduces the amount of data exchanged, then it may reduce network usage and therefore operating cost.

This is especially important for systems that send **small but frequent messages**, such as telemetry, sensor sampling, robotic control, AR/VR edge services, or real-time mobile applications.

### Critical comments

The overall reasoning is plausible, but the paragraph has a few weaknesses.

First, the statement that HTTP/1.1 is larger mainly because “TCP has a larger header size than UDP and QUIC” is too broad. TCP’s base header is larger than UDP’s, but HTTP/3 includes QUIC overhead too. A more accurate explanation would mention the full stack:

**HTTP/1.1 over TCP versus HTTP/3 over QUIC/UDP**, including connection reuse, handshakes, ACK behavior, stream frames, and header compression.

Second, the result may depend heavily on whether HTTP/1.1 uses persistent connections. If HTTP/1.1 repeatedly opens new TCP sessions, it will look much worse. But if HTTP/1.1 keep-alive is enabled and connections are reused properly, the data-volume gap may shrink.

Third, the experiment uses only a 128-byte payload. That makes protocol overhead highly visible. With larger payloads, the relative difference between HTTP/1.1 and HTTP/3 may become smaller because the payload dominates the total data volume.

### Summary

HTTP/3 performs better in this data-volume experiment because the workload consists of many small messages, where protocol overhead matters a lot. HTTP/1.1 suffers from larger TCP-related overhead and, in the high-frequency case, repeated TCP connection establishments. HTTP/3 still has QUIC control overhead, but it appears to manage frequent small transfers more efficiently, making it the more economical option for this 5G edge-cloud scenario.

## Local packet-capture replication attempt

Date: 2026-05-18

This repository does not currently implement HTTP/1.1. The closest local comparison available without changing the benchmark is persistent HTTP/2 over TLS/TCP versus HTTP/3 over QUIC. That means this is not a strict reproduction of the paper's HTTP/1.1-versus-HTTP/3 result. It is a packet-level check of the same small-payload/high-frequency claim using the available transports.

Workload:

```sh
target/release/transportbench-rust \
  -payloads /tmp/sbi_payload_128.json \
  -protocols h2,h3 \
  -users 32 \
  -messages 200 \
  -repeat 1
```

The payload corpus contained one JSON string with exactly 128 bytes on the wire as the benchmark request body. Each protocol sent 6,400 measured requests, plus the benchmark's built-in warm-up requests. The run is more aggressive than the paper's 5 ms sampling interval because the benchmark has no pacing flag; requests are issued as fast as each serial user stream can complete.

Packet capture method:

```sh
tcpdump -i lo -s 0 -U -w /tmp/data_volume_h2_32x200.pcap 'tcp and host 127.0.0.1'
tcpdump -i lo -s 0 -U -w /tmp/data_volume_h3_32x200.pcap 'udp and host 127.0.0.1'
```

`frame.len` below is the whole captured loopback frame length reported by `tshark`. `ip.len` is also shown because loopback captures include link-layer framing that is not the same as a physical 5G interface.

| Protocol | Requests | Duration ms | Throughput msg/s | Packets | Whole-frame bytes | IP bytes | Whole-frame bytes/request | IP bytes/request | Whole-frame Mbps over benchmark window | Frame size min / p50 / mean / p95 / p99 / max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| h2 TLS/TCP | 6,400 | 73.583 | 86,976.74 | 9,361 | 1,921,606 | 1,790,552 | 300.25 | 279.77 | 208.918 | 66 / 247 / 205.28 / 247 / 883 / 16,450 |
| h3 QUIC/UDP | 6,400 | 70.738 | 90,474.79 | 13,217 | 2,164,600 | 1,979,562 | 338.22 | 309.31 | 244.802 | 81 / 100 / 163.77 / 239 / 239 / 1,242 |

Observation:

In this local run, HTTP/3 produced smaller median whole packets, but more packets. Total captured bytes per measured request were therefore higher for HTTP/3 than for persistent HTTP/2/TLS. This points to QUIC ACK/control-frame overhead being very visible under tiny, high-frequency payloads. It does not reproduce the paper's HTTP/1.1 result because the main paper effect depends on HTTP/1.1 behavior, especially repeated TCP session establishment in the high-frequency case.
