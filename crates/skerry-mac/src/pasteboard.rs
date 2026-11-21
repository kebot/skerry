//! The Mac's general `NSPasteboard`, read and written directly (`ARCH-017`): gpui's clipboard
//! has no change count, no HTML flavour, and no concealed mark.

use crate::clipboard::Content;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardType, NSPasteboardTypeHTML, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF,
};
use objc2_foundation::{NSData, NSString};

/// The nspasteboard.org mark clipboard managers honour: do not keep this copy.
const CONCEALED: &str = "org.nspasteboard.ConcealedType";
const JPEG: &str = "public.jpeg";

fn general() -> objc2::rc::Retained<NSPasteboard> {
    NSPasteboard::generalPasteboard()
}

/// Bumped by every write from any app; reading it shows no content and raises no prompt.
pub fn change_count() -> isize {
    general().changeCount()
}

fn data(board: &NSPasteboard, kind: &NSPasteboardType) -> Option<Vec<u8>> {
    board
        .dataForType(kind)
        .map(|data| data.to_vec())
        .filter(|bytes| !bytes.is_empty())
}

/// What the pasteboard holds, as the session takes it: markup with text to show is rich text,
/// then a picture, then plain text.
pub fn read() -> Option<Content> {
    read_from(&general())
}

fn read_from(board: &NSPasteboard) -> Option<Content> {
    // The pasteboard type statics are immutable AppKit constants.
    let (string, html, png, tiff) = unsafe {
        (
            NSPasteboardTypeString,
            NSPasteboardTypeHTML,
            NSPasteboardTypePNG,
            NSPasteboardTypeTIFF,
        )
    };
    let text = board
        .stringForType(string)
        .map(|text| text.to_string())
        .filter(|text| !text.is_empty());
    let markup = data(board, html)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .filter(|html| markup_has_text(html));
    if let Some(html) = markup {
        let text = text.unwrap_or_default();
        return Some(Content::Rich {
            html,
            text: Content::text(&text).into_text(),
        });
    }
    let image = data(board, png).or_else(|| data(board, tiff).and_then(|tiff| tiff_to_png(&tiff)));
    if let Some(bytes) = image {
        return Some(Content::Image {
            mime: "image/png".into(),
            bytes,
        });
    }
    text.map(|text| Content::text(&text))
}

fn tiff_to_png(tiff: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory_with_format(tiff, image::ImageFormat::Tiff).ok()?;
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

/// Replace the pasteboard with `content`, marked concealed when it is a secret; `None` only
/// clears it. Returns the change count the write left, so the next focus check knows it.
pub fn write(content: Option<&Content>, secret: bool) -> isize {
    write_to(&general(), content, secret)
}

fn write_to(board: &NSPasteboard, content: Option<&Content>, secret: bool) -> isize {
    board.clearContents();
    let (string, html, png) = unsafe {
        (
            NSPasteboardTypeString,
            NSPasteboardTypeHTML,
            NSPasteboardTypePNG,
        )
    };
    match content {
        Some(Content::Text(text)) => {
            board.setString_forType(&NSString::from_str(text), string);
        }
        Some(Content::Rich { html: markup, text }) => {
            board.setData_forType(Some(&NSData::with_bytes(markup.as_bytes())), html);
            board.setString_forType(&NSString::from_str(text), string);
        }
        Some(Content::Image { mime, bytes }) => {
            let data = NSData::with_bytes(bytes);
            if mime == "image/jpeg" {
                board.setData_forType(Some(&data), &NSString::from_str(JPEG));
            } else {
                board.setData_forType(Some(&data), png);
            }
        }
        None => {}
    }
    if secret && content.is_some() {
        board.setData_forType(
            Some(&NSData::with_bytes(b"secret")),
            &NSString::from_str(CONCEALED),
        );
    }
    board.changeCount()
}

/// Clear the pasteboard only if it still holds the write that left `count`.
pub fn clear_if_unchanged(count: isize) {
    let board = general();
    if board.changeCount() == count {
        board.clearContents();
    }
}

/// Whether markup shows any text outside tags, `head`, `style`, and `script`: a copied picture
/// offers markup that is only an `img` tag, and that copy is the picture.
fn markup_has_text(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let mut rest = lower.as_str();
    let mut visible = String::new();
    while let Some(open) = rest.find('<') {
        visible.push_str(&rest[..open]);
        rest = &rest[open..];
        let hidden = ["<head", "<style", "<script", "<title", "<!--"]
            .into_iter()
            .find(|tag| rest.starts_with(tag));
        let close = match hidden {
            Some("<!--") => rest.find("-->").map(|end| end + 3),
            Some(tag) => {
                let end_tag = format!("</{}", &tag[1..]);
                rest.find(&end_tag)
                    .and_then(|end| rest[end..].find('>').map(|gt| end + gt + 1))
            }
            None => rest.find('>').map(|gt| gt + 1),
        };
        let Some(close) = close else {
            rest = "";
            break;
        };
        rest = &rest[close..];
    }
    visible.push_str(rest);
    visible
        .replace("&nbsp;", " ")
        .chars()
        .any(|ch| !ch.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private pasteboard, so tests never touch the person's clipboard.
    struct Scratch(objc2::rc::Retained<NSPasteboard>);

    impl Drop for Scratch {
        fn drop(&mut self) {
            // SAFETY: the pasteboard is this test's alone and is not used after.
            unsafe { objc2::msg_send![&*self.0, releaseGlobally] }
        }
    }

    #[test]
    fn contents_round_trip_and_secrets_are_concealed() {
        let board = Scratch(NSPasteboard::pasteboardWithUniqueName());
        let board = &board.0;

        let before = board.changeCount();
        let after = write_to(board, Some(&Content::Text("日本語\r\nline".into())), false);
        assert_ne!(before, after);
        assert_eq!(read_from(board), Some(Content::Text("日本語\nline".into())));

        let rich = Content::Rich {
            html: "<b>bold</b>".into(),
            text: "bold".into(),
        };
        write_to(board, Some(&rich), false);
        assert_eq!(read_from(board), Some(rich));

        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(2, 2)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let picture = Content::Image {
            mime: "image/png".into(),
            bytes: png.into_inner(),
        };
        write_to(board, Some(&picture), false);
        assert_eq!(read_from(board), Some(picture));

        write_to(board, Some(&Content::Text("hunter2".into())), true);
        assert!(data(board, &NSString::from_str(CONCEALED)).is_some());
        write_to(board, None, false);
        assert_eq!(read_from(board), None);
    }

    #[test]
    fn markup_with_only_a_picture_is_not_text() {
        assert!(markup_has_text("<meta charset='utf-8'><b>Hello</b>"));
        assert!(!markup_has_text(
            "<html><head><style>p{}</style></head><body><img src='x.png'>&nbsp;</body></html>"
        ));
        assert!(!markup_has_text("<!-- note --><script>var a</script>"));
    }
}
