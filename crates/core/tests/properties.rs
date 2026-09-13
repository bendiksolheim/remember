use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use proptest::prelude::*;
use todo_core::{Command, Doc, FixedClock, IdSource, Snapshot, TaskRow, ViewFilter};

/// Deterministic, per-peer-unique id source. `SeqIdSource` alone would let
/// two independently-generated peer sequences collide on the same ids
/// ("0", "1", ...), corrupting the very convergence properties this file
/// exists to check.
struct PrefixedIds {
    prefix: &'static str,
    counter: AtomicU64,
}

impl PrefixedIds {
    fn new(prefix: &'static str) -> Self {
        Self {
            prefix,
            counter: AtomicU64::new(0),
        }
    }
}

impl IdSource for PrefixedIds {
    fn new_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("{}-{n}", self.prefix)
    }
}

/// A command template that only ever references ids the replay has actually
/// seen — `target_seed`/`after_seed` are resolved modulo however many ids
/// are known *at replay time*, so any generated sequence is valid by
/// construction. Undo/Redo are deliberately excluded: they can invalidate
/// `known` (undoing an Add removes an id `known` still tracks), which is
/// exactly the scenario property 7 tests in isolation, not something the
/// other properties need to reason about.
#[derive(Debug, Clone)]
enum Op {
    Add {
        title: String,
        after_seed: Option<u8>,
    },
    SetTitle {
        target_seed: u8,
        title: String,
    },
    SetNotes {
        target_seed: u8,
        notes: String,
    },
    SetDone {
        target_seed: u8,
        done: bool,
    },
    SetDue {
        target_seed: u8,
        due: Option<i64>,
    },
    Move {
        target_seed: u8,
        after_seed: Option<u8>,
    },
    Delete {
        target_seed: u8,
    },
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        ("[a-zA-Z ]{0,8}", proptest::option::of(any::<u8>()))
            .prop_map(|(title, after_seed)| Op::Add { title, after_seed }),
        (any::<u8>(), "[a-zA-Z ]{0,8}")
            .prop_map(|(target_seed, title)| Op::SetTitle { target_seed, title }),
        (any::<u8>(), "[a-zA-Z ]{0,8}")
            .prop_map(|(target_seed, notes)| Op::SetNotes { target_seed, notes }),
        (any::<u8>(), any::<bool>())
            .prop_map(|(target_seed, done)| Op::SetDone { target_seed, done }),
        (
            any::<u8>(),
            proptest::option::of(-1_000_000i64..1_000_000i64)
        )
            .prop_map(|(target_seed, due)| Op::SetDue { target_seed, due }),
        (any::<u8>(), proptest::option::of(any::<u8>())).prop_map(|(target_seed, after_seed)| {
            Op::Move {
                target_seed,
                after_seed,
            }
        }),
        any::<u8>().prop_map(|target_seed| Op::Delete { target_seed }),
    ]
}

fn arb_ops(max_len: usize) -> impl Strategy<Value = Vec<Op>> {
    proptest::collection::vec(arb_op(), 0..max_len)
}

fn resolve_target(known: &[String], seed: u8) -> Option<String> {
    if known.is_empty() {
        None
    } else {
        Some(known[seed as usize % known.len()].clone())
    }
}

/// Applies one `Op`, skipping it (returning `false`) if it needs an id that
/// doesn't exist yet — this can never happen for `Add` (it needs no target),
/// so `Add` always applies.
fn replay_one(
    op: &Op,
    doc: &mut Doc,
    clock: &FixedClock,
    ids_src: &dyn IdSource,
    known: &mut Vec<String>,
) -> bool {
    match op {
        Op::Add { title, after_seed } => {
            let after = after_seed.and_then(|s| resolve_target(known, s));
            let before: HashSet<String> = known.iter().cloned().collect();
            doc.apply(
                Command::Add {
                    title: title.clone(),
                    after,
                },
                clock,
                ids_src,
            )
            .unwrap();
            let snap = doc.read(ViewFilter::All, clock);
            // An empty/whitespace-only title is a no-op (see apply_add), so
            // there may be no new row to find here.
            if let Some(new_id) = snap
                .rows
                .iter()
                .map(|r| r.id.clone())
                .find(|id| !before.contains(id))
            {
                known.push(new_id);
            }
            true
        }
        Op::SetTitle { target_seed, title } => resolve_target(known, *target_seed)
            .map(|id| {
                doc.apply(
                    Command::SetTitle {
                        id,
                        title: title.clone(),
                    },
                    clock,
                    ids_src,
                )
                .unwrap();
            })
            .is_some(),
        Op::SetNotes { target_seed, notes } => resolve_target(known, *target_seed)
            .map(|id| {
                doc.apply(
                    Command::SetNotes {
                        id,
                        notes: notes.clone(),
                    },
                    clock,
                    ids_src,
                )
                .unwrap();
            })
            .is_some(),
        Op::SetDone { target_seed, done } => resolve_target(known, *target_seed)
            .map(|id| {
                doc.apply(Command::SetDone { id, done: *done }, clock, ids_src)
                    .unwrap();
            })
            .is_some(),
        Op::SetDue { target_seed, due } => resolve_target(known, *target_seed)
            .map(|id| {
                doc.apply(Command::SetDue { id, due: *due }, clock, ids_src)
                    .unwrap();
            })
            .is_some(),
        Op::Move {
            target_seed,
            after_seed,
        } => resolve_target(known, *target_seed)
            .map(|id| {
                let after = after_seed.and_then(|s| resolve_target(known, s));
                doc.apply(Command::Move { id, after }, clock, ids_src)
                    .unwrap();
            })
            .is_some(),
        Op::Delete { target_seed } => resolve_target(known, *target_seed)
            .map(|id| {
                doc.apply(Command::Delete { id: id.clone() }, clock, ids_src)
                    .unwrap();
                known.retain(|x| x != &id);
            })
            .is_some(),
    }
}

