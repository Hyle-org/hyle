use s2n_quic::client::Connect;
use s2n_quic::Client;
use tracing::info;

use anyhow::Context;
use anyhow::Result;
use s2n_quic::Server;
use std::net::SocketAddr;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();
    let mut server = Server::builder()
        .with_io("0.0.0.0:3000")?
        .start()
        .context("Failed to start server")?;

    tokio::spawn(async move {
        for i in 1..10 {
            tokio::time::sleep(Duration::from_secs(1)).await;

            info!("Making a server call");
            // Call server

            let client = Client::builder()
                .with_io("127.0.0.1:0")
                .unwrap()
                .start()
                .unwrap();

            let addr: SocketAddr = "0.0.0.0:3000".parse().unwrap();
            let connect = Connect::new(addr).with_server_name("localhost");
            let mut connection = client.connect(connect).await.expect("");

            // ensure the connection doesn't time out with inactivity
            connection.keep_alive(true).unwrap();

            // open a new stream and split the receiving and sending sides
            let stream = connection.open_bidirectional_stream().await.unwrap();
            let (mut receive_stream, mut send_stream) = stream.split();

            // spawn a task that copies responses from the server to stdout
            tokio::spawn(async move {
                let mut stdout = tokio::io::stdout();
                let _ = tokio::io::copy(&mut receive_stream, &mut stdout)
                    .await
                    .unwrap();
            });

            // copy data from stdin and send it to the server
            let mut stdin = tokio::io::stdin();
            tokio::io::copy(&mut stdin, &mut send_stream).await.unwrap();

            ()
        }
    });

    while let Some(mut connection) = server.accept().await {
        // spawn a new task for the connection
        tokio::spawn(async move {
            eprintln!("Connection accepted from {:?}", connection.remote_addr());

            while let Ok(Some(mut stream)) = connection.accept_bidirectional_stream().await {
                // spawn a new task for the stream
                tokio::spawn(async move {
                    info!("Stream opened from {:?}", stream.connection().remote_addr());

                    // echo any data back to the stream
                    while let Ok(Some(data)) = stream.receive().await {
                        stream.send(data).await.expect("stream should be open");
                    }
                });
            }
        });
    }

    Ok(())
}
