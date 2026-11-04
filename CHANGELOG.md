# Changelog

## 0.2.0

- `autotel::tonic::layer()` (`tonic` feature) records gRPC SERVER spans and `rpc.server.call.duration` with the stable RPC conventions.
- The axum layer records URL, server, client and network attributes, and gains `skip_routes`, `trace_response_header`, `trust_forwarded_for` and `raw_query`. `url.query` keeps keys and redacts values.
- `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`, `OTEL_EXPORTER_OTLP_PROTOCOL` and `OTEL_{TRACES,METRICS,LOGS}_EXPORTER=none` take precedence over `AutotelConfig`. `OtlpProtocol::HttpProtobuf` adds protobuf export.
- `autotel::config!()` reads the service name and version from `Cargo.toml`. `LogFile` and `AUTOTEL_LOG_FILE_*` write rolling log files.
- `shutdown_signal()`, `current_trace_id()`, and an `init` log line naming where each signal goes.

## 0.1.0

First release.

- `init` sets up the OpenTelemetry SDK and exports traces, metrics and logs over OTLP/HTTP, or OTLP/gRPC with the `grpc` feature.
- `trace` and `trace_result` create async spans whose context follows the future, record `Err` results, and carry attributes, events and baggage.
- `span_bridge()` and `log_bridge()` send `tracing` spans and logs into the same traces.
- W3C `traceparent` and `baggage` propagation, `fetch`, and an `axum` middleware layer.
- HTTP, database, messaging, workflow and GenAI helpers record semconv attributes and duration histograms.
- `sqlx` feature: transaction spans and connection pool metrics.
- Process-local tail sampling (`AdaptiveSampler`) keeps or drops each trace's spans by error, latency, baseline rate, `force_keep()` or attribute rules.
- `redact_pii` scrubs PII from attributes, span events, log records and business events. HTTP URLs and SQL text get redacted by default.
- Wide events (`request_logger`), structured errors, business event subscribers, cohort analysis, SLOs and burn-rate alerts.
- The Rust OpenTelemetry Workshop in `workshop/`: thirteen parts from `println!` to one trace across three services.
