---
title: Configuration
description: The OTEL_* and AUTOTEL_* variables autotel reads, and how to reach a hosted backend.
---

Set the standard variables and change no code. They beat the values in `AutotelConfig`.

| Variable | Effect |
| --- | --- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Turns export on. `OTEL_EXPORTER_OTLP_{TRACES,METRICS,LOGS}_ENDPOINT` override one signal. |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `http/json` (default), `http/protobuf`, or `grpc` with the `grpc` feature. Anything else fails `init`. |
| `OTEL_EXPORTER_OTLP_HEADERS` | Credentials, as `key=value,key=value`. |
| `OTEL_SERVICE_NAME` | `service.name`. |
| `OTEL_RESOURCE_ATTRIBUTES` | Any resource attribute, such as `service.version=1.2,deployment.environment.name=prod`. |
| `OTEL_TRACES_SAMPLER` | Head sampling, such as `parentbased_traceidratio`. |
| `OTEL_{TRACES,METRICS,LOGS}_EXPORTER` | `none` silences one signal, for backends like Jaeger that take only traces. |
| `RUST_LOG` | Log filter for `.with_tracing_subscriber()`. |
| `AUTOTEL_LOG_FILE_DIR` | Turns on log files in this directory, no code needed. |
| `AUTOTEL_LOG_FILE_ROTATION` | `minutely`, `hourly`, `daily` (default) or `never`. |
| `AUTOTEL_LOG_FILE_MAX_FILES` | Files to keep: 2 or more, or 0 for all. Default 7. |

`init` logs where telemetry is going, or that it stays local, so a missing endpoint shows at startup.

## Hosted backends

For a hosted backend such as Grafana Cloud:

```sh
export OTEL_EXPORTER_OTLP_ENDPOINT=https://otlp-gateway-<region>.grafana.net/otlp
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Basic%20<base64 instance:token>"
```

## JSON logs

Use `.json_logs()` in place of `.with_tracing_subscriber()` to print JSON lines with `trace_id` and `span_id`.
