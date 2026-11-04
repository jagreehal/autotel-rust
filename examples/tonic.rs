//! `cargo run --example tonic --features tonic`: a gRPC server and one call to it, in one trace.
use autotel::{inject_trace_context, trace};
use tonic::{
    metadata::MetadataMap,
    transport::{Channel, Server},
};
use tonic_health::pb::{HealthCheckRequest, health_client::HealthClient};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber())?;

    let (reporter, health) = tonic_health::server::health_reporter();
    reporter
        .set_service_status("orders", tonic_health::ServingStatus::Serving)
        .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(
        Server::builder()
            .layer(autotel::tonic::layer())
            .add_service(health)
            .serve_with_incoming(incoming),
    );

    let channel = Channel::from_shared(format!("http://{address}"))?
        .connect()
        .await?;
    let id = trace("check-orders", |ctx| async move {
        let mut request = tonic::Request::new(HealthCheckRequest {
            service: "orders".into(),
        });
        *request.metadata_mut() = MetadataMap::from_headers(inject_trace_context());
        let reply = HealthClient::new(channel).check(request).await?;
        println!("health: {:?}", reply.into_inner().status());
        Ok::<_, tonic::Status>(ctx.trace_id())
    })
    .await?;
    println!("trace_id={id}");
    Ok(())
}
