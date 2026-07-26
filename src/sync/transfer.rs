//! HTTP transport policy and byte-moving helpers for Dropbox sync.

use std::io::{self, Read, Write};
use std::time::Duration;

use super::{SyncError, SyncResult};

/// Buffer size for streaming transfers.
const COPY_BUFFER_SIZE: usize = 64 * 1024;

/// Time allowed for DNS resolution plus the TCP and TLS handshakes.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Time allowed for a single step of a request.
///
/// reqwest's blocking client applies its timeout to the whole request, body
/// included, and defaults to 30 seconds. That default caps *total* transfer
/// time, so it fails any download large enough to matter regardless of how
/// healthy the connection is. Downloads here stream the body through `Read`,
/// which reqwest bounds per read call, so this value acts as a stall detector:
/// it fires when the peer stops sending, not when the file is merely big.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Time allowed for an upload request.
///
/// Uploads send the entire body inside one request, so unlike the streaming
/// download path this has to cover the whole transfer rather than a stall.
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Build the HTTP client used for every Dropbox call.
pub fn build_client() -> SyncResult<reqwest::blocking::Client> {
    build_client_with_stall_timeout(STALL_TIMEOUT)
}

/// Build a client with an explicit stall timeout, so tests can exercise the
/// policy without waiting out the production value.
fn build_client_with_stall_timeout(stall: Duration) -> SyncResult<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("grans/", env!("GRANS_VERSION")))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(stall)
        .build()
        .map_err(|source| SyncError::Transport {
            operation: "HTTP client setup".to_string(),
            hint: transport_hint(&source),
            source,
        })
}

/// Describe a transport failure in terms the reader can act on.
///
/// reqwest renders a stalled body read as "error decoding response body", which
/// reads like a parsing bug rather than a network problem.
pub fn transport_hint(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        format!(
            "no data received for {}s (connection stalled or network is down)",
            STALL_TIMEOUT.as_secs()
        )
    } else if err.is_connect() {
        "could not reach Dropbox (DNS, proxy, or TLS failure)".to_string()
    } else if err.is_body() || err.is_decode() {
        "the connection dropped mid-transfer".to_string()
    } else {
        "network error".to_string()
    }
}

/// Attach operation context to a transport failure, preserving its source chain.
pub fn transport_error(operation: impl Into<String>, source: reqwest::Error) -> SyncError {
    SyncError::Transport {
        operation: operation.into(),
        hint: transport_hint(&source),
        source,
    }
}

/// Stream `reader` into `writer`, invoking `on_progress` with the running total.
///
/// Returns the number of bytes copied.
pub fn stream_with_progress<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    on_progress: &mut dyn FnMut(u64),
) -> io::Result<u64> {
    let mut buffer = vec![0u8; COPY_BUFFER_SIZE];
    let mut total = 0u64;

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            return Ok(total);
        }

        writer.write_all(&buffer[..bytes_read])?;
        total += bytes_read as u64;
        on_progress(total);
    }
}

/// Classify a streaming failure, keeping network causes distinguishable from
/// local disk failures.
///
/// reqwest surfaces body-read failures as `io::Error`s wrapping its own error
/// type, so unwrapping one restores the network diagnosis.
pub fn stream_error(operation: impl Into<String>, err: io::Error) -> SyncError {
    match err.downcast::<reqwest::Error>() {
        Ok(network) => transport_error(operation, network),
        Err(local) => SyncError::Io(local),
    }
}

