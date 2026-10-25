//! Receiving Google's authorization response.
//!
//! Google no longer lets an installed app show the user a code to paste. It
//! sends the browser to a loopback address instead, so grans listens on one
//! and reads the response from the request the browser makes.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// How long a connection may sit before sending its request.
///
/// Browsers open speculative connections that never carry one, and the listener
/// handles connections one at a time, so this bounds how long such a connection
/// can delay the request that does.
const READ_TIMEOUT: Duration = Duration::from_secs(3);

/// The longest request head read before giving up on a connection.
const MAX_REQUEST_HEAD: u64 = 16 * 1024;

const DONE_PAGE: &str = "<!doctype html><title>grans</title>\
    <p>Google has answered grans. You can close this tab and return to the terminal.</p>";

/// Listens on a loopback port for the browser's redirect.
pub struct RedirectListener {
    listener: TcpListener,
    read_timeout: Duration,
}

impl RedirectListener {
    /// Listen on a free port chosen by the operating system.
    pub fn bind() -> io::Result<Self> {
        Self::bind_with_read_timeout(READ_TIMEOUT)
    }

    fn bind_with_read_timeout(read_timeout: Duration) -> io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(("127.0.0.1", 0))?,
            read_timeout,
        })
    }

    fn port(&self) -> u16 {
        self.listener
            .local_addr()
            .expect("a bound listener has an address")
            .port()
    }

    /// The address Google is told to send the browser back to.
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }

    /// Wait for the request that carries Google's response, and return its
    /// target (`/?state=...&code=...`).
    ///
    /// Every other request is turned away, and a connection that fails or goes
    /// quiet is dropped, so neither can end the wait.
    pub fn wait_for_callback(self) -> io::Result<String> {
        loop {
            let (stream, _) = self.listener.accept()?;
            if let Some(target) = self.serve(stream) {
                return Ok(target);
            }
        }
    }

    /// Answer one connection, returning the target if it carried the response.
    fn serve(&self, mut stream: TcpStream) -> Option<String> {
        stream.set_read_timeout(Some(self.read_timeout)).ok()?;
        let target = read_request_target(&stream)?;

        if is_authorization_response(&target) {
            respond(&mut stream, "200 OK", DONE_PAGE);
            Some(target)
        } else {
            respond(&mut stream, "404 Not Found", "");
            None
        }
    }
}

/// Wait for Google's response, from whichever arrives first: the browser
/// reaching the listener, or a line pasted on standard input.
///
/// Pasting covers a machine with no browser of its own, where the browser on
/// another machine fails to load the loopback address but still shows it,
/// response included.
pub fn wait_for_response(listener: RedirectListener) -> io::Result<String> {
    let (sender, receiver) = mpsc::channel();

    let from_browser = sender.clone();
    thread::spawn(move || {
        let _ = from_browser.send(listener.wait_for_callback());
    });

    // Left blocked on stdin once the browser wins; it ends with the process.
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            match line {
                Ok(line) if line.trim().is_empty() => continue,
                other => {
                    let _ = sender.send(other);
                    return;
                }
            }
        }
    });

    receiver
        .recv()
        .map_err(|_| io::Error::other("stopped waiting for Google's response"))?
}

/// Read a request head and return its target, or `None` for anything that is
/// not a readable HTTP request.
fn read_request_target(stream: &TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream.take(MAX_REQUEST_HEAD));
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;

    // The rest of the head is unused, but a browser may not read the response
    // until it has finished sending it.
    let mut header = String::new();
    while reader.read_line(&mut header).ok()? > 2 {
        header.clear();
    }

    let mut parts = request_line.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some("GET"), Some(target)) => Some(target.to_string()),
        _ => None,
    }
}

fn is_authorization_response(target: &str) -> bool {
    let Some((_, query)) = target.split_once('?') else {
        return false;
    };
    url::form_urlencoded::parse(query.as_bytes()).any(|(key, _)| key == "code" || key == "error")
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{}",
        status,
        body.len(),
        body
    );
    // The browser hanging up early costs nothing: the response it carried has
    // already been read.
    let _ = stream.write_all(response.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Make one request to the listener's port and return the status line
    /// answered.
    fn request(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(stream, "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", target).unwrap();
        let mut status = String::new();
        BufReader::new(stream).read_line(&mut status).unwrap();
        status.trim_end().to_string()
    }

    fn listener() -> (RedirectListener, u16) {
        let listener =
            RedirectListener::bind_with_read_timeout(Duration::from_millis(300)).unwrap();
        let port = listener.port();
        (listener, port)
    }

    #[test]
    fn the_redirect_uri_is_a_loopback_address_on_the_bound_port() {
        let (listener, port) = listener();

        assert_eq!(
            listener.redirect_uri(),
            format!("http://127.0.0.1:{}", port)
        );
    }

    #[test]
    fn the_request_carrying_the_response_is_returned_and_answered() {
        let (listener, port) = listener();
        let browser = thread::spawn(move || request(port, "/?state=s&code=c"));

        let target = listener.wait_for_callback().unwrap();

        assert_eq!(
            (target.as_str(), browser.join().unwrap().as_str()),
            ("/?state=s&code=c", "HTTP/1.1 200 OK")
        );
    }

    /// Browsers ask for a favicon too, sometimes first.
    #[test]
    fn requests_without_a_response_are_turned_away() {
        let (listener, port) = listener();
        let browser = thread::spawn(move || {
            let favicon = request(port, "/favicon.ico");
            request(port, "/?state=s&code=c");
            favicon
        });

        let target = listener.wait_for_callback().unwrap();

        assert_eq!(
            (target.as_str(), browser.join().unwrap().as_str()),
            ("/?state=s&code=c", "HTTP/1.1 404 Not Found")
        );
    }

    /// Browsers open speculative connections they may never send on; one of
    /// those must not hold up the request that carries the response.
    #[test]
    fn a_connection_that_sends_nothing_does_not_block_the_response() {
        let (listener, port) = listener();
        let idle = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let browser = thread::spawn(move || request(port, "/?state=s&error=access_denied"));

        let target = listener.wait_for_callback().unwrap();

        drop(idle);
        browser.join().unwrap();
        assert_eq!(target, "/?state=s&error=access_denied");
    }
}
