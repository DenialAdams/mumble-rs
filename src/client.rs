use connection::{Connection, SendError};
use error;
use options::ClientOptions;

use std::net::IpAddr;
use std::sync::Arc;
use std::{thread, time};

use openssl;

// Version Exchange
const VERSION_RELEASE_PREFIX: &'static str = "mumble-rs";
const VERSION_RELEASE: Option<&'static str> = option_env!("CARGO_PKG_VERSION");
// These sizes are important, and correspond to the number of bytes sent in the Version message
const VERSION_MAJOR: u16 = 1;
const VERSION_MINOR: u8 = 3;
const VERSION_PATCH: u8 = 0;

// Ping thread
const PING_INTERVAL: u64 = 5; // (in seconds)

pub struct Client {
    connection: Arc<Connection>,
    options: ClientOptions
}

impl Client {
    pub fn new(host: IpAddr, port: u16, username: &str, password: &str) -> Result<Client, error::Error> {
        let connection = try!(Connection::new(host, port, false, false));
        let mut client = Client { connection: Arc::new(connection) };
        try!(client.auto_reconnect(&Client::version_exchange));
        try!(client.connection.authenticate(username, password));
        // Set up ping thread
        {
            let ping_connection = Arc::downgrade(&client.connection.clone());
            thread::spawn(move || {
                while let Some(connection) = ping_connection.upgrade() {
                    thread::sleep(time::Duration::from_secs(PING_INTERVAL));
                    // If ping fails, either everything is crashing and burning
                    // or it was just a one off issue. If it's crashing and burning the loop will end
                    // and if it's a temporary re-pinging next iteration is desired anyway.
                    let _ = connection.ping();
                }
            });
        }
        Ok(client)
    }

    // TODO WIP
    /*
    fn auto_reconnect(&mut self, method: &Fn(&Client) -> Result<(), SendError>) -> Result<(), SendError> {
        let mut retry_attempts: u8 = 0;
        while retry_attempts <  3 {
            match method(self) {
                Err(e) => match e {
                    SendError::Ssl(ssl_error) => match ssl_error {
                        openssl::ssl::Error::ZeroReturn => {
                            // We've been disconnected
                            // call our thing again (oh god)
                        },
                        _ => return Err(SendError::Ssl(ssl_error))
                    },
                    _ => return Err(e)
                },
                Ok(val) => return Ok(val)
            }
            retry_attempts += 1;
            // Wait a few seconds before retrying
            thread::sleep(time::Duration::from_secs(PING_INTERVAL));
        }
        // out of attempts, can't reconnect
        // TODO TEMP
        Err(SendError::MessageTooLarge("aaah"))
    } */

    fn reconnect(&mut self, host: IpAddr, port: u16, username: &str, password: &str) -> Result<(), error::Error> {
        let connection = try!(Connection::new(host, port, false, false));
        self.connection = Arc::new(connection);
        try!(self.version_exchange());
        try!(self.connection.authenticate(username, password));
        Ok(())
    }

    fn version_exchange(&self) -> Result<(), SendError> {
        let major = (VERSION_MAJOR as u32) << 16;
        let minor = (VERSION_MINOR as u32) << 8;
        let patch = VERSION_PATCH as u32;
        // TODO: os and os version (some sort of cross platform uname needed)
        self.connection.version_exchange(major | minor | patch,
                              format!("{} {}", VERSION_RELEASE_PREFIX, VERSION_RELEASE.unwrap_or("Unknown")),
                              String::from("DenialAdams OS"),
                              String::from("1.3.3.7"))
    }
}
