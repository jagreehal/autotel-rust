---
title: gRPC
description: Trace a tonic server with one layer.
---

```toml
autotel = { version = "0.3", features = ["tonic"] }
```

```rust
tonic::transport::Server::builder()
    .layer(autotel::tonic::layer())
    .add_service(service)
    .serve(address)
    .await?;
```

Run `cargo run --example tonic --features tonic` from the repository to see one trace across a client and server.

The `grpc` feature is separate: it exports telemetry over OTLP/gRPC.

## What each call records

Each call gets a SERVER span named `package.Service/Method`, joined to the caller's trace. The span ends with the response body, because a successful call reports its status in trailers. A streaming call lasts as long as its stream, and a client that hangs up reads as `CANCELLED`. `rpc.server.call.duration` measures the same span.

Only codes that are the server's fault (`INTERNAL`, `UNAVAILABLE` and so on) mark the span as an error and set `error.type`.

## Calling a gRPC service

Wrap the channel, and every call through it gets a CLIENT span and carries `traceparent` and `baggage` to the server:

```rust,ignore
let endpoint = Channel::from_static("http://[::1]:50051");
let channel = endpoint.connect().await?;
let mut client = GreeterClient::new(autotel::tonic::client(channel).server(endpoint.uri()));
```

`.server(..)` names the endpoint as `server.address` and `server.port`. A `Channel` adds its address after the request leaves the wrapper, so the span can't see it otherwise. The span records `rpc.method` and `rpc.status_code`, stays open until the trailers arrive, and feeds `rpc.client.call.duration`, which carries the same `server.address` and `server.port`. On the client every code other than `OK` is an error, as the conventions say. An error from the channel goes through tonic's own `Status::from_error`, and the wrapper returns that `Status`, so the span records the code the caller gets: `UNAVAILABLE` for a server it can't reach, `CANCELLED` for a channel timeout. For `tower::ServiceBuilder`, use `autotel::tonic::client_layer()`.

## Method names

An unknown method becomes `rpc.method: _OTHER` on a span named `grpc`, and so does a call a router or middleware answers with plain HTTP. If auth or rate-limit layers answer before tonic routes a call, list your methods with `.methods(["package.Service/Method"])` to keep made-up paths out of span names and metric series.

## Client addresses and TLS

`client.address` works over plain TCP and TLS, including a TLS stream you accept yourself. Pair autotel with tonic's `tls-aws-lc` feature, which matches its HTTP client. For `tls-ring`, install a provider with `CryptoProvider::install_default`.

To serve tonic through an axum router, put the layer on the router and serve it with `into_make_service_with_connect_info::<SocketAddr>()`.
