//! autotel version of `tracing-grpc`'s server. Run it, then `grpc-client`.
use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};
use tonic::{Request, Response, Status, transport::Server};

#[allow(clippy::derive_partial_eq_without_eq)]
pub mod hello_world {
    tonic::include_proto!("helloworld");
}

#[derive(Debug, Default)]
pub struct MyGreeter {}

#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        // No metadata extractor and no span to build: the layer joined the caller's
        // trace and opened `helloworld.Greeter/SayHello` with the RPC attributes.
        let name = request.into_inner().name;
        std::thread::sleep(std::time::Duration::from_millis(20));
        autotel::ctx().add_event(format!("Got name: {name:?}"), Default::default());
        Ok(Response::new(HelloReply {
            message: format!("Hello {name}!"),
        }))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = autotel::init(autotel::config!().with_tracing_subscriber())?;
    Server::builder()
        .layer(autotel::tonic::layer())
        .add_service(GreeterServer::new(MyGreeter::default()))
        .serve_with_shutdown("[::1]:50052".parse()?, autotel::shutdown_signal())
        .await?;
    Ok(())
}
