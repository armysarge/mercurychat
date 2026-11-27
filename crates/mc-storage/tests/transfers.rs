//! M4 Task 5 (C5): the transfers table as the Files view and restart
//! recovery read it.

use mc_storage::{Direction, Store, TransferRow, TransferState};

fn row(id: u8, dir: Direction, state: TransferState) -> TransferRow {
    TransferRow {
        id: [id; 16],
        direction: dir,
        peer: "ZS6PEER".into(),
        session_id: None,
        original_name: format!("file{id}.txt"),
        saved_name: None,
        size: 2048,
        wire_size: 611,
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
        encoding: Some("zstd".into()),
        state,
        bytes_done: 0,
        reason: None,
        created_utc: String::new(),
        updated_utc: String::new(),
        saved_path: None,
    }
}

#[test]
fn upsert_and_list_newest_first() {
    let mut s = Store::open_in_memory().unwrap();
    s.transfer_upsert(&row(1, Direction::Out, TransferState::Offered)).unwrap();
    s.transfer_upsert(&row(2, Direction::In, TransferState::Offered)).unwrap();
    s.transfer_set_state(Direction::In, &[2; 16], TransferState::Receiving, 300, None).unwrap();
    let list = s.transfers(10).unwrap();
    assert_eq!(list.iter().map(|r| r.id[0]).collect::<Vec<_>>(), [2, 1], "newest first");
    let r = &list[0];
    assert_eq!(
        (r.state, r.bytes_done, r.wire_size, r.encoding.as_deref()),
        (TransferState::Receiving, 300, 611, Some("zstd"))
    );
    assert!(!r.created_utc.is_empty() && !r.updated_utc.is_empty());
    // An upsert of the same transfer updates it; it is not a second row.
    s.transfer_upsert(&row(1, Direction::Out, TransferState::Sending)).unwrap();
    assert_eq!(s.transfers(10).unwrap().len(), 2);
    assert_eq!(s.transfer(Direction::Out, &[1; 16]).unwrap().unwrap().state, TransferState::Sending);
}

#[test]
fn complete_records_the_saved_file() {
    let mut s = Store::open_in_memory().unwrap();
    s.transfer_upsert(&row(3, Direction::In, TransferState::Verifying)).unwrap();
    s.transfer_complete(Direction::In, &[3; 16], Some(("file3 (1).txt", "C:/rx/file3 (1).txt"))).unwrap();
    let r = s.transfer(Direction::In, &[3; 16]).unwrap().unwrap();
    assert_eq!(r.state, TransferState::Complete);
    assert_eq!(
        (r.saved_name.as_deref(), r.saved_path.as_deref()),
        (Some("file3 (1).txt"), Some("C:/rx/file3 (1).txt"))
    );
    assert_eq!(r.bytes_done, r.wire_size);
}

#[test]
fn interrupt_unfinished_leaves_final_rows() {
    let mut s = Store::open_in_memory().unwrap();
    let states = [
        TransferState::Offered,
        TransferState::Waiting,
        TransferState::Sending,
        TransferState::Receiving,
        TransferState::Verifying,
        TransferState::Complete,
        TransferState::Rejected,
        TransferState::Cancelled,
        TransferState::Failed,
        TransferState::Interrupted,
    ];
    for (i, st) in states.iter().enumerate() {
        s.transfer_upsert(&row(i as u8 + 10, Direction::Out, *st)).unwrap();
    }
    assert_eq!(s.transfers_interrupt_unfinished().unwrap(), 5);
    for (i, st) in states.iter().enumerate() {
        let r = s.transfer(Direction::Out, &[i as u8 + 10; 16]).unwrap().unwrap();
        if st.is_final() {
            assert_eq!(r.state, *st, "a final row is untouched");
        } else {
            assert_eq!((r.state, r.reason.as_deref()), (TransferState::Interrupted, Some("restart")));
        }
    }
}

/// L4: a station that keeps offering files this one refuses must not grow the
/// list without end. What the operator decided, and everything else, stays.
#[test]
fn only_the_newest_refused_offers_are_kept() {
    let mut s = Store::open_in_memory().unwrap();
    let add = |s: &mut Store, id: u8, dir, state, reason: Option<&str>| {
        s.transfer_upsert(&TransferRow { reason: reason.map(Into::into), ..row(id, dir, state) }).unwrap();
    };
    use TransferState::{Complete, Rejected};
    add(&mut s, 1, Direction::In, Complete, None);
    add(&mut s, 2, Direction::In, Rejected, Some("declined"));
    add(&mut s, 3, Direction::In, Rejected, Some("timeout"));
    add(&mut s, 4, Direction::Out, Rejected, Some("busy"));
    for (id, why) in (10..).zip(["busy", "too large", "no folder", "write failed", "no space", "busy", "too large"]) {
        add(&mut s, id, Direction::In, Rejected, Some(why));
    }
    assert_eq!(s.transfers_prune_refused(3).unwrap(), 4);
    let mut ids: Vec<u8> = s.transfers(100).unwrap().iter().map(|r| r.id[0]).collect();
    ids.sort_unstable();
    assert_eq!(ids, [1, 2, 3, 4, 14, 15, 16]);
}
