//! The link to the client.
//!
//! Connects to loopback, where an SSH tunnel is listening. Nothing input-bearing touches
//! the LAN, and the client binds loopback on its side too, so neither end is reachable
//! from the network even briefly.

use std::io::Write;
use std::net::{Ipv4Addr, TcpStream};

use wayflow_proto::{Input, Msg};

pub struct Sink {
    stream: Option<TcpStream>,
    port: u16,
}

impl Sink {
    #[must_use]
    pub fn new(port: u16) -> Self {
        Self { stream: None, port }
    }

    /// Connect if not already connected, and announce ourselves.
    ///
    /// Deferred until the pointer first crosses rather than done at startup: the tunnel
    /// or the client may come and go, and a host that refused to start without them would
    /// be far more annoying than one that reconnects when needed.
    ///
    /// # Errors
    /// Returns the connection error, which the caller should report without aborting.
    pub fn ensure_connected(&mut self, host: &str) -> Result<(), std::io::Error> {
        if self.stream.is_some() {
            return Ok(());
        }
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port))?;
        // Input that arrives late is worthless. Nagle would batch small writes waiting
        // for more data, which is precisely the wrong trade for single keystrokes.
        stream.set_nodelay(true)?;
        self.stream = Some(stream);
        self.send(&Msg::Hello {
            proto_version: 1,
            host: host.to_owned(),
        });
        Ok(())
    }

    /// Send one message, dropping the connection if the write fails.
    ///
    /// A failed write means the client is gone; clearing the stream lets the next
    /// crossing reconnect instead of failing forever against a dead socket.
    pub fn send(&mut self, msg: &Msg) {
        let Some(stream) = &mut self.stream else {
            return;
        };
        let Ok(line) = msg.to_line() else { return };
        if stream.write_all(line.as_bytes()).is_err() {
            eprintln!("wayflow-host: client connection lost");
            self.stream = None;
        }
    }

    pub fn send_input(&mut self, input: Input) {
        self.send(&Msg::Input(input));
    }
}
