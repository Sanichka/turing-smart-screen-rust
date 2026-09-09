// SPDX-License-Identifier: GPL-3.0-or-later
//! Simulated-display web preview. Ports `SimulatedLcdWebServer`: serves
//! `screencap.png` with an auto-refreshing index page on port 5678.
//! Pure std (`TcpListener`), one thread, SIMU mode only.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::JoinHandle;
use std::time::Duration;

pub const PORT: u16 = 5678;

const INDEX: &str = concat!(
    "HTTP/1.0 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n",
    "<img src=\"screencap.png\" id=\"myImage\" /><script>",
    "setInterval(function() {",
    "var myImageElement = document.getElementById('myImage');",
    "myImageElement.src = 'screencap.png?rand=' + Math.random();",
    "}, 250);",
    "</script>",
);

fn handle(mut stream: std::net::TcpStream) {
    let mut head = [0u8; 1024];
    let n = stream.read(&mut head).unwrap_or(0);
    let line = String::from_utf8_lossy(&head[..n]);
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    if path == "/" {
        let _ = stream.write_all(INDEX.as_bytes());
    } else if path.starts_with("/screencap.png") {
        match std::fs::read("screencap.png") {
            Ok(png) => {
                let header = format!(
                    "HTTP/1.0 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    png.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&png);
            }
            Err(_) => {
                let _ = stream.write_all(b"HTTP/1.0 404 Not Found\r\nConnection: close\r\n\r\n");
            }
        }
    } else {
        let _ = stream.write_all(b"HTTP/1.0 404 Not Found\r\nConnection: close\r\n\r\n");
    }
}

/// Serve until `stopping` is set. Non-fatal: port conflicts only warn.
pub fn spawn(stopping: Arc<AtomicBool>) -> Option<JoinHandle<()>> {
    let listener = match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("simulated display web preview unavailable (port {PORT}): {e}");
            return None;
        }
    };
    // Short accept timeout so shutdown reacts promptly.
    log::info!("simulated screen preview at http://localhost:{PORT}");
    Some(
        std::thread::Builder::new()
            .name("simu-web".into())
            .spawn(move || {
                listener
                    .set_nonblocking(true)
                    .expect("listener nonblocking");
                while !stopping.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            handle(stream);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(e) => {
                            log::debug!("preview accept failed: {e}");
                            break;
                        }
                    }
                }
                log::info!("simu-web thread exiting");
            })
            .expect("cannot spawn simu-web thread"),
    )
}
