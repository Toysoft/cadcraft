//! Embedded OLE objects (OLE2FRAME). They are not modelled: the entity stays an [`Unknown`] with
//! its raw groups, so it is saved as it was read. What is read from those groups here is enough
//! to show the object: the rectangle it sits in and the data its picture is taken from.

use cadcraft_geom::{Bounds2, Vec2};

use crate::{Handle, Unknown};

impl Unknown {
    /// An embedded OLE object.
    pub fn is_ole(&self) -> bool {
        self.dxf_type.eq_ignore_ascii_case("OLE2FRAME")
    }

    /// The first group `code` as a finite number.
    fn number(&self, code: i32) -> Option<f64> {
        let tag = self.tags.iter().find(|t| t.code == code)?;
        tag.value.trim().parse::<f64>().ok().filter(|v| v.is_finite())
    }

    /// The rectangle an OLE object is shown in: its upper-left (groups 10, 20) and lower-right
    /// (11, 21) corners. `None` for another kind of entity and for a rectangle without area.
    pub fn ole_frame(&self) -> Option<Bounds2> {
        if !self.is_ole() {
            return None;
        }
        let a = Vec2::new(self.number(10)?, self.number(20)?);
        let b = Vec2::new(self.number(11)?, self.number(21)?);
        let frame = Bounds2::from_points([a, b]);
        (frame.width() > 0.0 && frame.height() > 0.0).then_some(frame)
    }

    /// The binary data of an OLE object (groups 310, hexadecimal text). `None` when there is
    /// none, when it is not hexadecimal or when it is longer than `max` bytes.
    pub fn ole_data(&self, max: usize) -> Option<Vec<u8>> {
        if !self.is_ole() {
            return None;
        }
        let mut data = Vec::new();
        for tag in self.tags.iter().filter(|t| t.code == 310) {
            let text = tag.value.trim().as_bytes();
            if !text.len().is_multiple_of(2) || data.len().saturating_add(text.len() / 2) > max {
                return None;
            }
            for pair in text.as_chunks::<2>().0 {
                let digit = |c: u8| char::from(c).to_digit(16).and_then(|d| u8::try_from(d).ok());
                data.push(digit(pair[0])? << 4 | digit(pair[1])?);
            }
        }
        (!data.is_empty()).then_some(data)
    }
}

/// The key in [`crate::Rasters`] of the picture of the OLE object with `handle`. No image file
/// has such a path.
pub fn ole_raster_key(handle: Handle) -> String {
    format!("\0ole:{}", handle.hex())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RawTag;

    fn ole(tags: &[(i32, &str)]) -> Unknown {
        Unknown { dxf_type: "OLE2FRAME".into(), tags: tags.iter().map(|(code, value)| RawTag { code: *code, value: (*value).into() }).collect() }
    }

    #[test]
    fn the_frame_is_read_from_the_two_corners() {
        let u = ole(&[(70, "2"), (10, "10.0"), (20, "50.0"), (30, "0.0"), (11, " 40.0"), (21, "30.0"), (31, "0.0")]);
        let f = u.ole_frame().unwrap();
        assert_eq!((f.min, f.max), (Vec2::new(10.0, 30.0), Vec2::new(40.0, 50.0)));
        // Another entity type, a missing corner, a flat rectangle, numbers that are not numbers.
        assert!(Unknown { dxf_type: "LIGHT".into(), ..u.clone() }.ole_frame().is_none());
        assert!(ole(&[(10, "1"), (20, "2"), (11, "3")]).ole_frame().is_none());
        assert!(ole(&[(10, "1"), (20, "2"), (11, "1"), (21, "5")]).ole_frame().is_none());
        assert!(ole(&[(10, "nan"), (20, "2"), (11, "3"), (21, "5")]).ole_frame().is_none());
        assert!(ole(&[(10, "x"), (20, "2"), (11, "3"), (21, "5")]).ole_frame().is_none());
        assert!(ole(&[(10, "inf"), (20, "2"), (11, "3"), (21, "5")]).ole_frame().is_none());
    }

    #[test]
    fn the_data_is_the_hexadecimal_groups_joined() {
        let u = ole(&[(90, "5"), (310, "8055aB"), (310, " 00ff "), (1, "OLE")]);
        assert_eq!(u.ole_data(16), Some(vec![0x80, 0x55, 0xab, 0x00, 0xff]));
        assert_eq!(u.ole_data(5).map(|d| d.len()), Some(5));
        assert_eq!(u.ole_data(4), None, "longer than allowed");
        assert_eq!(ole(&[(310, "8")]).ole_data(16), None, "half a byte");
        assert_eq!(ole(&[(310, "zz")]).ole_data(16), None);
        assert_eq!(ole(&[(310, "+1")]).ole_data(16), None);
        assert_eq!(ole(&[(310, "é")]).ole_data(16), None);
        assert_eq!(ole(&[(90, "0")]).ole_data(16), None, "no data");
        assert_eq!(Unknown { dxf_type: "LIGHT".into(), ..u }.ole_data(16), None);
    }

    #[test]
    fn the_key_is_no_file_path() {
        assert_eq!(ole_raster_key(Handle(0x2a)), "\0ole:2A");
    }
}
