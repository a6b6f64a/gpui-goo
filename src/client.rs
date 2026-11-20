use crate::protocol::{Command, Node, Response, Selector, unique};
use base64::Engine;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

const MAX_FRAME: usize = 32 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum Error {
    #[error("connection failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(#[from] serde_json::Error),
    #[error("remote error {code}: {message}")]
    Remote { code: String, message: String },
    #[error("selector {0}")]
    Selector(&'static str),
    #[error("client endpoint must be loopback")]
    NonLoopback,
    #[error("authentication token must contain at least 32 bytes")]
    InvalidToken,
    #[error("invalid PNG signature")]
    InvalidPng,
    #[error("response frame exceeds {MAX_FRAME} bytes")]
    FrameTooLarge,
    #[error("timed out waiting for visible node")]
    Timeout,
}

pub struct Client {
    stream: TcpStream,
    token: String,
}

impl Client {
    pub fn connect(endpoint: SocketAddr, token: String) -> Result<Self, Error> {
        if !endpoint.ip().is_loopback() {
            return Err(Error::NonLoopback);
        }
        if token.len() < 32 {
            return Err(Error::InvalidToken);
        }
        let stream = TcpStream::connect(endpoint)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        Ok(Self { stream, token })
    }

    pub fn command(&mut self, command: Command) -> Result<Response, Error> {
        let request = serde_json::json!({"version": 1, "token": self.token, "command": command});
        let mut writer = BufWriter::new(&mut self.stream);
        serde_json::to_writer(&mut writer, &request)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        drop(writer);
        let reader = BufReader::new(&mut self.stream);
        let mut frame = Vec::new();
        let count = reader
            .take((MAX_FRAME + 1) as u64)
            .read_until(b'\n', &mut frame)?;
        if count > MAX_FRAME {
            return Err(Error::FrameTooLarge);
        }
        if frame.last() == Some(&b'\n') {
            frame.pop();
        }
        let response: Response = serde_json::from_slice(&frame)?;
        if let Response::Error { code, message } = &response {
            return Err(Error::Remote {
                code: code.clone(),
                message: message.clone(),
            });
        }
        Ok(response)
    }

    pub fn snapshot(&mut self) -> Result<Vec<Node>, Error> {
        match self.command(Command::Snapshot)? {
            Response::Nodes { nodes } => Ok(nodes),
            _ => Err(unexpected("expected nodes")),
        }
    }

    pub fn click(&mut self, selector: Selector) -> Result<(), Error> {
        expect_done(self.command(Command::Click { selector })?)
    }
    /// Scrolls over a visible element by pixels; positive deltas move right and down.
    pub fn scroll(&mut self, selector: Selector, delta: [f32; 2]) -> Result<(), Error> {
        expect_done(self.command(Command::Scroll { selector, delta })?)
    }
    pub fn input(&mut self, text: &str) -> Result<(), Error> {
        expect_done(self.command(Command::Input { text: text.into() })?)
    }
    pub fn press(&mut self, key: &str) -> Result<(), Error> {
        expect_done(self.command(Command::Press { key: key.into() })?)
    }

    pub fn screenshot(&mut self, path: &Path) -> Result<(), Error> {
        let encoded = match self.command(Command::Screenshot)? {
            Response::Png { base64 } => base64,
            _ => return Err(unexpected("expected PNG")),
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| Error::Remote {
                code: "invalid_png".into(),
                message: error.to_string(),
            })?;
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(Error::InvalidPng);
        }
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn wait_visible(&mut self, selector: &Selector, timeout: Duration) -> Result<Node, Error> {
        let deadline = Instant::now() + timeout;
        loop {
            let nodes = self.snapshot()?;
            let node = match unique(&nodes, selector) {
                Ok(node) => node,
                Err("not_found") => {
                    if Instant::now() >= deadline {
                        return Err(Error::Timeout);
                    }
                    thread::sleep(
                        Duration::from_millis(20)
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                    continue;
                }
                Err(reason) => return Err(Error::Selector(reason)),
            };
            if node.visible {
                return Ok(node.clone());
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            thread::sleep(
                Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}
fn unexpected(message: &str) -> Error {
    Error::Remote {
        code: "unexpected_response".into(),
        message: message.into(),
    }
}
fn expect_done(response: Response) -> Result<(), Error> {
    match response {
        Response::Done => Ok(()),
        _ => Err(unexpected("expected done")),
    }
}
