//! Test utilies for Hyle.

#![deny(unused_crate_dependencies)]

pub mod conf_maker;
pub mod node_process;

// Assume that we can reuse the OS-provided port.
pub fn find_available_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    addr.port()
}
