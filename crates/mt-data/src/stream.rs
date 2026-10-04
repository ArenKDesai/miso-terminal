//! Streams: long-lived WebSocket feeds (quotes, trade updates, news) owned by
//! the hub.
//!
//! A source implements [`Stream`] once per endpoint. Panels call
//! [`DataHub::watch_stream`] every frame with the topics they need (`quotes:XLU`);
//! the hub keeps **one connection per endpoint**, subscribed to the union of
//! every panel's topics, and folds each incoming frame into a shared state that
//! panels read like any other [`Snapshot`]. Dropped connections reconnect with
//! backoff and resubscribe. Topics nobody has watched for a while are
//! unsubscribed, and a stream nobody watches is closed. A refused login stops
//! the reconnecting until the keys change ([`DataHub::restart_streams`]).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::hub::AnyValue;
use crate::transport::BoxFuture;
use crate::{DataHub, FetchCtx, FetchError, Request, Snapshot};

/// One message from the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Binary(Bytes),
    /// The answer to a keep-alive ping.
    Pong,
}

impl Frame {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(t) => Some(t),
            _ => None,
        }
    }
}

/// An open WebSocket, as a transport provides it.
pub trait StreamConn: Send {
    fn send(&mut self, text: String) -> BoxFuture<'_, Result<(), FetchError>>;
    /// The next frame; `None` once the server has closed the connection.
    /// Must be cancel-safe (the hub drops it to do other work).
    fn recv(&mut self) -> BoxFuture<'_, Option<Result<Frame, FetchError>>>;
    fn ping(&mut self) -> BoxFuture<'_, Result<(), FetchError>>;
    fn close(&mut self) -> BoxFuture<'_, ()>;
}

/// What a frame did to the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Nothing panels show changed (heartbeats, acknowledgements).
    Ignored,
    Changed,
    /// The login was accepted: subscriptions can be sent.
    Ready,
}

/// A typed WebSocket feed. Implement once per endpoint.
///
/// ```ignore
/// impl Stream for QuoteStream {
///     type State = HashMap<String, Quote>;
///     fn key(&self) -> String { "alpaca/stream/iex".into() }
///     fn request(&self, _: &FetchCtx) -> Result<Request, FetchError> { Ok(Request::get(URL)) }
///     fn hello(&self, ctx: &FetchCtx) -> Result<Vec<String>, FetchError> { /* auth message */ }
///     fn waits_for_ready(&self) -> bool { true }
///     fn subscribe(&self, topics: &[String]) -> Vec<String> { /* {"action":"subscribe",…} */ }
///     fn unsubscribe(&self, topics: &[String]) -> Vec<String> { … }
///     fn apply(&self, state: &mut Self::State, frame: &Frame) -> Result<Applied, FetchError> { … }
/// }
/// ```
pub trait Stream: Clone + Send + Sync + 'static {
    /// What panels read. Cloned only when a panel still holds the previous
    /// value while a frame arrives.
    type State: Clone + Default + Send + Sync + 'static;

    /// One connection per key, by convention `<source>/stream/<endpoint>`.
    fn key(&self) -> String;

    /// Name for LOG.
    fn label(&self) -> String {
        self.key()
    }

    /// The WebSocket URL and any handshake headers (API keys as
    /// [`Request::secret_header`]).
    fn request(&self, ctx: &FetchCtx) -> Result<Request, FetchError>;

    /// Messages sent first on every connection, before any subscription
    /// (a login). Never logged.
    fn hello(&self, _ctx: &FetchCtx) -> Result<Vec<String>, FetchError> {
        Ok(Vec::new())
    }

    /// Whether subscriptions must wait for [`Applied::Ready`] (a login acknowledgement).
    fn waits_for_ready(&self) -> bool {
        false
    }

    /// Messages that add these topics.
    fn subscribe(&self, topics: &[String]) -> Vec<String>;

    /// Messages that drop these topics.
    fn unsubscribe(&self, topics: &[String]) -> Vec<String>;

    /// Fold one frame into the state. A [`FetchError::Parse`] skips the frame;
    /// [`FetchError::Auth`] stops the stream; any other error reconnects.
    fn apply(&self, state: &mut Self::State, frame: &Frame) -> Result<Applied, FetchError>;

    /// Called on every (re)connection before `hello`, to drop anything a
    /// reconnect invalidates. The state is otherwise kept across reconnects.
    fn on_connect(&self, _state: &mut Self::State) {}
}

