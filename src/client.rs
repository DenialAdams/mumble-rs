use proto;

use byteorder::{BigEndian, WriteBytesExt};

use std;
use std::net::{IpAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::{thread, time};

use openssl;
use openssl::ssl::{HandshakeError, SslContext, SslMethod, SslStream};

use protobuf;

// Connect
const SSL_HANDSHAKE_RETRIES: u8 = 3;

// Version Exchange
const VERSION_RELEASE_PREFIX: &'static str = "mumble-rs";
const VERSION_RELEASE: Option<&'static str> = option_env!("CARGO_PKG_VERSION");
// These sizes are important, and correspond to the number of bytes sent in the Version message
const VERSION_MAJOR: u16 = 1;
const VERSION_MINOR: u8 = 3;
const VERSION_PATCH: u8 = 0;

// Ping thread
const PING_INTERVAL: u64 = 5; // (in seconds)

#[derive(Debug)]
pub enum Error {
    ConnectionError(ConnectionError),
    SendError(SendError)
} // TODO: this should impl error, display

impl From<ConnectionError> for Error {
    fn from(e: ConnectionError) -> Self {
        Error::ConnectionError(e)
    }
}

impl From<SendError> for Error {
    fn from(e: SendError) -> Self {
        Error::SendError(e)
    }
}

#[derive(Debug)]
pub enum ConnectionError {
    ExceededHandshakeRetries(&'static str),
    Ssl(openssl::ssl::Error),
    TcpStream(std::io::Error)
} // TODO: this should impl error, display, from

#[derive(Debug)]
pub enum SendError {
    MessageTooLarge(&'static str),
    Ssl(openssl::ssl::Error)
} // TODO: this should impl error, display, from

pub struct Client {
    control_channel: Mutex<SslStream<TcpStream>>
}

// TODO: auto reconnect on ZeroReturnError
// for that, perhaps a different impl?
impl Client {
    pub fn new(host: IpAddr, port: u16, username: &str, password: &str) -> Result<Arc<Client>, Error> {
        let control_channel = try!(Client::connect(host, port));
        let client = Arc::new(Client { control_channel: Mutex::new(control_channel) });
        try!(client.version_exchange());
        try!(client.authenticate(username, password));
        let ping_client = Arc::downgrade(&client.clone());
        thread::spawn(move || {
            while let Some(client) = ping_client.upgrade() {
                thread::sleep(time::Duration::from_secs(PING_INTERVAL));
                // If ping fails, either everything is crashing and burning
                // or it was just a one off issue. If it's crashing and burning the loop will end
                // and if it's a one off issue re-pinging next iteration is desired anyway.
                let _ = client.ping();
            }
        });
        Ok(client)
    }

    pub fn reconnect(&mut self, host: IpAddr, port: u16, username: &str, password: &str) -> Result<(), Error> {
        let control_channel = try!(Client::connect(host, port));
        self.control_channel = Mutex::new(control_channel);
        try!(self.version_exchange());
        try!(self.authenticate(username, password));
        Ok(())
    }

    fn connect(host: IpAddr, port: u16) -> Result<SslStream<TcpStream>, ConnectionError> {
        let mut context: SslContext;
        match SslContext::new(SslMethod::Tlsv1) {
            Ok(val) => context = val,
            Err(err) => return Err(ConnectionError::Ssl(openssl::ssl::Error::from(err)))
        }
        // TODO: This will do no cert verification. We should have an option for this.
        context.set_verify(openssl::ssl::SSL_VERIFY_NONE);
        //context.set_verify(openssl::ssl::SSL_VERIFY_PEER);
        let stream: TcpStream;
        match TcpStream::connect((host, port)) {
            Ok(val) => stream = val,
            Err(err) => return Err(ConnectionError::TcpStream(err))
        }
        match SslStream::connect(&context, stream) {
            Ok(val) => Ok(val),
            Err(err) => match err {
                HandshakeError::Failure(handshake_err) => Err(ConnectionError::Ssl(handshake_err)),
                HandshakeError::Interrupted(interrupted_stream) => {
                    let mut ssl_stream = interrupted_stream;
                    let mut tries: u8 = 1;
                    while tries < SSL_HANDSHAKE_RETRIES {
                        match ssl_stream.handshake() {
                            Ok(val) => return Ok(val),
                            Err(err) => match err {
                                HandshakeError::Failure(handshake_err) => return Err(ConnectionError::Ssl(handshake_err)),
                                HandshakeError::Interrupted(new_interrupted_stream) => {
                                    ssl_stream = new_interrupted_stream;
                                    tries += 1;
                                    continue
                                }
                            }
                        }
                    }
                    Err(ConnectionError::ExceededHandshakeRetries("Exceeded number of handshake retries"))
                }
            }
        }
    }

    fn version_exchange(&self) -> Result<(), SendError> {
        let major = (VERSION_MAJOR as u32) << 16;
        let minor = (VERSION_MINOR as u32) << 8;
        let patch = VERSION_PATCH as u32;
        let mut version_message = proto::Version::new();
        version_message.set_version(major | minor | patch);
        version_message.set_release(format!("{} {}", VERSION_RELEASE_PREFIX, VERSION_RELEASE.unwrap_or("Unknown")));
        // TODO: os and os version (some sort of cross platform uname needed)
        version_message.set_os(String::from("DenialAdams OS"));
        version_message.set_os_version(String::from("1.3.3.7"));
        self.send_message(0, version_message)
    }

    // TODO: authentication with tokens
    fn authenticate(&self, username: &str, password: &str) -> Result<(), SendError> {
        let mut auth_message = proto::Authenticate::new();
        auth_message.set_username(String::from(username));
        auth_message.set_password(String::from(password));
        // TODO: register 0 celt versions
        auth_message.set_opus(true);
        self.send_message(2, auth_message)
    }

    fn ping(&self) -> Result<(), SendError> {
        let ping_message = proto::Ping::new();
        // TODO: fill the ping with info
        self.send_message(3, ping_message)
    }

    // TODO: error handling
    fn send_message<M: protobuf::core::Message>(&self, id: u16, message: M) -> Result<(), SendError> {
        let mut packet = vec![];
        // ID - what type of message are we sending
        packet.write_u16::<BigEndian>(id).unwrap();
        let payload = message.write_to_bytes().unwrap();
        if payload.len() as u64 > u32::max_value() as u64  {
            // We can't send a message with a payload bigger than this
            // TODO: figure out what to do here
            panic!();
        }
        // The length of the payload
        packet.write_u32::<BigEndian>(payload.len() as u32).unwrap();
        // The payload itself
        packet.extend(payload);
        // Panic on poisoned mutex - this is desired.
        // https://doc.rust-lang.org/std/sync/struct.Mutex.html#poisoning
        let mut channel = self.control_channel.lock().unwrap();
        match channel.ssl_write(&*packet) {
            Err(err) => Err(SendError::Ssl(err)),
            Ok(_) => Ok(())
        }
    }
}