fn replay(ops: &[Op], doc: &mut Doc, clock: &FixedClock, ids_src: &dyn IdSource) -> Vec<String> {
    let mut known = Vec::new();
    for op in ops {
        replay_one(op, doc, clock, ids_src, &mut known);
    }
    known
}

/// Compares only the CRDT-content-derived fields. `revision`/`can_undo`/
/// `can_redo` are per-`Doc`-instance local bookkeeping (an undo stack, a
/// local apply counter) — not part of the merged document state, so two
/// independently-built `Doc`s that converge on identical content will
/// legitimately disagree on these.
fn content(snap: &Snapshot) -> (Vec<TaskRow>, u32, ViewFilter) {
    (snap.rows.clone(), snap.active_count, snap.view)
}

fn arb_raw_command() -> impl Strategy<Value = Command> {
    let id = "[a-zA-Z0-9]{0,6}";
    prop_oneof![
        (".*", proptest::option::of(id)).prop_map(|(title, after)| Command::Add { title, after }),
        (id, ".*").prop_map(|(id, title)| Command::SetTitle { id, title }),
        (id, ".*").prop_map(|(id, notes)| Command::SetNotes { id, notes }),
        (id, any::<bool>()).prop_map(|(id, done)| Command::SetDone { id, done }),
        (id, proptest::option::of(any::<i64>())).prop_map(|(id, due)| Command::SetDue { id, due }),
        (id, proptest::option::of(id)).prop_map(|(id, after)| Command::Move { id, after }),
        id.prop_map(|id| Command::Delete { id }),
        Just(Command::Undo),
        Just(Command::Redo),
    ]
}