/// Fail when a transfer delivered a different number of bytes than advertised.
///
/// A download cut short still produces a readable file, so without this check a
/// truncated database would quietly replace a good one.
pub fn verify_transfer_size(actual: u64, expected: u64, what: &str) -> SyncResult<()> {
    if actual == expected {
        return Ok(());
    }

    Err(SyncError::IncompleteTransfer {
        what: what.to_string(),
        actual,
        expected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// Serve one HTTP response whose body arrives in timed pieces, standing in
    /// for a healthy but slow connection.
    ///
    /// Returns the URL to request; the server thread exits after one response.
    fn serve_slow_body(chunk: &'static [u8], chunks: usize, gap: Duration) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");

        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");

            // Consume the request head so the client's write completes.
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => request.push(byte[0]),
                }
            }

            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
                chunk.len() * chunks
            );
            if stream.write_all(head.as_bytes()).is_err() {
                return;
            }

            for _ in 0..chunks {
                if stream.write_all(chunk).is_err() || stream.flush().is_err() {
                    return;
                }
                thread::sleep(gap);
            }
        });

        format!("http://{}/", addr)
    }

    /// Yields data in fixed-size pieces so tests exercise the short-read path
    /// that a real network body takes.
    struct ChunkedReader {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
    }

    impl ChunkedReader {
        fn new(data: Vec<u8>, chunk: usize) -> Self {
            Self {
                data,
                pos: 0,
                chunk,
            }
        }
    }

    impl Read for ChunkedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = (self.data.len() - self.pos).min(self.chunk).min(buf.len());
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    /// Fails partway through, standing in for a connection that drops mid-transfer.
    struct FailingReader {
        before_error: usize,
        sent: usize,
    }

    impl Read for FailingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.sent >= self.before_error {
                return Err(io::Error::new(io::ErrorKind::ConnectionReset, "reset"));
            }
            let n = (self.before_error - self.sent).min(buf.len());
            buf[..n].fill(b'x');
            self.sent += n;
            Ok(n)
        }
    }

    #[test]
    fn stream_copies_all_bytes() {
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 256) as u8).collect();
        let mut reader = ChunkedReader::new(data.clone(), 512);
        let mut sink = Vec::new();

        let copied = stream_with_progress(&mut reader, &mut sink, &mut |_| {}).unwrap();

        assert_eq!(copied, data.len() as u64);
        assert_eq!(sink, data);
    }

    #[test]
    fn stream_reports_monotonic_progress_ending_at_total() {
        let data = vec![7u8; 5_000];
        let mut reader = ChunkedReader::new(data, 300);
        let mut sink = Vec::new();
        let mut seen = Vec::new();

        stream_with_progress(&mut reader, &mut sink, &mut |n| seen.push(n)).unwrap();

        assert!(!seen.is_empty(), "progress callback was never invoked");
        assert!(
            seen.windows(2).all(|w| w[0] < w[1]),
            "progress must increase: {:?}",
            seen
        );
        assert_eq!(*seen.last().unwrap(), 5_000);
    }

    #[test]
    fn stream_handles_empty_body() {
        let mut reader = ChunkedReader::new(Vec::new(), 512);
        let mut sink = Vec::new();
        let mut calls = 0;

        let copied = stream_with_progress(&mut reader, &mut sink, &mut |_| calls += 1).unwrap();

        assert_eq!(copied, 0);
        assert!(sink.is_empty());
        assert_eq!(calls, 0);
    }

    #[test]
    fn stream_propagates_read_errors() {
        let mut reader = FailingReader {
            before_error: 1_000,
            sent: 0,
        };
        let mut sink = Vec::new();

        let err = stream_with_progress(&mut reader, &mut sink, &mut |_| {}).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
    }

    #[test]
    fn stream_error_keeps_local_io_failures_as_io() {
        let disk_full = io::Error::new(io::ErrorKind::StorageFull, "no space left on device");

        let err = stream_error("download of /grans.db", disk_full);

        assert!(
            matches!(err, SyncError::Io(_)),
            "a disk failure must not be reported as a network problem: {}",
            err
        );
    }

    /// A transfer longer than the client timeout must still succeed, because the
    /// timeout bounds each read rather than the transfer as a whole.
    ///
    /// Buffering the body with `Response::bytes()` instead makes the timeout a
    /// cap on total transfer time, which is what broke `dropbox pull` on any
    /// database too large to arrive within it.
    #[test]
    fn streaming_outlives_a_transfer_longer_than_the_stall_timeout() {
        const CHUNK: &[u8] = &[b'z'; 4096];
        let stall = Duration::from_millis(500);
        let url = serve_slow_body(CHUNK, 10, Duration::from_millis(100));

        let client = build_client_with_stall_timeout(stall).unwrap();
        let mut response = client.get(&url).send().expect("request should succeed");
        let mut sink = Vec::new();

        let copied = stream_with_progress(&mut response, &mut sink, &mut |_| {})
            .expect("a steady body must not time out because the transfer is long");

        assert_eq!(copied, (CHUNK.len() * 10) as u64);
        assert!(sink.iter().all(|&b| b == b'z'));
    }

    /// The same body buffered in one shot trips the timeout, pinning the bug to
    /// the buffering strategy rather than to the network.
    #[test]
    fn buffering_the_whole_body_trips_the_stall_timeout() {
        const CHUNK: &[u8] = &[b'z'; 4096];
        let stall = Duration::from_millis(500);
        let url = serve_slow_body(CHUNK, 10, Duration::from_millis(100));

        let client = build_client_with_stall_timeout(stall).unwrap();
        let response = client.get(&url).send().expect("request should succeed");

        let err = response.bytes().expect_err("buffering should time out");

        assert!(err.is_timeout(), "expected a timeout, got: {}", err);
    }

    #[test]
    fn verify_transfer_size_accepts_exact_match() {
        assert!(verify_transfer_size(1024, 1024, "database").is_ok());
    }

    #[test]
    fn verify_transfer_size_rejects_truncated_transfer() {
        let err = verify_transfer_size(512, 1024, "database").unwrap_err();
        let msg = err.to_string();

        assert!(msg.contains("512"), "message should report actual: {}", msg);
        assert!(msg.contains("1024"), "message should report expected: {}", msg);
    }
}
