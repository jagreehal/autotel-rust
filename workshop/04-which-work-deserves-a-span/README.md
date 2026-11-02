---
title: "Part 4: Which work deserves a span"
description: "Decide which of six operations get their own span, and why."
sidebar:
  order: 4
  label: "4. Which work deserves a span"
---

# Part 4: Which work deserves a span

## The scene

One withdrawal took about 360ms. Your trace holds one span, `atm.withdraw`,
lasting 360ms. The customer already told you it was slow, so the trace adds
nothing you did not know.

Six operations run inside that withdrawal. You could give each one its own
span, or none of them. You need a rule for choosing.

```sh
cargo ex 04
```

## Predict, before you run it

For each of the six, answer yes or no before reading further:

| Operation                                                         | Span? |
| ----------------------------------------------------------------- | ----- |
| `verify_pin`: checks the PIN, counts failures, can lock the card  |       |
| `find_user_by_card`: looks the customer up                        |       |
| `get_account`: looks the account up                               |       |
| `debit`: moves the money                                          |       |
| `choose_notes`: greedy arithmetic over four denominations         |       |
| `format_pence`: turns `4000` into `"£40.00"`                      |       |

## Your turn

Wrap the ones that earn it. `trace(name, work)` takes a closure that returns a
future, and a bank function called with its arguments is already a future. So
you can instrument code you did not write without editing it, right where you
call it:

```rust
let receipt = trace_result("bank.debit", |_| debit(&account, amount_pence)).await?;
```

`trace_result` is `trace` for work that returns a `Result`. `trace` hands back
whatever the future returned and cannot tell that an `Err` is a failure, so the
span would end looking healthy. `trace_result` also marks the span's status as
an error and records an `exception` event carrying the error's message when the
work returns `Err`. Use it for `debit`, `verify_pin` and `find_user_by_card`,
which return `Result`. `get_account` cannot fail, so plain `trace` covers it.

The wrapped call produces a **child span**: a span whose parent is the span
that was active when it started, here `atm.withdraw`.

## Our solution

```sh
cargo sol 04
```

The solution wraps four of the six:

```rust
let user =
    trace_result("bank.find_user_by_card", |_| find_user_by_card(card_number)).await?;
trace_result("bank.verify_pin", |_| verify_pin(card_number, pin)).await?;
let account = trace("bank.get_account", |_| get_account(&user.id)).await;
let receipt = trace_result("bank.debit", |_| debit(&account, amount_pence)).await?;
```

`choose_notes` and `format_pence` get no span. Their results land on
`atm.withdraw` as attributes instead:

```mermaid
flowchart TD
  W["atm.withdraw<br/>withdrawal.note_count<br/>withdrawal.display_amount"]
  W --> F["bank.find_user_by_card"]
  W --> V["bank.verify_pin"]
  W --> G["bank.get_account"]
  W --> D["bank.debit"]
  N["choose_notes()"] -. "result becomes an attribute" .-> W
  P["format_pence()"] -. "result becomes an attribute" .-> W
```

Open a trace in devtools and you see a **waterfall**: each span drawn as a
horizontal bar, placed by its start time and sized by its duration, with
children indented under their parent. Compare a typical withdrawal with the
`u_007` one:

```mermaid
gantt
  title Two withdrawals from the same run (ms)
  dateFormat x
  axisFormat %L
  section Typical
  bank.find_user_by_card :0, 17
  bank.verify_pin        :17, 23
  bank.get_account       :23, 35
  bank.debit             :35, 77
  section Slow
  bank.find_user_by_card :0, 17
  bank.verify_pin        :17, 23
  bank.get_account       :23, 35
  bank.debit             :crit, 35, 357
```

The slow withdrawal spends 322 of its 358ms inside `bank.debit`. With one span
you had a total. With five you can point at the step that ate the time.

![Waterfall of a slow withdrawal: bank.debit takes 321ms of the 354ms atm.withdraw span](./images/waterfall.png)

### Why this solution

_Observability Engineering_ (Chapter 7) gives two questions that guide the
choice:

> 1. Is it "interesting"? Does the work meaningfully impact performance (e.g.,
>    latency or failures) for the overall request?
> 2. Is it "aggregable"? If you group this span by name and/or attributes, will
>    it produce useful trends and comparisons?

