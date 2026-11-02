---
title: "Part 9: The dashboard"
description: "Record a counter and a histogram, see where a dashboard's p99 comes from, and learn when a metric is the right tool."
sidebar:
  order: 9
  label: "9. The dashboard"
---

# Part 9: The dashboard

## The scene

Every part so far started with someone already knowing something was wrong.
In production, someone has to notice first.

Operations want a dashboard on the wall: how long withdrawals take at each
branch, and how many withdrawals each branch handles, split by outcome. They
want a year of history, and they want the cost to stay flat whether the
estate handles two hundred withdrawals a day or two million. Keeping every
span for a year fails that last test.

```sh
cargo ex 09
```

The exercise runs the same 200 withdrawals part 10 will investigate. It sends
spans and wide events, and devtools' Metrics view stays empty.

## Predict, before you run it

> A histogram of withdrawal time, split by branch, costs the same at two
> hundred withdrawals a day or two million. What question will it never be
> able to answer, however long you keep it?

## Your turn

Record two things once per withdrawal, whether it succeeded or was refused,
so refusals count too:

1. How long the withdrawal took, split by `atm.branch`.
2. One more withdrawal, split by `atm.branch` and `withdrawal.result`.

`create_histogram` and `create_counter` are in `autotel`, and so is
`KeyValue`, the type a label is written in.

## Our solution

```sh
cargo sol 09
```

Create each instrument once, in `main`, after `init`:

```rust
let _telemetry = autotel::init(AutotelConfig::new("atm").devtools())?;

let duration = create_histogram(
    "atm.withdraw.duration",
    "ms",
    "How long each withdrawal took, start to finish",
);
let withdrawals = create_counter("atm.withdrawals", "Withdrawals, by branch and result");
```

Record into them as each withdrawal finishes, whatever its outcome:

```rust
async move {
    let started_at = Instant::now();
    let result = withdraw(machine, &user.card_number, amount_pence).await;

    let outcome = if result.is_ok() {
        "dispensed"
    } else {
        "refused"
    };
    duration.record(
        started_at.elapsed().as_secs_f64() * 1000.0,
        &[KeyValue::new("atm.branch", machine.branch)],
    );
    withdrawals.add(
        1,
        &[
            KeyValue::new("atm.branch", machine.branch),
            KeyValue::new("withdrawal.result", outcome),
        ],
    );
}
```

The recording sits around the call to `withdraw`, in the batch loop, so
`withdraw` itself is unchanged and both outcomes reach the same two lines. A
metric needs no span to record into, so it does not matter that the span has
ended by then. Order does matter in one place: an instrument created before
`init` binds to a no-op meter and records into nothing, without an error.

Open devtools' Metrics view. `atm.withdraw.duration` has one series per
branch. Here is what each one holds after the run:

| Branch        | Withdrawals | 50 to 100ms | 250 to 500ms |
| ------------- | ----------- | ----------- | ------------ |
| bridge-street | 80          | 80          | 0            |
| canada-water  | 80          | 72          | 8            |
| kings-cross   | 40          | 32          | 8            |

![Devtools Metrics view for atm.withdraw.duration: 200 withdrawals in total, most between 50 and 100ms and 16 between 250 and 500ms, with 80, 40 and 80 in the bridge-street, kings-cross and canada-water series](./images/metrics.png)

The dashboard's verdict: bridge-street is fine, one withdrawal in ten at
canada-water is slow, and one in five at kings-cross is. The numbers are
correct. Hold on to the verdict until part 10.

### Why this solution

**A metric is a number you keep updating.** A span is a record: one per
withdrawal, with every fact attached. A metric keeps no records. It holds a
few running numbers and throws each withdrawal away once it has updated them.
That is why its cost stays flat: two million withdrawals update the same few
numbers as two hundred.

**A counter only goes up.** `withdrawals.add(1, ...)` bumps a running total,
the same tally counter at a door from part 3. It is a `Counter<u64>`, so the
type system will not let you subtract. The dashboard turns the total into a
rate by comparing it minute to minute: "40 withdrawals a minute at
kings-cross". Use a counter for anything you count: withdrawals, refusals,
retries.

