use crate::error::{Error, Result};
use snow::{Builder, HandshakeState, TransportState};
use std::io::{Read, Write};

/// Mutual PSK handshake. Both peers share a 32-byte key. There is no
/// certificate authority and no static identity key. `PROLOGUE` binds the
/// transcript to this protocol version so a different app fails closed.
pub const PATTERN: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";
pub const PROLOGUE: &[u8] = b"photosync-v1";

/// snow refuses messages at 65535 bytes. Stay under that, including the tag.
pub const MAX_PLAINTEXT: usize = 60_000;
const MAX_HANDSHAKE: usize = 512;
const MAX_CIPHERTEXT: usize = 65_535;

pub struct SecureConn<S> {
    stream: S,
    transport: TransportState,
}

impl<S: Read + Write> SecureConn<S> {
    pub fn handshake_initiator(stream: S, psk: &[u8; 32]) -> Result<Self> {
        let mut state = handshake(psk, true)?;
        let mut stream = stream;
        let msg1 = write_handshake(&mut state)?;
        write_handshake_frame(&mut stream, &msg1)?;
        let msg2 = read_handshake_frame(&mut stream)?;
        read_handshake(&mut state, &msg2)?;
        let transport = state.into_transport_mode().map_err(crypto_err)?;
        Ok(Self { stream, transport })
    }

    pub fn handshake_responder(stream: S, psk: &[u8; 32]) -> Result<Self> {
        let mut state = handshake(psk, false)?;
        let mut stream = stream;
        let msg1 = read_handshake_frame(&mut stream)?;
        read_handshake(&mut state, &msg1)?;
        let msg2 = write_handshake(&mut state)?;
        write_handshake_frame(&mut stream, &msg2)?;
        let transport = state.into_transport_mode().map_err(crypto_err)?;
        Ok(Self { stream, transport })
    }

    pub fn send(&mut self, plain: &[u8]) -> Result<()> {
        if plain.len() > MAX_PLAINTEXT {
            return Err(Error::Protocol(format!(
                "plaintext of {} bytes exceeds {MAX_PLAINTEXT}",
                plain.len()
            )));
        }
        let mut buf = vec![0u8; plain.len() + 64];
        let n = self
            .transport
            .write_message(plain, &mut buf)
            .map_err(crypto_err)?;
        let len = u32::try_from(n).map_err(|_| Error::Protocol("ciphertext too large".into()))?;
        self.stream.write_all(&len.to_be_bytes())?;
        self.stream.write_all(&buf[..n])?;
        self.stream.flush()?;
        Ok(())
    }

    pub fn recv(&mut self) -> Result<Vec<u8>> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).map_err(closed)?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len == 0 || len > MAX_CIPHERTEXT {
            return Err(Error::Protocol(format!("bad frame length {len}")));
        }
        let mut cipher = vec![0u8; len];
        self.stream.read_exact(&mut cipher).map_err(closed)?;
        let mut plain = vec![0u8; len];
        let n = self
            .transport
            .read_message(&cipher, &mut plain)
            .map_err(crypto_err)?;
        plain.truncate(n);
        Ok(plain)
    }
}

fn handshake(psk: &[u8; 32], initiator: bool) -> Result<HandshakeState> {
    let params = PATTERN
        .parse::<snow::params::NoiseParams>()
        .map_err(crypto_err)?;
    let builder = Builder::new(params).prologue(PROLOGUE).psk(0, psk);
    let state = if initiator {
        builder.build_initiator()
    } else {
        builder.build_responder()
    };
    state.map_err(crypto_err)
}

fn write_handshake(state: &mut HandshakeState) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; MAX_HANDSHAKE];
    let n = state.write_message(&[], &mut buf).map_err(crypto_err)?;
    buf.truncate(n);
    if buf.is_empty() {
        return Err(Error::Crypto("empty handshake message".into()));
    }
    Ok(buf)
}

fn read_handshake(state: &mut HandshakeState, message: &[u8]) -> Result<()> {
    let mut buf = vec![0u8; message.len().max(1)];
    let n = state.read_message(message, &mut buf).map_err(crypto_err)?;
    if n != 0 {
        return Err(Error::Protocol("unexpected handshake payload".into()));
    }
    Ok(())
}

fn write_handshake_frame(stream: &mut impl Write, message: &[u8]) -> Result<()> {
    if message.is_empty() || message.len() > MAX_HANDSHAKE {
        return Err(Error::Protocol("handshake message size".into()));
    }
    let len = u16::try_from(message.len()).expect("handshake fits in u16");
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(message)?;
    stream.flush()?;
    Ok(())
}

fn read_handshake_frame(stream: &mut impl Read) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).map_err(closed)?;
    let len = u16::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_HANDSHAKE {
        return Err(Error::Protocol(format!("bad handshake length {len}")));
    }
    let mut message = vec![0u8; len];
    stream.read_exact(&mut message).map_err(closed)?;
    Ok(message)
}

fn closed(err: std::io::Error) -> Error {
    if matches!(
        err.kind(),
        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
    ) {
        Error::Protocol("connection closed".into())
    } else {
        Error::Io(err)
    }
}

fn crypto_err(err: impl std::fmt::Display) -> Error {
    Error::Crypto(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = thread::spawn(move || TcpStream::connect(addr).unwrap());
        let (server, _) = listener.accept().unwrap();
        let client = client.join().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        server
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        (server, client)
    }

    #[test]
    fn handshake_round_trips_payloads() {
        let psk = [9u8; 32];
        let (server, client) = pair();
        let responder = thread::spawn(move || {
            let mut conn = SecureConn::handshake_responder(server, &psk).unwrap();
            assert_eq!(conn.recv().unwrap(), b"hello");
            conn.send(b"world").unwrap();
        });
        let mut conn = SecureConn::handshake_initiator(client, &psk).unwrap();
        conn.send(b"hello").unwrap();
        assert_eq!(conn.recv().unwrap(), b"world");
        responder.join().unwrap();
    }

    #[test]
    fn wrong_psk_fails_the_handshake() {
        let (server, client) = pair();
        let responder = thread::spawn(move || {
            let result = SecureConn::handshake_responder(server, &[2u8; 32]);
            assert!(result.is_err());
        });
        let result = SecureConn::handshake_initiator(client, &[3u8; 32]);
        assert!(result.is_err());
        responder.join().unwrap();
    }
}
