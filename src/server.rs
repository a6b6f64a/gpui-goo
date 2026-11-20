use crate::protocol::{Command, Request, Response};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;

const MAX_FRAME: usize = 64 * 1024;
const MAX_RESPONSE: usize = 32 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum Error {
    #[error("server address must be loopback")]
    NonLoopback,
    #[error("authentication token must contain at least 32 bytes")]
    InvalidToken,
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("could not start server thread: {0}")]
    Thread(#[source] io::Error),
}

pub struct Server {
    listener: TcpListener,
    token: String,
}

impl Server {
    pub fn bind(address: SocketAddr, token: String) -> Result<Self, Error> {
        if !address.ip().is_loopback() {
            return Err(Error::NonLoopback);
        }
        if token.len() < 32 {
            return Err(Error::InvalidToken);
        }
        Ok(Self {
            listener: TcpListener::bind(address)?,
            token,
        })
    }

    pub fn endpoint(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("bound listener has local address")
    }

    /// Starts server. Handler must return within a bounded time supplied by caller.
    pub fn start(
        self,
        handler: impl Fn(Command) -> Response + Send + 'static,
    ) -> Result<RunningServer, Error> {
        let endpoint = self.endpoint();
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(Mutex::new(None));
        let thread_stop = Arc::clone(&stop);
        let thread_active = Arc::clone(&active);
        let listener = self.listener;
        let token = self.token;
        let thread = thread::Builder::new()
            .name("native-ui-automation-server".into())
            .spawn(move || serve(listener, token, handler, thread_stop, thread_active))
            .map_err(Error::Thread)?;
        Ok(RunningServer {
            endpoint,
            stop,
            active,
            thread: Some(thread),
        })
    }
}

pub struct RunningServer {
    endpoint: SocketAddr,
    stop: Arc<AtomicBool>,
    active: Arc<Mutex<Option<TcpStream>>>,
    thread: Option<JoinHandle<()>>,
}

impl RunningServer {
    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(stream) = self
            .active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
        let _ = TcpStream::connect_timeout(&self.endpoint, Duration::from_millis(100));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(
    listener: TcpListener,
    token: String,
    handler: impl Fn(Command) -> Response,
    stop: Arc<AtomicBool>,
    active: Arc<Mutex<Option<TcpStream>>>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let mut active_guard = active.lock().unwrap_or_else(|poison| poison.into_inner());
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let active_stream = match stream.try_clone() {
                    Ok(stream) => stream,
                    Err(_) => break,
                };
                *active_guard = Some(active_stream);
                drop(active_guard);

                let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                let _ = serve_connection(stream, &token, &handler);
                active
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .take();
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn serve_connection(
    stream: TcpStream,
    token: &str,
    handler: &impl Fn(Command) -> Response,
) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    loop {
        let frame = match read_frame(&mut reader)? {
            Ok(frame) => frame,
            Err((code, message)) => {
                write_response(reader.get_mut(), &Response::Error { code, message })?;
                return Ok(());
            }
        };
        let request: Request = match serde_json::from_slice(&frame) {
            Ok(request) => request,
            Err(_) => {
                write_response(
                    reader.get_mut(),
                    &error("malformed_request", "invalid request"),
                )?;
                return Ok(());
            }
        };
        if request.version != 1 {
            write_response(
                reader.get_mut(),
                &error("unsupported_version", "unsupported protocol version"),
            )?;
            return Ok(());
        }
        if !constant_time_eq(request.token.as_bytes(), token.as_bytes()) {
            write_response(
                reader.get_mut(),
                &error("unauthorized", "authentication failed"),
            )?;
            return Ok(());
        }
        if let Command::Click { selector } = &request.command
            && selector.id.is_none()
            && selector.role.is_none()
            && selector.label.is_none()
        {
            write_response(
                reader.get_mut(),
                &error("invalid_selector", "selector must constrain a node"),
            )?;
            return Ok(());
        }
        let response = handler(request.command);
        write_response(reader.get_mut(), &response)?;
    }
}

fn read_frame(reader: &mut BufReader<TcpStream>) -> io::Result<Result<Vec<u8>, (String, String)>> {
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut frame = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "request frame timed out",
            ));
        }
        reader.get_ref().set_read_timeout(Some(remaining))?;
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(Err((
                "malformed_request".into(),
                "request must end with newline".into(),
            )));
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if frame.len() + newline + 1 > MAX_FRAME {
                return Ok(Err((
                    "frame_too_large".into(),
                    "request frame exceeds 64 KiB".into(),
                )));
            }
            frame.extend_from_slice(&available[..=newline]);
            reader.consume(newline + 1);
            return Ok(Ok(frame));
        }
        let available_len = available.len();
        if frame.len() + available_len >= MAX_FRAME {
            return Ok(Err((
                "frame_too_large".into(),
                "request frame exceeds 64 KiB".into(),
            )));
        }
        frame.extend_from_slice(available);
        reader.consume(available_len);
    }
}

fn write_response(stream: &mut TcpStream, response: &Response) -> io::Result<()> {
    let mut bytes = CappedVec::new(MAX_RESPONSE);
    let serialized = serde_json::to_writer(&mut bytes, response).is_ok();
    if serialized && bytes.0.len() < MAX_RESPONSE {
        bytes.0.push(b'\n');
        stream.write_all(&bytes.0)
    } else {
        stream.write_all(b"{\"kind\":\"error\",\"code\":\"response_too_large\",\"message\":\"response exceeds 32 MiB\"}\n")
    }
}

struct CappedVec(Vec<u8>, usize);

impl CappedVec {
    fn new(limit: usize) -> Self {
        Self(Vec::new(), limit)
    }
}

impl Write for CappedVec {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.1.saturating_sub(self.0.len()) {
            return Err(io::Error::other("response exceeds limit"));
        }
        self.0.try_reserve(bytes.len()).map_err(io::Error::other)?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn error(code: &str, message: &str) -> Response {
    Response::Error {
        code: code.into(),
        message: message.into(),
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}
