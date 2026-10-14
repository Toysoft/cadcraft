//! Bounded ToUnicode bfchar/bfrange interpretation (ISO 32000-1, 9.10.3).
//! The surrounding CMap dictionary is metadata, not a PostScript program to execute.
use super::{Result, error};
use lopdf::{Object, content::Content};
use std::collections::BTreeMap;
pub(super) type Mapping = BTreeMap<(u32, u8), String>;
fn code(o: &Object) -> Result<(u32, u8)> {
    let bytes = o.as_str().map_err(error)?;
    if bytes.is_empty() || bytes.len() > 4 {
        return Err(error("invalid CMap source code"));
    }
    Ok((bytes.iter().fold(0u32, |n, b| n * 256 + u32::from(*b)), bytes.len() as u8))
}
fn target(bytes: &[u8]) -> Result<String> {
    if !bytes.len().is_multiple_of(2) || bytes.len() > 512 {
        return Err(error("invalid CMap Unicode target"));
    }
    let chars: Vec<_> = bytes.as_chunks::<2>().0.iter().map(|p| u16::from_be_bytes(*p)).collect();
    String::from_utf16(&chars).map_err(error)
}
fn insert(map: &mut Mapping, code: (u32, u8), text: String, budget: &mut usize) -> Result<()> {
    *budget = budget.saturating_add(text.len().saturating_add(64));
    if *budget > 8 << 20 {
        return Err(error("expanded Unicode map exceeds 8 MiB"));
    }
    if map.len() >= 65536 {
        return Err(error("more than 65536 Unicode mappings in a font"));
    }
    map.insert(code, text);
    Ok(())
}
pub(super) fn parse(bytes: &[u8]) -> Result<Mapping> {
    if bytes.len() > 1 << 20 {
        return Err(error("Unicode map exceeds 1 MiB"));
    }
    let content = Content::decode_strict(bytes).map_err(error)?;
    if content.operations.len() > 100_000 {
        return Err(error("too many CMap operations"));
    }
    let mut map = Mapping::new();
    let mut budget = 0usize;
    for op in content.operations {
        match op.operator.as_str() {
            "endbfchar" => {
                let (rows, remainder) = op.operands.as_chunks::<2>();
                if !remainder.is_empty() {
                    return Err(error("incomplete bfchar mapping"));
                }
                for row in rows {
                    let source = code(row.first().ok_or_else(|| error("missing CMap source"))?)?;
                    let dest = row.get(1).ok_or_else(|| error("missing CMap target"))?.as_str().map_err(error)?;
                    insert(&mut map, source, target(dest)?, &mut budget)?;
                }
            }
            "endbfrange" => {
                let (rows, remainder) = op.operands.as_chunks::<3>();
                if !remainder.is_empty() {
                    return Err(error("incomplete bfrange mapping"));
                }
                for row in rows {
                    let (lo, size) = code(row.first().ok_or_else(|| error("missing CMap source"))?)?;
                    let (hi, end_size) = code(row.get(1).ok_or_else(|| error("missing CMap range"))?)?;
                    let count = hi
                        .checked_sub(lo)
                        .and_then(|n| n.checked_add(1))
                        .filter(|n| *n <= 65536)
                        .ok_or_else(|| error("invalid or oversized CMap range"))?;
                    if size != end_size {
                        return Err(error("CMap range codes have different lengths"));
                    }
                    let target_object = row.get(2).ok_or_else(|| error("missing CMap range target"))?;
                    if let Object::Array(values) = target_object {
                        if values.len() != count as usize {
                            return Err(error("CMap range target count differs"));
                        }
                        for (i, obj) in values.iter().enumerate() {
                            insert(&mut map, (lo + i as u32, size), target(obj.as_str().map_err(error)?)?, &mut budget)?;
                        }
                    } else {
                        let mut value = target_object.as_str().map_err(error)?.to_vec();
                        for source in lo..=hi {
                            insert(&mut map, (source, size), target(&value)?, &mut budget)?;
                            if source < hi {
                                let mut carry = true;
                                for b in value.iter_mut().rev() {
                                    if carry {
                                        let (next, overflow) = b.overflowing_add(1);
                                        *b = next;
                                        carry = overflow;
                                    }
                                }
                                if carry {
                                    return Err(error("CMap target range overflows"));
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if map.is_empty() {
        return Err(error("font has no supported ToUnicode mappings"));
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn postscript_dictionary_syntax_and_czech_ranges_are_metadata() {
        let map=parse(b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo 3 dict dup begin /Registry (Adobe) def /Ordering (Identity) def /Supplement 0 def end def WMode 0 def 1 begincodespacerange <0000> <FFFF> endcodespacerange 2 beginbfrange <0160> <0161> <0160> <0159> <0159> <0159> endbfrange endcmap end end").unwrap();
        assert_eq!(map.get(&(0x160, 2)).map(String::as_str), Some("Š"));
        assert_eq!(map.get(&(0x161, 2)).map(String::as_str), Some("š"));
        assert_eq!(map.get(&(0x159, 2)).map(String::as_str), Some("ř"));
    }
    #[test]
    fn array_targets_and_surrogate_pairs_are_decoded() {
        let map = parse(b"1 beginbfrange <0001> <0003> [<0041> <010C> <D83DDE00>] endbfrange").unwrap();
        assert_eq!(map.get(&(3, 2)).map(String::as_str), Some("😀"));
    }
    #[test]
    fn oversized_malformed_maps_fail_without_expanding() {
        for bytes in [
            b"1 beginbfrange <00000001> <FFFFFFFF> <0000> endbfrange".as_slice(),
            b"1 beginbfchar <01> <D800> endbfchar",
            b"1 beginbfrange <0001> <0003> [<0041>] endbfrange",
        ] {
            assert!(parse(bytes).is_err());
        }
    }
    #[test]
    fn compact_range_cannot_expand_into_unbounded_text() {
        let target = "0041".repeat(256);
        let source = format!("1 beginbfrange <0000> <7FFF> <{target}> endbfrange");
        let error = parse(source.as_bytes()).err().unwrap().to_string();
        assert!(error.contains("8 MiB"), "{error}");
    }
}
