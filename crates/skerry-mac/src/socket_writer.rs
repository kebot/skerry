//! The session socket's writer task (`ARCH-038`, M-review change 9).
//!
//! A WebSocket message can't be interleaved with another, and `send` flushes, so one camera
//! keyframe (50–150 KB) sent inline on a congested uplink would hold the transport's whole
//! `select!` loop: incoming video, `kd`/`ku`, pointer motion, gamepad heartbeats and the
//! microphone. So the socket is split:
//! - the transport's loop keeps the read half and only *queues* writes;
//! - a writer task owns the write half and serves, in strict priority, the **control queue**
//!   (every text command and the microphone's `0x02` frames, in order, unbounded) and then
//!   the **camera slot** (one frame; a newer one replaces it).
//!
//! A camera frame is written only while the kernel's unsent backlog is small:
//! `TCP_NOTSENT_LOWAT` makes the socket report writable only below [`UNSENT_LIMIT`] unsent
//! bytes, and the writer polls that before starting a frame. Camera bytes therefore never
//! pile up in the kernel ahead of input; a frame that can't go is dropped, the camera is
//! asked for a keyframe, and frames are skipped until one arrives (a P frame after a gap
//! can't decode). Input still waits for the one camera frame being written, if any: a
//! WebSocket can't split it.

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use std::os::fd::RawFd;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// Unsent bytes the kernel may hold before a camera frame waits: about 30 ms at 1 Mbps.
pub const UNSENT_LIMIT: usize = 4 * 1024;
/// How long a close waits for the writer to drain and send the close frame.
const CLOSE_WAIT: Duration = Duration::from_secs(2);
/// `<netinet/tcp.h>`: report writable only below this many unsent bytes (not in `libc`).
const TCP_NOTSENT_LOWAT: libc::c_int = 0x201;

/// Whether the socket can take a camera frame now.
pub trait Backlog: Send + 'static {
    fn clear(&mut self) -> bool;
}

/// Never holds a frame back (no socket to ask, or a socket that can't say).
pub struct NoBacklog;

impl Backlog for NoBacklog {
    fn clear(&mut self) -> bool {
        true
    }
}

/// The TCP socket's own answer: writable means fewer than [`UNSENT_LIMIT`] unsent bytes.
pub struct UnsentBacklog(RawFd);

