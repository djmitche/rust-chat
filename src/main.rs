extern crate bytes;
extern crate futures;
extern crate tokio_io;
extern crate tokio_proto;
extern crate tokio_core;
extern crate tokio_service;

use std::io;
use std::str;
use std::net::SocketAddr;
use std::rc::Rc;
use std::cell::RefCell;
use bytes::BytesMut;
use futures::{Future, Stream, Sink};
use futures::sync::mpsc;
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::{Core, Handle};
use tokio_io::AsyncRead;
use tokio_io::codec::{Encoder, Decoder};

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
}

impl ChatConnection {
    fn new(
        socket: TcpStream,
        peer: SocketAddr,
        incoming_tx: mpsc::UnboundedSender<String>,
    ) -> (ChatConnection, Box<Future<Item = (), Error = ()>>) {
        let (writer, reader) = socket.framed(LineCodec).split();

        // writer.send takes ownership of self until its future completes, so it's not something we
        // can store in a struct.  Happily, futures::sync::mpsc channels can send without a Future,
        // so we can use that as a frontend to the writer.
        let (outgoing_tx, outgoing_rx) = mpsc::unbounded();

        // bind the rx end of the channel to the writer, using `fold` to transfer ownership of
        // `writer` between the send operation fold's accumulator.
        let writer_fut = outgoing_rx
            .fold(writer, |writer, msg| writer.send(msg).map_err(|_| ()))
            .map(|_| ());

        // similarly, bind the reader to the tx end of the channel we were given for incoming
        // messages
        let reader_fut = reader
            .and_then(move |msg| {
                incoming_tx.unbounded_send(format!("{}: {}", peer, msg)).unwrap();
                Ok(())
            })
            // uhhhhh...
            .for_each(|_| Ok(()))
            .map_err(|_| ());

        (
            ChatConnection { outgoing: outgoing_tx },
            // fut.select(fut) returns a future with a tuple for Item; map that to nothing
            Box::new(writer_fut.select(reader_fut).then(|_| Ok((()))).map(
                move |_| {
                    println!("Connection from {} terminated", peer);
                    ()
                },
            )),
        )
    }

    fn send<S: Into<String>>(&self, msg: S) -> Result<(), mpsc::SendError<String>> {
        self.outgoing.unbounded_send(msg.into())
    }
}

// ---

struct ChatServer {
    handle: Handle,
    port: u16,
    inner: Rc<RefCell<ChatServerInner>>,
}

struct ChatServerInner {
    connections: Vec<ChatConnection>,
}

impl ChatServer {
    fn new(handle: Handle, port: u16) -> ChatServer {
        ChatServer {
            handle: handle,
            port: port,
            inner: Rc::new(RefCell::new(ChatServerInner { connections: vec![] })),
        }
    }

    fn serve(self) -> Box<Future<Item = (), Error = io::Error>> {
        let address = format!("0.0.0.0:{}", self.port).parse().unwrap();
        let listener = TcpListener::bind(&address, &self.handle).unwrap();

        // all incoming messages will be delivered this channel.
        let (incoming_tx, incoming_rx) = mpsc::unbounded();

        // arrange to send to all on every incoming message
        let inner2 = self.inner.clone();
        let incoming_fut = incoming_rx
            .map(move |msg: String| for conn in inner2
                .borrow()
                .connections
                .iter()
            {
                println!("msg: {:?}", msg);
                conn.send(msg.clone()).unwrap();
            })
            .for_each(|_| Ok(()));

        println!("Listening on TCP port {}", self.port);
        let listener_fut = listener
            .incoming()
            .for_each(move |(socket, peer_addr)| {
                println!("New connection from {}", peer_addr);
                let (conn, fut) = ChatConnection::new(socket, peer_addr, incoming_tx.clone());

                self.inner.borrow_mut().connections.push(conn);
                self.handle.spawn(fut);

                incoming_tx
                    .unbounded_send(format!("{}: *joined the chat*", peer_addr))
                    .unwrap();
                Ok(())
            })
            .map_err(|_| ());

        Box::new(listener_fut.select(incoming_fut).then(|_| Ok(())))
    }
}

impl ChatServerInner {}

// ---

fn server() -> io::Result<()> {
    let mut core = Core::new()?;
    let chatserver = ChatServer::new(core.handle(), 12345);

    let fut = chatserver.serve();
    core.run(fut)
}

fn main() {
    if let Err(e) = server() {
        println!("Server failed with {}", e);
    }
}
