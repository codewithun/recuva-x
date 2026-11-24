use crc32fast::Hasher;

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

pub fn validate_png(data: &[u8]) -> Result<(), String> {
    if !data.starts_with(PNG_SIGNATURE) {
        return Err("Invalid PNG signature".into());
    }

    let mut offset = PNG_SIGNATURE.len();
    let mut seen_ihdr = false;
    let mut seen_idat = false;
    let mut idat_ended = false;

    while offset < data.len() {
        if data.len() - offset < 12 {
            return Err("Incomplete PNG chunk".into());
        }

        let chunk_length = u32::from_be_bytes(
            data[offset..offset + 4]
                .try_into()
                .map_err(|_| "Invalid chunk length")?,
        ) as usize;

        let chunk_type = &data[offset + 4..offset + 8];

        let chunk_end = offset
            .checked_add(12)
            .and_then(|value| value.checked_add(chunk_length))
            .ok_or("PNG chunk length overflow")?;

        if chunk_end > data.len() {
            return Err("PNG chunk exceeds file boundary".into());
        }

        if !chunk_type.iter().all(u8::is_ascii_alphabetic) {
            return Err("Invalid PNG chunk type".into());
        }

        if chunk_type[2].is_ascii_lowercase() {
            return Err("Invalid PNG reserved bit".into());
        }

        let chunk_data_start = offset + 8;
        let chunk_data_end = chunk_data_start + chunk_length;

        let stored_crc = u32::from_be_bytes(
            data[chunk_data_end..chunk_end]
                .try_into()
                .map_err(|_| "Invalid PNG CRC")?,
        );

        let mut hasher = Hasher::new();
        hasher.update(chunk_type);
        hasher.update(&data[chunk_data_start..chunk_data_end]);

        if hasher.finalize() != stored_crc {
            return Err("PNG chunk CRC mismatch".into());
        }

        match chunk_type {
            b"IHDR" => {
                if seen_ihdr || offset != PNG_SIGNATURE.len() {
                    return Err("Invalid IHDR position or duplicate".into());
                }

                if chunk_length != 13 {
                    return Err("Invalid IHDR length".into());
                }

                let width = u32::from_be_bytes(
                    data[chunk_data_start..chunk_data_start + 4]
                        .try_into()
                        .map_err(|_| "Invalid PNG width")?,
                );

                let height = u32::from_be_bytes(
                    data[chunk_data_start + 4..chunk_data_start + 8]
                        .try_into()
                        .map_err(|_| "Invalid PNG height")?,
                );

                let bit_depth = data[chunk_data_start + 8];
                let color_type = data[chunk_data_start + 9];
                let compression = data[chunk_data_start + 10];
                let filter = data[chunk_data_start + 11];
                let interlace = data[chunk_data_start + 12];

                let valid_bit_depth = match color_type {
                    0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
                    2 => matches!(bit_depth, 8 | 16),
                    3 => matches!(bit_depth, 1 | 2 | 4 | 8),
                    4 | 6 => matches!(bit_depth, 8 | 16),
                    _ => false,
                };

                if width == 0 || height == 0 {
                    return Err("PNG dimensions cannot be zero".into());
                }

                if !valid_bit_depth {
                    return Err("Invalid PNG bit depth or color type".into());
                }

                if compression != 0 || filter != 0 || interlace > 1 {
                    return Err("Invalid PNG IHDR configuration".into());
                }

                seen_ihdr = true;
            }

            b"IDAT" => {
                if !seen_ihdr {
                    return Err("IDAT encountered before IHDR".into());
                }

                if idat_ended {
                    return Err("PNG IDAT chunks must be consecutive".into());
                }

                seen_idat = true;
            }

            b"IEND" => {
                if !seen_ihdr || !seen_idat {
                    return Err("PNG is missing required chunks".into());
                }

                if chunk_length != 0 {
                    return Err("IEND chunk must be empty".into());
                }

                if chunk_end != data.len() {
                    return Err("Unexpected data after PNG IEND".into());
                }

                return Ok(());
            }

            _ => {
                if seen_idat {
                    idat_ended = true;
                }

                if chunk_type[0].is_ascii_uppercase() && chunk_type != b"PLTE" {
                    return Err("Unknown critical PNG chunk".into());
                }
            }
        }

        if chunk_type != b"IDAT" && seen_idat {
            idat_ended = true;
        }

        offset = chunk_end;
    }

    Err("PNG is missing IEND chunk".into())
}

fn is_jpeg_sof(marker: u8) -> bool {
    matches!(
        marker,
        0xC0..=0xC3
            | 0xC5..=0xC7
            | 0xC9..=0xCB
            | 0xCD..=0xCF
    )
}