/// Where a stream's connection is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamPhase {
    /// Not connected, and not trying (nobody watching, or paused).
    Idle,
    Connecting,
    /// Connected; `ready` once the login is accepted.
    Connected {
        ready: bool,
    },
    /// Waiting to reconnect after a failure.
    Retrying,
    /// The server refused the credentials; waits for a restart.
    Failed,
}

/// One row of LOG's stream table.
#[derive(Clone, Debug)]
pub struct StreamStatus {
    pub key: String,
    pub label: String,
    pub phase: StreamPhase,
    pub error: Option<String>,
    pub connected_since: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    /// Frames that changed the state, since launch.
    pub messages: u64,
    /// Successful connections, since launch (more than one means reconnects).
    pub connects: u64,
    /// Topics currently subscribed.
    pub topics: usize,
    pub watched: bool,
}

#[cfg(not(test))]
mod timing {
    use std::time::Duration;
    pub const TOPIC_LINGER: Duration = Duration::from_secs(30);
    pub const STREAM_LINGER: Duration = Duration::from_secs(60);
    pub const RETRY_BASE: Duration = Duration::from_secs(1);
    pub const RETRY_MAX: Duration = Duration::from_secs(60);
    pub const READY_TIMEOUT: Duration = Duration::from_secs(15);
    pub const PING_EVERY: Duration = Duration::from_secs(30);
    pub const HOUSEKEEPING: Duration = Duration::from_secs(1);
}

#[cfg(test)]
mod timing {
    use std::time::Duration;
    pub const TOPIC_LINGER: Duration = Duration::from_millis(150);
    pub const STREAM_LINGER: Duration = Duration::from_millis(300);
    pub const RETRY_BASE: Duration = Duration::from_millis(10);
    pub const RETRY_MAX: Duration = Duration::from_millis(80);
    pub const READY_TIMEOUT: Duration = Duration::from_millis(500);
    pub const PING_EVERY: Duration = Duration::from_millis(100);
    pub const HOUSEKEEPING: Duration = Duration::from_millis(20);
}

use timing::*;

/// How often a busy stream may ask the UI to repaint.
const NOTIFY_EVERY: Duration = Duration::from_millis(100);
/// A connection that stayed up this long resets the retry backoff.
const STABLE_AFTER: Duration = Duration::from_secs(30);

pub(crate) struct StreamShared {
    entry: Mutex<StreamEntry>,
    /// A new topic arrived; subscribe without waiting for housekeeping.
    wake: Notify,
    /// Cut a retry wait short.
    restart: Notify,
}

struct StreamEntry {
    label: String,
    /// Topic -> when a panel last asked for it.
    topics: HashMap<String, Instant>,
    last_watched: Instant,
    value: Option<AnyValue>,
    updated: Option<DateTime<Utc>>,
    generation: u64,
    phase: StreamPhase,
    error: Option<FetchError>,
    connected_since: Option<DateTime<Utc>>,
    messages: u64,
    connects: u64,
    subscribed: usize,
    running: bool,
}

impl StreamShared {
    fn new(label: String) -> Self {
        Self {
            entry: Mutex::new(StreamEntry {
                label,
                topics: HashMap::new(),
                last_watched: Instant::now(),
                value: None,
                updated: None,
                generation: 0,
                phase: StreamPhase::Idle,
                error: None,
                connected_since: None,
                messages: 0,
                connects: 0,
                subscribed: 0,
                running: false,
            }),
            wake: Notify::new(),
            restart: Notify::new(),
        }
    }
}

