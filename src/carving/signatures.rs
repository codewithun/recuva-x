#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Jpeg,
    Png,
    Pdf,
}

impl FileFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::Pdf => "PDF",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Pdf => "pdf",
        }
    }

    pub fn header(self) -> &'static [u8] {
        match self {
            Self::Jpeg => &[0xFF, 0xD8, 0xFF],
            Self::Png => &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            Self::Pdf => b"%PDF-",
        }
    }

    pub fn footer(self) -> &'static [u8] {
        match self {
            Self::Jpeg => &[0xFF, 0xD9],
            Self::Png => &[
                0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
            ],
            Self::Pdf => b"%%EOF",
        }
    }

    pub fn all() -> &'static [FileFormat] {
        &[FileFormat::Jpeg, FileFormat::Png, FileFormat::Pdf]
    }
}