pub fn validate_jpeg(data: &[u8]) -> Result<(), String> {
    if data.len() < 4 || !data.starts_with(&[0xFF, 0xD8]) {
        return Err("Invalid JPEG SOI marker".into());
    }

    let mut offset = 2;
    let mut seen_sof = false;
    let mut seen_sos = false;

    while offset < data.len() {
        if data[offset] != 0xFF {
            return Err("Expected JPEG marker".into());
        }

        while offset < data.len() && data[offset] == 0xFF {
            offset += 1;
        }

        if offset >= data.len() {
            return Err("Incomplete JPEG marker".into());
        }

        let marker = data[offset];
        offset += 1;

        if marker == 0x00 {
            return Err("Unexpected stuffed byte outside JPEG scan".into());
        }

        if marker == 0xD9 {
            if !seen_sof || !seen_sos {
                return Err("JPEG is missing SOF or SOS marker".into());
            }

            if offset != data.len() {
                return Err("Unexpected data after JPEG EOI".into());
            }

            return Ok(());
        }

        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            return Err("Unexpected standalone JPEG marker".into());
        }

        if offset + 2 > data.len() {
            return Err("Incomplete JPEG segment length".into());
        }

        let segment_length = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;

        if segment_length < 2 {
            return Err("Invalid JPEG segment length".into());
        }

        let segment_end = offset
            .checked_add(segment_length)
            .ok_or("JPEG segment length overflow")?;

        if segment_end > data.len() {
            return Err("JPEG segment exceeds file boundary".into());
        }

        if is_jpeg_sof(marker) {
            if seen_sof {
                return Err("Multiple JPEG frame headers".into());
            }

            if segment_length < 11 {
                return Err("JPEG SOF segment is too short".into());
            }

            let payload = offset + 2;

            let height = u16::from_be_bytes([data[payload + 1], data[payload + 2]]);

            let width = u16::from_be_bytes([data[payload + 3], data[payload + 4]]);

            let components = data[payload + 5] as usize;

            if width == 0 || height == 0 || components == 0 {
                return Err("Invalid JPEG dimensions or components".into());
            }

            if segment_length != 8 + 3 * components {
                return Err("Invalid JPEG SOF component table".into());
            }

            seen_sof = true;
        }

        if marker == 0xDA {
            if !seen_sof {
                return Err("JPEG SOS encountered before SOF".into());
            }

            if segment_length < 8 {
                return Err("JPEG SOS segment is too short".into());
            }

            let payload = offset + 2;
            let components = data[payload] as usize;

            if components == 0 || segment_length != 6 + 2 * components {
                return Err("Invalid JPEG SOS component table".into());
            }

            seen_sos = true;
            offset = segment_end;

            let mut found_next_marker = false;

            while offset < data.len() {
                if data[offset] != 0xFF {
                    offset += 1;
                    continue;
                }

                let marker_start = offset;
                offset += 1;

                while offset < data.len() && data[offset] == 0xFF {
                    offset += 1;
                }

                if offset >= data.len() {
                    return Err("Incomplete JPEG scan marker".into());
                }

                let scan_marker = data[offset];

                if scan_marker == 0x00 || (0xD0..=0xD7).contains(&scan_marker) {
                    offset += 1;
                    continue;
                }

                offset = marker_start;
                found_next_marker = true;
                break;
            }

            if !found_next_marker {
                return Err("JPEG scan is missing EOI marker".into());
            }

            continue;
        }

        offset = segment_end;
    }

    Err("JPEG is missing EOI marker".into())
}

#[cfg(test)]
pub(crate) fn test_png() -> Vec<u8> {
    fn append_chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        output.extend_from_slice(&(data.len() as u32).to_be_bytes());
        output.extend_from_slice(kind);
        output.extend_from_slice(data);

        let mut hasher = Hasher::new();
        hasher.update(kind);
        hasher.update(data);

        output.extend_from_slice(&hasher.finalize().to_be_bytes());
    }

    let mut png = PNG_SIGNATURE.to_vec();

    let ihdr = [0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0];

    append_chunk(&mut png, b"IHDR", &ihdr);

    let idat = [
        0x78, 0x01, 0x01, 0x02, 0x00, 0xFD, 0xFF, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01,
    ];

    append_chunk(&mut png, b"IDAT", &idat);
    append_chunk(&mut png, b"IEND", &[]);

    png
}

#[cfg(test)]
pub(crate) fn test_jpeg() -> Vec<u8> {
    vec![
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00,
        0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0x11, 0xFF, 0xD9,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_png_structure() {
        assert!(validate_png(&test_png()).is_ok());
    }

    #[test]
    fn rejects_corrupted_png_crc() {
        let mut png = test_png();
        png[29] ^= 0xFF;
        assert!(validate_png(&png).is_err());
    }

    #[test]
    fn rejects_truncated_png() {
        let mut png = test_png();
        png.pop();
        assert!(validate_png(&png).is_err());
    }

    #[test]
    fn rejects_png_without_iend() {
        let mut png = test_png();
        png.truncate(png.len() - 12);
        assert!(validate_png(&png).is_err());
    }

    #[test]
    fn accepts_valid_jpeg_structure() {
        assert!(validate_jpeg(&test_jpeg()).is_ok());
    }

    #[test]
    fn rejects_jpeg_without_sof() {
        let jpeg = vec![
            0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0x11, 0xFF,
            0xD9,
        ];

        assert!(validate_jpeg(&jpeg).is_err());
    }

    #[test]
    fn rejects_corrupted_jpeg_segment_length() {
        let mut jpeg = test_jpeg();
        jpeg[4] = 0xFF;
        jpeg[5] = 0xFF;
        assert!(validate_jpeg(&jpeg).is_err());
    }

    #[test]
    fn rejects_jpeg_without_eoi() {
        let mut jpeg = test_jpeg();
        jpeg.truncate(jpeg.len() - 2);
        assert!(validate_jpeg(&jpeg).is_err());
    }
}