impl UnsentBacklog {
    /// Sets `TCP_NOTSENT_LOWAT` on `fd`; `None` if the socket refuses it.
    pub fn new(fd: RawFd) -> Option<Self> {
        let limit = UNSENT_LIMIT as libc::c_int;
        // SAFETY: a live socket fd and an int option value.
        let status = unsafe {
            libc::setsockopt(
                fd,
                libc::IPPROTO_TCP,
                TCP_NOTSENT_LOWAT,
                (&limit as *const libc::c_int).cast(),
                size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        (status == 0).then_some(Self(fd))
    }
}

impl Backlog for UnsentBacklog {
    fn clear(&mut self) -> bool {
        let mut poll = libc::pollfd {
            fd: self.0,
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: one pollfd, no wait. The writer owns half of the socket, so the fd stays
        // open while it asks.
        let ready = unsafe { libc::poll(&mut poll, 1, 0) };
        ready == 1 && poll.revents & libc::POLLOUT != 0
    }
}

/// The socket's fd under any TLS layer, for [`UnsentBacklog`].
pub fn socket_fd<S: std::os::fd::AsRawFd>(
    stream: &tokio_tungstenite::MaybeTlsStream<S>,
) -> Option<RawFd> {
    match stream {
        tokio_tungstenite::MaybeTlsStream::Plain(tcp) => Some(tcp.as_raw_fd()),
        tokio_tungstenite::MaybeTlsStream::Rustls(tls) => Some(tls.get_ref().0.as_raw_fd()),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

enum Out {
    Message(Message),
    Flush,
}

/// The newest camera frame waiting for the writer.
#[derive(Default)]
struct CameraSlot {
    frame: Mutex<Option<Vec<u8>>>,
    /// A frame was replaced before it went out.
    replaced: AtomicBool,
    ready: Notify,
}

impl CameraSlot {
    fn put(&self, frame: Vec<u8>) {
        if let Ok(mut slot) = self.frame.lock() {
            if slot.replace(frame).is_some() {
                self.replaced.store(true, Ordering::Release);
            }
        }
        self.ready.notify_one();
    }

    async fn take(&self) -> Vec<u8> {
        loop {
            if let Some(frame) = self.frame.lock().ok().and_then(|mut slot| slot.take()) {
                return frame;
            }
            self.ready.notified().await;
        }
    }
}

/// The camera frames' path through the writer: the gap rule and the keyframe request.
struct CameraGate {
    keyframe: Arc<AtomicBool>,
    /// A frame was dropped: skip until a keyframe.
    gap: bool,
}

impl CameraGate {
    /// Whether `frame` goes out now, given whether the socket is clear.
    fn admit(&mut self, frame: &[u8], replaced: bool, clear: bool) -> bool {
        if replaced {
            self.gap = true;
        }
        let key = crate::capture::webcam_keyframe(frame);
        let send = clear && (!self.gap || key);
        if send && key {
            self.gap = false;
        }
        if !send {
            self.gap = true;
        }
        if self.gap {
            self.keyframe.store(true, Ordering::Release);
        }
        send
    }
}

async fn write_loop<S>(
    mut sink: SplitSink<S, Message>,
    mut control: mpsc::UnboundedReceiver<Out>,
    camera: Arc<CameraSlot>,
    mut gate: CameraGate,
    mut backlog: Box<dyn Backlog>,
) -> Result<(), WsError>
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    loop {
        tokio::select! {
            biased;
            out = control.recv() => match out {
                Some(Out::Message(message)) => {
                    let close = matches!(message, Message::Close(_));
                    sink.send(message).await?;
                    if close {
                        return sink.close().await;
                    }
                }
                Some(Out::Flush) => sink.flush().await?,
                None => return Ok(()),
            },
            frame = camera.take() => {
                let replaced = camera.replaced.swap(false, Ordering::AcqRel);
                if gate.admit(&frame, replaced, backlog.clear()) {
                    sink.send(Message::Binary(frame.into())).await?;
                }
            }
        }
    }
}

/// The session socket, split: reads here, writes queued for the writer task.
pub struct SplitSocket<S> {
    reader: SplitStream<S>,
    control: mpsc::UnboundedSender<Out>,
    camera: Arc<CameraSlot>,
    writer: Option<tokio::task::JoinHandle<Result<(), WsError>>>,
}

impl<S> SplitSocket<S>
where
    S: Stream<Item = Result<Message, WsError>> + Sink<Message, Error = WsError> + Unpin,
    S: Send + 'static,
{
    /// Splits `socket` and starts its writer. `keyframe` is set when a camera frame is
    /// dropped; `backlog` says when one may be written.
    pub fn new(socket: S, keyframe: Arc<AtomicBool>, backlog: Box<dyn Backlog>) -> Self {
        let (sink, reader) = socket.split();
        let (control, queued) = mpsc::unbounded_channel();
        let camera = Arc::new(CameraSlot::default());
        let gate = CameraGate {
            keyframe,
            gap: false,
        };
        let writer = tokio::spawn(write_loop(sink, queued, camera.clone(), gate, backlog));
        Self {
            reader,
            control,
            camera,
            writer: Some(writer),
        }
    }

    /// Queues `message` behind every earlier one; fails once the writer has stopped.
    pub async fn send(&mut self, message: Message) -> Result<(), WsError> {
        self.control
            .send(Out::Message(message))
            .map_err(|_| WsError::ConnectionClosed)
    }

    /// Flushes what the read side queued (a pong).
    pub async fn flush(&mut self) -> Result<(), WsError> {
        self.control
            .send(Out::Flush)
            .map_err(|_| WsError::ConnectionClosed)
    }

    /// Offers a camera frame; it replaces one still waiting.
    pub fn send_camera(&self, frame: Vec<u8>) {
        self.camera.put(frame);
    }

    /// Sends everything queued, then the close frame, and waits (briefly) for the writer.
    pub async fn close(&mut self, frame: Option<CloseFrame>) -> Result<(), WsError> {
        let _ = self.control.send(Out::Message(Message::Close(frame)));
        let Some(writer) = self.writer.take() else {
            return Ok(());
        };
        match tokio::time::timeout(CLOSE_WAIT, writer).await {
            Ok(Ok(result)) => result,
            _ => Err(WsError::ConnectionClosed),
        }
    }
}

impl<S> Drop for SplitSocket<S> {
    fn drop(&mut self) {
        // A stream that ends without a close: the writer goes with it.
        if let Some(writer) = self.writer.take() {
            writer.abort();
        }
    }
}

impl<S> Stream for SplitSocket<S>
where
    S: Stream<Item = Result<Message, WsError>> + Unpin,
{
    type Item = Result<Message, WsError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.reader.poll_next_unpin(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    fn gate() -> (CameraGate, Arc<AtomicBool>) {
        let keyframe = Arc::new(AtomicBool::new(false));
        (
            CameraGate {
                keyframe: keyframe.clone(),
                gap: false,
            },
            keyframe,
        )
    }

    const KEY: [u8; 4] = [6, 1, 1, 0];
    const DELTA: [u8; 4] = [6, 1, 0, 0];

    #[test]
    fn a_dropped_frame_skips_to_the_next_keyframe() {
        let (mut gate, keyframe) = gate();
        assert!(gate.admit(&KEY, false, true));
        assert!(gate.admit(&DELTA, false, true));
        // The socket is busy: dropped, and a keyframe is asked for.
        assert!(!gate.admit(&DELTA, false, false));
        assert!(keyframe.swap(false, Ordering::AcqRel));
        // Clear again, but a P frame after a gap can't decode.
        assert!(!gate.admit(&DELTA, false, true));
        assert!(gate.admit(&KEY, false, true));
        assert!(gate.admit(&DELTA, false, true));
        // A frame replaced in the slot is a gap too.
        assert!(!gate.admit(&DELTA, true, true));
        assert!(gate.admit(&KEY, false, true));
    }

    /// A slow uplink between the client and the loopback socket: writes fill a send buffer
    /// of `capacity` bytes (the kernel's), which drains at `rate` bytes per second. Loopback
    /// itself can't be slowed (macOS grows its buffers to megabytes), so this stands in for
    /// the kernel's send buffer, and [`LinkBacklog`] for `TCP_NOTSENT_LOWAT` on it.
    struct Link {
        read: tokio::net::tcp::OwnedReadHalf,
        state: Arc<Mutex<LinkState>>,
    }

    #[derive(Default)]
    struct LinkState {
        queue: std::collections::VecDeque<u8>,
        capacity: usize,
        waker: Option<std::task::Waker>,
        closed: bool,
    }

    /// Clear while fewer than [`UNSENT_LIMIT`] bytes wait in the link's send buffer.
    struct LinkBacklog(Arc<Mutex<LinkState>>);

    impl Backlog for LinkBacklog {
        fn clear(&mut self) -> bool {
            self.0.lock().unwrap().queue.len() < UNSENT_LIMIT
        }
    }

    impl Link {
        fn new(tcp: tokio::net::TcpStream, rate: f64, capacity: usize) -> Self {
            let state = Arc::new(Mutex::new(LinkState {
                capacity,
                ..LinkState::default()
            }));
            let (read, mut write) = tcp.into_split();
            let drain = state.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt as _;
                let start = Instant::now();
                let mut sent = 0u64;
                loop {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    let budget = (start.elapsed().as_secs_f64() * rate) as u64;
                    let chunk: Vec<u8> = {
                        let mut state = drain.lock().unwrap();
                        if state.closed && state.queue.is_empty() {
                            break;
                        }
                        let take = (budget.saturating_sub(sent) as usize).min(state.queue.len());
                        let chunk: Vec<u8> = state.queue.drain(..take).collect();
                        if let Some(waker) = state.waker.take() {
                            waker.wake();
                        }
                        chunk
                    };
                    if chunk.is_empty() {
                        // An idle link saves up no burst.
                        sent = budget;
                        continue;
                    }
                    sent += chunk.len() as u64;
                    if write.write_all(&chunk).await.is_err() {
                        break;
                    }
                }
                let _ = write.shutdown().await;
            });
            Self { read, state }
        }
    }

    impl AsyncRead for Link {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.read).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for Link {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            let mut state = self.state.lock().unwrap();
            let room = state.capacity.saturating_sub(state.queue.len());
            if room == 0 {
                state.waker = Some(cx.waker().clone());
                return Poll::Pending;
            }
            let take = room.min(buf.len());
            state.queue.extend(&buf[..take]);
            Poll::Ready(Ok(take))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            // Bytes in the send buffer count as flushed, as with a kernel socket.
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            self.state.lock().unwrap().closed = true;
            Poll::Ready(Ok(()))
        }
    }

    /// Floods a 1 Mbps uplink (with a 64 KB send buffer) with 2.5 Mbps of camera frames, and
    /// returns how long each `kd` queued during the flood took to reach the server, and how
    /// many camera frames did.
    async fn flood(budgeted: bool) -> (Vec<Duration>, usize) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (arrived_tx, mut arrived) = mpsc::unbounded_channel::<(String, Instant)>();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut camera = 0usize;
            while let Some(Ok(message)) = socket.next().await {
                match message {
                    Message::Text(text) => {
                        let _ = arrived_tx.send((text.to_string(), Instant::now()));
                    }
                    Message::Binary(_) => camera += 1,
                    _ => {}
                }
            }
            camera
        });
        let tcp = tokio::net::TcpStream::connect(address).await.unwrap();
        let link = Link::new(tcp, 125_000.0, 64 * 1024);
        let backlog: Box<dyn Backlog> = if budgeted {
            Box::new(LinkBacklog(link.state.clone()))
        } else {
            Box::new(NoBacklog)
        };
        let (socket, _) = tokio_tungstenite::client_async(format!("ws://{address}/"), link)
            .await
            .unwrap();
        let keyframe = Arc::new(AtomicBool::new(false));
        let mut socket = SplitSocket::new(socket, keyframe.clone(), backlog);
        let start = Instant::now();
        let mut queued = Vec::new();
        let mut frame = 0u32;
        let mut tick = tokio::time::interval(Duration::from_millis(33));
        while start.elapsed() < Duration::from_millis(3000) {
            tick.tick().await;
            // 10.4 KB at 30 fps: 2.5 Mbps; a keyframe whenever one is asked for.
            let key = frame == 0 || keyframe.swap(false, Ordering::AcqRel);
            let mut bytes = crate::capture::webcam_frame(key, &[0u8; 10_400]);
            bytes[3] = frame as u8;
            socket.send_camera(bytes);
            frame += 1;
            if start.elapsed() > Duration::from_millis(1000) && frame.is_multiple_of(9) {
                let text = format!("kd,{frame}");
                queued.push((text.clone(), Instant::now()));
                socket.send(text.into()).await.unwrap();
            }
        }
        let mut latencies = Vec::new();
        for (text, at) in &queued {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                let (got, when) = tokio::time::timeout_at(deadline, arrived.recv())
                    .await
                    .expect("the kd arrives")
                    .unwrap();
                if &got == text {
                    latencies.push(when.duration_since(*at));
                    break;
                }
            }
        }
        let _ = socket.close(None).await;
        drop(socket);
        let camera = tokio::time::timeout(Duration::from_secs(20), server)
            .await
            .map_or(0, |camera| camera.unwrap_or(0));
        (latencies, camera)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn camera_flood_never_holds_input_back_on_a_slow_uplink() {
        let (latencies, camera) = flood(true).await;
        let worst = latencies.iter().max().copied().unwrap();
        assert!(latencies.len() >= 5);
        // A kd waits at most for the camera frame being written (10.4 KB is 83 ms at
        // 1 Mbps) and the 4 KB unsent limit.
        assert!(worst <= Duration::from_millis(150), "{latencies:?}");
        // The camera still gets about what the link carries (1 Mbps is ~12 frames/s).
        assert!(camera >= 20, "{camera} camera frames");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_the_budget_a_flood_delays_input() {
        // The control: the same flood with nothing held back fills the send buffer, and a kd
        // waits behind it.
        let (latencies, _) = flood(false).await;
        let worst = latencies.iter().max().copied().unwrap();
        assert!(worst > Duration::from_millis(400), "{latencies:?}");
    }

    #[tokio::test]
    async fn the_unsent_limit_is_set_on_a_real_socket() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move { listener.accept().await.unwrap() });
        let tcp = tokio::net::TcpStream::connect(address).await.unwrap();
        let _server = accept.await.unwrap();
        use std::os::fd::AsRawFd;
        let mut backlog = UnsentBacklog::new(tcp.as_raw_fd()).expect("TCP_NOTSENT_LOWAT");
        let mut value: libc::c_int = 0;
        let mut len = size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: a live socket fd and an int out value.
        let status = unsafe {
            libc::getsockopt(
                tcp.as_raw_fd(),
                libc::IPPROTO_TCP,
                TCP_NOTSENT_LOWAT,
                (&mut value as *mut libc::c_int).cast(),
                &mut len,
            )
        };
        assert_eq!((status, value as usize), (0, UNSENT_LIMIT));
        // An idle socket is clear.
        assert!(backlog.clear());
        let stream = tokio_tungstenite::MaybeTlsStream::Plain(tcp);
        assert!(socket_fd(&stream).is_some());
    }

    #[tokio::test]
    async fn writes_keep_their_order_and_close_after_them() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut seen = Vec::new();
            while let Some(Ok(message)) = socket.next().await {
                match message {
                    Message::Text(text) => seen.push(text.to_string()),
                    Message::Binary(data) => seen.push(format!("bin {}", data.len())),
                    Message::Close(_) => seen.push("close".into()),
                    _ => {}
                }
            }
            seen
        });
        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
            .await
            .unwrap();
        let mut socket = SplitSocket::new(
            socket,
            Arc::new(AtomicBool::new(false)),
            Box::new(NoBacklog),
        );
        for i in 0..50 {
            socket.send(format!("kd,{i}").into()).await.unwrap();
            if i == 10 {
                socket
                    .send(Message::Binary(crate::capture::mic_frame(&[1, 2]).into()))
                    .await
                    .unwrap();
            }
        }
        socket.close(None).await.unwrap();
        let seen = server.await.unwrap();
        assert_eq!(seen.len(), 52);
        assert_eq!(seen[11], "bin 3");
        assert_eq!(seen[12], "kd,11");
        assert_eq!(seen.last().unwrap(), "close");
        // Sending after the writer stopped fails, as a dead socket's send did.
        assert!(socket.send("kd,1".into()).await.is_err());
    }
}
