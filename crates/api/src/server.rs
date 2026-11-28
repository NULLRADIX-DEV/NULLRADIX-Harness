use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio_util::sync::CancellationToken;

use crate::Limits;

/// How long the accept loop rests after an error such as running out of files.
const ACCEPT_PAUSE: Duration = Duration::from_secs(1);

/// Serves HTTP/1 with a head timeout and a connection cap until shutdown.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    limits: &Limits,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    let stop = CancellationToken::new();
    let trigger = stop.clone();
    tokio::spawn(async move {
        shutdown.await;
        trigger.cancel();
    });
    // take a slot before accept; extra connections wait in the backlog
    let slots = Arc::new(Semaphore::new(limits.max_connections));
    // closed once every connection task has dropped its receiver
    let (running, alive) = watch::channel(());
    loop {
        let slot = tokio::select! {
            () = stop.cancelled() => break,
            slot = slots.clone().acquire_owned() => match slot {
                Ok(slot) => slot,
                Err(_) => break,
            },
        };
        let stream = tokio::select! {
            () = stop.cancelled() => break,
            stream = accept(&listener) => stream,
        };
        tokio::spawn(serve_connection(
            stream,
            router.clone(),
            limits.header_read_timeout,
            stop.clone(),
            (slot, alive.clone()),
        ));
    }
    drop(listener);
    drop(alive);
    running.closed().await;
    Ok(())
}

async fn accept(listener: &TcpListener) -> TcpStream {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => return stream,
            Err(error) if is_connection_error(&error) => {}
            Err(error) => {
                tracing::error!(%error, "accept failed");
                tokio::time::sleep(ACCEPT_PAUSE).await;
            }
        }
    }
}

/// Errors of one dead connection, not of the listener.
fn is_connection_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    )
}

async fn serve_connection(
    stream: TcpStream,
    router: Router,
    header_read_timeout: Duration,
    stop: CancellationToken,
    _held: (OwnedSemaphorePermit, watch::Receiver<()>),
) {
    let mut http = http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(header_read_timeout);
    let connection = http.serve_connection(TokioIo::new(stream), TowerToHyperService::new(router));
    tokio::pin!(connection);
    let mut stopping = false;
    loop {
        tokio::select! {
            result = connection.as_mut() => {
                if let Err(error) = result {
                    tracing::debug!(%error, "connection ended with an error");
                }
                return;
            }
            () = stop.cancelled(), if !stopping => {
                stopping = true;
                connection.as_mut().graceful_shutdown();
            }
        }
    }
}

/// A real server on a loopback port and a client that speaks to it by hand.
#[cfg(test)]
pub(crate) mod wire {
    use std::io;
    use std::net::SocketAddr;

    use axum::Router;
    use tokio::io::AsyncReadExt;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;
    use tokio_util::sync::CancellationToken;

    use crate::Limits;

    pub(crate) struct Running {
        pub(crate) address: SocketAddr,
        pub(crate) stop: CancellationToken,
        pub(crate) task: JoinHandle<io::Result<()>>,
    }

    pub(crate) async fn run(router: Router, limits: Limits) -> Running {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let stopped = stop.clone().cancelled_owned();
        let task =
            tokio::spawn(async move { super::serve(listener, router, &limits, stopped).await });
        Running {
            address,
            stop,
            task,
        }
    }

    pub(crate) async fn start(router: Router, limits: Limits) -> SocketAddr {
        run(router, limits).await.address
    }