In plain terms: does this step change how long a withdrawal takes or whether it
succeeds? And would you ever compare this step across many withdrawals, such as
the average `bank.debit` time per branch? Run the six through them:

| Operation           | Interesting?                            | Aggregable?                            | Span    |
| ------------------- | --------------------------------------- | -------------------------------------- | ------- |
| `verify_pin`        | Yes: can fail, and failure locks a card | Yes: group by outcome                  | **Yes** |
| `find_user_by_card` | Yes: a lookup that can miss             | Yes: group by hit or miss              | **Yes** |
| `get_account`       | Yes: a lookup with variable latency     | Yes: group by account attributes       | **Yes** |
| `debit`             | Yes: moves the money, and can be slow   | Yes: group by account, amount, outcome | **Yes** |
| `choose_notes`      | No: sub-millisecond, deterministic      | No: nothing to group by                | No      |
| `format_pence`      | No: it is a `format!` call              | No                                     | No      |

`atm.withdraw` passes for a different reason. The book separates orchestration
code, a function that only calls other functions in order and takes as long as
they do, from a business transaction with an outcome of its own. The first
earns no span; the second does. A withdrawal ends
dispensed or refused, and you will group by that outcome.

**Too many spans cost storage and attention.** A trace with a hundred
two-millisecond spans buries the boundaries you care about among implementation
details:

> If your trace has a hundred two-millisecond spans, you probably have too many.
> Roll them up: combine them into a single span, or represent the details as an
> attribute.

To find which function inside a step burns the time, the book points you to a
profiler, a tool that samples which function the CPU is running, rather than
more spans.

The solution takes the second option for `choose_notes` and `format_pence`. You
keep their results without timing them:

```rust
let notes = choose_notes(amount_pence);
ctx()
    .set_attribute("withdrawal.note_count", notes.len())
    .set_attribute("withdrawal.display_amount", format_pence(amount_pence));
```

When a fact is worth recording but the work behind it is not worth timing, put
the fact on the span that already exists.

**Too few spans leave you with the 360ms you cannot explain**, the problem you
started this part with. You fix the two failure modes in opposite directions.

**A span explains one request; an attribute explains all of them.** The
waterfall shows where one withdrawal spent its time. Ask "which branch has the
slowest debits?" and you have to join two records: `atm.branch` lives on
`atm.withdraw` and the duration lives on `bank.debit`, so for each debit you
look up its parent to find the branch, then match the two. The book names
wrapping everything in its own span as the most common failure mode among
engineers new to tracing (Chapter 6). Its fix is to copy a few key timings,
such as a `debit.duration_ms`, onto the parent as attributes, so one record
answers the question. Part 5 builds that parent record.

### Automatic nesting

You never told `bank.debit` that `atm.withdraw` is its parent. When a span
starts, it reads the active span from the context, the badge pinned to the
future from part 2, and stores that span's ID as its parent ID. The bank's
future runs inside the `atm.withdraw` future, so it wears the same badge.
Part 2 introduced the mechanism. Part 11 shows it breaking when the call
crosses a network.

### Which functions to wrap in your own code

Wrap the places where your process hands work to something else and waits: an
outgoing HTTP call, a database query, a cache lookup, a queue publish or
consume. Wrap each incoming request handler too. Then add a span for the
business operation that names the request's purpose, such as `atm.withdraw` or
`refund.issue`. You will group by that name when someone asks how the product is
doing.

In a real service, automatic instrumentation creates those spans for you: HTTP
clients, servers and database drivers that emit `tracing` spans, turned into
OpenTelemetry spans by `tracing-opentelemetry`, as part 2 described. It errs
toward too many. Review what it produces and switch off the spans you never
query. Spend your own spans on the business operations it cannot see.

Draw span boundaries around components and services, even where they cross team
ownership, as _Observability Engineering_ recommends. An incident follows the
calls your code makes, and those ignore your organisation chart.

---

Previous: [Part 3: Attributes, not messages](../03-attributes-not-messages/README.md) ·
Next: [Part 5: One wide event per request](../05-one-wide-event/README.md)
