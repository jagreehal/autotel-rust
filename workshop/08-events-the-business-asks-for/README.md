---
title: "Part 8: The questions the business asks"
description: "Answer a product question with business events, without a devtools tab."
sidebar:
  order: 8
  label: "8. The questions the business asks"
---

# Part 8: The questions the business asks

## The scene

Three questions arrived this week, and no engineer asked any of them.

1. How much cash left the estate at bridge-street yesterday?
2. Of the customers we refused on the daily limit, how many had enough money?
3. When we refuse someone, do they come back and try a smaller amount?

Part 5 answered question 2 with one filter. Questions 1 and 3 are about what
customers did. The product team asks them every week, and the person asking
should not need a login to your tracing tool to get an answer.

```sh
cargo ex 08
```

The exercise runs six withdrawals of £100, alternating bridge-street and
kings-cross. Then customer `u_001` comes back to bridge-street, asks for £250,
gets refused on the daily limit, and asks again for £150.

## Predict, before you run it

> Your spans already contain the amount, the branch and the outcome of every
> withdrawal. So what is left for a separate event to do?

## Your turn

Emit a business event for each completed and each refused withdrawal, and send
it somewhere a product team could read without asking you.

A **business event** records something that happened in the world ("cash
withdrawn", "withdrawal refused") in the terms a product team uses. `track()` in
`autotel` emits one. A **subscriber** is a destination for those events: a
file, a webhook, or an analytics tool such as Mixpanel. You add subscribers to
the config you pass to `init()`, and every `track()` call goes to each of them.

## Our solution

```sh
cargo sol 08
```

```rust
let _telemetry = autotel::init(
    AutotelConfig::new("atm")
        .devtools()
        .subscriber(FileSubscriber::new(EVENTS_FILE)?),
)?;

track(
    "atm.cash_withdrawn",
    Attributes::from([
        ("user.id".into(), json!(user.id)),
        ("atm.branch".into(), json!(branch)),
        ("withdrawal.amount_pence".into(), json!(amount_pence)),
        ("account.balance_after_pence".into(), json!(receipt.balance_pence)),
    ]),
);

let mut attributes = Attributes::from([
    ("atm.branch".into(), json!(branch)),
    ("withdrawal.amount_pence".into(), json!(amount_pence)),
    ("withdrawal.refusal_code".into(), json!(code)),
]);
if let Some(user_id) = &user_id {
    attributes.insert("user.id".into(), json!(user_id));
}
track("atm.withdrawal_refused", attributes);
```

Open `events.ndjson`, one JSON object per line:

```
{"attributes":{"account.balance_after_pence":42830,"atm.branch":"bridge-street","spanId":"1fce0b8bda5a01ec","traceId":"37ab53c68d3f7729ac6de7408e7f812a","user.id":"u_001","withdrawal.amount_pence":10000},"name":"atm.cash_withdrawn","timestamp":1792584951575,"type":"event"}
```

The run writes eight events, and the last two answer question 3 without any
help from you:

```
  atm.withdrawal_refused       £ 250.00  trace 9ce87c359c3a
  atm.cash_withdrawn           £ 150.00  trace 44b53ff47e32
```

Your trace ids will differ. `u_001` asked for too much, got refused, and came
back for less. Both events carry `user.id`, so the product team can count that
pattern even with thousands of other events in between. The solution declares
`user_id` outside the inner `async` block so the refusal event can use it; an
unknown card has no user, the variable stays `None`, and the field is left off.

Question 1 comes out of the same file. Sum `withdrawal.amount_pence` over the
`atm.cash_withdrawn` events at bridge-street: three withdrawals of £100 plus the
£150 retry, £450 in total.

### Why this solution

The ATM now sends two streams from the same withdrawal:

```mermaid
flowchart LR
  W["atm.withdraw"] -->|"spans and log.set fields"| D["devtools<br/>(engineers)"]
  W -->|"track(name, attributes)"| S["subscribers in the config"]
  S --> F["FileSubscriber<br/>events.ndjson (this workshop)"]
  S -.-> P["your own EventSubscriber:<br/>PostHog, Mixpanel, a webhook<br/>(production)"]
  F -. "traceId on every event" .-> D
```

**The event carries selected facts to another destination.** The wide record
from part 5 stays on the trace. `track()` copies the facts a product team needs
to a subscriber that team can use.

**The two streams serve different people.** A span goes to a tracing tool and
answers "why was this slow" and "what broke". A business event goes to an
analytics tool such as PostHog or Mixpanel and answers "what did customers do".
Both are a name plus key-value attributes. They differ in who reads them, how
long they are kept, and who may log in.

**Send one event per withdrawal, and skip the counters.** A shop can keep its
till receipts, or only the day's total. The total tells you the takings. Only
the receipts tell you who bought what, and whether a customer came back.
Adding numbers up before you store them is called **pre-aggregation**, and a
counter such as "cash withdrawn per branch per day" is the day's total. It
answers question 1 and nothing else. The per-withdrawal events are the receipts:
they carry `user.id`, so question 3 is a group-by, and so is next month's
question that nobody has asked yet. Chapter 1 of _Observability Engineering_
states the rule:

> The cost of pre-aggregation is the permanent destruction of future
> investigative options.

```mermaid
flowchart LR
  W["8 withdrawals"] --> C["cash total per branch<br/>bridge-street: £450, kings-cross: £300"]
  W --> E["8 events with user.id,<br/>branch, amount, outcome"]
  C --> Q1["question 1: yes"]
  C -.-> Q3x["question 3: no, the customer is gone"]
  E --> Q1b["question 1: yes"]
  E --> Q3["question 3: yes, group by user.id"]
  E --> QN["questions nobody has asked yet"]
```

Product teams already work this way. The book points out that business
intelligence tools have worked on raw events for at least 20 years. Marketing
teams run **funnels** (how many customers get through each step of a journey)
and **cohort comparisons** (how one group of customers behaves against
another). A cohort comparison is the move part 10 makes to find a cause.

**Each copy lives in its own silo.** A **silo** is a store that holds one kind
of data and cannot join it with the rest. The book calls the split of telemetry
into separate stores the "three pillars" model and counts product analytics
among the silos. Keep the wide event on the trace as the full record, send the
analytics tool only the fields a product question needs, and include the
`traceId` so anyone can join the two back up. If your **warehouse** (the
company's central analytics database) can query trace data itself, you may not
need the copy at all.

**Changing the destination means writing one small type.** The workshop uses
`FileSubscriber` so you need no API key. A subscriber is anything that
implements `autotel::EventSubscriber`, a trait with two methods, and the
`track()` calls stay the same whichever you plug in:

```rust
use autotel::{EventPayload, EventSubscriber};

struct Analytics; // holds your client and token

impl EventSubscriber for Analytics {
    fn name(&self) -> &str {
        "analytics"
    }

    fn send(&self, payload: &EventPayload) {
        // payload.name, payload.attributes, payload.timestamp:
        // hand them to your analytics client
    }
}

AutotelConfig::new("atm").devtools().subscriber(Analytics)
```

`send` is called inline, inside `track()`, on the request's own task, and it
is not `async`. `FileSubscriber` can afford that, because appending one line to
a local file is quick. A subscriber that talks to a hosted service should push
the payload onto a channel in `send` and make the network call from a
background task, or every withdrawal waits on the analytics vendor.

**Name events for what happened in the world.** Call it `atm.cash_withdrawn`,
and avoid names like `withdraw_handler_completed`. The first name survives a
rewrite of the handler. The second describes your call stack, and a product team
has to translate it each time they read it.

**The trace id links the two streams.** The solution prints one at the end:

```
trace_id = "9ce87c359c3a3e111196b5cff00fc05b"
```

`track()` reads the active span (part 2) and stamps its `traceId` and `spanId`
onto the event. That link matters because, as Chapter 19 puts it, product
analytics "merely reports what happened and to whom, nothing more." The trace
holds the why. "Refusals at bridge-street are up 30% on Tuesday" turns into a
list of trace ids, and each one opens the exact withdrawal with its timings,
its child spans and its error. Paste the printed query into devtools to try it.

**Events do not queue like spans.** Spans wait in a batch until the guard
flushes them. `FileSubscriber` writes each event to the file inside the
`track()` call, so the file is complete the moment the last withdrawal returns,
and the solution reads it back straight away, while `_telemetry` is still
alive. The spans still need the guard to drop at the end of `main`.

### One warning

If the config has no subscribers, `track()` sends the event to nobody and says
nothing: no error, no warning. Add a `track()` call to `exercise.rs` without
touching its `init()` and run it, and nothing tells you the event went
nowhere. Treat an empty destination as a configuration failure and check for
it, or a dashboard will report zero events when delivery never started.

---

Previous: [Part 7: Errors that explain themselves](../07-errors-that-explain-themselves/README.md) ·
Next: [Part 9: The dashboard](../09-the-dashboard/README.md)
