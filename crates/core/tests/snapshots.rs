use todo_core::snapshot::due_label;
use todo_core::{Command, Doc, FixedClock, SeqIdSource, ViewFilter};

const DAY: i64 = 86_400;

/// Fixed "now": day 10 since the epoch. Chosen so overdue (day 7), today
/// (day 10), tomorrow (day 11), and far-future (day 40) due dates are all
/// expressible as small, readable multiples of `DAY`.
const NOW: i64 = 10 * DAY;

fn fixture() -> Doc {
    let mut doc = Doc::new(1).unwrap();
    let clock = FixedClock(NOW);
    let ids = SeqIdSource::new();

    doc.apply(
        Command::Add {
            title: "Buy milk".to_string(),
            after: None,
        },
        &clock,
        &ids,
    )
    .unwrap();

    doc.apply(
        Command::Add {
            title: "Write report".to_string(),
            after: None,
        },
        &clock,
        &ids,
    )
    .unwrap();
    let write_report = doc.read(ViewFilter::All, &clock).rows[0].id.clone();
    doc.apply(
        Command::SetDue {
            id: write_report,
            due: Some(NOW),
        },
        &clock,
        &ids,
    )
    .unwrap();

    doc.apply(
        Command::Add {
            title: "Call dentist".to_string(),
            after: None,
        },
        &clock,
        &ids,
    )
    .unwrap();
    let call_dentist = doc.read(ViewFilter::All, &clock).rows[0].id.clone();
    doc.apply(
        Command::SetDue {
            id: call_dentist,
            due: Some(NOW + DAY),
        },
        &clock,
        &ids,
    )
    .unwrap();

    doc.apply(
        Command::Add {
            title: "Pay rent".to_string(),
            after: None,
        },
        &clock,
        &ids,
    )
    .unwrap();
    let pay_rent = doc.read(ViewFilter::All, &clock).rows[0].id.clone();
    doc.apply(
        Command::SetDue {
            id: pay_rent,
            due: Some(NOW - 3 * DAY),
        },
        &clock,
        &ids,
    )
    .unwrap();

    doc.apply(
        Command::Add {
            title: "Clean garage".to_string(),
            after: None,
        },
        &clock,
        &ids,
    )
    .unwrap();
    let clean_garage = doc.read(ViewFilter::All, &clock).rows[0].id.clone();
    doc.apply(
        Command::SetDone {
            id: clean_garage,
            done: true,
        },
        &clock,
        &ids,
    )
    .unwrap();

    doc.apply(
        Command::Add {
            title: "Submit taxes".to_string(),
            after: None,
        },
        &clock,
        &ids,
    )
    .unwrap();
    let submit_taxes = doc.read(ViewFilter::All, &clock).rows[0].id.clone();
    doc.apply(
        Command::SetDue {
            id: submit_taxes.clone(),
            due: Some(30 * DAY),
        },
        &clock,
        &ids,
    )
    .unwrap();
    doc.apply(
        Command::SetDone {
            id: submit_taxes,
            done: true,
        },
        &clock,
        &ids,
    )
    .unwrap();

    doc
}

#[test]
fn snapshot_all_view() {
    let doc = fixture();
    let clock = FixedClock(NOW);
    insta::assert_yaml_snapshot!(doc.read(ViewFilter::All, &clock));
}

#[test]
fn snapshot_active_view() {
    let doc = fixture();
    let clock = FixedClock(NOW);
    insta::assert_yaml_snapshot!(doc.read(ViewFilter::Active, &clock));
}

#[test]
fn snapshot_completed_view() {
    let doc = fixture();
    let clock = FixedClock(NOW);
    insta::assert_yaml_snapshot!(doc.read(ViewFilter::Completed, &clock));
}

#[test]
fn snapshot_due_label_offsets() {
    let offsets: Vec<(&str, i64)> = vec![
        ("overdue", NOW - 3 * DAY),
        ("today", NOW),
        ("tomorrow", NOW + DAY),
        ("this_week", NOW + 4 * DAY),
        ("next_month", NOW + 30 * DAY),
        ("next_year", NOW + 365 * DAY),
    ];
    let labels: Vec<(String, String)> = offsets
        .into_iter()
        .map(|(name, due)| (name.to_string(), due_label(due, NOW)))
        .collect();
    insta::assert_yaml_snapshot!(labels);
}