    /// Reads one response off the wire and returns its status line; None when the server closed.
    pub(crate) async fn read_response(stream: &mut TcpStream) -> Option<String> {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0_u8; 1];
            if stream.read(&mut byte).await.ok()? == 0 {
                return None;
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8(head).unwrap();
        let length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0_u8; length];
        stream.read_exact(&mut body).await.ok()?;
        Some(head.lines().next().unwrap().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::wire::{read_response, run, start};
    use crate::Limits;
    use crate::tests::{test_api, test_api_with};

    const HEALTH: &[u8] = b"GET /api/v1/health HTTP/1.1\r\nHost: 127.0.0.1:4711\r\n\r\n";
    const OK: &str = "HTTP/1.1 200 OK";

    fn limits(header: Duration, connections: usize) -> Limits {
        Limits::new(header, Duration::from_secs(30), connections, 8)
    }

    /// Reads until the server closes the connection; panics when it stays open.
    async fn closed_within(stream: &mut TcpStream, limit: Duration) -> Duration {
        let started = Instant::now();
        let mut sink = Vec::new();
        tokio::time::timeout(limit, stream.read_to_end(&mut sink))
            .await
            .expect("the server dropped the client")
            .ok();
        started.elapsed()
    }

    #[tokio::test]
    async fn serves_a_request() {
        let api = test_api().await;
        let address = start(api.router, Limits::default()).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(HEALTH).await.unwrap();
        assert_eq!(read_response(&mut stream).await.as_deref(), Some(OK));
    }

    #[tokio::test]
    async fn a_client_that_sends_its_head_too_slowly_is_dropped() {
        let api = test_api().await;
        let address = start(api.router, limits(Duration::from_millis(200), 8)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: 127.0.0.1:4711\r\n")
            .await
            .unwrap();
        let waited = closed_within(&mut stream, Duration::from_secs(30)).await;
        assert!(waited >= Duration::from_millis(150), "{waited:?}");
    }

    #[tokio::test]
    async fn a_client_that_sends_nothing_is_dropped() {
        let api = test_api().await;
        let address = start(api.router, limits(Duration::from_millis(200), 8)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        closed_within(&mut stream, Duration::from_secs(30)).await;
    }

    #[tokio::test]
    async fn a_client_that_starts_an_h2_preface_and_stops_is_dropped() {
        let api = test_api().await;
        let address = start(api.router, limits(Duration::from_millis(200), 8)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(b"PRI * HTTP/2.0\r\n").await.unwrap();
        closed_within(&mut stream, Duration::from_secs(30)).await;
    }

    #[tokio::test]
    async fn an_idle_kept_connection_is_closed_after_the_head_timeout() {
        let api = test_api().await;
        let address = start(api.router, limits(Duration::from_millis(200), 8)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(HEALTH).await.unwrap();
        assert_eq!(read_response(&mut stream).await.as_deref(), Some(OK));
        closed_within(&mut stream, Duration::from_secs(30)).await;
    }

    #[tokio::test]
    async fn connections_above_the_cap_wait_until_a_place_is_free() {
        let api = test_api().await;
        let address = start(api.router, limits(Duration::from_secs(60), 2)).await;
        let mut held = Vec::new();
        for _ in 0..2 {
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream.write_all(HEALTH).await.unwrap();
            assert_eq!(read_response(&mut stream).await.as_deref(), Some(OK));
            held.push(stream);
        }
        let mut waiting = TcpStream::connect(address).await.unwrap();
        waiting.write_all(HEALTH).await.unwrap();
        let early =
            tokio::time::timeout(Duration::from_millis(300), read_response(&mut waiting)).await;
        assert!(early.is_err(), "answered while all places were taken");
        held.pop();
        let answer = tokio::time::timeout(Duration::from_secs(30), read_response(&mut waiting))
            .await
            .expect("answered once a place was free");
        assert_eq!(answer.as_deref(), Some(OK));
    }

    #[tokio::test]
    async fn an_event_stream_survives_the_head_and_request_timeouts() {
        let short = Limits::new(Duration::from_millis(200), Duration::from_millis(200), 8, 8);
        let api = test_api_with(None, short.clone()).await;
        let token = api.token.expose().to_owned();
        let address = start(api.router.clone(), short).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        let request = format!(
            "GET /api/v1/events HTTP/1.1\r\nHost: 127.0.0.1:4711\r\nAuthorization: Bearer {token}\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut seen = String::new();
        read_until(&mut stream, &mut seen, "event: ready").await;
        // three times both timeouts
        tokio::time::sleep(Duration::from_millis(600)).await;
        api.projects.create("ARC", "Arches").await.unwrap();
        read_until(&mut stream, &mut seen, "event: change").await;
    }

    async fn read_until(stream: &mut TcpStream, seen: &mut String, needle: &str) {
        while !seen.contains(needle) {
            let mut buffer = [0_u8; 1024];
            let read = tokio::time::timeout(Duration::from_secs(20), stream.read(&mut buffer))
                .await
                .expect("the stream sends in time")
                .unwrap();
            assert!(read > 0, "the stream was closed: {seen}");
            seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
        }
    }

    #[tokio::test]
    async fn a_stopped_server_ends_even_with_an_idle_connection() {
        let api = test_api().await;
        let server = run(api.router, Limits::default()).await;
        let mut stream = TcpStream::connect(server.address).await.unwrap();
        stream.write_all(HEALTH).await.unwrap();
        assert_eq!(read_response(&mut stream).await.as_deref(), Some(OK));
        server.stop.cancel();
        let ended = tokio::time::timeout(Duration::from_secs(30), server.task)
            .await
            .expect("the server ended");
        assert!(ended.unwrap().is_ok());
        assert!(TcpStream::connect(server.address).await.is_err());
    }
}