impl StreamEntry {
    fn snapshot<T: Send + Sync + 'static>(&self) -> Snapshot<T> {
        Snapshot {
            data: self.value.clone().and_then(|v| v.downcast::<T>().ok()),
            updated: self.updated,
            loading: matches!(
                self.phase,
                StreamPhase::Connecting | StreamPhase::Connected { ready: false }
            ),
            error: self.error.clone(),
            stale: self.value.is_some() && !matches!(self.phase, StreamPhase::Connected { .. }),
            generation: self.generation,
        }
    }

    /// Drop topics nobody asked for lately; whether the stream is still wanted.
    fn prune(&mut self, now: Instant) -> bool {
        self.topics
            .retain(|_, at| now.duration_since(*at) < TOPIC_LINGER);
        !self.topics.is_empty() || now.duration_since(self.last_watched) < STREAM_LINGER
    }
}

/// Why a connection ended without an error.
enum SessionEnd {
    Unwatched,
    Paused,
}

impl DataHub {
    /// The current state of stream `s`, connecting in the background if
    /// needed, with `topics` added to its subscriptions. Call every frame,
    /// like [`DataHub::watch`]; topics not asked for again are dropped after a while.
    pub fn watch_stream<S: Stream>(&self, s: &S, topics: &[&str]) -> Snapshot<S::State> {
        let shared = self
            .streams()
            .lock()
            .entry(s.key())
            .or_insert_with(|| Arc::new(StreamShared::new(s.label())))
            .clone();
        let now = Instant::now();
        let mut e = shared.entry.lock();
        e.last_watched = now;
        let mut new_topic = false;
        for t in topics {
            match e.topics.get_mut(*t) {
                Some(at) => *at = now,
                None => {
                    e.topics.insert((*t).to_owned(), now);
                    new_topic = true;
                }
            }
        }
        let start = !e.running && !self.is_paused() && e.phase != StreamPhase::Failed;
        if start {
            e.running = true;
            e.phase = StreamPhase::Connecting;
        }
        let snap = e.snapshot();
        drop(e);
        if start {
            self.spawn_stream(s.clone(), shared);
        } else if new_topic {
            shared.wake.notify_one();
        }
        snap
    }

    /// The state of stream `s` without registering interest or connecting.
    pub fn peek_stream<S: Stream>(&self, s: &S) -> Snapshot<S::State> {
        let shared = self.streams().lock().get(&s.key()).cloned();
        shared
            .map(|sh| sh.entry.lock().snapshot())
            .unwrap_or_default()
    }

    /// Clear refused logins and cut retry waits short: call after the
    /// credentials change. Streams reconnect on their next watch.
    pub fn restart_streams(&self) {
        let all: Vec<Arc<StreamShared>> = self.streams().lock().values().cloned().collect();
        for sh in all {
            let mut e = sh.entry.lock();
            if e.phase == StreamPhase::Failed {
                e.phase = StreamPhase::Idle;
                e.error = None;
            }
            drop(e);
            sh.restart.notify_one();
        }
    }

