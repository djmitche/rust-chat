extern crate bytes;
extern crate futures;
extern crate tokio_io;
extern crate tokio_proto;
extern crate tokio_core;
extern crate tokio_service;

use std::io;
use std::str;
use std::net::SocketAddr;
use bytes::BytesMut;
use futures::{future, Future, Stream, Sink};
use futures::sync::mpsc;
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::{Core, Handle};
use tokio_io::AsyncRead;
use tokio_io::codec::{Encoder, Decoder, Framed};
use tokio_service::Service;

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

pub struct ChatConnection {
    outgoing: mpsc::UnboundedSender<String>,
    peer: SocketAddr,
}

impl ChatConnection {
    fn new(
        socket: TcpStream,
        peer: SocketAddr,
    ) -> (ChatConnection, Box<Future<Item = (), Error = ()>>) {
        let (writer, reader) = socket.framed(LineCodec).split();

        // writer.send takes ownership of self until its future completes, so it's not something we
        // can store in a struct.  Happily, futures::sync::mpsc channels can send without a Future,
        // so we can use that as a frontend to the writer.
        let (tx, rx) = mpsc::unbounded();

        // bind the rx end of the channel to the writer, using `fold` to transfer ownership of
        // `writer` between the send operation fold's accumulator.
        let writer_fut = rx.fold(writer, |writer, msg| writer.send(msg).map_err(|_| ()));

        (
            ChatConnection {
                outgoing: tx,
                peer: peer,
            },
            Box::new(writer_fut.map(move |_| {
                println!("Connection from {} terminated", peer);
                ()
            })),
        )
    }

    fn send<S: Into<String>>(&self, msg: S) -> Result<(), mpsc::SendError<String>> {
        self.outgoing.unbounded_send(msg.into())
    }
}

// ---

pub struct ChatService {
    connections: Vec<ChatConnection>,
}

impl ChatService {
    fn new() -> ChatService {
        ChatService { connections: vec![] }
    }

    fn serve(mut self, handle: Handle, port: u16) -> Box<Future<Item = (), Error = io::Error>> {
        let address = format!("0.0.0.0:{}", port).parse().unwrap();
        let listener = TcpListener::bind(&address, &handle).unwrap();

        println!("Listening on TCP port {}", port);

        Box::new(listener.incoming().for_each(move |(socket, peer_addr)| {
            println!("New connection from {}", peer_addr);
            let (conn, fut) = ChatConnection::new(socket, peer_addr);

            self.connections.push(conn);
            handle.spawn(fut);

            self.send_all(format!("{} has joined the chat", peer_addr))
                .unwrap();
            Ok(())
        }))
    }

    fn send_all<S: Into<String>>(&self, msg: S) -> Result<(), mpsc::SendError<String>> {
        let msg = msg.into();
        for conn in &self.connections {
            conn.send(msg.clone())?;
        }
        Ok(())
    }
}


fn server() -> io::Result<()> {
    let mut core = Core::new()?;
    let svc = ChatService::new();

    let fut = svc.serve(core.handle(), 12345);
    core.run(fut)
}

fn main() {
    if let Err(e) = server() {
        println!("Server failed with {}", e);
    }
}