**A histogram keeps counts per range.** A _histogram_ records a spread of
values, such as durations, without keeping them. It splits the number line
into ranges, called _buckets_, and counts how many values land in each. The
defaults are 0 to 5ms, 5 to 10ms, 10 to 25ms, and so on up to 10 seconds. The
table above is the whole of what the histogram stored: bucket counts, plus a
total count, a sum, a minimum and a maximum. It joins the 50 to 75ms and 75 to
100ms buckets, because typical withdrawals take about 75ms and land on either
side of the line from one run to the next.

```mermaid
flowchart LR
  W["kings-cross withdrawal<br/>took 76ms"] --> R{"which bucket?"}
  R --> B1["50 to 75ms"]
  R --> B2["75 to 100ms: one more"]
  R --> B3["250 to 500ms: 8"]
  B2 --> D["dashboard reads the counts<br/>and estimates p50, p99"]
```

**A dashboard's p99 is an estimate from those buckets.** Part 10 explains
percentiles in full; for now, the p99 is the time that 99 withdrawals in 100
beat. The histogram no longer knows any single withdrawal's time, so the
dashboard works it out from the counts. For kings-cross:

1. 99% of 40 withdrawals is 39.6, so the p99 sits at the 39.6th fastest.
2. 32 withdrawals fall below 250ms, and 8 fall between 250 and 500ms, so
   the 39.6th is in that bucket, 7.6 of the way through its 8.
3. Assume the 8 spread evenly across the bucket: 250 plus 7.6/8 of 250 gives
   about 488ms.

That is how Prometheus estimates it, and most dashboards do the same. The
slowest real kings-cross withdrawal took about 358ms. The estimate is off by
more than 100ms because the bucket is 250ms wide. You can set narrower
buckets when you create the histogram; the real answer stays a range.

**Labels decide every question the metric can answer.** A _label_ is an
attribute on a metric, such as `atm.branch`. Each distinct combination of
label values becomes its own series, a separate set of running numbers:

- `atm.withdraw.duration` by branch: 3 series.
- `atm.withdrawals` by branch and result: up to 3 × 2 = 6 series.
- Add `user.id` as a label: 25 times as many.
- Add `withdrawal.transaction_id`: one series per withdrawal, and the flat
  cost is gone.

That is part 3's cardinality warning with a price attached. Pick labels with
a handful of values, and pick them before the data arrives. The histogram
cannot split by a field you did not label, which answers the Predict
question: a metric can never tell you which account, customer or card sat
behind a slow withdrawal.

**Metrics leave on a timer.** Spans leave in batches (part 2). Metrics go out
on a schedule: every so often the SDK reads each instrument and sends its
numbers. `autotel::shutdown()`, or dropping the `_telemetry` guard, sends a
final reading, which is why a short program still delivers them.

### When a metric is the right tool

A metric wins when you know the question in advance and need the answer
cheaply, for a long time:

- **Machine health.** Memory in use, CPU, how long tasks wait for a tokio
  worker. A log tells you the process died because it ran out of memory. A
  metric shows memory climbing toward 90% for the hour before, and an alert on
  it can page someone before the crash.
- **Totals and rates over months.** Withdrawals per branch per day, for a
  year, at a flat cost.
- **Alerts on a known symptom.** Part 7's burn-rate alert counts bad
  withdrawals against a budget. A counter of good and bad withdrawals feeds
  it for almost nothing.

A metric loses when the question is about the individual withdrawal: which
account, which customer, which combination of fields. Part 5's wide event
answers those, and many backends can compute the same charts from wide
events when you ask. You keep both: metrics to notice cheaply, events to
explain.

### What the dashboard told you

kings-cross and canada-water look slow. The numbers are true, and they point
at two branches. An engineer reading this dashboard would book visits to two
ATM sites. Part 10 starts from this dashboard and asks what the slow
withdrawals have in common.

---

Previous: [Part 8: The questions the business asks](../08-events-the-business-asks-for/README.md) ·
Next: [Part 10: Finding the cause](../10-finding-the-cause/README.md)
