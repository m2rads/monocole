//! Tests for the Wi-Fi data plane client.
//!
//! The framing tests are pure. The rest run against a fake device on a real
//! loopback socket — same approach as the download and chat tests, and it
//! covers the parts a pure test cannot: partial reads, frame boundaries, and
//! what happens when the device lies about how much it sent.

use super::*;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// --- framing ----------------------------------------------------------------

#[test]
fn frame_length_covers_the_type_byte() {
    let frame = encode_frame(FRAME_ECHO_REQ, b"hi");
    assert_eq!(&frame[..4], &3u32.to_be_bytes()); // 1 type + 2 payload
    assert_eq!(frame[4], FRAME_ECHO_REQ);
    assert_eq!(&frame[5..], b"hi");
}

#[test]
fn empty_payload_still_has_length_one() {
    let frame = encode_frame(FRAME_BULK_END, b"");
    assert_eq!(&frame[..4], &1u32.to_be_bytes());
    assert_eq!(frame.len(), FRAME_HEADER_LEN);
}

#[test]
fn header_decodes_type_and_payload_length() {
    let frame = encode_frame(FRAME_BULK_DATA, &[0u8; 100]);
    let header: [u8; FRAME_HEADER_LEN] = frame[..FRAME_HEADER_LEN].try_into().unwrap();
    assert_eq!(decode_header(&header).unwrap(), (FRAME_BULK_DATA, 100));
}

#[test]
fn header_rejects_zero_length() {
    // Length must count the type byte, so zero is always malformed.
    assert!(decode_header(&[0, 0, 0, 0, FRAME_ECHO_RESP]).is_err());
}

#[test]
fn header_rejects_oversized_payload() {
    // Without this bound a hostile or confused peer could make us allocate
    // 4 GB from a 5-byte header.
    let len = (MAX_PAYLOAD + 2) as u32;
    let mut header = [0u8; FRAME_HEADER_LEN];
    header[..4].copy_from_slice(&len.to_be_bytes());
    header[4] = FRAME_BULK_DATA;
    assert!(decode_header(&header).is_err());
}

#[test]
fn header_accepts_exactly_the_maximum() {
    let len = (MAX_PAYLOAD + 1) as u32;
    let mut header = [0u8; FRAME_HEADER_LEN];
    header[..4].copy_from_slice(&len.to_be_bytes());
    header[4] = FRAME_BULK_DATA;
    assert_eq!(decode_header(&header).unwrap(), (FRAME_BULK_DATA, MAX_PAYLOAD));
}

// --- fake device ------------------------------------------------------------

/// How the fake device should misbehave, so the client's error paths are
/// exercised rather than assumed.
#[derive(Clone, Copy)]
enum Behaviour {
    Correct,
    /// Reports a byte count that doesn't match what it sent.
    LiesAboutCount,
    /// Answers an echo request with the wrong frame type.
    WrongFrameType,
    /// Sends fewer bytes than asked, then ends cleanly.
    ShortTransfer,
}

async fn read_frame_from(stream: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut header = [0u8; FRAME_HEADER_LEN];
    stream.read_exact(&mut header).await.ok()?;
    let (frame_type, len) = decode_header(&header).ok()?;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.ok()?;
    Some((frame_type, payload))
}

/// Spawns a one-shot fake monocle and returns the address to point at.
async fn fake_device(behaviour: Behaviour) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();

        while let Some((frame_type, payload)) = read_frame_from(&mut stream).await {
            match frame_type {
                FRAME_ECHO_REQ => {
                    let reply_type = match behaviour {
                        Behaviour::WrongFrameType => FRAME_BULK_DATA,
                        _ => FRAME_ECHO_RESP,
                    };
                    let _ = stream.write_all(&encode_frame(reply_type, &payload)).await;
                }
                FRAME_BULK_REQ => {
                    let requested =
                        u32::from_be_bytes(payload[..4].try_into().unwrap());
                    let target = match behaviour {
                        Behaviour::ShortTransfer => requested / 2,
                        _ => requested,
                    };

                    let mut sent = 0u32;
                    while sent < target {
                        let n = (target - sent).min(1024) as usize;
                        let _ = stream
                            .write_all(&encode_frame(FRAME_BULK_DATA, &vec![7u8; n]))
                            .await;
                        sent += n as u32;
                    }

                    let claimed = match behaviour {
                        Behaviour::LiesAboutCount => sent.wrapping_add(1),
                        _ => sent,
                    };
                    let _ = stream
                        .write_all(&encode_frame(FRAME_BULK_END, &claimed.to_be_bytes()))
                        .await;
                }
                _ => break,
            }
        }
    });

    addr
}

