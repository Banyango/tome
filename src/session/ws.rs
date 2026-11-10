//! A minimal blocking WebSocket client (RFC 6455) over a local TCP stream:
//! text messages only, no extensions. Enough to talk to Bulb's daemon
//! without a dependency.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub struct Ws {
    stream: TcpStream,
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

impl Ws {
    /// Open `ws://127.0.0.1:<port><path>`.
    pub fn connect(port: u16, path: &str, timeout: Duration) -> io::Result<Ws> {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        // The key only has to be 16 bytes in base64; nothing here is secret.
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n"
        )?;
        // Read the response head a byte at a time so no frame bytes are lost.
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte)?;
            head.push(byte[0]);
            if head.len() > 16 * 1024 {
                return Err(bad("oversized handshake response"));
            }
        }
        let status = String::from_utf8_lossy(&head);
        let first = status.lines().next().unwrap_or_default();
        if first.split_whitespace().nth(1) != Some("101") {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("handshake refused: {first}"),
            ));
        }
        Ok(Ws { stream })
    }

    pub fn send_text(&mut self, text: &str) -> io::Result<()> {
        self.send_frame(0x1, text.as_bytes())
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        let mut frame = vec![0x80 | opcode];
        let len = payload.len();
        match len {
            0..=125 => frame.push(0x80 | len as u8),
            126..=0xFFFF => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(len as u16).to_be_bytes());
            }
            _ => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(len as u64).to_be_bytes());
            }
        }
        // Clients must mask; the key needn't be unpredictable here.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0x5a5a_5a5a);
        let mask = nanos.to_be_bytes();
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.stream.write_all(&frame)
    }

    /// The next whole text (or binary) message, answering pings on the way.
    pub fn recv_text(&mut self) -> io::Result<String> {
        let mut message = Vec::new();
        loop {
            let mut h = [0u8; 2];
            self.stream.read_exact(&mut h)?;
            let (fin, opcode, masked) = (h[0] & 0x80 != 0, h[0] & 0x0F, h[1] & 0x80 != 0);
            let len = match h[1] & 0x7F {
                126 => {
                    let mut b = [0u8; 2];
                    self.stream.read_exact(&mut b)?;
                    u64::from(u16::from_be_bytes(b))
                }
                127 => {
                    let mut b = [0u8; 8];
                    self.stream.read_exact(&mut b)?;
                    u64::from_be_bytes(b)
                }
                n => u64::from(n),
            };
            if len > 128 * 1024 * 1024 {
                return Err(bad("oversized frame"));
            }
            let mut mask = [0u8; 4];
            if masked {
                self.stream.read_exact(&mut mask)?;
            }
            let mut payload = vec![0u8; len as usize];
            self.stream.read_exact(&mut payload)?;
            if masked {
                payload
                    .iter_mut()
                    .enumerate()
                    .for_each(|(i, b)| *b ^= mask[i % 4]);
            }
            match opcode {
                0x0..=0x2 => {
                    message.extend_from_slice(&payload);
                    if fin {
                        return String::from_utf8(message).map_err(|_| bad("message isn't UTF-8"));
                    }
                }
                0x8 => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "closed")),
                0x9 => self.send_frame(0xA, &payload)?,
                _ => {}
            }
        }
    }
}

impl Drop for Ws {
    fn drop(&mut self) {
        let _ = self.send_frame(0x8, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Reads one masked client frame off `s`, returning its payload.
    pub fn read_client_frame(s: &mut TcpStream) -> Vec<u8> {
        let mut h = [0u8; 2];
        s.read_exact(&mut h).unwrap();
        assert_eq!(h[1] & 0x80, 0x80, "client frames are masked");
        let len = match h[1] & 0x7F {
            126 => {
                let mut b = [0u8; 2];
                s.read_exact(&mut b).unwrap();
                usize::from(u16::from_be_bytes(b))
            }
            n => usize::from(n),
        };
        let mut mask = [0u8; 4];
        s.read_exact(&mut mask).unwrap();
        let mut p = vec![0u8; len];
        s.read_exact(&mut p).unwrap();
        p.iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b ^= mask[i % 4]);
        p
    }

    #[test]
    fn round_trips_a_message_split_across_frames() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut b).unwrap();
                head.push(b[0]);
            }
            assert!(String::from_utf8_lossy(&head).starts_with("GET /?token=t HTTP/1.1"));
            s.write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n")
                .unwrap();
            let got = read_client_frame(&mut s);
            assert_eq!(got.len(), 300);
            // A ping, then "hel" + "lo" as a fragmented message.
            s.write_all(&[0x89, 0x01, b'x']).unwrap();
            s.write_all(&[0x01, 0x03, b'h', b'e', b'l']).unwrap();
            s.write_all(&[0x80, 0x02, b'l', b'o']).unwrap();
            let pong = {
                let mut h = [0u8; 2];
                s.read_exact(&mut h).unwrap();
                h[0]
            };
            assert_eq!(pong, 0x8A);
        });
        let mut ws = Ws::connect(port, "/?token=t", Duration::from_secs(3)).unwrap();
        ws.send_text(&"a".repeat(300)).unwrap();
        assert_eq!(ws.recv_text().unwrap(), "hello");
        server.join().unwrap();
    }

    #[test]
    fn a_refused_handshake_is_an_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 512];
            let _ = s.read(&mut buf);
            s.write_all(b"HTTP/1.1 401 Unauthorized\r\n\r\n").unwrap();
        });
        assert!(Ws::connect(port, "/", Duration::from_secs(3)).is_err());
    }
}