    pub fn stream_status(&self) -> Vec<StreamStatus> {
        let now = Instant::now();
        let mut out: Vec<StreamStatus> = self
            .streams()
            .lock()
            .iter()
            .map(|(key, sh)| {
                let e = sh.entry.lock();
                StreamStatus {
                    key: key.clone(),
                    label: e.label.clone(),
                    phase: e.phase,
                    error: e.error.as_ref().map(ToString::to_string),
                    connected_since: e.connected_since,
                    updated: e.updated,
                    messages: e.messages,
                    connects: e.connects,
                    topics: e.subscribed,
                    watched: now.duration_since(e.last_watched) < crate::hub::WATCH_WINDOW,
                }
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    /// Forget streams that are closed and unwatched for `idle`.
    pub(crate) fn gc_streams(&self, idle: Duration) -> usize {
        let now = Instant::now();
        let mut streams = self.streams().lock();
        let before = streams.len();
        streams.retain(|_, sh| {
            let e = sh.entry.lock();
            e.running || now.duration_since(e.last_watched) < idle
        });
        before - streams.len()
    }

    fn spawn_stream<S: Stream>(&self, s: S, shared: Arc<StreamShared>) {
        let hub = self.clone();
        self.runtime().spawn(async move {
            // Run as its own task so a panicking parser is reported instead of
            // leaving the stream marked running forever.
            let task = tokio::spawn(run(hub.clone(), s, shared.clone())).await;
            if let Err(e) = task {
                let mut entry = shared.entry.lock();
                entry.running = false;
                entry.phase = StreamPhase::Failed;
                entry.error = Some(FetchError::Other(format!("stream task failed: {e}")));
                drop(entry);
                hub.notify();
            }
        });
    }
}

/// Connect, run, and reconnect until nobody wants the stream.
async fn run<S: Stream>(hub: DataHub, s: S, shared: Arc<StreamShared>) {
    let events = hub.ctx().events().clone();
    let label = shared.entry.lock().label.clone();
    let mut failures: u32 = 0;
    loop {
        {
            let mut e = shared.entry.lock();
            let wanted = e.prune(Instant::now());
            if !wanted || hub.is_paused() || e.phase == StreamPhase::Failed {
                if e.phase != StreamPhase::Failed {
                    e.phase = StreamPhase::Idle;
                }
                e.running = false;
                e.connected_since = None;
                break;
            }
            e.phase = StreamPhase::Connecting;
        }
        hub.notify();
        let started = Instant::now();
        let end = session(&hub, &s, &shared).await;
        let retry_in = {
            let mut e = shared.entry.lock();
            e.connected_since = None;
            e.subscribed = 0;
            match end {
                Ok(why) => {
                    let why = match why {
                        SessionEnd::Unwatched => "not watched",
                        SessionEnd::Paused => "paused",
                    };
                    events.info(format!("stream {label}: closed ({why})"));
                    e.phase = StreamPhase::Idle;
                    e.running = false;
                    None
                }
                Err(err @ FetchError::Auth(_)) => {
                    events.error(format!("stream {label}: {err}; not retrying"));
                    e.phase = StreamPhase::Failed;
                    e.error = Some(err);
                    e.running = false;
                    None
                }
                Err(err) => {
                    if started.elapsed() >= STABLE_AFTER {
                        failures = 0;
                    }
                    failures = failures.saturating_add(1);
                    let wait = RETRY_BASE
                        .saturating_mul(1 << (failures - 1).min(10))
                        .min(RETRY_MAX);
                    events.warn(format!(
                        "stream {label}: {err}; reconnecting in {} s",
                        wait.as_secs_f32().ceil()
                    ));
                    e.phase = StreamPhase::Retrying;
                    e.error = Some(err);
                    Some(wait)
                }
            }
        };
        let Some(wait) = retry_in else { break };
        hub.notify();
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = shared.restart.notified() => {}
        }
    }
    hub.notify();
}

/// One connection, from handshake to disconnect.
async fn session<S: Stream>(
    hub: &DataHub,
    s: &S,
    shared: &StreamShared,
) -> Result<SessionEnd, FetchError> {
    let ctx = hub.ctx();
    let label = shared.entry.lock().label.clone();
    let req = s.request(ctx)?;
    ctx.events()
        .info(format!("stream {label}: connecting {}", req.describe()));
    let mut conn = ctx.transport().connect(&req).await?;
    {
        let mut e = shared.entry.lock();
        e.phase = StreamPhase::Connected { ready: false };
        e.connected_since = Some(Utc::now());
        e.connects += 1;
        let mut state = take_state::<S>(&mut e);
        s.on_connect(Arc::make_mut(&mut state));
        e.value = Some(state);
    }
    for msg in s.hello(ctx)? {
        conn.send(msg).await?;
    }
    let mut ready = !s.waits_for_ready();
    let mut subscribed = BTreeSet::new();
    if ready {
        set_ready(shared);
        sync(&mut conn, s, shared, &mut subscribed, ctx).await?;
    }
    hub.notify();

    let connected = Instant::now();
    let (mut last_seen, mut last_ping, mut last_chores) = (connected, connected, connected);
    let mut last_notify = connected;
    let mut pending_notify = false;
    let mut tick = tokio::time::interval(NOTIFY_EVERY.min(HOUSEKEEPING));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    enum Event {
        Frame(Option<Result<Frame, FetchError>>),
        Tick,
        Wake,
    }
    loop {
        let event = tokio::select! {
            f = conn.recv() => Event::Frame(f),
            _ = tick.tick() => Event::Tick,
            () = shared.wake.notified() => Event::Wake,
        };
        let now = Instant::now();
        match event {
            Event::Frame(None) => {
                return Err(FetchError::Network("closed by the server".into()));
            }
            Event::Frame(Some(Err(e))) => return Err(e),
            Event::Frame(Some(Ok(frame))) => {
                last_seen = now;
                if frame == Frame::Pong {
                    continue;
                }
                match apply(s, shared, &frame) {
                    Ok(Applied::Ready) if !ready => {
                        ready = true;
                        set_ready(shared);
                        sync(&mut conn, s, shared, &mut subscribed, ctx).await?;
                        pending_notify = true;
                    }
                    Ok(Applied::Changed) => pending_notify = true,
                    Ok(Applied::Ready | Applied::Ignored) => {}
                    Err(e @ FetchError::Parse { .. }) => {
                        ctx.events()
                            .warn(format!("stream {label}: skipped a frame: {e}"));
                    }
                    Err(e) => {
                        conn.close().await;
                        return Err(e);
                    }
                }
            }
            Event::Wake => {
                if ready {
                    sync(&mut conn, s, shared, &mut subscribed, ctx).await?;
                }
            }
            Event::Tick => {}
        }
        if pending_notify && now.duration_since(last_notify) >= NOTIFY_EVERY {
            hub.notify();
            pending_notify = false;
            last_notify = now;
        }
        if now.duration_since(last_chores) < HOUSEKEEPING {
            continue;
        }
        last_chores = now;
        if hub.is_paused() {
            conn.close().await;
            return Ok(SessionEnd::Paused);
        }
        if !shared.entry.lock().prune(now) {
            conn.close().await;
            return Ok(SessionEnd::Unwatched);
        }
        if !ready && now.duration_since(connected) >= READY_TIMEOUT {
            conn.close().await;
            return Err(FetchError::Network("no answer to the login".into()));
        }
        if ready {
            sync(&mut conn, s, shared, &mut subscribed, ctx).await?;
        }
        if now.duration_since(last_seen) >= PING_EVERY * 2 {
            conn.close().await;
            return Err(FetchError::Network(format!(
                "silent for {} s",
                now.duration_since(last_seen).as_secs()
            )));
        }
        if now.duration_since(last_ping) >= PING_EVERY {
            conn.ping().await?;
            last_ping = now;
        }
    }
}

fn take_state<S: Stream>(e: &mut StreamEntry) -> Arc<S::State> {
    e.value
        .take()
        .and_then(|v| v.downcast::<S::State>().ok())
        .unwrap_or_default()
}

/// The login was accepted: the stream is healthy again.
fn set_ready(shared: &StreamShared) {
    let mut e = shared.entry.lock();
    e.phase = StreamPhase::Connected { ready: true };
    e.error = None;
}

fn apply<S: Stream>(s: &S, shared: &StreamShared, frame: &Frame) -> Result<Applied, FetchError> {
    let mut e = shared.entry.lock();
    let mut state = take_state::<S>(&mut e);
    let result = s.apply(Arc::make_mut(&mut state), frame);
    e.value = Some(state);
    if result == Ok(Applied::Changed) {
        e.generation += 1;
        e.messages += 1;
        e.updated = Some(Utc::now());
    }
    result
}

/// Bring the server's subscriptions in line with what panels watch.
async fn sync<S: Stream>(
    conn: &mut Box<dyn crate::StreamConn>,
    s: &S,
    shared: &StreamShared,
    subscribed: &mut BTreeSet<String>,
    ctx: &FetchCtx,
) -> Result<(), FetchError> {
    let (want, label) = {
        let mut e = shared.entry.lock();
        e.prune(Instant::now());
        let want: BTreeSet<String> = e.topics.keys().cloned().collect();
        (want, e.label.clone())
    };
    let add: Vec<String> = want.difference(subscribed).cloned().collect();
    let drop: Vec<String> = subscribed.difference(&want).cloned().collect();
    if !drop.is_empty() {
        for msg in s.unsubscribe(&drop) {
            conn.send(msg).await?;
        }
        ctx.events()
            .info(format!("stream {label}: unsubscribed {}", drop.join(" ")));
    }
    if !add.is_empty() {
        for msg in s.subscribe(&add) {
            conn.send(msg).await?;
        }
        ctx.events()
            .info(format!("stream {label}: subscribed {}", add.join(" ")));
    }
    *subscribed = want;
    shared.entry.lock().subscribed = subscribed.len();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tokio::sync::mpsc;

    use super::*;
    use crate::{EventLog, FetchCtxOptions, Response, Transport};

    /// The server side of one fake connection.
    struct Server {
        frames: mpsc::UnboundedSender<Option<Frame>>,
        sent: Arc<Mutex<Vec<String>>>,
        closed: Arc<Mutex<bool>>,
    }

    impl Server {
        fn push(&self, text: &str) {
            let _ = self.frames.send(Some(Frame::Text(text.into())));
        }
        fn hang_up(&self) {
            let _ = self.frames.send(None);
        }
        fn sent(&self) -> Vec<String> {
            self.sent.lock().clone()
        }
    }

    struct Client {
        frames: mpsc::UnboundedReceiver<Option<Frame>>,
        /// For answering pings.
        pong: mpsc::UnboundedSender<Option<Frame>>,
        sent: Arc<Mutex<Vec<String>>>,
        closed: Arc<Mutex<bool>>,
    }

    impl StreamConn for Client {
        fn send(&mut self, text: String) -> BoxFuture<'_, Result<(), FetchError>> {
            self.sent.lock().push(text);
            Box::pin(async { Ok(()) })
        }
        fn recv(&mut self) -> BoxFuture<'_, Option<Result<Frame, FetchError>>> {
            Box::pin(async move { self.frames.recv().await.flatten().map(Ok) })
        }
        fn ping(&mut self) -> BoxFuture<'_, Result<(), FetchError>> {
            let _ = self.pong.send(Some(Frame::Pong));
            Box::pin(async { Ok(()) })
        }
        fn close(&mut self) -> BoxFuture<'_, ()> {
            *self.closed.lock() = true;
            Box::pin(async {})
        }
    }

    /// Hands out scripted connections and records every one.
    #[derive(Default)]
    struct FakeServer {
        servers: Mutex<Vec<Arc<Server>>>,
    }

    impl Transport for FakeServer {
        fn send<'a>(&'a self, _: &'a Request) -> BoxFuture<'a, Result<Response, FetchError>> {
            Box::pin(async { Err(FetchError::Other("no requests here".into())) })
        }
        fn connect<'a>(
            &'a self,
            _: &'a Request,
        ) -> BoxFuture<'a, Result<Box<dyn StreamConn>, FetchError>> {
            let (tx, rx) = mpsc::unbounded_channel();
            let sent = Arc::new(Mutex::new(Vec::new()));
            let closed = Arc::new(Mutex::new(false));
            self.servers.lock().push(Arc::new(Server {
                frames: tx.clone(),
                sent: sent.clone(),
                closed: closed.clone(),
            }));
            Box::pin(async move {
                Ok(Box::new(Client {
                    frames: rx,
                    pong: tx,
                    sent,
                    closed,
                }) as Box<dyn StreamConn>)
            })
        }
        fn describe(&self) -> String {
            "fake".into()
        }
    }

    /// A toy protocol: `ok` accepts the login, `deny` refuses it, `T=v` sets topic T.
    #[derive(Clone)]
    struct Ticks;

    impl Stream for Ticks {
        type State = BTreeMap<String, String>;
        fn key(&self) -> String {
            "test/stream/ticks".into()
        }
        fn request(&self, ctx: &FetchCtx) -> Result<Request, FetchError> {
            let key = ctx.secret("test/key")?;
            Ok(Request::get("wss://fake/ticks").secret_header("X-Key", key))
        }
        fn hello(&self, _: &FetchCtx) -> Result<Vec<String>, FetchError> {
            Ok(vec!["login".into()])
        }
        fn waits_for_ready(&self) -> bool {
            true
        }
        fn subscribe(&self, topics: &[String]) -> Vec<String> {
            vec![format!("sub {}", topics.join(","))]
        }
        fn unsubscribe(&self, topics: &[String]) -> Vec<String> {
            vec![format!("unsub {}", topics.join(","))]
        }
        fn apply(&self, state: &mut Self::State, frame: &Frame) -> Result<Applied, FetchError> {
            match frame.as_text() {
                Some("ok") => Ok(Applied::Ready),
                Some("deny") => Err(FetchError::Auth("bad key".into())),
                Some(t) => match t.split_once('=') {
                    Some((k, v)) => {
                        state.insert(k.into(), v.into());
                        Ok(Applied::Changed)
                    }
                    None => Err(FetchError::parse("tick", t)),
                },
                None => Ok(Applied::Ignored),
            }
        }
    }

    fn setup() -> (tokio::runtime::Runtime, DataHub, Arc<FakeServer>) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let fake = Arc::new(FakeServer::default());
        let secrets = crate::MemorySecrets::with([("test/key", crate::Secret::new("k"))]);
        let ctx = FetchCtx::new(
            fake.clone(),
            None,
            FetchCtxOptions {
                secrets: Arc::new(secrets),
                ..FetchCtxOptions::default()
            },
            EventLog::default(),
        );
        let hub = DataHub::new(rt.handle().clone(), ctx);
        (rt, hub, fake)
    }

    fn wait_for(mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn server(fake: &FakeServer, i: usize) -> Arc<Server> {
        wait_for(|| fake.servers.lock().len() > i);
        fake.servers.lock()[i].clone()
    }

    fn phase(hub: &DataHub) -> StreamPhase {
        hub.stream_status()[0].phase
    }

    #[test]
    fn panels_share_one_connection_and_its_subscriptions() {
        let (_rt, hub, fake) = setup();
        let first = hub.watch_stream(&Ticks, &["A"]);
        assert!(first.data.is_none() && first.loading);
        hub.watch_stream(&Ticks, &["B"]);
        let srv = server(&fake, 0);
        wait_for(|| srv.sent() == ["login"]);
        srv.push("ok");
        wait_for(|| srv.sent().len() == 2);
        assert_eq!(srv.sent()[1], "sub A,B", "subscriptions wait for the login");
        srv.push("A=1");
        srv.push("B=2");
        wait_for(|| hub.peek_stream(&Ticks).generation == 2);
        let snap = hub.watch_stream(&Ticks, &["A", "B"]);
        let state = snap.data().unwrap();
        assert_eq!((state["A"].as_str(), state["B"].as_str()), ("1", "2"));
        assert!(!snap.loading && !snap.stale);
        // A topic added later is subscribed at once.
        hub.watch_stream(&Ticks, &["C"]);
        wait_for(|| srv.sent().last().map(String::as_str) == Some("sub C"));
        assert_eq!(fake.servers.lock().len(), 1, "one connection for everyone");
        // A malformed frame is skipped, not fatal.
        srv.push("garbage");
        srv.push("A=3");
        wait_for(|| {
            hub.peek_stream(&Ticks)
                .data()
                .is_some_and(|s| s["A"] == "3")
        });
        assert_eq!(hub.stream_status()[0].connects, 1);
    }

    #[test]
    fn reconnects_and_resubscribes_keeping_the_state() {
        let (_rt, hub, fake) = setup();
        let keep_watching = || {
            hub.watch_stream(&Ticks, &["A", "B"]);
        };
        keep_watching();
        let srv = server(&fake, 0);
        wait_for(|| srv.sent() == ["login"]);
        srv.push("ok");
        srv.push("A=1");
        wait_for(|| hub.peek_stream(&Ticks).generation == 1);
        srv.hang_up();
        let again = server(&fake, 1);
        wait_for(|| {
            keep_watching();
            again.sent() == ["login"]
        });
        let snap = hub.peek_stream(&Ticks);
        assert_eq!(snap.data().unwrap()["A"], "1", "the last state survives");
        assert!(snap.stale || snap.loading);
        assert!(hub.stream_status()[0].error.is_some());
        again.push("ok");
        wait_for(|| again.sent().len() == 2);
        assert_eq!(again.sent()[1], "sub A,B");
        assert_eq!(hub.stream_status()[0].connects, 2);
        assert!(hub.stream_status()[0].error.is_none());
    }

    #[test]
    fn a_refused_login_stops_until_restarted() {
        let (_rt, hub, fake) = setup();
        hub.watch_stream(&Ticks, &["A"]);
        let srv = server(&fake, 0);
        srv.push("deny");
        wait_for(|| phase(&hub) == StreamPhase::Failed);
        assert!(*srv.closed.lock());
        for _ in 0..5 {
            hub.watch_stream(&Ticks, &["A"]);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fake.servers.lock().len(), 1, "no retries with refused keys");
        assert!(matches!(
            hub.peek_stream(&Ticks).error,
            Some(FetchError::Auth(_))
        ));
        hub.restart_streams();
        hub.watch_stream(&Ticks, &["A"]);
        server(&fake, 1);
    }

    #[test]
    fn missing_keys_fail_without_connecting() {
        let (_rt, hub, fake) = setup();
        hub.ctx().secrets().delete("test/key").unwrap();
        hub.watch_stream(&Ticks, &["A"]);
        wait_for(|| hub.peek_stream(&Ticks).error.is_some());
        assert!(matches!(
            hub.peek_stream(&Ticks).error,
            Some(FetchError::Auth(_))
        ));
        assert!(fake.servers.lock().is_empty());
    }

    #[test]
    fn unwatched_topics_and_streams_are_dropped() {
        let (_rt, hub, fake) = setup();
        hub.watch_stream(&Ticks, &["A", "B"]);
        let srv = server(&fake, 0);
        srv.push("ok");
        wait_for(|| srv.sent().len() == 2);
        // Keep watching A only: B lingers, then is unsubscribed.
        wait_for(|| {
            hub.watch_stream(&Ticks, &["A"]);
            srv.sent().iter().any(|m| m == "unsub B")
        });
        wait_for(|| hub.stream_status()[0].topics == 1);
        // Stop watching altogether: the connection closes.
        wait_for(|| *srv.closed.lock());
        wait_for(|| phase(&hub) == StreamPhase::Idle);
        assert!(
            hub.peek_stream(&Ticks).stale,
            "the last state is kept, marked stale"
        );
        assert_eq!(hub.gc(Duration::ZERO), 1);
        assert!(hub.stream_status().is_empty());
    }

    #[test]
    fn pausing_closes_and_blocks_streams() {
        let (_rt, hub, fake) = setup();
        hub.watch_stream(&Ticks, &["A"]);
        let srv = server(&fake, 0);
        hub.set_paused(true);
        wait_for(|| {
            hub.watch_stream(&Ticks, &["A"]);
            *srv.closed.lock()
        });
        wait_for(|| phase(&hub) == StreamPhase::Idle);
        hub.watch_stream(&Ticks, &["A"]);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(fake.servers.lock().len(), 1);
        hub.set_paused(false);
        hub.watch_stream(&Ticks, &["A"]);
        server(&fake, 1);
    }

    #[test]
    fn notify_fires_for_new_frames() {
        let (_rt, hub, fake) = setup();
        let fired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let f = fired.clone();
        hub.set_notify(move || {
            f.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        hub.watch_stream(&Ticks, &["A"]);
        let srv = server(&fake, 0);
        srv.push("ok");
        wait_for(|| srv.sent().len() == 2);
        std::thread::sleep(Duration::from_millis(150));
        let before = fired.load(std::sync::atomic::Ordering::SeqCst);
        srv.push("A=1");
        wait_for(|| fired.load(std::sync::atomic::Ordering::SeqCst) > before);
    }
}
