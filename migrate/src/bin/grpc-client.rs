//! autotel version of `tracing-grpc`'s client.
use hello_world::{HelloRequest, greeter_client::GreeterClient};
use tonic::transport::Channel;

#[allow(clippy::derive_partial_eq_without_eq)]
pub mod hello_world {
    tonic::include_proto!("helloworld");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber())?;
    // Every call through the channel gets a CLIENT span with the RPC attributes and
    // status, and carries `traceparent` and `baggage`: no Injector impl to write.
    let endpoint = Channel::from_static("http://[::1]:50052");
    let channel = endpoint.connect().await?;
    // `server` names the endpoint on each span, as `server.address` and `server.port`.
    let mut client = GreeterClient::new(autotel::tonic::client(channel).server(endpoint.uri()));
    let request = HelloRequest {
        name: "Tonic".into(),
    };
    let reply = client.say_hello(request).await?.into_inner();
    tracing::info!(reply = reply.message, "Got response!");
    Ok(())
}
