---
title: Workshop
description: Learn autotel in thirteen parts, from println! to one trace across three services.
---

The [Rust OpenTelemetry Workshop](https://github.com/jagreehal/autotel-rust/tree/main/workshop) teaches autotel in thirteen parts, taking a bank's ATM from `println!` to one trace across three services. Every part runs against a real receiver in CI and checks what arrived.

1. [The console log wall](https://github.com/jagreehal/autotel-rust/tree/main/workshop/01-the-console-log-wall)
2. [Your first span](https://github.com/jagreehal/autotel-rust/tree/main/workshop/02-your-first-span)
3. [Attributes not messages](https://github.com/jagreehal/autotel-rust/tree/main/workshop/03-attributes-not-messages)
4. [Which work deserves a span](https://github.com/jagreehal/autotel-rust/tree/main/workshop/04-which-work-deserves-a-span)
5. [One wide event](https://github.com/jagreehal/autotel-rust/tree/main/workshop/05-one-wide-event)
6. [Logs you already have](https://github.com/jagreehal/autotel-rust/tree/main/workshop/06-logs-you-already-have)
7. [Errors that explain themselves](https://github.com/jagreehal/autotel-rust/tree/main/workshop/07-errors-that-explain-themselves)
8. [Events the business asks for](https://github.com/jagreehal/autotel-rust/tree/main/workshop/08-events-the-business-asks-for)
9. [The dashboard](https://github.com/jagreehal/autotel-rust/tree/main/workshop/09-the-dashboard)
10. [Finding the cause](https://github.com/jagreehal/autotel-rust/tree/main/workshop/10-finding-the-cause)
11. [One trace three services](https://github.com/jagreehal/autotel-rust/tree/main/workshop/11-one-trace-three-services)
12. [Facts that travel](https://github.com/jagreehal/autotel-rust/tree/main/workshop/12-facts-that-travel)
13. [A cause in three parts](https://github.com/jagreehal/autotel-rust/tree/main/workshop/13-a-cause-in-three-parts)

## Run a part

Clone the repository, start `npx autotel-devtools` to see the telemetry, then run a part by number:

```sh
cargo sol 10
```

`make workshop` checks every part against a throwaway receiver. It needs Node 22 or newer for `autotel-devtools`.
