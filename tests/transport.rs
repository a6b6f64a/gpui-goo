use gpui_goo::protocol::{Command, Response};
use gpui_goo::server::Server;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn read_response(reader: &mut BufReader<TcpStream>) -> Response {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn exchange(endpoint: SocketAddr, bytes: &[u8]) -> Response {
    let mut reader = BufReader::new(TcpStream::connect(endpoint).unwrap());
    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    reader.get_mut().write_all(bytes).unwrap();
    read_response(&mut reader)
}

fn rejected_and_closed(endpoint: SocketAddr, bytes: &[u8]) -> Response {
    let mut reader = BufReader::new(TcpStream::connect(endpoint).unwrap());
    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    reader.get_mut().write_all(bytes).unwrap();
    let response = read_response(&mut reader);
    let mut extra = String::new();
    assert_eq!(reader.read_line(&mut extra).unwrap(), 0);
    response
}

fn error_code(response: Response) -> String {
    match response {
        Response::Error { code, .. } => code,
        other => panic!("expected error response, received {other:?}"),
    }
}

#[test]
fn authenticates_bounded_frames_and_dispatches_valid_request() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = Arc::clone(&calls);
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), TOKEN.into())
        .unwrap()
        .start(move |command| {
            handler_calls.fetch_add(1, Ordering::SeqCst);
            match command {
                Command::Snapshot => Response::Nodes { nodes: Vec::new() },
                _ => Response::Done,
            }
        })
        .unwrap();
    let endpoint = server.endpoint();

    assert_eq!(error_code(rejected_and_closed(endpoint, b"{broken}\n{\"version\":1,\"token\":\"0123456789abcdef0123456789abcdef\",\"command\":{\"op\":\"snapshot\"}}\n")), "malformed_request");
    assert_eq!(error_code(rejected_and_closed(endpoint, b"{\"version\":1,\"token\":\"invalid\",\"command\":{\"op\":\"snapshot\"}}\n{\"version\":1,\"token\":\"0123456789abcdef0123456789abcdef\",\"command\":{\"op\":\"snapshot\"}}\n")), "unauthorized");
    assert_eq!(error_code(exchange(endpoint, b"{\"version\":1,\"token\":\"0123456789abcdef0123456789abcdef\",\"command\":{\"op\":\"click\",\"selector\":{\"id\":null,\"role\":null,\"label\":null}}}\n")), "invalid_selector");

    let mut oversized = vec![b'x'; 64 * 1024];
    oversized.push(b'\n');
    assert_eq!(
        error_code(exchange(endpoint, &oversized)),
        "frame_too_large"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    assert_eq!(exchange(endpoint, b"{\"version\":1,\"token\":\"0123456789abcdef0123456789abcdef\",\"command\":{\"op\":\"snapshot\"}}\n"), Response::Nodes { nodes: Vec::new() });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn keeps_connection_for_pipelined_snapshot_action_snapshot() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = Arc::clone(&calls);
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), TOKEN.into())
        .unwrap()
        .start(move |command| {
            handler_calls.fetch_add(1, Ordering::SeqCst);
            match command {
                Command::Snapshot => Response::Nodes { nodes: Vec::new() },
                _ => Response::Done,
            }
        })
        .unwrap();
    let mut reader = BufReader::new(TcpStream::connect(server.endpoint()).unwrap());
    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    reader.get_mut().write_all(
        b"{\"version\":1,\"token\":\"0123456789abcdef0123456789abcdef\",\"command\":{\"op\":\"snapshot\"}}\n\
          {\"version\":1,\"token\":\"0123456789abcdef0123456789abcdef\",\"command\":{\"op\":\"click\",\"selector\":{\"id\":\"node\",\"role\":null,\"label\":null}}}\n\
          {\"version\":1,\"token\":\"0123456789abcdef0123456789abcdef\",\"command\":{\"op\":\"snapshot\"}}\n",
    ).unwrap();

    assert_eq!(
        read_response(&mut reader),
        Response::Nodes { nodes: Vec::new() }
    );
    assert_eq!(read_response(&mut reader), Response::Done);
    assert_eq!(
        read_response(&mut reader),
        Response::Nodes { nodes: Vec::new() }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[test]
fn dropping_server_closes_idle_active_connection() {
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), TOKEN.into())
        .unwrap()
        .start(|_| Response::Nodes { nodes: Vec::new() })
        .unwrap();
    let mut reader = BufReader::new(TcpStream::connect(server.endpoint()).unwrap());
    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    reader.get_mut().write_all(
        b"{\"version\":1,\"token\":\"0123456789abcdef0123456789abcdef\",\"command\":{\"op\":\"snapshot\"}}\n",
    ).unwrap();
    assert_eq!(
        read_response(&mut reader),
        Response::Nodes { nodes: Vec::new() }
    );

    let started = Instant::now();
    drop(server);
    assert!(started.elapsed() < Duration::from_secs(2));
    let mut extra = String::new();
    assert_eq!(reader.read_line(&mut extra).unwrap(), 0);
}

#[test]
fn dropping_server_interrupts_trickling_frame() {
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), TOKEN.into())
        .unwrap()
        .start(|_| Response::Done)
        .unwrap();
    let stream = TcpStream::connect(server.endpoint()).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let trickler = thread::spawn(move || {
        loop {
            if writer.write_all(b"{").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    });
    thread::sleep(Duration::from_millis(100));

    let started = Instant::now();
    drop(server);
    assert!(started.elapsed() < Duration::from_secs(2));
    trickler.join().unwrap();
}
