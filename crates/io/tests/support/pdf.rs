//! Small original PDF fixtures. Objects and xref are generated here, never copied from a CAD.
#![allow(dead_code)]
pub fn stream(content: &[u8], options: &str) -> Vec<u8> {
    let mut b = format!("<< /Length {} {options} >>\nstream\n", content.len()).into_bytes();
    b.extend_from_slice(content);
    b.extend_from_slice(b"\nendstream");
    b
}
pub fn file(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0usize];
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend(o);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for o in offsets.iter().skip(1) {
        out.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Root 1 0 R /Size {} >>\nstartxref\n{xref}\n%%EOF\n", offsets.len()).as_bytes());
    out
}
pub fn page(content: &[u8], resources: &str, extra: &str) -> Vec<u8> {
    file(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 360 240] >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << {resources} >> {extra} >>").into_bytes(),
        stream(content, ""),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>".to_vec(),
    ])
}
pub fn sample() -> Vec<u8> {
    file(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 7 0 R] /Count 2 /MediaBox [0 0 360 240] /Resources << /Font << /F1 5 0 R >> /XObject << /Form1 6 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        stream(b"0 G 1 w 20 20 240 140 re S\n20 90 m 260 90 l S\n30 35 m 60 65 90 5 120 35 c S\nq 1 0 0 1 20 20 cm /Form1 Do Q\nBT /F1 12 Tf 30 185 Td (SYNTHETIC PLAN) Tj ET", ""),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>".to_vec(),
        stream(b"0 0 m 30 30 l S","/Type /XObject /Subtype /Form /BBox [0 0 40 40] /Matrix [1 0 0 1 50 50]"),
        b"<< /Type /Page /Parent 2 0 R /Contents 8 0 R /CropBox [10 20 110 220] /Rotate 90 >>".to_vec(),
        stream(b"10 20 m 110 20 l 110 220 l S",""),
    ])
}
