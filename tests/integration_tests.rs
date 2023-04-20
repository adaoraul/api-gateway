use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

// Start the server process
fn start_server() -> Child {
    Command::new("cargo")
        .args(&["run"])
        .spawn()
        .expect("Failed to start server")
}

// End-to-end test for the health-check endpoint
#[test]
fn health_check_test() {
    // Start the server
    let mut server = start_server();
    // Give the server some time to start up before sending requests
    thread::sleep(Duration::from_secs(5));

    // Send a request to the health-check endpoint
    let response = reqwest::blocking::get("http://localhost:8080/health-check")
        .expect("Failed to send request");

    // Assert that the response has a 200 status code
    assert_eq!(response.status().as_u16(), 200);

    // Terminate the server process
    server.kill().expect("Failed to kill the server process");
}
