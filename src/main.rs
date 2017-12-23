extern crate bytes;
extern crate futures;
extern crate tokio_io;
extern crate tokio_proto;
extern crate tokio_core;
extern crate tokio_service;

use std::io;
use std::str;
use bytes::BytesMut;
use futures::{future, Future, Stream, Sink};
use tokio_core::net::TcpListener;
use tokio_core::reactor::{Core, Handle};
use tokio_io::AsyncRead;
use tokio_io::codec::{Encoder, Decoder};
use tokio_service::{NewService, Service};

// Ideas:
//
// - build a protocol that's not request/response; call a function on every message
// - spawn a new task on every message

// ---

pub struct LineCodec;

impl Decoder for LineCodec {
    type Item = String;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> io::Result<Option<String>> {
        if let Some(i) = buf.iter().position(|&b| b == b'\n') {
            // remove the serialized frame from the buffer.
            let line = buf.split_to(i);

            // Also remove the '\n'
            buf.split_to(1);

            // Turn this data into a UTF string and return it in a Frame.
            match str::from_utf8(&line) {
                Ok(s) => Ok(Some(s.to_string())),
                Err(_) => Err(io::Error::new(io::ErrorKind::Other, "invalid UTF-8")),
            }
        } else {
            Ok(None)
        }
    }
}

impl Encoder for LineCodec {
    type Item = String;
    type Error = io::Error;

    fn encode(&mut self, msg: String, buf: &mut BytesMut) -> io::Result<()> {
        buf.extend(msg.as_bytes());
        buf.extend(b"\n");
        Ok(())
    }
}

// ---

pub struct Echo;

impl Service for Echo {
    // These types must match the corresponding protocol types:
    type Request = String;
    type Response = String;

    // For non-streaming protocols, service errors are always io::Error
    type Error = io::Error;

    // The future for computing the response; box it for simplicity.
    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    // Produce a future for computing a response from a request.
    fn call(&self, req: Self::Request) -> Self::Future {
        println!("echoing {:?}", req);
        // In this case, the response is immediate.
        Box::new(future::ok(req))
    }
}

// ---

fn serve<S>(handle: Handle, s: S, port: u16) -> Box<Future<Item = (), Error = io::Error>>
where
    S: NewService<Request = String, Response = String, Error = io::Error> + 'static,
{
    let address = format!("0.0.0.0:{}", port).parse().unwrap();
    let listener = TcpListener::bind(&address, &handle).unwrap();

    let connections = listener.incoming();
    Box::new(connections.for_each(move |(socket, _peer_addr)| {
        let (writer, reader) = socket.framed(LineCodec).split();
        let service = s.new_service()?;

        let responses = reader.and_then(move |req| service.call(req));
        let server = writer.send_all(responses).then(|_| Ok(()));
        handle.spawn(server);

        Ok(())
    }))
}

fn server() -> io::Result<()> {
    let mut core = Core::new()?;

    let fut = serve(core.handle(), || Ok(Echo), 12345);
    let fut = fut.join(serve(core.handle(), || Ok(Echo), 12346)).map(
        |_| (),
    );

    core.run(fut)
}

fn main() {
    if let Err(e) = server() {
        println!("Server failed with {}", e);
    }
}
