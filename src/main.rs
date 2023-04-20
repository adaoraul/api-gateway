// Import necessary dependencies
use hyper::service::{make_service_fn, service_fn};
use hyper::Server;
use std::net::SocketAddr;
use tracing::{error, info};
use tracing_subscriber;

// Import the modules we created
use config::load_config;
use service::handle_request;

// Declare modules
mod config;
mod service;

// Main function
#[tokio::main]
async fn main() {
    // Initialize the tracing subscriber
    tracing_subscriber::fmt::init();

    // Load the configuration file
    let config = load_config("config.toml");

    // Set the address the server will listen on
    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));

    // Log that the server is starting
    info!("Starting server on {}", addr);

    // Define the make service function which creates a new service for each incoming connection
    let make_svc = make_service_fn(move |_conn| {
        // Clone the config for use within the service
        let config = config.clone();

        // Create the service function for each incoming request
        async {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                // Clone the config and handle the request
                let config = config.clone();
                handle_request(req, config)
            }))
        }
    });

    // Bind the server to the specified address and serve requests using the make service function
    let server = Server::bind(&addr).serve(make_svc);

    // Handle any errors that occur during server execution
    if let Err(e) = server.await {
        error!("server error: {}", e);
    }
}
