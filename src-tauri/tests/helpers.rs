//! Helpers shared across the per-domain test files. Wired into the crate
//! from lib.rs as `crate::test_helpers` (test builds only).

/// Serves requests with `handler` on a random localhost port for the
/// lifetime of the process.
pub fn spawn_server(handler: impl Fn(tiny_http::Request) + Send + 'static) -> u16 {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            handler(request);
        }
    });
    port
}
