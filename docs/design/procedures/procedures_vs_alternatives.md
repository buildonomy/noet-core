---
title = "Procedures Compared: Notebooks, Workflow DAGs, and Reconcilers"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-09-14"
status = "Draft"
version = "0.1"
dependencies = ["procedure_model.md"]
---

# Procedures Compared

**Why observable, auditable, record-driven procedures rather than one of the
established alternatives.**

This is a positioning document. [`procedure_model.md`](./procedure_model.md)
specifies the model; this one argues for it by contrast with three widely used
tools that solve adjacent problems: Jupyter notebooks, Apache Airflow, and
Terraform.

The claim being defended is narrow and can be stated up front:

> **Separating intention from execution, and recording execution as immutable
> attributable observations, makes the difference between plan and reality into
> analysable data.** Each alternative below collapses one of those separations,
> and loses that data as a result.

## 1. Summary

| Aspect | Jupyter/Binder | Airflow | Terraform | noet |
|---|---|---|---|---|
| Domain | data analysis | data pipelines | infrastructure | any procedural domain |
| Computation | embedded **in** the document | Python DAG tasks | declarative config | observations, external to the document |
| State | hidden in the kernel | task status | desired vs. actual | a marking derived from records |
| Mutability | edit cells, no history | modify DAG code | detect drift, reconcile | append a record, keep the history |
| Transparency | opaque execution | task logs | plan/apply output | an attributable record per act |
| Deviation | none | retries only | drift detection | recorded, analysed, promotable |
| Learning from reality | none | none | none — it reconciles away | promotion to a revised template |
| Output | a static snapshot | completion records | a state file | an audit trail that is also a query surface |
| Modality | one kernel | one task type | one provider type | sensor, system, and human, uniformly |

## 2. vs. Jupyter — computation embedded in the document

A notebook puts the computation inside the document:

```python
data = load_csv("sample.csv")     # cell 1
result = analyze(data)            # cell 2
plot(result)                      # cell 3
```

This is genuinely good for exploration, and the cost only appears when the
notebook becomes a procedure someone else must rely on:

- **Opaque state.** The kernel holds state invisibly. Which variables exist, and
  with what values, is not answerable from the document.
- **Hidden execution order.** Cell 5 depends on cell 3, but cell 4 was re-run in
  between. Nothing records that.
- **"Trust me" reproducibility.** Papermill parameterizes and Binder
  containerizes, but execution remains a black box — you can re-run it, not
  inspect what it did.
- **No deviation tracking.** When something goes wrong, the debugging method is
  to run it again.

The root cause is a single design choice: **the document and the execution are
the same object.** There is no template to compare a run against, because the
template *is* the run.

Separating them is what buys everything else. The document declares intention;
records report what happened; the difference is computable. That is why the
comparison is worth making even though notebooks are not competitors — they are
the clearest case of the collapse the model avoids.

## 3. vs. Airflow — completion without deviation

Airflow orchestrates task DAGs and does it well: scheduling, dependency
management, retries, completion tracking.

```python
task1 = PythonOperator(task_id='load_data', ...)
task2 = PythonOperator(task_id='process_data', ...)
task1 >> task2
```

Airflow knows whether `task2` completed. It does not know *how* `task2`
executed relative to what was intended, because a task has no declared shape to
deviate from — it has a function body and an exit code.

Four differences follow:

1. **Completion versus deviation.** A binary success/failure per task cannot
   express "this ran, but took twice as long with a substituted input".
2. **No promotion path.** "We consistently skip this task" is not a statement
   Airflow can make, let alone act on.
3. **Code-centric.** A DAG is Python. Changing a procedure is a code change,
   which puts it out of reach of the people who usually own procedures.
4. **Single modality.** Tasks run code. A step that is discharged by a human
   measurement, a barcode scan, or a sensor reading is not a task
   ([`observation_model.md`](./observation_model.md)).

Consider a lab protocol. In Airflow, adding a reagent is a `BashOperator` that
succeeded. That reagent B was used because A was out of stock, and that the wait
ran twelve minutes rather than ten, are facts with nowhere to live.

**Use Airflow** for batch pipelines, ETL, and scheduled jobs. **Use this model**
where the gap between plan and execution is itself the subject.

## 4. vs. Terraform — reconciliation destroys the signal

Terraform is the closest of the three, because it explicitly compares reality to
a declared template. It declares desired state, detects drift, plans, and
reconciles.

```hcl
resource "kubernetes_deployment" "app" {
  replicas = 3
}
```

Drift detected: five replicas, because someone scaled manually. Terraform
reconciles back to three.

**That reconciliation is correct for infrastructure and wrong for procedures**,
and the difference is worth being precise about. Terraform's premise is that the
template is authoritative and divergence is error. Under that premise, erasing
the divergence is the right move.

