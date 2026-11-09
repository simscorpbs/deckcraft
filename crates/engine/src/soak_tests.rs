//! Editor soak: long random command sequences over a big deck, with invariants checked after
//! every step: no panic, a valid document, an in-range selection, a bounded undo stack, undo then
//! redo restoring the exact document, and periodic save/reopen in both formats.
//!
//! Seeded and deterministic. A failure prints the seed and the last commands so it replays.
//! `DECKCRAFT_SOAK_STEPS=50000 cargo test -p deckcraft-engine soak -- --ignored` runs a long soak.

use std::collections::VecDeque;

use serde_json::{Value, json};

use crate::{EngineError, Session, command_specs, sample};

/// Commands that touch the file system, other windows or real time; the soak drives the rest.
fn skipped(id: &str) -> bool {
    matches!(id, "file.close" | "file.open" | "file.save" | "file.saveAs" | "file.export" | "file.saveTemplate" | "file.openBytes" | "file.new")
        || id.starts_with("window.")
        || id.starts_with("show.")
        || id.starts_with("media.")
}

/// xorshift64*: tiny, deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    fn f(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (self.next() >> 11) as f64 / (1u64 << 53) as f64 * (hi - lo)
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> Option<&'a T> {
        v.get(self.below(v.len()))
    }
}

fn shape_ids(s: &Session) -> Vec<u64> {
    s.active().map(|d| d.shapes().iter().map(|x| u64::from(x.id.0)).collect()).unwrap_or_default()
}

/// Plausible parameters built from the live document, plus the odd hostile value.
fn params(r: &mut Rng, s: &Session) -> Value {
    let ids = shape_ids(s);
    let id = r.pick(&ids).copied().unwrap_or(1);
    let slides = s.active().map(|d| d.doc.slides.len()).unwrap_or(1);
    let index = r.below(slides + 2);
    match r.below(10) {
        0 => json!({}),
        1 => json!({"ids": [id]}),
        2 => json!({"id": id, "text": "Soak ✓ שלום 漢字", "name": "S"}),
        3 => json!({"index": index, "to": r.below(slides + 1)}),
        4 => json!({"preset": "rect", "rect": [r.f(0.0, 600.0), r.f(0.0, 400.0), r.f(1.0, 300.0), r.f(1.0, 200.0)]}),
        5 => json!({"ids": [id], "color": "#3366CC", "size": r.f(1.0, 96.0), "value": r.f(-10.0, 10.0)}),
        6 => json!({"ids": ids.iter().take(3).collect::<Vec<_>>()}),
        7 => json!({"effect": "fade", "start": "afterPrevious", "duration": r.below(5000), "delay": r.below(2000)}),
        8 => json!({"layout": "titleAndContent", "title": "T", "body": "B"}),
        _ => json!({"index": u64::MAX, "ids": [u64::MAX], "id": -1, "size": 1e308, "rect": [f64::MAX, 0, -1, 0]}),
    }
}

fn doc(s: &Session) -> Option<deckcraft_model::Presentation> {
    s.active().map(|d| (*d.doc).clone())
}

fn check(s: &Session, seed: u64, step: usize, log: &VecDeque<String>) {
    let ctx = || format!("seed {seed} step {step}; last commands:\n{}", log.iter().cloned().collect::<Vec<_>>().join("\n"));
    let Some(d) = s.active() else { panic!("no active document; {}", ctx()) };
    let problems = d.doc.validate();
    assert!(problems.is_empty(), "{problems:?}; {}", ctx());
    assert!(d.selection.slide < d.doc.slides.len().max(1), "selection slide {} of {}; {}", d.selection.slide, d.doc.slides.len(), ctx());
    assert!(d.history.undo.len() <= d.history.limit.max(1), "undo stack {} over limit; {}", d.history.undo.len(), ctx());
}

