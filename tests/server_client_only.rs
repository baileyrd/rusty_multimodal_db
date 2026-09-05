//! `ECO-FR-003` (ADR-0043): the `client` feature alone — `cargo test
//! --features client` — compiles no `serve`, no `dispatch`, no
//! `ConnectionStore`, no domain adapter, and still gives a consumer
//! everything needed to *talk* to a server: the framing, the protocol
//! shapes, and `SchemaDrivenClient`. This target's `required-features =
//! ["client"]` is what makes CI prove that; the body proves the pieces
//! work with no server compiled and nothing listening.

use rusty_multimodal_db::server::client::{ConnectOptions, SchemaDrivenClient};
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::protocol::{Request, Response, PROTOCOL_VERSION};
use std::net::TcpListener;

/// The two halves of the wire a client needs — framing and the codec —
/// round-trip through an in-memory buffer with exactly `SERVER-002`'s
/// bytes: a 4-byte little-endian length, then the payload.
#[test]
fn framing_and_protocol_round_trip_with_no_server_compiled() {
    let mut buf: Vec<u8> = Vec::new();
    write_message(
        &mut buf,
        &Request::Hello {
            protocol_version: PROTOCOL_VERSION,
        },
    )
    .unwrap();
    assert_eq!(
        buf,
        vec![0x08, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00],
        "SERVER-002 §4's worked example: Hello {{ 12 }} is 12 bytes on the wire"
    );
    let back: Request = read_message(&mut &buf[..]).unwrap();
    assert!(matches!(
        back,
        Request::Hello {
            protocol_version: PROTOCOL_VERSION
        }
    ));

    let mut buf: Vec<u8> = Vec::new();
    write_message(&mut buf, &Response::Ok).unwrap();
    assert_eq!(buf, vec![0x04, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00]);
    let back: Response = read_message(&mut &buf[..]).unwrap();
    assert_eq!(back, Response::Ok);
}

/// The client library exists and behaves under `client` alone: a connect
/// to a port nothing listens on is an error, not a compile failure or a
/// hang.
#[test]
fn the_client_library_is_usable_without_the_server_half() {
    // Bind then drop, so the port is known free.
    let addr = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let result = SchemaDrivenClient::connect_with(addr, ConnectOptions::default());
    assert!(result.is_err(), "nothing is listening on {addr}");
}