A procedure executed by people inverts the premise. The person who scaled to
five replicas may have known something the template did not, and the reconcile
discards exactly that: *why* it was scaled, whether it was deliberate, and
whether the template should change. Repeated drift in the same direction is
evidence about the template, and a reconciler is a mechanism for destroying that
evidence one apply at a time.

| | Terraform | noet |
|---|---|---|
| Template is | authoritative | a claim about what should happen |
| Divergence is | error | evidence |
| Response | force reality to match | record, attribute, analyse, and possibly revise the template |
| Durable artifact | a state snapshot | an append-only record set |

**Use Terraform** for provisioning and declarative state management. **Use this
model** when deviations are signals rather than faults.

## 5. The Separation That Makes This Work

Two layers, not three. The distinction the alternatives collapse:

**Intention** is an authored document. Steps are nodes with stable identities,
each declaring what would discharge it. It is source, under version control,
readable and reviewable by whoever owns the process.

**Reality** is a set of immutable records, each naming an actor and a moment.
They are not stored in the document, because an observation about content is not
content — putting them in source would make every observation a commit.

What people often draw as a third layer, the "as-run record", is neither a layer
nor an object. It is a **query**: partition the records by run, fold them
against the template, and read the result
([`procedure_model.md`](./procedure_model.md) §5.1). A rendering like this —

```
Run:       run_789          Template: lab_protocol_A
Started:   10:15:00         Completed: 10:25:30
Deviations:
  step 2   duration 18 min (declared: 5-10)
  step 3   used reagent_B (declared: reagent_A)
  step 5   no discharging record
```

— is a *view* of that fold, not a stored artifact that something updated as the
run progressed.

### 5.1 Records project into the graph; they never mutate it

This is the layering constraint, and getting it backwards is the most
consequential mistake available in this area.

**A record is an assertion. A `BeliefEvent` is an instruction.** An assertion
must be interpreted before it affects anything; an instruction is applied
directly. **Assertions project into mutations, and never the reverse**
(`living_corpus.md` §4).

So a redline does not write into the compiled graph. It is appended to the
record store, and the fold — the only bridge between the two kinds — projects it
into `BeliefEvent`s applied to a **held-out** BeliefBase, an overlay maintained
separately from the compiled corpus (`living_corpus.md` §2). Reading the corpus
with annotations live means reading the compiled graph with that overlay merged
on top; reading it without means dropping the overlay. Nothing is written into
the compiled graph to make the first true, and nothing is undone to make the
second.

No channel bypasses interpretation, and none is privileged with direct write
access. The uniformity is the point: a sensor reading, an agent's observation,
and a human's redline all enter the same way, and all are equally subject to the
fold.

## 6. The Unified Observation Model

A step advances when an observation matches what it declared — regardless of who
or what observed it.

This is a single mechanism, not three integrated ones. A barcode scan, a service
health check, and a human confirming a temperature reading are the same shape:
a channel, a producer, and a pattern to match. **A prompt is therefore not a
step type** — it is an observable step whose channel happens to be a
participant.

Three things fall out rather than being built:

- **Multi-modal steps.** "An automatic reading *or* a manual entry" is an
  ordinary composition, not a special case.
- **Uniform recording.** Every observation produces the same record shape,
  distinguished by its kind.
- **Uniform extension.** A new observation source is a new channel, not a new
  step type and not a code path through the fold.

[`observation_model.md`](./observation_model.md) specifies this.

## 7. Where This Model Is the Wrong Choice

The honest boundary. Prefer something else when:

- **Execution should be forced to match the template.** That is reconciliation,
  and Terraform is better at it.
- **The work is machine-to-machine with no meaningful human deviation.** Task
  orchestration is a solved problem; Airflow solves it.
- **Exploration is the activity.** A notebook's tight loop is an advantage until
  the result must be relied upon by someone else.
- **Observation volume is high.** Telemetry and build logs do not belong in an
  annotation store sized for deliberate acts. They connect by citation as
  evidence (`living_corpus.md` §2).

The model earns its cost where procedures are executed by people, where the gap
between written and done carries information, and where someone will later need
to answer what happened and who says so.

## 8. References

- [`procedure_model.md`](./procedure_model.md) — the model this document argues for
- [`observation_model.md`](./observation_model.md) — the unified observation schema
- [`deviation_model.md`](./deviation_model.md) — recording and analysing the delta
- [`../annotation/redline_model.md`](../annotation/redline_model.md) — proposing a change to the corpus
- [`lifecycle_grammar.md`](./lifecycle_grammar.md) — the notation, pending
- [`../annotation/living_corpus.md`](../annotation/living_corpus.md) §2, §4 — the layers and the assert/mutate boundary
- [`../annotation/overlay_model.md`](../annotation/overlay_model.md) — what the projection produces