fn soak(seed: u64, steps: usize) {
    let mut s = Session::new();
    sample::open_sample(&mut s).unwrap();
    // A big deck: the sample several times over.
    for _ in 0..3 {
        let n = s.doc().unwrap().doc.slides.len();
        for i in 0..n {
            let _ = s.execute("slide.duplicate", &json!({"index": i}));
        }
    }
    let ids: Vec<&'static str> = command_specs().iter().map(|c| c.id).filter(|id| !skipped(id)).collect();
    let mut r = Rng(seed | 1);
    let mut log: VecDeque<String> = VecDeque::new();
    let mut edits = 0usize;
    for step in 0..steps {
        let id = *r.pick(&ids).unwrap();
        let p = params(&mut r, &s);
        log.push_back(format!("{id} {p}"));
        if log.len() > 20 {
            log.pop_front();
        }
        let before = doc(&s);
        let undoable = command_specs().iter().any(|c| c.id == id && c.undoable);
        let res = s.execute(id, &p);
        if let Err(EngineError::Internal(..)) = &res {
            panic!("command panicked (caught by the guard): {res:?}; seed {seed} step {step}; {id} {p}");
        }
        let ok = res.is_ok();
        check(&s, seed, step, &log);
        // Undo then redo of a real edit gets the same document back.
        let after = doc(&s);
        if before != after {
            edits += 1;
        }
        if ok && undoable && before != after && r.below(4) == 0 {
            s.execute("edit.undo", &json!({})).unwrap_or_else(|e| panic!("undo failed: {e}; seed {seed} step {step} {id} {p}"));
            assert!(doc(&s) == before, "undo didn't restore; seed {seed} step {step} {id} {p}");
            s.execute("edit.redo", &json!({})).unwrap_or_else(|e| panic!("redo failed: {e}; seed {seed} step {step} {id} {p}"));
            assert!(doc(&s) == after, "redo didn't reapply; seed {seed} step {step} {id} {p}");
        }
        // Keep the deck from vanishing or exploding.
        let n = s.doc().map(|d| d.doc.slides.len()).unwrap_or(0);
        if n == 0 || s.documents().len() != 1 {
            panic!("documents {} slides {n}; seed {seed} step {step}; {log:?}", s.documents().len());
        }
        if step % 100 == 99 {
            for format in ["deckcraft", "pptx"] {
                let data =
                    s.execute("file.saveBytes", &json!({"format": format})).unwrap_or_else(|e| panic!("save {format}: {e}; seed {seed} step {step}"));
                let b64 = data["data"].as_str().unwrap_or_default().to_string();
                s.execute("file.openBytes", &json!({"name": format!("soak.{format}"), "data": b64}))
                    .unwrap_or_else(|e| panic!("reopen {format}: {e}; seed {seed} step {step}; {log:?}"));
                let back = s.doc().unwrap().doc.slides.len();
                assert_eq!(back, n, "{format} reopen slide count; seed {seed} step {step}");
                assert!(s.doc().unwrap().doc.validate().is_empty(), "{format} reopen invalid; seed {seed} step {step}");
                let last = s.documents().len() - 1;
                s.close_document(last);
                s.set_active(0);
            }
        }
    }
    // The soak must really edit, not just bounce off disabled commands.
    assert!(edits * 10 >= steps, "only {edits} of {steps} steps changed the document; seed {seed}");
    eprintln!("soak seed {seed}: {edits} of {steps} steps edited");
}

