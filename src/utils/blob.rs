use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// Maximum number of bytes of a `BLOB` sent with a query result. Larger
/// values are cut to this many bytes; the `BLOB:` header still reports the
/// true size, so Tabularis knows the value is longer than its preview.
pub const MAX_BLOB_PREVIEW_SIZE: usize = 10_240;

const DEFAULT_MIME: &str = "application/octet-stream";

/// Encodes the first bytes of a binary value in the host's blob wire format,
/// `BLOB:<total_size>:<mime_type>:<base64_data>`. `total_size` is the length
/// of the whole value, which may be more than `preview` holds; the MIME type
/// is sniffed from the preview.
pub fn encode_blob_preview(preview: &[u8], total_size: usize) -> String {
    let mime_type = infer::get(preview)
        .map(|kind| kind.mime_type())
        .unwrap_or(DEFAULT_MIME);
    format!("BLOB:{total_size}:{mime_type}:{}", STANDARD.encode(preview))
}

#[cfg(test)]
mod tests {
    use super::encode_blob_preview;

    #[test]
    fn encodes_bytes_with_their_size() {
        assert_eq!(
            encode_blob_preview(&[0xCA, 0xFE, 0xBA, 0xBE], 4),
            "BLOB:4:application/octet-stream:yv66vg=="
        );
        assert_eq!(
            encode_blob_preview(&[], 0),
            "BLOB:0:application/octet-stream:"
        );
    }

    #[test]
    fn sniffs_the_mime_type() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        assert!(encode_blob_preview(&png, 8).starts_with("BLOB:8:image/png:"));
    }

    #[test]
    fn reports_the_true_size_of_a_truncated_value() {
        let wire = encode_blob_preview(&[1, 2, 3], 2_000_000);
        assert_eq!(wire, "BLOB:2000000:application/octet-stream:AQID");
    }
}