// --- echo -------------------------------------------------------------------

#[tokio::test]
async fn echo_round_trips_a_payload() {
    let addr = fake_device(Behaviour::Correct).await;
    let result = echo_at(addr, b"minicole").await.unwrap();
    assert_eq!(result.bytes, 8);
}

#[tokio::test]
async fn echo_survives_a_payload_spanning_many_reads() {
    // 8 KB will not arrive in one TCP segment; read_exact must reassemble it.
    let addr = fake_device(Behaviour::Correct).await;
    let payload = vec![b'x'; MAX_PAYLOAD];
    let result = echo_at(addr, &payload).await.unwrap();
    assert_eq!(result.bytes, MAX_PAYLOAD);
}

#[tokio::test]
async fn echo_rejects_an_oversized_payload_before_connecting() {
    // No device involved: the guard must fire client-side.
    let unroutable = "127.0.0.1:1".parse().unwrap();
    let err = echo_at(unroutable, &vec![0u8; MAX_PAYLOAD + 1])
        .await
        .unwrap_err();
    assert!(err.contains("exceeds"), "{err}");
}

#[tokio::test]
async fn echo_rejects_an_unexpected_frame_type() {
    let addr = fake_device(Behaviour::WrongFrameType).await;
    let err = echo_at(addr, b"hi").await.unwrap_err();
    assert!(err.contains("echo reply"), "{err}");
}

#[tokio::test]
async fn echo_reports_a_refused_connection() {
    // Port 1 on loopback: nothing listens, so this fails fast rather than
    // waiting out the connect timeout.
    let err = echo_at("127.0.0.1:1".parse().unwrap(), b"hi")
        .await
        .unwrap_err();
    assert!(err.contains("connect"), "{err}");
}

// --- benchmark --------------------------------------------------------------

#[tokio::test]
async fn benchmark_counts_every_byte_and_frame() {
    let addr = fake_device(Behaviour::Correct).await;
    let result = benchmark_at(addr, 100 * 1024).await.unwrap();

    assert_eq!(result.bytes, 100 * 1024);
    assert_eq!(result.frames, 100); // 1 KB chunks from the fake device
    assert!(result.kbps > 0, "throughput should be measurable");
}

#[tokio::test]
async fn benchmark_never_divides_by_zero_on_a_fast_transfer() {
    // Loopback can finish inside a millisecond; elapsed_ms is clamped to 1.
    let addr = fake_device(Behaviour::Correct).await;
    let result = benchmark_at(addr, 1024).await.unwrap();
    assert!(result.elapsed_ms >= 1);
}

#[tokio::test]
async fn benchmark_rejects_a_device_that_miscounts() {
    let addr = fake_device(Behaviour::LiesAboutCount).await;
    let err = benchmark_at(addr, 4096).await.unwrap_err();
    assert!(err.contains("reported sending"), "{err}");
}

#[tokio::test]
async fn benchmark_rejects_a_short_transfer() {
    let addr = fake_device(Behaviour::ShortTransfer).await;
    let err = benchmark_at(addr, 4096).await.unwrap_err();
    assert!(err.contains("received"), "{err}");
}

#[tokio::test]
async fn benchmark_rejects_out_of_range_requests() {
    let addr = "127.0.0.1:1".parse().unwrap();
    assert!(benchmark_at(addr, 0).await.is_err());
    assert!(benchmark_at(addr, MAX_BENCHMARK_BYTES + 1).await.is_err());
}

// --- address handling -------------------------------------------------------

#[test]
fn device_addr_uses_the_agreed_port() {
    let addr = device_addr("192.168.4.1").unwrap();
    assert_eq!(addr.port(), MONOCLE_TCP_PORT);
    assert_eq!(addr.ip().to_string(), "192.168.4.1");
}

#[test]
fn device_addr_rejects_a_hostname() {
    // wifi_state delivers a dotted quad; anything else is a firmware bug and
    // should not silently become a DNS lookup.
    assert!(device_addr("monocle.local").is_err());
}