fn steps(default: usize) -> usize {
    std::env::var("DECKCRAFT_SOAK_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[test]
fn soak_random_edits_keep_the_document_valid() {
    for seed in [1, 0xDEC0_C2AF, 42, 7_777_777] {
        soak(seed, steps(400));
    }
}

#[test]
#[ignore = "long soak; set DECKCRAFT_SOAK_STEPS"]
fn soak_long() {
    let seed = std::env::var("DECKCRAFT_SOAK_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0x5EED);
    soak(seed, steps(20_000));
}

#[test]
fn undo_stack_stays_bounded_and_drains() {
    let mut s = Session::with_new();
    let limit = s.doc().unwrap().history.limit;
    for i in 0..limit + 50 {
        s.execute("shape.insert", &json!({"preset": "rect", "rect": [i % 500, 10, 20, 20]})).unwrap();
    }
    assert_eq!(s.doc().unwrap().history.undo.len(), limit);
    let mut undone = 0;
    while s.execute("edit.undo", &json!({})).is_ok() {
        undone += 1;
        assert!(undone <= limit, "undo ran past the stack");
    }
    assert_eq!(undone, limit);
    assert!(s.doc().unwrap().doc.validate().is_empty());
}

/// Hostile values for one parameter: empty, huge, negative, out of range, wrong type, deeply nested.
/// (JSON cannot carry NaN or infinity; they arrive as `null`, which is in the list.)
fn hostile_values() -> Vec<Value> {
    let mut nested = json!(0);
    for _ in 0..40 {
        nested = json!([nested, {"a": 1}]);
    }
    vec![
        Value::Null,
        json!(""),
        json!("\u{0}"),
        json!("x".repeat(100_000)),
        json!("../../../etc/passwd\u{0}"),
        json!("\u{202e}\u{200d}\u{fffd}𝒳"),
        json!(0),
        json!(-1),
        json!(-1e308),
        json!(1e308),
        json!(f64::MIN_POSITIVE),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(0.5),
        json!(true),
        json!([]),
        json!([null, null, null, null]),
        json!([-1e308, 1e308, 0, -1]),
        json!([u64::MAX, u64::MAX]),
        json!(vec![1; 5000]),
        json!({}),
        json!({"x": -1, "y": null, "w": 1e308, "h": -1e308}),
        nested,
    ]
}

/// Every registered command, given every hostile value for each of its parameters (and for all of
/// them at once), on a document with real content: none may panic. An `Err` is the right answer.
/// The guard turns a panic into `EngineError::Internal` and counts it, so both are checked, and
/// a panic that got past the guard fails the test too.
#[test]
fn hostile_params_never_panic() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let values = hostile_values();
    let mut failures: Vec<String> = vec![];
    let mut runs = 0usize;
    for spec in command_specs().iter().filter(|c| !skipped(c.id)) {
        let keys = crate::cmd::param_keys(spec.params).unwrap_or_default();
        let mut sets: Vec<Value> = vec![json!({}), Value::Null, json!([]), json!("x"), json!(1)];
        for v in &values {
            for k in &keys {
                sets.push(json!({ *k: v.clone() }));
            }
            // A selection's worth of ids with the hostile value beside it, and every key at once.
            sets.push(json!({"ids": [1], "id": 1, "index": 0, "value": v.clone()}));
            sets.push(Value::Object(keys.iter().map(|k| (k.to_string(), v.clone())).collect()));
        }
        let mut s = Session::new();
        if sample::open_sample(&mut s).is_err() {
            failures.push("the sample deck did not open".into());
            continue;
        }
        for p in &sets {
            runs += 1;
            let before = s.panics_caught;
            let res = catch_unwind(AssertUnwindSafe(|| s.execute(spec.id, p)));
            let short = |p: &Value| p.to_string().chars().take(200).collect::<String>();
            match res {
                Err(_) => failures.push(format!("{} panicked past the guard with {}", spec.id, short(p))),
                Ok(r) => {
                    if matches!(r, Err(EngineError::Internal(..))) || s.panics_caught != before {
                        failures.push(format!("{} panicked (caught by the guard) with {}: {r:?}", spec.id, short(p)));
                    }
                }
            }
            // A command may close the document or open another; keep going on a fresh sample.
            if s.documents().len() != 1 || s.active().is_none() {
                s = Session::new();
                let _ = sample::open_sample(&mut s);
            }
        }
    }
    assert!(runs > 1000, "only {runs} runs");
    assert!(failures.is_empty(), "{} hostile calls panicked:\n{}", failures.len(), failures.iter().take(30).cloned().collect::<Vec<_>>().join("\n"));
}
