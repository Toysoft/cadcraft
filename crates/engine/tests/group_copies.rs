//! A copied group is a group (#1151): as in AutoCAD, COPY, MIRROR, ARRAY and paste of every
//! member of a group make a new unnamed group (`*A1`, `*A2`…) of the copies. A group copied only
//! in part makes none.

use cadcraft_engine::Session;
use cadcraft_engine::doc::{Group, Handle};
use serde_json::{Value, json};

/// A session with three lines; the first two make the group "Pair", not selectable.
fn grouped() -> (Session, Vec<Handle>) {
    let mut s = Session::new();
    for y in [0, 5, 10] {
        s.execute("line", &json!({ "points": [[0, y], [10, y]] })).unwrap();
    }
    let hs: Vec<Handle> = s.doc().unwrap().model.iter().map(|e| e.handle).collect();
    s.execute("group", &json!({ "name": "Pair", "description": "two lines", "handles": hex(&hs[..2]), "selectable": false })).unwrap();
    (s, hs)
}

fn hex(hs: &[Handle]) -> Vec<String> {
    hs.iter().map(|h| h.hex()).collect()
}

fn groups(s: &Session) -> Vec<Group> {
    s.doc().unwrap().groups.clone()
}

fn model_handles(s: &Session) -> Vec<Handle> {
    s.doc().unwrap().model.iter().map(|e| e.handle).collect()
}

fn new_handles(s: &Session, before: &[Handle]) -> Vec<Handle> {
    model_handles(s).into_iter().filter(|h| !before.contains(h)).collect()
}

/// The new groups after `cmd`, each with the objects `cmd` created.
fn run(s: &mut Session, cmd: &str, p: Value) -> (Vec<Group>, Vec<Handle>) {
    let (before, n) = (model_handles(s), s.doc().unwrap().groups.len());
    s.execute(cmd, &p).unwrap();
    (groups(s).split_off(n), new_handles(s, &before))
}

fn unnamed(name: &str, members: Vec<Handle>) -> Group {
    Group { name: name.into(), description: "two lines".into(), selectable: false, members }
}

#[test]
fn copy_of_a_whole_group_is_an_unnamed_group() {
    let (mut s, hs) = grouped();
    let (added, copies) = run(&mut s, "copy", json!({ "handles": hex(&hs[..2]), "delta": [0, 20] }));
    assert_eq!(copies.len(), 2);
    assert_eq!(added, vec![unnamed("*A1", copies)]);
    assert_eq!(groups(&s)[0].members, hs[..2].to_vec(), "the original group keeps its objects");

    // Each copy of a multiple COPY is a group of its own.
    let (added, copies) = run(&mut s, "copy", json!({ "handles": hex(&hs[..2]), "delta": [0, 20], "count": 2 }));
    assert_eq!(added, vec![unnamed("*A2", copies[..2].to_vec()), unnamed("*A3", copies[2..].to_vec())]);

    s.execute("u", &json!({})).unwrap();
    assert_eq!(groups(&s).len(), 2, "undo takes the copied groups away");
}

#[test]
fn copy_of_part_of_a_group_makes_no_group() {
    let (mut s, hs) = grouped();
    let (added, copies) = run(&mut s, "copy", json!({ "handles": hex(&hs[..1]), "delta": [0, 20] }));
    assert_eq!(copies.len(), 1);
    assert!(added.is_empty(), "{added:?}");
    let (added, _) = run(&mut s, "copy", json!({ "handles": hex(&hs[1..]), "delta": [0, 20] }));
    assert!(added.is_empty(), "the third line is not in the group: {added:?}");
}

#[test]
fn mirror_array_and_paste_copy_the_group_too() {
    let (mut s, hs) = grouped();
    let (added, copies) = run(&mut s, "mirror", json!({ "handles": hex(&hs[..2]), "p1": [0, -1], "p2": [10, -1] }));
    assert_eq!(added, vec![unnamed("*A1", copies)], "MIRROR");

    // MIRROR that erases the source moves the objects: the group stays as it is.
    let (added, copies) = run(&mut s, "mirror", json!({ "handles": hex(&hs[..2]), "p1": [0, -1], "p2": [10, -1], "erase": true }));
    assert!(added.is_empty() && copies.is_empty());

    let (added, copies) = run(&mut s, "arrayrect", json!({ "handles": hex(&hs[..2]), "rows": 1, "cols": 3, "colSpacing": 20, "associative": false }));
    assert_eq!(copies.len(), 4);
    assert_eq!(added, vec![unnamed("*A2", copies[..2].to_vec()), unnamed("*A3", copies[2..].to_vec())], "ARRAYRECT");

    s.execute("copyclip", &json!({ "handles": hex(&hs) })).unwrap();
    let (added, copies) = run(&mut s, "pasteclip", json!({ "at": [100, 100] }));
    assert_eq!(copies.len(), 3);
    assert_eq!(added, vec![unnamed("*A4", copies[..2].to_vec())], "PASTECLIP");

    s.execute("copyclip", &json!({ "handles": hex(&hs[1..]) })).unwrap();
    let (added, _) = run(&mut s, "pasteclip", json!({ "at": [200, 100] }));
    assert!(added.is_empty(), "part of a group pastes no group: {added:?}");
}

#[test]
fn a_large_array_of_a_group_makes_a_group_per_item() {
    let (mut s, hs) = grouped();
    let (added, copies) =
        run(&mut s, "arrayrect", json!({ "handles": hex(&hs[..2]), "rows": 1, "cols": 2001, "colSpacing": 20, "associative": false }));
    assert_eq!(copies.len(), 4000);
    assert_eq!(added.len(), 2000);
    for (i, (g, pair)) in added.iter().zip(copies.chunks(2)).enumerate() {
        assert_eq!(*g, unnamed(&format!("*A{}", i + 1), pair.to_vec()));
    }
}
