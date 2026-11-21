//! Remove every glyph of a document's text (the artwork alone remains), used for the background
//! behind exported text. Ported from the Android fork onto the current text engine.

use pdfcraft_content::{Op, parse, serialize_ops};
use pdfcraft_cos::{Dict, Document, Object, Stream};

use crate::text::content_streams;

/// Remove every glyph a document draws in page content and form XObjects (soft-mask groups
/// included), keeping all other painting and every graphics-state change, so the pages render as
/// their artwork alone (a background behind exported text). Annotation appearances keep their
/// text: export doesn't read it. Work on a copy: this is not an edit to keep.
pub fn remove_all_text(doc: &mut Document) {
    let shows = |op: &Op| matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"");
    // Text that clips (rendering modes 4–7) shapes the artwork drawn through it: keep it.
    let strip = |data: &[u8]| -> Option<Vec<u8>> {
        let ops = parse(data).ops;
        let mut mode = 0i64;
        let mut saved: Vec<i64> = Vec::new();
        let mut keep = Vec::with_capacity(ops.len());
        let mut changed = false;
        for op in ops {
            match op.op.as_slice() {
                b"q" if saved.len() < 4096 => saved.push(mode),
                b"Q" => mode = saved.pop().unwrap_or(mode),
                b"Tr" => mode = op.operands.first().and_then(Object::as_int).unwrap_or(0),
                _ => {}
            }
            if shows(&op) && !(4..=7).contains(&mode) {
                changed = true;
                continue;
            }
            keep.push(op);
        }
        changed.then(|| serialize_ops(&keep))
    };
    // Appearance streams of annotations, which keep their text.
    let mut appearances = std::collections::HashSet::new();
    for page in pdfcraft_model::pages(doc) {
        let annots = page.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for a in annots.iter().take(10_000) {
            let Some(ap) = doc.dict(a).and_then(|a| a.get(b"AP").and_then(|ap| doc.dict(ap))) else { continue };
            for (_, entry) in ap.iter() {
                if let Some(r) = entry.as_ref() {
                    appearances.insert(r.num);
                }
                if let Some(states) = doc.dict(entry) {
                    appearances.extend(states.iter().filter_map(|(_, s)| s.as_ref()).map(|r| r.num));
                }
            }
        }
    }
    for num in doc.object_numbers() {
        if appearances.contains(&num) {
            continue;
        }
        let Ok(obj) = doc.try_get(num) else { continue };
        let Object::Stream(s) = &*obj else { continue };
        if s.dict.name(b"Subtype") != Some(b"Form") {
            continue;
        }
        let Some(data) = s.decoded().ok().and_then(|d| strip(&d)) else { continue };
        let mut dict = s.dict.clone();
        dict.remove(b"Length");
        let r = pdfcraft_cos::ObjRef::new(num, doc.generation(num));
        doc.set(r, Stream::flate(dict, &data));
    }
    for page in pdfcraft_model::pages(doc) {
        let Ok(streams) = content_streams(doc, &page.dict) else { continue };
        let joined: Vec<u8> = streams.iter().flat_map(|c| c.data.iter().copied().chain(std::iter::once(b'\n'))).collect();
        let Some(data) = strip(&joined) else { continue };
        let contents = doc.add(Stream::flate(Dict::new(), &data));
        let _ = doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(contents)));
    }
}