proptest! {
    /// Property 1 (convergence): two peers with independent, unrelated
    /// command sequences, cross-imported in either order, produce identical
    /// snapshots. The most important test in the project.
    #[test]
    fn convergence_across_peers(ops_a in arb_ops(8), ops_b in arb_ops(8)) {
        let clock = FixedClock(0);
        let ids_a = PrefixedIds::new("a");
        let ids_b = PrefixedIds::new("b");

        let mut doc_a = Doc::new(1).unwrap();
        replay(&ops_a, &mut doc_a, &clock, &ids_a);
        let mut doc_b = Doc::new(2).unwrap();
        replay(&ops_b, &mut doc_b, &clock, &ids_b);

        let updates_a = doc_a.export_updates().unwrap();
        let updates_b = doc_b.export_updates().unwrap();

        let mut merged_via_a = Doc::load(1, &doc_a.export_snapshot().unwrap()).unwrap();
        merged_via_a.import_updates(&updates_b).unwrap();

        let mut merged_via_b = Doc::load(2, &doc_b.export_snapshot().unwrap()).unwrap();
        merged_via_b.import_updates(&updates_a).unwrap();

        prop_assert_eq!(
            content(&merged_via_a.read(ViewFilter::All, &clock)),
            content(&merged_via_b.read(ViewFilter::All, &clock))
        );
    }

    /// Property 2: importing the same update bytes twice changes nothing.
    #[test]
    fn import_idempotence(ops in arb_ops(8)) {
        let clock = FixedClock(0);
        let ids_src = PrefixedIds::new("a");
        let mut doc = Doc::new(1).unwrap();
        replay(&ops, &mut doc, &clock, &ids_src);
        let updates = doc.export_updates().unwrap();

        let mut target = Doc::new(2).unwrap();
        target.import_updates(&updates).unwrap();
        let once = content(&target.read(ViewFilter::All, &clock));
        target.import_updates(&updates).unwrap();
        let twice = content(&target.read(ViewFilter::All, &clock));
        prop_assert_eq!(once, twice);
    }

    /// Property 3: a set of updates imported in any permutation converges
    /// to the same state.
    #[test]
    fn import_order_independence(ops_a in arb_ops(8), ops_b in arb_ops(8)) {
        let clock = FixedClock(0);
        let ids_a = PrefixedIds::new("a");
        let ids_b = PrefixedIds::new("b");
        let mut doc_a = Doc::new(1).unwrap();
        replay(&ops_a, &mut doc_a, &clock, &ids_a);
        let mut doc_b = Doc::new(2).unwrap();
        replay(&ops_b, &mut doc_b, &clock, &ids_b);
        let updates_a = doc_a.export_updates().unwrap();
        let updates_b = doc_b.export_updates().unwrap();

        let mut order1 = Doc::new(3).unwrap();
        order1.import_updates(&updates_a).unwrap();
        order1.import_updates(&updates_b).unwrap();

        let mut order2 = Doc::new(4).unwrap();
        order2.import_updates(&updates_b).unwrap();
        order2.import_updates(&updates_a).unwrap();

        prop_assert_eq!(
            content(&order1.read(ViewFilter::All, &clock)),
            content(&order2.read(ViewFilter::All, &clock))
        );
    }

    /// Property 4: `export_snapshot` -> `load` -> identical snapshot.
    #[test]
    fn snapshot_round_trip(ops in arb_ops(8)) {
        let clock = FixedClock(0);
        let ids_src = PrefixedIds::new("a");
        let mut doc = Doc::new(1).unwrap();
        replay(&ops, &mut doc, &clock, &ids_src);
        let before = content(&doc.read(ViewFilter::All, &clock));
        let bytes = doc.export_snapshot().unwrap();
        let reloaded = Doc::load(1, &bytes).unwrap();
        let after = content(&reloaded.read(ViewFilter::All, &clock));
        prop_assert_eq!(before, after);
    }

    /// Property 5: the key set of `tasks` exactly equals the set of ids in
    /// `order` — no orphans, no duplicates.
    #[test]
    fn structural_consistency(ops in arb_ops(8)) {
        let clock = FixedClock(0);
        let ids_src = PrefixedIds::new("a");
        let mut doc = Doc::new(1).unwrap();
        replay(&ops, &mut doc, &clock, &ids_src);
        prop_assert_eq!(doc.task_count(), doc.order_len());
    }

    /// Property 6: `active_count` always equals the number of rows with
    /// `!done` under `ViewFilter::All`.
    #[test]
    fn active_count_matches_undone_rows(ops in arb_ops(8)) {
        let clock = FixedClock(0);
        let ids_src = PrefixedIds::new("a");
        let mut doc = Doc::new(1).unwrap();
        replay(&ops, &mut doc, &clock, &ids_src);
        let snap = doc.read(ViewFilter::All, &clock);
        let expected = snap.rows.iter().filter(|r| !r.done).count() as u32;
        prop_assert_eq!(snap.active_count, expected);
    }

    /// Property 7: for any single command, `apply(c); undo()` yields the
    /// snapshot from before `c`.
    #[test]
    fn undo_inverts_single_command(mut ops in arb_ops(8)) {
        prop_assume!(!ops.is_empty());
        let last = ops.pop().unwrap();
        let clock = FixedClock(0);
        let ids_src = PrefixedIds::new("a");
        let mut doc = Doc::new(1).unwrap();
        let mut known = replay(&ops, &mut doc, &clock, &ids_src);

        let before = content(&doc.read(ViewFilter::All, &clock));
        let applied = replay_one(&last, &mut doc, &clock, &ids_src, &mut known);
        let changed = applied && content(&doc.read(ViewFilter::All, &clock)) != before;
        // A command that sets a field to its current value (or moves an
        // item to where it already is) is a genuine no-op: Loro's LWW
        // registers dedupe same-value writes, so nothing is pushed onto the
        // undo stack for it. Undo afterward correctly reverts the *previous*
        // real change instead — there's nothing of `last`'s own to invert.
        if changed {
            doc.apply(Command::Undo, &clock, &ids_src).unwrap();
            let after = content(&doc.read(ViewFilter::All, &clock));
            prop_assert_eq!(before, after);
        }
    }

    /// Property 8: any sequence, including invalid ids, returns `Ok` or
    /// `Err` — never unwinds. Unlike the other properties, this
    /// deliberately generates ids that usually don't exist.
    #[test]
    fn no_panics_on_arbitrary_commands(cmds in proptest::collection::vec(arb_raw_command(), 0..12)) {
        let clock = FixedClock(0);
        let ids_src = PrefixedIds::new("a");
        let mut doc = Doc::new(1).unwrap();
        for cmd in cmds {
            let _ = doc.apply(cmd, &clock, &ids_src);
        }
    }
}
