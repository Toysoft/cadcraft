//! A damaged DXF can split its entities over several ENTITIES sections (with stray header
//! variables and ENDSECs between them, and no EOF). Every section is read, not just the last
//! one (#865).

use cadcraft_doc::{EntityKind, Handle};

#[test]
fn entities_of_every_entities_section_are_read() {
    let text = "0\nSECTION\n2\nENTITIES\n\
                0\nTEXT\n8\n0\n10\n0\n20\n0\n40\n1\n1\nA\n\
                0\nLINE\n8\n0\n10\n0\n20\n0\n11\n1\n21\n0\n\
                0\nENDSEC\n9\n$EXTMIN\n10\n0\n20\n0\n0\nENDSEC\n\
                0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0\n20\n1\n11\n1\n21\n1\n0\nENDSEC\n\
                0\nSECTION\n2\nENTITIES\n0\nENDSEC\n";
    let d = cadcraft_io::read(text.as_bytes(), "t.dxf").unwrap();
    let lines = d.model.iter().filter(|e| matches!(e.kind, EntityKind::Line(_))).count();
    let texts = d.model.iter().filter(|e| matches!(e.kind, EntityKind::Text(_))).count();
    assert_eq!((lines, texts, d.model.len()), (2, 1, 3));
}

#[test]
fn repeated_handles_across_entities_sections_keep_both_entities() {
    // A damaged file can repeat a handle across its sections (within one section duplicates are
    // already renumbered). The store replaces an entity with the same handle, so the earlier
    // section's line was silently lost — and a save kept only the later one (#939).
    let text = "0\nSECTION\n2\nENTITIES\n\
                0\nLINE\n5\n300\n8\n0\n10\n0\n20\n0\n11\n1\n21\n0\n\
                0\nENDSEC\n\
                0\nSECTION\n2\nENTITIES\n0\nLINE\n5\n300\n8\n0\n10\n0\n20\n1\n11\n1\n21\n1\n0\nENDSEC\n\
                0\nEOF\n";
    let d = cadcraft_io::read(text.as_bytes(), "t.dxf").unwrap();
    let lines = d.model.iter().filter(|e| matches!(e.kind, EntityKind::Line(_))).count();
    assert_eq!(lines, 2, "the second section's line replaced the first");
    let handles: Vec<Handle> = d.model.iter().map(|e| e.handle).collect();
    let unique: std::collections::HashSet<Handle> = handles.iter().copied().collect();
    assert_eq!(unique.len(), handles.len(), "duplicate handles: {handles:?}");
    assert!(d.entity(Handle::parse_hex("300").unwrap()).is_some(), "the file's own handle stays with one of them");
}
